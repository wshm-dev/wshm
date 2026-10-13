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
}

impl Rest {
    pub fn new(label: &'static str, auth: Auth) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("wshm")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;
        Ok(Self { http, auth, label })
    }

    pub fn is_authenticated(&self) -> bool {
        !matches!(self.auth, Auth::None)
    }

    pub fn label(&self) -> &'static str {
        self.label
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
    /// reached the server.
    pub async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        content_type: Option<&str>,
        accept: Option<&str>,
    ) -> Result<Response> {
        let what = format!("{} {method}", self.label);
        let op = || async {
            let resp = self
                .send_once(method.clone(), url, body, content_type, accept)
                .await?;
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
        if method == Method::POST {
            crate::retry::with_retry_connect_only(&what, op).await
        } else {
            crate::retry::with_retry(&what, op).await
        }
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
