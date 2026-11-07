//! Azure DevOps service hooks (the "Web Hooks" consumer), received on the
//! same `/webhook` endpoint as GitHub/GitLab deliveries.
//!
//! Azure sends no event header and no signature: a delivery is recognised
//! by its JSON (`publisherId = "tfs"`) and authenticated by a shared secret
//! configured on the subscription, either as the Basic authentication
//! password (any user name) or as an `X-Wshm-Token: <secret>` HTTP header.
//! The secret is visible to the project administrators who manage service
//! hooks, so it is never the GitHub HMAC secret: secret store key
//! [`AZURE_WEBHOOK_SECRET_KEY`] (repo scope, then global) or
//! `WSHM_AZURE_DEVOPS_WEBHOOK_SECRET`.
//!
//! Events are translated to the GitHub shapes the processor handles:
//! `workitem.created` → `issues.opened`, `workitem.commented` and the PR
//! comment event → `issue_comment.created`, `git.pullrequest.created` →
//! `pull_request.opened`, `git.pullrequest.updated` → `.synchronize`
//! (active) or `.closed` (completed / abandoned).
//! https://learn.microsoft.com/azure/devops/service-hooks/events

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde_json::{json, Value};
use tracing::warn;

/// Secret-store key of the service hook secret.
pub const AZURE_WEBHOOK_SECRET_KEY: &str = "azure_devops_webhook_secret";
const SECRET_ENV: &str = "WSHM_AZURE_DEVOPS_WEBHOOK_SECRET";
/// Custom header carrying the secret (alternative to Basic auth).
const TOKEN_HEADER: &str = "x-wshm-token";
/// Optional custom header naming the target repo (`org/project/repo`) of
/// work item events, which carry a project but no repository.
const REPO_HEADER: &str = "x-wshm-repo";

/// The payload, when `body` is an Azure DevOps service hook delivery.
pub fn parse(body: &[u8]) -> Option<Value> {
    let v: Value = serde_json::from_slice(body).ok()?;
    (v["publisherId"] == "tfs" && v["eventType"].is_string()).then_some(v)
}

fn reject(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

/// Authenticate and translate a delivery recognised by [`parse`]. Returns
/// (event_type, action, payload) like the other forges; events wshm does
/// not handle come back as `azure:<eventType>` and are ignored.
#[allow(clippy::result_large_err)]
pub fn validate(headers: &HeaderMap, payload: Value) -> Result<(String, String, Value), Response> {
    let repo_header = headers
        .get(REPO_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let translated = translate(&payload, repo_header);
    let slug = translated
        .as_ref()
        .and_then(|(_, _, p)| p["repository"]["full_name"].as_str())
        .map(String::from);
    let expected = crate::secrets::resolve(
        crate::secrets::global().as_ref(),
        slug.as_deref(),
        AZURE_WEBHOOK_SECRET_KEY,
        SECRET_ENV,
    )
    .filter(|t| !t.trim().is_empty());
    match expected {
        Some(expected) => {
            if !presented_secrets(headers)
                .iter()
                .any(|given| super::server::constant_time_eq(&expected, given))
            {
                warn!("Invalid Azure DevOps service hook credentials");
                return Err(reject(StatusCode::UNAUTHORIZED, "invalid credentials"));
            }
        }
        None => {
            let allow_unsigned = std::env::var("WSHM_ALLOW_UNSIGNED_WEBHOOKS")
                .ok()
                .filter(|v| v == "1" || v == "true")
                .is_some();
            if !allow_unsigned {
                warn!(
                    "Rejecting Azure DevOps service hook: no secret configured (secret \
                     `{AZURE_WEBHOOK_SECRET_KEY}` or {SECRET_ENV})"
                );
                return Err(reject(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "azure devops webhook secret not configured",
                ));
            }
        }
    }
    let event = payload["eventType"].as_str().unwrap_or("").to_string();
    Ok(translated.unwrap_or_else(|| (format!("azure:{event}"), String::new(), payload)))
}

/// Secrets a delivery presents: the `X-Wshm-Token` header and the Basic
/// authentication password.
fn presented_secrets(headers: &HeaderMap) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(t) = headers.get(TOKEN_HEADER).and_then(|v| v.to_str().ok()) {
        out.push(t.to_string());
    }
    let basic = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Basic ")
                .or_else(|| v.strip_prefix("basic "))
        })
        .and_then(|b| {
            base64::engine::general_purpose::STANDARD
                .decode(b.trim())
                .ok()
        })
        .and_then(|raw| String::from_utf8(raw).ok());
    if let Some(pair) = basic {
        if let Some((_, password)) = pair.split_once(':') {
            out.push(password.to_string());
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Organization (Services) or collection (Server) of the delivery, from
/// the `resourceContainers` base URL.
fn org_of(p: &Value) -> Option<String> {
    let rc = &p["resourceContainers"];
    let base = rc["collection"]["baseUrl"]
        .as_str()
        .or_else(|| rc["account"]["baseUrl"].as_str())?;
    let url = reqwest::Url::parse(base).ok()?;
    if let Some(org) = url.host_str()?.strip_suffix(".visualstudio.com") {
        return Some(org.to_string());
    }
    url.path_segments()?.rfind(|s| !s.is_empty()).map(|s| {
        urlencoding::decode(s)
            .map(|d| d.into_owned())
            .unwrap_or(s.to_string())
    })
}

/// Unique name of an identity field: `{uniqueName}` objects, or the
/// legacy `"Display Name <unique name>"` strings of work item fields.
fn unique_name(v: &Value) -> String {
    if let Some(n) = v["uniqueName"].as_str() {
        return n.to_string();
    }
    let s = v.as_str().unwrap_or("");
    match (s.rfind('<'), s.rfind('>')) {
        (Some(a), Some(b)) if a < b => s[a + 1..b].to_string(),
        _ => s.to_string(),
    }
}

/// Plain text of a work item comment (`System.History` is HTML).
fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    for br in ["<br>", "<br/>", "<br />", "</div>", "</p>"] {
        s = s.replace(br, "\n");
    }
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

/// Translate an Azure DevOps event to the GitHub event the processor
/// handles. `None` for everything else.
pub fn translate(p: &Value, repo_header: Option<&str>) -> Option<(String, String, Value)> {
    let org = org_of(p)?;
    let event = p["eventType"].as_str()?;
    let r = &p["resource"];
    let delivery = p["id"].clone();
    // Target of the event: PR events name their repository; work item
    // events only their project.
    let target = |project: &str, repo: Option<&str>| {
        let full_name = match (repo, repo_header) {
            (Some(repo), _) => format!("{org}/{project}/{repo}"),
            (None, Some(h)) => h.to_string(),
            (None, None) => format!("{org}/{project}"),
        };
        json!({
            "repository": {"full_name": full_name},
            "wshm_azure": {"project": format!("{org}/{project}"), "repo": repo, "explicit": repo.is_none() && repo_header.is_some()},
            "wshm_delivery_id": delivery,
        })
    };
    let with = |mut base: Value, extra: Value| {
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    };
    match event {
        "workitem.created" => {
            let number = r["id"].as_u64()?;
            let project = r["fields"]["System.TeamProject"].as_str()?;
            Some((
                "issues".into(),
                "opened".into(),
                with(
                    target(project, None),
                    json!({"action": "opened", "issue": {"number": number}}),
                ),
            ))
        }
        "workitem.commented" => {
            let number = r["id"].as_u64()?;
            let f = &r["fields"];
            let project = f["System.TeamProject"].as_str()?;
            let body = html_to_text(f["System.History"].as_str().unwrap_or(""));
            let user = unique_name(&f["System.ChangedBy"]);
            Some((
                "issue_comment".into(),
                "created".into(),
                with(
                    target(project, None),
                    json!({
                        "action": "created",
                        "issue": {"number": number},
                        "comment": {"id": r["rev"], "body": body, "user": {"login": user}},
                        "sender": {"login": user},
                    }),
                ),
            ))
        }
        "git.pullrequest.created" | "git.pullrequest.updated" => {
            let number = r["pullRequestId"].as_u64()?;
            let repo = r["repository"]["name"].as_str()?;
            let project = r["repository"]["project"]["name"].as_str()?;
            let action = match (event, r["status"].as_str()) {
                ("git.pullrequest.created", _) => "opened",
                (_, Some("completed" | "abandoned")) => "closed",
                // Subscribe with the "Source branch updated" filter: any
                // other change would also re-run the analysis.
                (_, Some("active")) => "synchronize",
                _ => return None,
            };
            Some((
                "pull_request".into(),
                action.into(),
                with(
                    target(project, Some(repo)),
                    json!({"action": action, "pull_request": {"number": number}}),
                ),
            ))
        }
        "ms.vss-code.git-pullrequest-comment-event" => {
            let c = &r["comment"];
            if c["commentType"] == "system" || c["isDeleted"].as_bool() == Some(true) {
                return None;
            }
            // The same event fires on edits: an edited comment must not
            // re-run its command.
            if let (Some(published), Some(updated)) = (
                c["publishedDate"].as_str(),
                c["lastContentUpdatedDate"].as_str(),
            ) {
                if published != updated {
                    return None;
                }
            }
            let pr = &r["pullRequest"];
            let number = pr["pullRequestId"].as_u64()?;
            let repo = pr["repository"]["name"].as_str()?;
            let project = pr["repository"]["project"]["name"].as_str()?;
            let user = unique_name(&c["author"]);
            Some((
                "issue_comment".into(),
                "created".into(),
                with(
                    target(project, Some(repo)),
                    json!({
                        "action": "created",
                        "issue": {"number": number, "pull_request": {}},
                        "comment": {"id": c["id"], "body": c["content"], "user": {"login": user}},
                        "sender": {"login": user},
                    }),
                ),
            ))
        }
        _ => None,
    }
}

/// Whether a translated Azure payload is for `slug`. Names compare
/// case-insensitively (Azure names are case-insensitive). A repository
/// named after its project may be configured as `org/project`; a work item
/// event (project only) matches every repo of its project.
pub fn targets(payload: &Value, slug: &str) -> bool {
    let full = payload["repository"]["full_name"].as_str().unwrap_or("");
    if full.eq_ignore_ascii_case(slug) {
        return true;
    }
    let a = &payload["wshm_azure"];
    let Some(project) = a["project"].as_str() else {
        return false;
    };
    match a["repo"].as_str() {
        Some(repo) => {
            let name = project.rsplit('/').next().unwrap_or("");
            repo.eq_ignore_ascii_case(name) && slug.eq_ignore_ascii_case(project)
        }
        None if a["explicit"].as_bool() == Some(true) => false,
        None => {
            slug.eq_ignore_ascii_case(project)
                || slug
                    .to_ascii_lowercase()
                    .starts_with(&format!("{}/", project.to_ascii_lowercase()))
        }
    }
}

/// The configured repo a translated Azure payload is for: an exact match,
/// else the only repo [`targets`] accepts. Several repos of the same
/// project match a work item event: it is then not routed (set the
/// `X-Wshm-Repo` header on the subscription).
pub fn route<'a>(payload: &Value, slugs: impl Iterator<Item = &'a String>) -> Option<String> {
    let full = payload["repository"]["full_name"].as_str().unwrap_or("");
    let candidates: Vec<&String> = slugs.filter(|s| targets(payload, s)).collect();
    if let Some(exact) = candidates.iter().find(|s| s.eq_ignore_ascii_case(full)) {
        return Some((*exact).clone());
    }
    match candidates.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn containers() -> Value {
        json!({"collection": {"baseUrl": "https://dev.azure.com/acme/"}})
    }

    fn pr_event(event: &str, status: &str) -> Value {
        json!({
            "id": "ev-1", "publisherId": "tfs", "eventType": event,
            "resourceContainers": containers(),
            "resource": {"pullRequestId": 7, "status": status,
                         "repository": {"name": "web", "project": {"name": "Proj"}}}
        })
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn detection_requires_the_tfs_publisher() {
        let body = pr_event("git.pullrequest.created", "active").to_string();
        assert!(parse(body.as_bytes()).is_some());
        assert!(parse(br#"{"action": "opened"}"#).is_none());
        assert!(parse(br#"{"publisherId": "tfs"}"#).is_none());
        assert!(parse(b"not json").is_none());
    }

    #[test]
    fn pull_request_events() {
        let (ev, action, p) =
            translate(&pr_event("git.pullrequest.created", "active"), None).unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("pull_request", "opened"));
        assert_eq!(p["pull_request"]["number"], 7);
        assert_eq!(p["repository"]["full_name"], "acme/Proj/web");
        assert_eq!(p["wshm_delivery_id"], "ev-1");
        let (_, action, _) =
            translate(&pr_event("git.pullrequest.updated", "active"), None).unwrap();
        assert_eq!(action, "synchronize");
        for closed in ["completed", "abandoned"] {
            let (_, action, _) =
                translate(&pr_event("git.pullrequest.updated", closed), None).unwrap();
            assert_eq!(action, "closed");
        }
        assert!(translate(&pr_event("git.pullrequest.merged", "active"), None).is_none());
    }

    #[test]
    fn pr_comment_event_and_edits() {
        let mut p = json!({
            "id": "ev-2", "publisherId": "tfs",
            "eventType": "ms.vss-code.git-pullrequest-comment-event",
            "resourceContainers": containers(),
            "resource": {
                "comment": {"id": 3, "content": "/wshm review", "commentType": "text",
                            "author": {"uniqueName": "bob@x.io"},
                            "publishedDate": "2026-05-01T00:00:00Z",
                            "lastContentUpdatedDate": "2026-05-01T00:00:00Z"},
                "pullRequest": {"pullRequestId": 7,
                                "repository": {"name": "web", "project": {"name": "Proj"}}}
            }
        });
        let (ev, action, t) = translate(&p, None).unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("issue_comment", "created"));
        assert_eq!(t["issue"]["number"], 7);
        assert!(t["issue"].get("pull_request").is_some());
        assert_eq!(t["comment"]["body"], "/wshm review");
        assert_eq!(t["sender"]["login"], "bob@x.io");
        p["resource"]["comment"]["lastContentUpdatedDate"] = json!("2026-05-02T00:00:00Z");
        assert!(
            translate(&p, None).is_none(),
            "an edit is not a new command"
        );
        p["resource"]["comment"]["commentType"] = json!("system");
        assert!(translate(&p, None).is_none());
    }

    #[test]
    fn work_item_events() {
        let created = json!({
            "id": "ev-3", "publisherId": "tfs", "eventType": "workitem.created",
            "resourceContainers": {"collection": {"baseUrl": "https://tfs.corp/tfs/Main/"}},
            "resource": {"id": 42, "fields": {"System.TeamProject": "Proj"}}
        });
        let (ev, action, t) = translate(&created, None).unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("issues", "opened"));
        assert_eq!(t["issue"]["number"], 42);
        // Azure DevOps Server: the collection is the owner.
        assert_eq!(t["repository"]["full_name"], "Main/Proj");

        let commented = json!({
            "id": "ev-4", "publisherId": "tfs", "eventType": "workitem.commented",
            "resourceContainers": containers(),
            "resource": {"id": 42, "rev": 5, "fields": {
                "System.TeamProject": "Proj",
                "System.History": "<div>/wshm fix &amp; go</div>",
                "System.ChangedBy": "Bob <bob@x.io>"}}
        });
        let (ev, _, t) = translate(&commented, Some("acme/Proj/web")).unwrap();
        assert_eq!(ev, "issue_comment");
        assert_eq!(t["comment"]["body"], "/wshm fix & go");
        assert_eq!(t["comment"]["user"]["login"], "bob@x.io");
        assert_eq!(t["repository"]["full_name"], "acme/Proj/web");
        assert!(t["issue"].get("pull_request").is_none());
    }

    #[test]
    fn routing_by_project_and_repo() {
        let slugs: Vec<String> = [
            "acme/Proj/web",
            "acme/Proj/api",
            "acme/Solo",
            "acme/Other/x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (_, _, pr) = translate(&pr_event("git.pullrequest.created", "active"), None).unwrap();
        assert_eq!(route(&pr, slugs.iter()).as_deref(), Some("acme/Proj/web"));
        // Case-insensitive names.
        assert!(targets(&pr, "ACME/proj/WEB"));
        assert!(!targets(&pr, "acme/Proj/api"));

        // A repo named after its project, configured as `org/project`.
        let mut solo = pr_event("git.pullrequest.created", "active");
        solo["resource"]["repository"] = json!({"name": "Solo", "project": {"name": "Solo"}});
        let (_, _, solo) = translate(&solo, None).unwrap();
        assert_eq!(route(&solo, slugs.iter()).as_deref(), Some("acme/Solo"));

        // Work item of a project with two configured repos: ambiguous...
        let wi = json!({
            "id": "e", "publisherId": "tfs", "eventType": "workitem.created",
            "resourceContainers": containers(),
            "resource": {"id": 1, "fields": {"System.TeamProject": "Proj"}}
        });
        let (_, _, t) = translate(&wi, None).unwrap();
        assert_eq!(route(&t, slugs.iter()), None);
        // ...unless the subscription names the repo.
        let (_, _, t) = translate(&wi, Some("acme/Proj/api")).unwrap();
        assert_eq!(route(&t, slugs.iter()).as_deref(), Some("acme/Proj/api"));
        assert!(!targets(&t, "acme/Proj/web"));
        // One repo in the project: routed to it.
        let (_, _, t) = translate(&wi, None).unwrap();
        let one = ["acme/Proj/web".to_string()];
        assert_eq!(route(&t, one.iter()).as_deref(), Some("acme/Proj/web"));
    }

    #[test]
    fn secret_by_header_or_basic_password() {
        // Only this test touches this variable.
        std::env::set_var(SECRET_ENV, "az-secret");
        let payload = pr_event("git.pullrequest.created", "active");
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("anyone:az-secret")
        );
        for ok in [
            headers(&[("x-wshm-token", "az-secret")]),
            headers(&[("authorization", basic.as_str())]),
        ] {
            let (ev, action, _) = validate(&ok, payload.clone()).unwrap();
            assert_eq!((ev.as_str(), action.as_str()), ("pull_request", "opened"));
        }
        let wrong = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("az-secret:nope")
        );
        for bad in [
            headers(&[]),
            headers(&[("x-wshm-token", "nope")]),
            headers(&[("authorization", wrong.as_str())]),
            headers(&[("authorization", "Bearer az-secret")]),
        ] {
            let err = validate(&bad, payload.clone()).unwrap_err();
            assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        }
        // Unhandled events authenticate, then come back as `azure:*`.
        let other = json!({"publisherId": "tfs", "eventType": "build.complete",
                           "resourceContainers": containers(), "resource": {}});
        let (ev, _, _) = validate(&headers(&[("x-wshm-token", "az-secret")]), other).unwrap();
        assert_eq!(ev, "azure:build.complete");
    }
}
