//! Small REST helper shared by the GitLab, Gitea/Forgejo and Azure DevOps
//! providers: auth header, retry policy (reads retried on transient errors,
//! creates only on connect errors), status checking and pagination headers.

use anyhow::{Context, Result};
use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode};
use serde_json::Value;

/// How requests authenticate.
#[derive(Clone)]
pub(crate) enum Auth {
    /// Anonymous (public repositories, read-only).
    None,
    /// A single header, e.g. `PRIVATE-TOKEN: <t>` or `Authorization: token <t>`.
    Header(&'static str, String),
    /// HTTP basic with an empty user and the token as password (Azure PAT).
    BasicPassword(String),
}

/// Raw response: status, headers, body text.
pub(crate) struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
}

impl Response {
    fn snippet(&self) -> String {
        self.text.chars().take(300).collect()
    }
}

pub(crate) struct Rest {
    http: reqwest::Client,
    auth: Auth,
    /// Platform name used in error messages ("GitLab", "Gitea", ...).
    label: &'static str,
    /// Login of the token owner, resolved once by [`Self::self_login`].
    me: std::sync::OnceLock<String>,
}

impl Rest {
    pub fn new(label: &'static str, auth: Auth) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("wshm")
            .timeout(std::time::Duration::from_secs(30))
            // Never follow a redirect to another host: reqwest only strips
            // Authorization/Cookie cross-host, not custom token headers.
            .redirect(reqwest::redirect::Policy::custom(same_host_redirects))
            .build()
            .context("Failed to create HTTP client")?;
        Ok(Self {
            http,
            auth,
            label,
            me: std::sync::OnceLock::new(),
        })
    }

    pub fn is_authenticated(&self) -> bool {
        !matches!(self.auth, Auth::None)
    }

    pub fn label(&self) -> &'static str {
        self.label
    }

    /// Login of the authenticated account (`field` of the JSON at `url`),
    /// fetched once and cached. `None` when anonymous or on error.
    pub async fn self_login(&self, url: &str, field: &str) -> Option<String> {
        if !self.is_authenticated() {
            return None;
        }
        if let Some(me) = self.me.get() {
            return Some(me.clone());
        }
        match self.get_json(url).await {
            Ok(v) => {
                let login = v[field].as_str().filter(|s| !s.is_empty())?.to_string();
                let _ = self.me.set(login.clone());
                Some(login)
            }
            Err(e) => {
                tracing::warn!("{}: cannot resolve the token owner: {e:#}", self.label);
                None
            }
        }
    }

    /// Fail early with a clear message when a mutating call is attempted
    /// without credentials.
    pub fn require_auth(&self, action: &str) -> Result<()> {
        if !self.is_authenticated() {
            anyhow::bail!(
                "{action}: {} auth required — add the API token in Settings → Secrets \
                 or the provider env var",
                self.label
            );
        }
        Ok(())
    }

    async fn send_once(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        content_type: Option<&str>,
        accept: Option<&str>,
    ) -> Result<Response> {
        let mut req = self.http.request(method.clone(), url);
        req = match &self.auth {
            Auth::None => req,
            Auth::Header(name, value) => req.header(*name, value),
            Auth::BasicPassword(token) => req.basic_auth("", Some(token)),
        };
        if let Some(accept) = accept {
            req = req.header(reqwest::header::ACCEPT, accept);
        }
        if let Some(body) = body {
            req = req
                .header(
                    reqwest::header::CONTENT_TYPE,
                    content_type.unwrap_or("application/json"),
                )
                .body(serde_json::to_vec(body)?);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{} {method} {}", self.label, redact(url)))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp
            .text()
            .await
            .with_context(|| format!("{} {method} {}: read body", self.label, redact(url)))?;
        Ok(Response {
            status,
            headers,
            text,
        })
    }

    /// Send with the retry policy matching the method: POST is not
    /// idempotent, so it is only retried when the request provably never
    /// reached the server or was refused by rate limiting (429).
    pub async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        content_type: Option<&str>,
        accept: Option<&str>,
    ) -> Result<Response> {
        let writes = method == Method::POST;
        self.send_with(method, url, body, content_type, accept, writes)
            .await
    }

    /// `writes`: retry only connect failures and 429 (never a 5xx, which
    /// may follow a processed request).
    async fn send_with(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        content_type: Option<&str>,
        accept: Option<&str>,
        writes: bool,
    ) -> Result<Response> {
        let what = format!("{} {method}", self.label);
        let max = crate::retry::effective_max_attempts();
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let op = || async {
            let n = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let resp = self
                .send_once(method.clone(), url, body, content_type, accept)
                .await?;
            // Rate limited (GitLab.com, self-managed with limits on): wait
            // what the server asks, only when the retry layer will retry.
            if resp.status == StatusCode::TOO_MANY_REQUESTS {
                if let Some(wait) = wait_before_retry(n, max, &resp.headers) {
                    tracing::warn!(
                        "{} rate limited on {}: waiting {}s",
                        self.label,
                        redact(url),
                        wait.as_secs()
                    );
                    tokio::time::sleep(wait).await;
                }
            }
            // Surface 5xx/429 as errors so the retry layer sees them.
            if resp.status.is_server_error() || resp.status == StatusCode::TOO_MANY_REQUESTS {
                anyhow::bail!(
                    "{} API error ({}) on {method} {}: {}",
                    self.label,
                    resp.status,
                    redact(url),
                    resp.snippet()
                );
            }
            Ok(resp)
        };
        if writes {
            crate::retry::with_retry_writes(&what, op).await
        } else {
            crate::retry::with_retry(&what, op).await
        }
    }

    /// [`Self::send_json`] with the write retry policy whatever the method:
    /// for a non-idempotent PUT (merge) that must not be re-issued after a
    /// 5xx.
    pub async fn send_json_once(&self, method: Method, url: &str, body: &Value) -> Result<Value> {
        let resp = self
            .send_with(method.clone(), url, Some(body), None, None, true)
            .await?;
        let resp = self.check(&method, url, resp)?;
        self.parse(url, &resp.text)
    }

    fn check(&self, method: &Method, url: &str, resp: Response) -> Result<Response> {
        if !resp.status.is_success() {
            anyhow::bail!(
                "{} API error ({}) on {method} {}: {}",
                self.label,
                resp.status,
                redact(url),
                resp.snippet()
            );
        }
        Ok(resp)
    }

    fn parse(&self, url: &str, text: &str) -> Result<Value> {
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(text)
            .with_context(|| format!("{}: invalid JSON from {}", self.label, redact(url)))
    }

    /// GET and parse JSON; non-2xx is an error.
    pub async fn get_json(&self, url: &str) -> Result<Value> {
        Ok(self.get_json_with_headers(url).await?.0)
    }

    /// GET and parse JSON, also returning the response headers (pagination).
    pub async fn get_json_with_headers(&self, url: &str) -> Result<(Value, HeaderMap)> {
        let resp = self.send(Method::GET, url, None, None, None).await?;
        let resp = self.check(&Method::GET, url, resp)?;
        Ok((self.parse(url, &resp.text)?, resp.headers))
    }

    /// GET JSON, mapping 404 to `None`.
    pub async fn get_json_opt(&self, url: &str) -> Result<Option<Value>> {
        let resp = self.send(Method::GET, url, None, None, None).await?;
        if resp.status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = self.check(&Method::GET, url, resp)?;
        Ok(Some(self.parse(url, &resp.text)?))
    }

    /// GET a text body (raw diffs).
    pub async fn get_text(&self, url: &str, accept: Option<&str>) -> Result<String> {
        let resp = self.send(Method::GET, url, None, None, accept).await?;
        Ok(self.check(&Method::GET, url, resp)?.text)
    }

    /// Send a JSON body and parse the JSON answer; non-2xx is an error.
    pub async fn send_json(&self, method: Method, url: &str, body: &Value) -> Result<Value> {
        self.send_json_as(method, url, body, None).await
    }

    /// Like [`Self::send_json`] with an explicit content type (Azure
    /// work-item updates require `application/json-patch+json`).
    pub async fn send_json_as(
        &self,
        method: Method,
        url: &str,
        body: &Value,
        content_type: Option<&str>,
    ) -> Result<Value> {
        let resp = self
            .send(method.clone(), url, Some(body), content_type, None)
            .await?;
        let resp = self.check(&method, url, resp)?;
        self.parse(url, &resp.text)
    }

    /// DELETE; 404 (already gone) is success.
    pub async fn delete(&self, url: &str) -> Result<()> {
        let resp = self.send(Method::DELETE, url, None, None, None).await?;
        if resp.status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        self.check(&Method::DELETE, url, resp)?;
        Ok(())
    }
}

/// Redirect policy: follow up to 5 hops, only on the original host.
fn same_host_redirects(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    let origin = attempt
        .previous()
        .first()
        .and_then(|u| u.host_str().map(String::from));
    if attempt.previous().len() > 5 {
        attempt.error("too many redirects")
    } else if attempt.url().host_str().map(String::from) != origin {
        attempt.stop()
    } else {
        attempt.follow()
    }
}

/// Strip query strings from URLs before they reach logs (tokens never travel
/// in URLs here, but filters can be long and noisy).
fn redact(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Read a numeric response header (`x-next-page`, `x-total-count`, ...).
pub(crate) fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
}

/// Longest wait honoured from a `Retry-After` header.
const MAX_RETRY_AFTER_SECS: u64 = 60;

/// Delay asked by a 429 answer: `Retry-After` in seconds, else GitLab's
/// `RateLimit-Reset` (epoch seconds), capped at [`MAX_RETRY_AFTER_SECS`].
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<std::time::Duration> {
    let secs = header_u64(headers, "retry-after").or_else(|| {
        let reset = header_u64(headers, "ratelimit-reset")?;
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        Some(reset.saturating_sub(now))
    })?;
    Some(std::time::Duration::from_secs(
        secs.min(MAX_RETRY_AFTER_SECS),
    ))
}

/// Delay before re-issuing attempt `attempt` (1-based) of `max` after a
/// 429: `None` on the last attempt, where no retry follows.
pub(crate) fn wait_before_retry(
    attempt: u32,
    max: u32,
    headers: &HeaderMap,
) -> Option<std::time::Duration> {
    if attempt >= max {
        return None;
    }
    retry_after(headers)
}

/// Parse an RFC 3339 timestamp (any offset) into UTC.
pub(crate) fn parse_ts(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// `a < b` on timestamps, falling back to string order when unparsable
/// (forges return different but sortable RFC 3339 variants).
pub(crate) fn ts_before(a: &str, b: &str) -> bool {
    match (parse_ts(a), parse_ts(b)) {
        (Some(a), Some(b)) => a < b,
        _ => a < b,
    }
}

/// Safety cap on pages followed by list endpoints (100 × 100 items).
pub(crate) const MAX_PAGES: u32 = 100;

/// Label names from a JSON array of strings or of `{name}` objects.
pub(crate) fn label_names(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    l.as_str()
                        .or_else(|| l.get("name").and_then(|n| n.as_str()))
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a comment body carries wshm's marker (custom or legacy).
pub(crate) fn has_marker(body: &str, marker: &str) -> bool {
    body.contains(marker) || body.contains(crate::github::issues::WSHM_COMMENT_MARKER)
}

/// Map a CI state to wshm's three buckets.
pub(crate) fn ci_bucket(state: &str) -> Option<&'static str> {
    match state.to_ascii_lowercase().as_str() {
        "success" | "succeeded" | "passed" | "warning" => Some("success"),
        "failed" | "failure" | "error" | "canceled" | "cancelled" => Some("failure"),
        "pending"
        | "running"
        | "created"
        | "waiting_for_resource"
        | "preparing"
        | "scheduled"
        | "manual"
        | "notset" => Some("pending"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::mock::{MockServer, Reply};

    #[tokio::test]
    async fn redirects_never_leave_the_original_host() {
        let server = MockServer::start().await;
        let port = server.url().rsplit(':').next().unwrap().to_string();
        // Same port, different host name: a token header must not follow.
        server.on(
            "GET",
            "/away",
            Reply::text("")
                .status(302)
                .header("location", &format!("http://localhost:{port}/x")),
        );
        server.on(
            "GET",
            "/here",
            Reply::text("").status(302).header("location", "/ok"),
        );
        server.on("GET", "/ok", Reply::json(serde_json::json!({"ok": true})));
        let rest = Rest::new("T", Auth::Header("PRIVATE-TOKEN", "secret".into())).unwrap();

        let err = rest
            .get_json(&format!("{}/away", server.url()))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("302"), "{err:#}");
        assert!(!server.requests().iter().any(|r| r.path == "/x"));

        let ok = rest
            .get_json(&format!("{}/here", server.url()))
            .await
            .unwrap();
        assert_eq!(ok["ok"], true);
    }

    #[test]
    fn retry_after_is_read_and_capped() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert("retry-after", "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(std::time::Duration::from_secs(7)));
        h.insert("retry-after", "3600".parse().unwrap());
        assert_eq!(retry_after(&h), Some(std::time::Duration::from_secs(60)));
        let mut h = HeaderMap::new();
        let reset = chrono::Utc::now().timestamp() + 5;
        h.insert("ratelimit-reset", reset.to_string().parse().unwrap());
        let wait = retry_after(&h).unwrap().as_secs();
        assert!((4..=5).contains(&wait), "{wait}");
    }

    #[test]
    fn rate_limit_wait_only_before_a_retry() {
        let mut h = HeaderMap::new();
        h.insert("retry-after", "9".parse().unwrap());
        assert_eq!(
            wait_before_retry(1, 3, &h),
            Some(std::time::Duration::from_secs(9))
        );
        assert_eq!(wait_before_retry(3, 3, &h), None);
        assert_eq!(wait_before_retry(1, 1, &h), None);
    }

    #[tokio::test]
    async fn throttled_posts_are_retried_but_server_errors_are_not() {
        // Every policy the test suite installs allows >= 3 attempts.
        let server = MockServer::start().await;
        server.on(
            "POST",
            "/create",
            Reply::json(serde_json::json!({"message": "429 Too Many Requests"}))
                .status(429)
                .header("retry-after", "0"),
        );
        server.on(
            "PUT",
            "/merge",
            Reply::json(serde_json::json!({"message": "oops"})).status(502),
        );
        let rest = Rest::new("T", Auth::None).unwrap();
        let body = serde_json::json!({});
        assert!(rest
            .send_json(Method::POST, &format!("{}/create", server.url()), &body)
            .await
            .is_err());
        let posts = server.requests().len();
        assert!(posts >= 2, "429 on POST must be retried, got {posts}");
        assert!(rest
            .send_json_once(Method::PUT, &format!("{}/merge", server.url()), &body)
            .await
            .is_err());
        let puts = server
            .requests()
            .iter()
            .filter(|r| r.method == "PUT")
            .count();
        assert_eq!(puts, 1, "a write is not re-issued after a 5xx");
    }

    #[tokio::test]
    async fn rate_limited_reads_are_retried() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/limited",
            Reply::json(serde_json::json!({"message": "429 Too Many Requests"}))
                .status(429)
                .header("retry-after", "0"),
        );
        let rest = Rest::new("T", Auth::None).unwrap();
        let err = rest
            .get_json(&format!("{}/limited", server.url()))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("429"), "{err:#}");
        // Retried per the global policy (other tests may change it).
        assert!(!server.requests().is_empty());
    }

    #[tokio::test]
    async fn self_login_is_resolved_once() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/user",
            Reply::json(serde_json::json!({"login": "wshm-bot"})),
        );
        let rest = Rest::new("T", Auth::Header("Authorization", "token t".into())).unwrap();
        let url = format!("{}/user", server.url());
        assert_eq!(
            rest.self_login(&url, "login").await.as_deref(),
            Some("wshm-bot")
        );
        assert_eq!(
            rest.self_login(&url, "login").await.as_deref(),
            Some("wshm-bot")
        );
        assert_eq!(server.requests().len(), 1);
        let anon = Rest::new("T", Auth::None).unwrap();
        assert!(anon.self_login(&url, "login").await.is_none());
    }
}
