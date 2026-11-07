//! Azure DevOps service hooks (the "Web Hooks" consumer), received on the
//! same `/webhook` endpoint as GitHub/GitLab deliveries.
//!
//! Azure sends no event header and no signature: a delivery is recognised
//! by its JSON (`publisherId = "tfs"`) and authenticated by a shared secret
//! configured on the subscription, either as the Basic authentication
//! password (any user name) or as an `X-Wshm-Token: <secret>` HTTP header.
//! The secret is visible to the project administrators who manage service
//! hooks, so it is never the GitHub HMAC secret: secret store key
//! [`AZURE_WEBHOOK_SECRET_KEY`] (scope of the target repo, then global) or
//! `WSHM_AZURE_DEVOPS_WEBHOOK_SECRET`. There is no unsigned mode.
//!
//! Order of the checks: unhandled events are answered "ignored" (nothing
//! is processed), then the delivery is routed to one configured
//! azure-devops repo (unroutable: rejected before any secret is looked
//! at), then authenticated with that repo's secret.
//!
//! Events are translated to the GitHub shapes the processor handles:
//! `workitem.created` → `issues.opened`, `workitem.commented` and the PR
//! comment event → `issue_comment.created`, `git.pullrequest.created` →
//! `pull_request.opened`, `git.pullrequest.updated` → `.synchronize`
//! (active) or `.closed` (completed / abandoned). Comment events carry the
//! comment ids only for the processor to read the comment back from the
//! API (`wshm_verify_comment`): the author and text of a payload are never
//! trusted. https://learn.microsoft.com/azure/devops/service-hooks/events

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::db::backend::DatabaseBackend;

/// Secret-store key of the service hook secret.
pub const AZURE_WEBHOOK_SECRET_KEY: &str = "azure_devops_webhook_secret";
const SECRET_ENV: &str = "WSHM_AZURE_DEVOPS_WEBHOOK_SECRET";
/// Custom header carrying the secret (alternative to Basic auth).
const TOKEN_HEADER: &str = "x-wshm-token";
/// Optional custom header naming the target repo (`org/project/repo`) of
/// work item events, which carry a project but no repository.
const REPO_HEADER: &str = "x-wshm-repo";

/// The payload, when the request is an Azure DevOps service hook
/// delivery: no GitHub/GitLab event header, JSON from the `tfs` publisher.
pub fn parse(headers: &HeaderMap, body: &[u8]) -> Option<Value> {
    if headers.contains_key("x-github-event") || headers.contains_key("x-gitlab-event") {
        return None;
    }
    let v: Value = serde_json::from_slice(body).ok()?;
    (v["publisherId"] == "tfs" && v["eventType"].is_string()).then_some(v)
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

/// Route, authenticate and translate a delivery recognised by [`parse`].
/// `slugs` are the configured repos whose provider is azure-devops.
/// Returns (event_type, action, payload) with `repository.full_name` set
/// to the routed slug; every other outcome is the response to send.
#[allow(clippy::result_large_err)]
pub fn validate(
    headers: &HeaderMap,
    payload: Value,
    slugs: &[String],
) -> Result<(String, String, Value), Response> {
    let event = payload["eventType"].as_str().unwrap_or("").to_string();
    let repo_header = headers
        .get(REPO_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    // Unhandled events: nothing is processed, so nothing to authenticate,
    // and a 200 keeps Azure from disabling the subscription.
    let Some((event_type, action, mut translated)) = translate(&payload, repo_header) else {
        info!("Ignoring Azure DevOps event: {event}");
        return Err(reply(
            StatusCode::OK,
            json!({"status": "ignored", "event": format!("azure:{event}")}),
        ));
    };
    if translated["wshm_delivery_id"]
        .as_str()
        .is_none_or(|s| s.is_empty())
    {
        warn!("Rejecting Azure DevOps delivery without an event id");
        return Err(reply(
            StatusCode::BAD_REQUEST,
            json!({"error": "missing event id"}),
        ));
    }
    let Some(slug) = route(&translated, slugs.iter()) else {
        let claimed = translated["repository"]["full_name"].as_str().unwrap_or("");
        info!("Ignoring Azure DevOps delivery for unconfigured repo: {claimed}");
        return Err(reply(
            StatusCode::NOT_FOUND,
            json!({"error": "repo not configured", "repo": claimed}),
        ));
    };
    let expected = crate::secrets::resolve(
        crate::secrets::global().as_ref(),
        Some(&slug),
        AZURE_WEBHOOK_SECRET_KEY,
        SECRET_ENV,
    )
    .filter(|t| !t.trim().is_empty());
    let Some(expected) = expected else {
        warn!(
            "Rejecting Azure DevOps service hook for {slug}: no secret configured (secret \
             `{AZURE_WEBHOOK_SECRET_KEY}` or {SECRET_ENV})"
        );
        return Err(reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "azure devops webhook secret not configured"}),
        ));
    };
    if !presented_secrets(headers)
        .iter()
        .any(|given| super::server::constant_time_eq(&expected, given))
    {
        warn!("Invalid Azure DevOps service hook credentials for {slug}");
        return Err(reply(
            StatusCode::UNAUTHORIZED,
            json!({"error": "invalid credentials"}),
        ));
    }
    translated["repository"]["full_name"] = json!(slug);
    Ok((event_type, action, translated))
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

/// A `git.pullrequest.updated` whose head is the one already cached:
/// votes, reviewers, labels or description changed, not the code. Dropped
/// so such changes never re-run the analysis.
pub fn is_unchanged_push(payload: &Value, db: &dyn DatabaseBackend) -> bool {
    if payload.get("wshm_azure").is_none() || payload["action"] != "synchronize" {
        return false;
    }
    let (Some(number), Some(head)) = (
        payload["pull_request"]["number"].as_u64(),
        payload["pull_request"]["head"]["sha"].as_str(),
    ) else {
        return false;
    };
    db.get_pull(number)
        .ok()
        .flatten()
        .and_then(|p| p.head_sha)
        .is_some_and(|cached| cached.eq_ignore_ascii_case(head))
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

/// Thread id of a PR comment, from its `_links` (`…/threads/{id}/…`).
fn thread_id(c: &Value) -> Option<u64> {
    ["self", "threads"].iter().find_map(|k| {
        let href = c["_links"][k]["href"].as_str()?;
        let rest = href.split("/threads/").nth(1)?;
        rest.split(['/', '?']).next()?.parse().ok()
    })
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
            let project = r["fields"]["System.TeamProject"].as_str()?;
            // The comment is read back by id: without one it cannot be
            // verified, so the event is not handled.
            let comment = r["commentVersionRef"]["commentId"].as_u64()?;
            Some((
                "issue_comment".into(),
                "created".into(),
                with(
                    target(project, None),
                    json!({
                        "action": "created",
                        "issue": {"number": number},
                        "comment": {"id": comment, "body": "", "user": {"login": ""}},
                        "sender": {"login": ""},
                        "wshm_verify_comment": {"issue": number, "id": comment},
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
                // Subscribe with the "Source branch updated" filter; an
                // update whose head is already cached is dropped anyway
                // (see is_unchanged_push).
                (_, Some("active")) => "synchronize",
                _ => return None,
            };
            let mut pr = json!({"number": number});
            if let Some(sha) = r["lastMergeSourceCommit"]["commitId"].as_str() {
                pr["head"] = json!({"sha": sha});
            }
            Some((
                "pull_request".into(),
                action.into(),
                with(
                    target(project, Some(repo)),
                    json!({"action": action, "pull_request": pr}),
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
            let id = c["id"].as_u64()?;
            let thread = thread_id(c)?;
            Some((
                "issue_comment".into(),
                "created".into(),
                with(
                    target(project, Some(repo)),
                    json!({
                        "action": "created",
                        "issue": {"number": number, "pull_request": {}},
                        "comment": {"id": id, "body": "", "user": {"login": ""}},
                        "sender": {"login": ""},
                        "wshm_verify_comment": {"pr": number, "thread": thread, "id": id},
                    }),
                ),
            ))
        }
        _ => None,
    }
}

/// The comment a translated payload asks the processor to read back.
pub fn comment_to_verify(payload: &Value) -> Option<crate::git_provider::CommentRef> {
    use crate::git_provider::{CommentRef, Thread};
    let v = payload.get("wshm_verify_comment")?;
    let id = v["id"].as_u64()?;
    if let Some(pr) = v["pr"].as_u64() {
        return Some(CommentRef {
            thread: Thread::Pull(pr),
            id,
            parent_id: Some(v["thread"].as_u64()?),
        });
    }
    Some(CommentRef {
        thread: Thread::Issue(v["issue"].as_u64()?),
        id,
        parent_id: None,
    })
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
                         "lastMergeSourceCommit": {"commitId": "h2"},
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

    fn slugs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detection_requires_the_tfs_publisher_and_no_forge_header() {
        let body = pr_event("git.pullrequest.created", "active").to_string();
        assert!(parse(&headers(&[]), body.as_bytes()).is_some());
        assert!(parse(&headers(&[("x-github-event", "push")]), body.as_bytes()).is_none());
        assert!(parse(&headers(&[("x-gitlab-event", "x")]), body.as_bytes()).is_none());
        assert!(parse(&headers(&[]), br#"{"action": "opened"}"#).is_none());
        assert!(parse(&headers(&[]), br#"{"publisherId": "tfs"}"#).is_none());
        assert!(parse(&headers(&[]), b"not json").is_none());
    }

    #[test]
    fn pull_request_events() {
        let (ev, action, p) =
            translate(&pr_event("git.pullrequest.created", "active"), None).unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("pull_request", "opened"));
        assert_eq!(p["pull_request"]["number"], 7);
        assert_eq!(p["pull_request"]["head"]["sha"], "h2");
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
    fn unchanged_head_update_is_dropped() {
        let db = crate::Database::open_memory().unwrap();
        let (_, _, p) = translate(&pr_event("git.pullrequest.updated", "active"), None).unwrap();
        // Not cached yet: processed.
        assert!(!is_unchanged_push(&p, &db));
        let mut pr = crate::db::pulls::PullRequest {
            number: 7,
            title: "t".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            author: None,
            head_sha: Some("h1".into()),
            base_sha: None,
            head_ref: None,
            base_ref: None,
            mergeable: None,
            ci_status: None,
            created_at: String::new(),
            updated_at: String::new(),
            review_decision: None,
            review_decision_at: None,
            reactions_plus1: 0,
        };
        db.upsert_pull(&pr).unwrap();
        // A push (new head): processed.
        assert!(!is_unchanged_push(&p, &db));
        pr.head_sha = Some("H2".into());
        db.upsert_pull(&pr).unwrap();
        // Same head (a vote, a reviewer, a label): dropped.
        assert!(is_unchanged_push(&p, &db));
        // Only Azure synchronize events are concerned.
        let (_, _, opened) =
            translate(&pr_event("git.pullrequest.created", "active"), None).unwrap();
        assert!(!is_unchanged_push(&opened, &db));
        let github =
            json!({"action": "synchronize", "pull_request": {"number": 7, "head": {"sha": "H2"}}});
        assert!(!is_unchanged_push(&github, &db));
    }

    #[test]
    fn pr_comment_event_carries_ids_not_trusted_text() {
        let mut p = json!({
            "id": "ev-2", "publisherId": "tfs",
            "eventType": "ms.vss-code.git-pullrequest-comment-event",
            "resourceContainers": containers(),
            "resource": {
                "comment": {"id": 3, "content": "/wshm fix", "commentType": "text",
                            "author": {"uniqueName": "admin@x.io"},
                            "publishedDate": "2026-05-01T00:00:00Z",
                            "lastContentUpdatedDate": "2026-05-01T00:00:00Z",
                            "_links": {"self": {"href": "https://dev.azure.com/acme/_apis/git/repositories/r/pullRequests/7/threads/41/comments/3"}}},
                "pullRequest": {"pullRequestId": 7,
                                "repository": {"name": "web", "project": {"name": "Proj"}}}
            }
        });
        let (ev, action, t) = translate(&p, None).unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("issue_comment", "created"));
        assert_eq!(t["issue"]["number"], 7);
        assert!(t["issue"].get("pull_request").is_some());
        // Author and text come from the API, never from the payload.
        assert_eq!(t["comment"]["body"], "");
        assert_eq!(t["sender"]["login"], "");
        let c = comment_to_verify(&t).unwrap();
        assert_eq!(c.thread, crate::git_provider::Thread::Pull(7));
        assert_eq!((c.id, c.parent_id), (3, Some(41)));

        p["resource"]["comment"]["lastContentUpdatedDate"] = json!("2026-05-02T00:00:00Z");
        assert!(
            translate(&p, None).is_none(),
            "an edit is not a new command"
        );
        p["resource"]["comment"]["lastContentUpdatedDate"] = json!("2026-05-01T00:00:00Z");
        p["resource"]["comment"]["_links"] = json!({});
        assert!(translate(&p, None).is_none(), "no thread id: cannot verify");
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

        let mut commented = json!({
            "id": "ev-4", "publisherId": "tfs", "eventType": "workitem.commented",
            "resourceContainers": containers(),
            "resource": {"id": 42, "rev": 5, "commentVersionRef": {"commentId": 77},
                "fields": {"System.TeamProject": "Proj",
                           "System.History": "<div>/wshm fix</div>",
                           "System.ChangedBy": "Admin <admin@x.io>"}}
        });
        let (ev, _, t) = translate(&commented, Some("acme/Proj/web")).unwrap();
        assert_eq!(ev, "issue_comment");
        assert_eq!(t["comment"]["body"], "");
        assert_eq!(t["repository"]["full_name"], "acme/Proj/web");
        assert!(t["issue"].get("pull_request").is_none());
        let c = comment_to_verify(&t).unwrap();
        assert_eq!(c.thread, crate::git_provider::Thread::Issue(42));
        assert_eq!((c.id, c.parent_id), (77, None));
        commented["resource"]
            .as_object_mut()
            .unwrap()
            .remove("commentVersionRef");
        assert!(translate(&commented, None).is_none(), "no comment id");
    }

    #[test]
    fn routing_by_project_and_repo() {
        let slugs = slugs(&[
            "acme/Proj/web",
            "acme/Proj/api",
            "acme/Solo",
            "acme/Other/x",
        ]);
        let (_, _, pr) = translate(&pr_event("git.pullrequest.created", "active"), None).unwrap();
        assert_eq!(route(&pr, slugs.iter()).as_deref(), Some("acme/Proj/web"));
        assert!(targets(&pr, "ACME/proj/WEB"));
        assert!(!targets(&pr, "acme/Proj/api"));

        let mut solo = pr_event("git.pullrequest.created", "active");
        solo["resource"]["repository"] = json!({"name": "Solo", "project": {"name": "Solo"}});
        let (_, _, solo) = translate(&solo, None).unwrap();
        assert_eq!(route(&solo, slugs.iter()).as_deref(), Some("acme/Solo"));

        let wi = json!({
            "id": "e", "publisherId": "tfs", "eventType": "workitem.created",
            "resourceContainers": containers(),
            "resource": {"id": 1, "fields": {"System.TeamProject": "Proj"}}
        });
        let (_, _, t) = translate(&wi, None).unwrap();
        assert_eq!(route(&t, slugs.iter()), None);
        let (_, _, t) = translate(&wi, Some("acme/Proj/api")).unwrap();
        assert_eq!(route(&t, slugs.iter()).as_deref(), Some("acme/Proj/api"));
        assert!(!targets(&t, "acme/Proj/web"));
        let (_, _, t) = translate(&wi, None).unwrap();
        let one = ["acme/Proj/web".to_string()];
        assert_eq!(route(&t, one.iter()).as_deref(), Some("acme/Proj/web"));
    }

    #[test]
    fn validate_ignores_routes_then_authenticates() {
        // Only this test touches this variable.
        std::env::set_var(SECRET_ENV, "az-secret");
        let repos = slugs(&["acme/Proj/web"]);
        let payload = pr_event("git.pullrequest.created", "active");
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("anyone:az-secret")
        );
        for ok in [
            headers(&[("x-wshm-token", "az-secret")]),
            headers(&[("authorization", basic.as_str())]),
        ] {
            let (ev, action, p) = validate(&ok, payload.clone(), &repos).unwrap();
            assert_eq!((ev.as_str(), action.as_str()), ("pull_request", "opened"));
            assert_eq!(p["repository"]["full_name"], "acme/Proj/web");
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
            let err = validate(&bad, payload.clone(), &repos).unwrap_err();
            assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        }
        // Unroutable: rejected before any secret is accepted, even a valid one.
        let good = headers(&[("x-wshm-token", "az-secret")]);
        let err = validate(&good, payload.clone(), &slugs(&["acme/Proj/api"])).unwrap_err();
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        let err = validate(&good, payload.clone(), &[]).unwrap_err();
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        // Unhandled events: 200 "ignored", whatever the credentials.
        let other = json!({"publisherId": "tfs", "eventType": "build.complete",
                           "resourceContainers": containers(), "resource": {}});
        let r = validate(&headers(&[]), other, &repos).unwrap_err();
        assert_eq!(r.status(), StatusCode::OK);
        // A handled event without an event id cannot be deduplicated.
        let mut no_id = payload.clone();
        no_id.as_object_mut().unwrap().remove("id");
        let r = validate(&good, no_id, &repos).unwrap_err();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }
}
