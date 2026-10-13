//! Minimal in-process HTTP mock (axum on 127.0.0.1:0) for provider tests:
//! canned replies by method + path (+ optional query substring), and a log
//! of every request received.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(Clone)]
pub struct Reply {
    status: u16,
    body: String,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    query: Option<String>,
}

impl Reply {
    pub fn json(v: serde_json::Value) -> Self {
        Self {
            status: 200,
            body: v.to_string(),
            content_type: "application/json",
            headers: Vec::new(),
            query: None,
        }
    }

    pub fn text(s: &str) -> Self {
        Self {
            status: 200,
            body: s.to_string(),
            content_type: "text/plain",
            headers: Vec::new(),
            query: None,
        }
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Only match when the query string contains `fragment`.
    pub fn when_query(mut self, fragment: &str) -> Self {
        self.query = Some(fragment.to_string());
        self
    }
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

#[derive(Default)]
struct Inner {
    rules: Mutex<Vec<(String, String, Reply)>>,
    log: Mutex<Vec<Recorded>>,
}

pub struct MockServer {
    addr: std::net::SocketAddr,
    inner: Arc<Inner>,
}

impl MockServer {
    pub async fn start() -> Self {
        let inner = Arc::new(Inner::default());
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&inner));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("mock addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, inner }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn on(&self, method: &str, path: &str, reply: Reply) {
        self.inner
            .rules
            .lock()
            .unwrap()
            .push((method.to_string(), path.to_string(), reply));
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.log.lock().unwrap().clone()
    }
}

async fn handle(
    State(inner): State<Arc<Inner>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    inner.log.lock().unwrap().push(Recorded {
        method: method.to_string(),
        path: path.clone(),
        query: query.clone(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    let reply = inner
        .rules
        .lock()
        .unwrap()
        .iter()
        .find(|(m, p, r)| {
            m == method.as_str()
                && *p == path
                && r.query.as_deref().is_none_or(|q| query.contains(q))
        })
        .map(|(_, _, r)| r.clone());
    match reply {
        Some(r) => {
            let mut resp = (
                StatusCode::from_u16(r.status).unwrap_or(StatusCode::OK),
                [("content-type", r.content_type)],
                r.body,
            )
                .into_response();
            for (k, v) in r.headers {
                resp.headers_mut().insert(
                    axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    axum::http::HeaderValue::from_str(&v).unwrap(),
                );
            }
            resp
        }
        None => (
            StatusCode::NOT_FOUND,
            format!("{{\"message\":\"no mock for {method} {path}?{query}\"}}"),
        )
            .into_response(),
    }
}
