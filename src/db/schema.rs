use anyhow::Result;
use rusqlite::Connection;

pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS issues (
            number      INTEGER PRIMARY KEY,
            title       TEXT NOT NULL,
            body        TEXT,
            state       TEXT NOT NULL DEFAULT 'open',
            labels      TEXT NOT NULL DEFAULT '[]',
            author      TEXT,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS pull_requests (
            number      INTEGER PRIMARY KEY,
            title       TEXT NOT NULL,
            body        TEXT,
            state       TEXT NOT NULL DEFAULT 'open',
            labels      TEXT NOT NULL DEFAULT '[]',
            author      TEXT,
            head_sha    TEXT,
            base_sha    TEXT,
            head_ref    TEXT,
            base_ref    TEXT,
            mergeable   INTEGER,
            ci_status   TEXT,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS comments (
            id            INTEGER PRIMARY KEY,
            issue_number  INTEGER NOT NULL,
            body          TEXT NOT NULL,
            author        TEXT,
            created_at    TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS labels (
            name        TEXT PRIMARY KEY,
            color       TEXT,
            description TEXT
        );

        CREATE TABLE IF NOT EXISTS triage_results (
            issue_number    INTEGER PRIMARY KEY,
            category        TEXT NOT NULL,
            confidence      REAL NOT NULL,
            priority        TEXT,
            summary         TEXT,
            suggested_labels TEXT NOT NULL DEFAULT '[]',
            is_duplicate_of INTEGER,
            is_simple_fix   INTEGER NOT NULL DEFAULT 0,
            relevant_files  TEXT NOT NULL DEFAULT '[]',
            acted_at        TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS pr_analyses (
            pr_number     INTEGER PRIMARY KEY,
            summary       TEXT NOT NULL,
            risk_level    TEXT NOT NULL,
            pr_type       TEXT NOT NULL,
            review_notes  TEXT,
            analyzed_at   TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sync_log (
            table_name     TEXT PRIMARY KEY,
            last_synced_at TEXT NOT NULL,
            etag           TEXT
        );

        CREATE TABLE IF NOT EXISTS webhook_events (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            event_type   TEXT NOT NULL,
            action       TEXT NOT NULL,
            number       INTEGER,
            payload      TEXT NOT NULL,
            status       TEXT NOT NULL DEFAULT 'pending',
            error        TEXT,
            received_at  TEXT NOT NULL,
            processed_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_issues_state ON issues(state);
        CREATE INDEX IF NOT EXISTS idx_pulls_state ON pull_requests(state);
        CREATE INDEX IF NOT EXISTS idx_comments_issue ON comments(issue_number);
        CREATE INDEX IF NOT EXISTS idx_webhook_status ON webhook_events(status);
        CREATE INDEX IF NOT EXISTS idx_triage_acted_at ON triage_results(acted_at);
        ",
    )?;

    // Migration: add reactions columns to issues
    let has_reactions: bool = conn
        .prepare("SELECT reactions_plus1 FROM issues LIMIT 0")
        .is_ok();
    if !has_reactions {
        conn.execute_batch(
            "
            ALTER TABLE issues ADD COLUMN reactions_plus1 INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE issues ADD COLUMN reactions_total INTEGER NOT NULL DEFAULT 0;
            ",
        )?;
    }

    // Migration: add content_hash to triage_results and pr_analyses (for LLM call deduplication)
    let has_triage_hash: bool = conn
        .prepare("SELECT content_hash FROM triage_results LIMIT 0")
        .is_ok();
    if !has_triage_hash {
        conn.execute_batch("ALTER TABLE triage_results ADD COLUMN content_hash TEXT;")?;
    }

    let has_pr_hash: bool = conn
        .prepare("SELECT content_hash FROM pr_analyses LIMIT 0")
        .is_ok();
    if !has_pr_hash {
        conn.execute_batch("ALTER TABLE pr_analyses ADD COLUMN content_hash TEXT;")?;
    }

    Ok(())
}

/// Compute a stable content hash for an issue (title + body + sorted labels).
/// Used to skip LLM re-analysis when content hasn't changed.
pub fn compute_issue_hash(title: &str, body: Option<&str>, labels: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update(b"\n");
    hasher.update(body.unwrap_or("").as_bytes());
    hasher.update(b"\n");
    let mut sorted_labels: Vec<&String> = labels.iter().collect();
    sorted_labels.sort();
    for label in sorted_labels {
        hasher.update(label.as_bytes());
        hasher.update(b",");
    }
    format!("{:x}", hasher.finalize())
}

/// Compute a stable content hash for a PR.
pub fn compute_pr_hash(
    title: &str,
    body: Option<&str>,
    head_sha: Option<&str>,
    labels: &[String],
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update(b"\n");
    hasher.update(body.unwrap_or("").as_bytes());
    hasher.update(b"\n");
    hasher.update(head_sha.unwrap_or("").as_bytes());
    hasher.update(b"\n");
    let mut sorted_labels: Vec<&String> = labels.iter().collect();
    sorted_labels.sort();
    for label in sorted_labels {
        hasher.update(label.as_bytes());
        hasher.update(b",");
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // should not fail
    }
}
