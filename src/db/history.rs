//! Append-only change history for issues and pull requests.
//!
//! Every sync overwrites the cached row (`issues.labels`, `pull_requests.*`,
//! …) so the store only ever knew the *current* snapshot. `change_events`
//! records each observed mutation instead, so a maintainer can answer
//! "when did `priority:high` land on #42, and was it wshm or a human?"
//! without diffing against the forge UI.
//!
//! Capture sites:
//! - [`crate::db::issues::upsert_issue`] / [`crate::db::pulls::upsert_pull`]
//!   diff the previous row against the incoming one (`source = "sync"`, or
//!   `"webhook"` when the write runs inside [`scoped`]`(SOURCE_WEBHOOK, ..)`).
//! - [`crate::db::Database::merge_issue_labels`] is the DB reflection of
//!   wshm's own label writes → `source = "wshm"`.
//!
//! First-seen rows deliberately produce no event: the initial sync of a
//! busy repo would otherwise flood the table with thousands of "created"
//! rows that carry no information the `issues` table doesn't already hold.
//!
//! Retention is enforced by the scheduler through
//! [`crate::db::backend::DatabaseBackend::cleanup_old_change_events`]
//! (`[daemon].history_retention_days`, default 90).

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

/// One observed mutation of a single field on an issue or PR.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeEvent {
    /// Row id; `0` before insertion.
    #[serde(default)]
    pub id: i64,
    /// `"issue"` or `"pr"`.
    pub kind: String,
    pub number: u64,
    /// `title` | `body` | `state` | `labels` | `head_sha` | `base_ref` | `ci_status`
    pub field: String,
    /// Previous value (JSON array for `labels`), `None` when unknown.
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    /// `sync` | `webhook` | `wshm`
    pub source: String,
    /// Forge login when known (wshm's bot login for `source = "wshm"`).
    pub actor: Option<String>,
    /// RFC 3339.
    pub observed_at: String,
}

/// Filter for [`list_change_events`]; every field is optional and ANDed.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChangeEventFilter {
    pub kind: Option<String>,
    pub number: Option<u64>,
    pub field: Option<String>,
    pub source: Option<String>,
    /// Inclusive lower bound on `observed_at` (RFC 3339).
    pub since: Option<String>,
    /// Exclusive upper bound on `observed_at` (RFC 3339).
    pub until: Option<String>,
}

pub const SOURCE_SYNC: &str = "sync";
pub const SOURCE_WEBHOOK: &str = "webhook";
pub const SOURCE_WSHM: &str = "wshm";

tokio::task_local! {
    /// Source tag for events recorded inside the current task. A Tokio
    /// task-local (not a thread-local) so the tag survives `.await`s and
    /// work-stealing: the webhook processor wraps a whole handler, and the
    /// batch upsert deep inside it still reads "webhook".
    static SOURCE: &'static str;
}

/// Run `fut` with every change event it records tagged `source`.
pub async fn scoped<F: std::future::Future>(source: &'static str, fut: F) -> F::Output {
    SOURCE.scope(source, fut).await
}

/// Synchronous variant of [`scoped`] for non-async code paths.
pub fn scoped_sync<T>(source: &'static str, f: impl FnOnce() -> T) -> T {
    SOURCE.sync_scope(source, f)
}

/// `"sync"` unless the caller opted into another tag via [`scoped`].
pub fn current_source() -> &'static str {
    SOURCE.try_with(|s| *s).unwrap_or(SOURCE_SYNC)
}

fn labels_json(labels: &[String]) -> String {
    let mut sorted: Vec<&String> = labels.iter().collect();
    sorted.sort();
    serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".to_string())
}

fn event(
    kind: &str,
    number: u64,
    field: &str,
    old: Option<String>,
    new: Option<String>,
) -> ChangeEvent {
    ChangeEvent {
        id: 0,
        kind: kind.to_string(),
        number,
        field: field.to_string(),
        old_value: old,
        new_value: new,
        source: current_source().to_string(),
        actor: None,
        observed_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn push_if_changed(
    out: &mut Vec<ChangeEvent>,
    kind: &str,
    number: u64,
    field: &str,
    old: Option<&str>,
    new: Option<&str>,
) {
    if old != new {
        out.push(event(
            kind,
            number,
            field,
            old.map(str::to_string),
            new.map(str::to_string),
        ));
    }
}

/// Events describing what changed between the cached `before` row and the
/// incoming `after` row. Empty when `before` is `None` (first sighting).
pub fn diff_issue(before: Option<&Issue>, after: &Issue) -> Vec<ChangeEvent> {
    let Some(b) = before else { return Vec::new() };
    let mut out = Vec::new();
    let n = after.number;
    push_if_changed(
        &mut out,
        "issue",
        n,
        "title",
        Some(&b.title),
        Some(&after.title),
    );
    push_if_changed(
        &mut out,
        "issue",
        n,
        "body",
        b.body.as_deref(),
        after.body.as_deref(),
    );
    push_if_changed(
        &mut out,
        "issue",
        n,
        "state",
        Some(&b.state),
        Some(&after.state),
    );
    let (ol, nl) = (labels_json(&b.labels), labels_json(&after.labels));
    push_if_changed(&mut out, "issue", n, "labels", Some(&ol), Some(&nl));
    out
}

/// Same as [`diff_issue`] for pull requests; also tracks pushes
/// (`head_sha`), retargeting (`base_ref`) and CI transitions.
pub fn diff_pull(before: Option<&PullRequest>, after: &PullRequest) -> Vec<ChangeEvent> {
    let Some(b) = before else { return Vec::new() };
    let mut out = Vec::new();
    let n = after.number;
    push_if_changed(
        &mut out,
        "pr",
        n,
        "title",
        Some(&b.title),
        Some(&after.title),
    );
    push_if_changed(
        &mut out,
        "pr",
        n,
        "body",
        b.body.as_deref(),
        after.body.as_deref(),
    );
    push_if_changed(
        &mut out,
        "pr",
        n,
        "state",
        Some(&b.state),
        Some(&after.state),
    );
    let (ol, nl) = (labels_json(&b.labels), labels_json(&after.labels));
    push_if_changed(&mut out, "pr", n, "labels", Some(&ol), Some(&nl));
    push_if_changed(
        &mut out,
        "pr",
        n,
        "head_sha",
        b.head_sha.as_deref(),
        after.head_sha.as_deref(),
    );
    push_if_changed(
        &mut out,
        "pr",
        n,
        "base_ref",
        b.base_ref.as_deref(),
        after.base_ref.as_deref(),
    );
    push_if_changed(
        &mut out,
        "pr",
        n,
        "ci_status",
        b.ci_status.as_deref(),
        after.ci_status.as_deref(),
    );
    out
}

/// Event for a label set rewritten by wshm itself (`merge_issue_labels`).
/// `None` when nothing actually changed.
pub fn label_merge_event(number: u64, before: &[String], after: &[String]) -> Option<ChangeEvent> {
    let (ol, nl) = (labels_json(before), labels_json(after));
    if ol == nl {
        return None;
    }
    let mut ev = event("issue", number, "labels", Some(ol), Some(nl));
    ev.source = SOURCE_WSHM.to_string();
    Some(ev)
}

// ── SQLite ──────────────────────────────────────────────────────────────

pub fn append_change_events(conn: &Connection, events: &[ChangeEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached(
        "INSERT INTO change_events (kind, number, field, old_value, new_value, source, actor, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for e in events {
        stmt.execute(params![
            e.kind,
            e.number,
            e.field,
            e.old_value,
            e.new_value,
            e.source,
            e.actor,
            e.observed_at,
        ])?;
    }
    Ok(())
}

/// Newest first. Returns `(page, total_matching)`.
pub fn list_change_events(
    conn: &Connection,
    filter: &ChangeEventFilter,
    limit: usize,
    offset: usize,
) -> Result<(Vec<ChangeEvent>, u64)> {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut bind = |clause: &str, value: Box<dyn rusqlite::ToSql>| {
        args.push(value);
        clauses.push(format!("{clause} ?{}", args.len()));
    };
    if let Some(k) = &filter.kind {
        bind("kind =", Box::new(k.clone()));
    }
    if let Some(n) = filter.number {
        bind("number =", Box::new(n));
    }
    if let Some(f) = &filter.field {
        bind("field =", Box::new(f.clone()));
    }
    if let Some(s) = &filter.source {
        bind("source =", Box::new(s.clone()));
    }
    if let Some(s) = &filter.since {
        bind("observed_at >=", Box::new(s.clone()));
    }
    if let Some(u) = &filter.until {
        bind("observed_at <", Box::new(u.clone()));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();

    let total: u64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM change_events{where_sql}"),
        params_ref.as_slice(),
        |r| r.get(0),
    )?;

    let sql = format!(
        "SELECT id, kind, number, field, old_value, new_value, source, actor, observed_at
         FROM change_events{where_sql}
         ORDER BY observed_at DESC, id DESC
         LIMIT {limit} OFFSET {offset}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_ref.as_slice(), |r| {
        Ok(ChangeEvent {
            id: r.get(0)?,
            kind: r.get(1)?,
            number: r.get::<_, i64>(2)? as u64,
            field: r.get(3)?,
            old_value: r.get(4)?,
            new_value: r.get(5)?,
            source: r.get(6)?,
            actor: r.get(7)?,
            observed_at: r.get(8)?,
        })
    })?;
    let items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((items, total))
}

/// Delete events older than `days`. Returns the number of rows removed.
pub fn cleanup_old_change_events(conn: &Connection, days: u32) -> Result<u64> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
    let deleted = conn.execute(
        "DELETE FROM change_events WHERE observed_at < ?1",
        params![cutoff],
    )?;
    Ok(deleted as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::backend::DatabaseBackend;
    use crate::db::Database;

    fn issue(number: u64, title: &str, labels: &[&str], state: &str) -> Issue {
        Issue {
            number,
            title: title.to_string(),
            body: Some("body".to_string()),
            state: state.to_string(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            author: Some("alice".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            reactions_plus1: 0,
            reactions_total: 0,
        }
    }

    #[test]
    fn first_sighting_produces_no_event() {
        assert!(diff_issue(None, &issue(1, "t", &["bug"], "open")).is_empty());
    }

    #[test]
    fn diff_reports_only_changed_fields_with_sorted_labels() {
        let before = issue(1, "t", &["bug", "a"], "open");
        let after = issue(1, "t2", &["a", "bug"], "closed");
        let events = diff_issue(Some(&before), &after);
        let fields: Vec<&str> = events.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["title", "state"]);
        assert_eq!(events[0].old_value.as_deref(), Some("t"));
        assert_eq!(events[0].new_value.as_deref(), Some("t2"));
        assert_eq!(events[0].source, SOURCE_SYNC);
    }

    #[test]
    fn source_scope_tags_and_restores() {
        assert_eq!(current_source(), SOURCE_SYNC);
        let ev = scoped_sync(SOURCE_WEBHOOK, || {
            diff_issue(
                Some(&issue(1, "a", &[], "open")),
                &issue(1, "b", &[], "open"),
            )
        });
        assert_eq!(ev[0].source, SOURCE_WEBHOOK);
        assert_eq!(current_source(), SOURCE_SYNC);
    }

    #[test]
    fn upsert_and_label_merge_are_captured_and_listed() {
        let db = Database::open_memory().unwrap();
        db.upsert_issue(&issue(7, "first", &["bug"], "open"))
            .unwrap();
        db.upsert_issue(&issue(7, "renamed", &["bug"], "open"))
            .unwrap();
        db.merge_issue_labels(7, &["priority:high".to_string()], &[])
            .unwrap();
        // No-op merge must not record anything.
        db.merge_issue_labels(7, &["priority:high".to_string()], &[])
            .unwrap();

        let (all, total) = db
            .list_change_events(&ChangeEventFilter::default(), 50, 0)
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(all.len(), 2);
        // Newest first: the wshm label merge, then the sync rename.
        assert_eq!(all[0].field, "labels");
        assert_eq!(all[0].source, SOURCE_WSHM);
        assert_eq!(all[0].old_value.as_deref(), Some("[\"bug\"]"));
        assert_eq!(
            all[0].new_value.as_deref(),
            Some("[\"bug\",\"priority:high\"]")
        );
        assert_eq!(all[1].field, "title");
        assert_eq!(all[1].source, SOURCE_SYNC);

        let filter = ChangeEventFilter {
            source: Some(SOURCE_WSHM.to_string()),
            ..Default::default()
        };
        let (only_wshm, total) = db.list_change_events(&filter, 50, 0).unwrap();
        assert_eq!((only_wshm.len(), total), (1, 1));

        let filter = ChangeEventFilter {
            number: Some(8),
            ..Default::default()
        };
        assert_eq!(db.list_change_events(&filter, 50, 0).unwrap().1, 0);
    }

    #[test]
    fn cleanup_removes_only_old_rows() {
        let db = Database::open_memory().unwrap();
        db.with_conn(|conn| {
            let mut old = event("issue", 1, "title", None, Some("x".into()));
            old.observed_at = "2020-01-01T00:00:00Z".to_string();
            let fresh = event("issue", 1, "title", None, Some("y".into()));
            append_change_events(conn, &[old, fresh])
        })
        .unwrap();
        assert_eq!(db.cleanup_old_change_events(90).unwrap(), 1);
        assert_eq!(
            db.list_change_events(&ChangeEventFilter::default(), 10, 0)
                .unwrap()
                .1,
            1
        );
    }
}
