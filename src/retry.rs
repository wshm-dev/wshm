//! Centralized, configurable retry for transient HTTP failures.
//!
//! A single helper, [`with_retry`], wraps any fallible async operation and
//! re-issues it on transient errors (premature connection EOF, resets,
//! timeouts, HTTP 5xx / 429) using exponential backoff with jitter.
//!
//! The policy is process-global and live-updatable: the daemon installs it
//! at startup via [`set_global`], and the Settings UI re-installs it on
//! every save, so a change takes effect without a daemon restart.

use std::future::Future;
use std::sync::RwLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Retry policy. Persisted in the global config (`~/.wshm/global.toml`,
/// `[retry]` table) and editable live from Settings -> Reliability.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetryConfig {
    /// Master switch. When false, [`with_retry`] runs the operation once.
    #[serde(default = "default_retry_enabled")]
    pub enabled: bool,

    /// Total attempts including the first one. Clamped to `1..=10`.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Delay before the first retry, in milliseconds. Doubles each attempt.
    #[serde(default = "default_initial_backoff_ms")]
    pub initial_backoff_ms: u64,

    /// Upper bound on any single backoff delay, in milliseconds.
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
}

fn default_retry_enabled() -> bool {
    true
}
fn default_max_attempts() -> u32 {
    3
}
fn default_initial_backoff_ms() -> u64 {
    500
}
fn default_max_backoff_ms() -> u64 {
    5_000
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: default_retry_enabled(),
            max_attempts: default_max_attempts(),
            initial_backoff_ms: default_initial_backoff_ms(),
            max_backoff_ms: default_max_backoff_ms(),
        }
    }
}

impl RetryConfig {
    /// Clamp user-supplied values into safe ranges. Applied before the
    /// policy is installed so a bad Settings entry can never wedge the
    /// daemon (e.g. zero attempts, or an hours-long backoff).
    pub fn sanitized(&self) -> Self {
        Self {
            enabled: self.enabled,
            max_attempts: self.max_attempts.clamp(1, 10),
            initial_backoff_ms: self.initial_backoff_ms.clamp(50, 60_000),
            max_backoff_ms: self.max_backoff_ms.clamp(50, 120_000),
        }
    }
}

/// Process-wide policy. `None` until [`set_global`] runs; callers fall back
/// to [`RetryConfig::default`] so retry works even before the daemon boots.
static GLOBAL: RwLock<Option<RetryConfig>> = RwLock::new(None);

/// Install the process-wide retry policy. Called at daemon startup and
/// again whenever Settings -> Reliability is saved.
pub fn set_global(cfg: RetryConfig) {
    if let Ok(mut guard) = GLOBAL.write() {
        *guard = Some(cfg.sanitized());
    }
}

/// Current retry policy (sanitized defaults if never installed).
pub fn global() -> RetryConfig {
    GLOBAL
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

/// Decide whether an error is worth retrying.
///
/// Call sites hand us `anyhow::Error` that has already lost its concrete
/// `reqwest` / `octocrab` / `hyper` type, so we classify on the rendered
/// error chain. Only transport-level failures and server-side 5xx / 429
/// are retried; 4xx (auth, not-found, validation) and parse errors are not.
pub fn is_transient(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}").to_lowercase();
    const TRANSIENT: &[&str] = &[
        // hyper: keep-alive connection closed by the peer mid-response.
        "end of file before message length reached",
        "incompletemessage",
        "connection closed before message completed",
        // transport-level resets and drops.
        "connection reset",
        "connection closed",
        "connection refused",
        "connection aborted",
        "broken pipe",
        "error sending request",
        "error reading a body",
        "tls handshake",
        // timeouts and name resolution.
        "timed out",
        "operation timed out",
        "dns error",
        "failed to lookup address",
        // retryable server-side responses.
        "(500 ",
        "(502 ",
        "(503 ",
        "(504 ",
        "(429 ",
        "500 internal server error",
        "502 bad gateway",
        "503 service unavailable",
        "504 gateway timeout",
        "429 too many requests",
    ];
    TRANSIENT.iter().any(|needle| msg.contains(needle))
}

/// Connect-level subset of [`is_transient`]: errors that prove the request
/// never reached (or was never accepted by) the server, so re-issuing it
/// cannot duplicate a server-side effect.
///
/// This deliberately EXCLUDES post-send failures like
/// `"error reading a body"` / `"end of file before message length reached"`:
/// those frequently happen AFTER the server has fully processed a POST but
/// the response read failed. Retrying a non-idempotent create on those would
/// duplicate the issue/PR/review/comment, so write call sites must use
/// [`with_retry_connect_only`] instead of [`with_retry`].
pub fn is_transient_connect(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}").to_lowercase();
    const CONNECT_TRANSIENT: &[&str] = &[
        "connection refused",
        "error sending request",
        "tls handshake",
        "dns error",
        "failed to lookup address",
        "timed out",
        "operation timed out",
    ];
    CONNECT_TRANSIENT.iter().any(|needle| msg.contains(needle))
}

/// Run `op`, retrying transient failures per the global [`RetryConfig`].
///
/// `op` is a closure that returns a fresh future on each call, so a retry
/// genuinely re-issues the request (and picks a fresh pooled connection).
/// `label` names the operation for log lines.
pub async fn with_retry<T, F, Fut>(label: &str, op: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    with_retry_classified(label, is_transient, op).await
}

/// Like [`with_retry`] but only retries connect-level failures (see
/// [`is_transient_connect`]). Use this for NON-idempotent writes (POST
/// creates: issues, PRs, reviews, comments, AI completions) so a response
/// body-read EOF on an already-processed request does not duplicate the
/// server-side effect.
pub async fn with_retry_connect_only<T, F, Fut>(label: &str, op: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    with_retry_classified(label, is_transient_connect, op).await
}

/// Shared retry loop parameterized by the transient-error classifier.
async fn with_retry_classified<T, F, Fut>(
    label: &str,
    is_retryable: fn(&anyhow::Error) -> bool,
    op: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let cfg = global();
    let max = if cfg.enabled {
        cfg.max_attempts.max(1)
    } else {
        1
    };

    let mut attempt: u32 = 1;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt >= max || !cfg.enabled || !is_retryable(&err) {
                    return Err(err);
                }
                let backoff = backoff_delay(&cfg, attempt);
                warn!(
                    "{label}: transient failure on attempt {attempt}/{max}, \
                     retrying in {}ms: {err:#}",
                    backoff.as_millis()
                );
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
        }
    }
}

/// Exponential backoff with full jitter, capped at `max_backoff_ms`.
///
/// `attempt` is 1-based: the first retry waits `initial_backoff_ms`, the
/// next `2x`, and so on. Jitter spreads the delay over `[half, full]` to
/// avoid a thundering herd when many calls fail at once.
fn backoff_delay(cfg: &RetryConfig, attempt: u32) -> Duration {
    let shift = (attempt - 1).min(16);
    let exp = cfg
        .initial_backoff_ms
        .saturating_mul(1_u64 << shift)
        .min(cfg.max_backoff_ms);
    let half = exp / 2;
    let jitter = if half > 0 {
        rand::random::<u64>() % half
    } else {
        0
    };
    Duration::from_millis(half + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn classifies_transient_vs_permanent() {
        let eof = anyhow::anyhow!(
            "error reading a body from connection: end of file before message length reached"
        );
        assert!(is_transient(&eof));
        let server = anyhow::anyhow!("GitLab API error (503 Service Unavailable): down");
        assert!(is_transient(&server));
        let throttled = anyhow::anyhow!("AI API error (429 Too Many Requests): slow down");
        assert!(is_transient(&throttled));

        let not_found = anyhow::anyhow!("GitLab API error (404 Not Found): missing");
        assert!(!is_transient(&not_found));
        let auth = anyhow::anyhow!("Anthropic API error (401 Unauthorized): bad key");
        assert!(!is_transient(&auth));
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        set_global(RetryConfig {
            enabled: true,
            max_attempts: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 100,
        });
        let calls = AtomicU32::new(0);
        let result: anyhow::Result<u32> = with_retry("test", || async {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                anyhow::bail!("connection reset by peer");
            }
            Ok(n)
        })
        .await;
        assert_eq!(result.unwrap(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn connect_only_excludes_post_send_eof() {
        // A response body-read EOF must NOT be retried for writes: the
        // server may have already processed the POST.
        let eof = anyhow::anyhow!(
            "error reading a body from connection: end of file before message length reached"
        );
        assert!(is_transient(&eof), "EOF is transient for idempotent reads");
        assert!(
            !is_transient_connect(&eof),
            "EOF must NOT be connect-transient (would duplicate a write)"
        );

        // Connect-level failures are safe to re-issue: the request never
        // reached the server.
        let refused = anyhow::anyhow!("error sending request: connection refused");
        assert!(is_transient_connect(&refused));
    }

    #[tokio::test]
    async fn connect_only_does_not_retry_body_eof() {
        set_global(RetryConfig {
            enabled: true,
            max_attempts: 5,
            initial_backoff_ms: 50,
            max_backoff_ms: 100,
        });
        let calls = AtomicU32::new(0);
        let result: anyhow::Result<u32> = with_retry_connect_only("test", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("error reading a body: end of file before message length reached")
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a post-send EOF must not be retried for writes"
        );
    }

    #[tokio::test]
    async fn does_not_retry_permanent_errors() {
        set_global(RetryConfig {
            enabled: true,
            max_attempts: 5,
            initial_backoff_ms: 50,
            max_backoff_ms: 100,
        });
        let calls = AtomicU32::new(0);
        let result: anyhow::Result<u32> = with_retry("test", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("AI API error (404 Not Found): missing")
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
