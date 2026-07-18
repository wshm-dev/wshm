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

    // Composite index covering the WHERE state='open' filter + the
    // ORDER BY reactions_total DESC used by get_issues_needing_triage and
    // get_untriaged_issues, so the scan no longer forces a full-table sort.
    // Created here (after the reactions columns exist) rather than in the
    // initial batch above, where reactions_total may not yet be present.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_issues_state_reactions
         ON issues(state, reactions_total DESC);",
    )?;

    // Migration: add review state to pull_requests (backs the "To Validate"
    // review-radar view: ready-to-merge / awaiting-re-review buckets).
    let has_review_decision: bool = conn
        .prepare("SELECT review_decision FROM pull_requests LIMIT 0")
        .is_ok();
    if !has_review_decision {
        conn.execute_batch(
            "ALTER TABLE pull_requests ADD COLUMN review_decision TEXT;
             ALTER TABLE pull_requests ADD COLUMN review_decision_at TEXT;",
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

    // One-shot migration: recompute every existing triage_results.content_hash
    // with the new label-free formula. Without this, an upgrading pod would
    // see hash mismatches on every previously-triaged issue and burn AI
    // credits re-triaging them all on first boot.
    let triage_hash_v2_done: bool = conn
        .query_row(
            "SELECT 1 FROM sync_log WHERE table_name = '__triage_hash_v2_migrated'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !triage_hash_v2_done {
        let tx = conn.unchecked_transaction()?;
        {
            let mut select = tx.prepare(
                "SELECT t.issue_number, i.title, i.body
                 FROM triage_results t JOIN issues i ON t.issue_number = i.number
                 WHERE t.content_hash IS NOT NULL",
            )?;
            let rows: Vec<(i64, String, Option<String>)> = select
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut update =
                tx.prepare("UPDATE triage_results SET content_hash = ?1 WHERE issue_number = ?2")?;
            for (issue_number, title, body) in rows {
                let new_hash = compute_issue_hash(&title, body.as_deref());
                update.execute(rusqlite::params![new_hash, issue_number])?;
            }
        }
        tx.execute(
            "INSERT INTO sync_log (table_name, last_synced_at, etag)
             VALUES ('__triage_hash_v2_migrated', CURRENT_TIMESTAMP, NULL)",
            [],
        )?;
        tx.commit()?;
    }

    // License storage. Single-row table (CHECK id = 1) so the active
    // license survives pod restarts and PVC-mounted file truncation —
    // the previous on-disk `~/.wshm/license.jwt` would silently vanish
    // when an `fs::write` got interrupted, leaving the daemon unable to
    // re-activate without operator intervention.
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS licenses (
            id                INTEGER PRIMARY KEY CHECK (id = 1),
            jwt               TEXT NOT NULL,
            license_key       TEXT,
            plan              TEXT,
            activated_at      TEXT NOT NULL,
            activated_by      TEXT,
            expires_at        TEXT,
            last_validated_at TEXT
        );
        ",
    )?;

    install_search_fts(conn)?;

    Ok(())
}

/// FTS5 virtual table that backs the cross-entity search endpoint.
///
/// One row per searchable artifact (issue, PR, triage result, comment).
/// We use a single table with a `kind` column rather than per-entity FTS
/// tables so the search handler can fire one query and merge naturally
/// by FTS rank instead of stitching N result sets together.
///
/// Triggers keep `search_fts` in sync with mutations to `issues`,
/// `pull_requests`, `triage_results`, and `comments`. On first
/// migration we backfill existing rows so the index is immediately
/// useful (no "search returns nothing until next sync" edge case).
fn install_search_fts(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
            kind        UNINDEXED,
            number      UNINDEXED,
            title,
            body,
            extra,
            updated_at  UNINDEXED,
            tokenize    = 'porter unicode61 remove_diacritics 1'
        );

        -- Issues
        CREATE TRIGGER IF NOT EXISTS search_fts_issues_ai
        AFTER INSERT ON issues BEGIN
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('issue', NEW.number, NEW.title, COALESCE(NEW.body, ''),
                   COALESCE(NEW.labels, ''), NEW.updated_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_issues_au
        AFTER UPDATE ON issues BEGIN
            DELETE FROM search_fts WHERE kind = 'issue' AND number = OLD.number;
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('issue', NEW.number, NEW.title, COALESCE(NEW.body, ''),
                   COALESCE(NEW.labels, ''), NEW.updated_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_issues_ad
        AFTER DELETE ON issues BEGIN
            DELETE FROM search_fts WHERE kind = 'issue' AND number = OLD.number;
        END;

        -- Pull requests
        CREATE TRIGGER IF NOT EXISTS search_fts_pulls_ai
        AFTER INSERT ON pull_requests BEGIN
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('pull', NEW.number, NEW.title, COALESCE(NEW.body, ''),
                   COALESCE(NEW.labels, ''), NEW.updated_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_pulls_au
        AFTER UPDATE ON pull_requests BEGIN
            DELETE FROM search_fts WHERE kind = 'pull' AND number = OLD.number;
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('pull', NEW.number, NEW.title, COALESCE(NEW.body, ''),
                   COALESCE(NEW.labels, ''), NEW.updated_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_pulls_ad
        AFTER DELETE ON pull_requests BEGIN
            DELETE FROM search_fts WHERE kind = 'pull' AND number = OLD.number;
        END;

        -- Triage results
        CREATE TRIGGER IF NOT EXISTS search_fts_triage_ai
        AFTER INSERT ON triage_results BEGIN
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('triage', NEW.issue_number, COALESCE(NEW.summary, ''),
                   COALESCE(NEW.summary, ''), NEW.category, NEW.acted_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_triage_au
        AFTER UPDATE ON triage_results BEGIN
            DELETE FROM search_fts WHERE kind = 'triage' AND number = OLD.issue_number;
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('triage', NEW.issue_number, COALESCE(NEW.summary, ''),
                   COALESCE(NEW.summary, ''), NEW.category, NEW.acted_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_triage_ad
        AFTER DELETE ON triage_results BEGIN
            DELETE FROM search_fts WHERE kind = 'triage' AND number = OLD.issue_number;
        END;

        -- Comments — search by issue_number so click-through opens the issue
        CREATE TRIGGER IF NOT EXISTS search_fts_comments_ai
        AFTER INSERT ON comments BEGIN
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('comment', NEW.issue_number, '', NEW.body,
                   COALESCE(NEW.author, ''), NEW.created_at);
        END;
        CREATE TRIGGER IF NOT EXISTS search_fts_comments_au
        AFTER UPDATE ON comments BEGIN
            DELETE FROM search_fts WHERE kind = 'comment' AND rowid IN (
                SELECT rowid FROM search_fts
                WHERE kind = 'comment' AND number = OLD.issue_number
            );
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            VALUES('comment', NEW.issue_number, '', NEW.body,
                   COALESCE(NEW.author, ''), NEW.created_at);
        END;
        ",
    )?;

    // First-run backfill. We tag a sentinel row in `sync_log` so the
    // backfill only happens once even if the FTS table was somehow
    // truncated (the CREATE VIRTUAL TABLE IF NOT EXISTS above keeps
    // existing data on subsequent boots).
    let already_backfilled: bool = conn
        .query_row(
            "SELECT 1 FROM sync_log WHERE table_name = '__search_fts_backfilled'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !already_backfilled {
        conn.execute_batch(
            "
            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            SELECT 'issue', number, title, COALESCE(body, ''),
                   COALESCE(labels, ''), updated_at
            FROM issues;

            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            SELECT 'pull', number, title, COALESCE(body, ''),
                   COALESCE(labels, ''), updated_at
            FROM pull_requests;

            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            SELECT 'triage', issue_number, COALESCE(summary, ''),
                   COALESCE(summary, ''), category, acted_at
            FROM triage_results;

            INSERT INTO search_fts(kind, number, title, body, extra, updated_at)
            SELECT 'comment', issue_number, '', body,
                   COALESCE(author, ''), created_at
            FROM comments;

            INSERT INTO sync_log (table_name, last_synced_at, etag)
            VALUES ('__search_fts_backfilled', CURRENT_TIMESTAMP, NULL);
            ",
        )?;
    }

    Ok(())
}

/// Compute a stable content hash for an issue (title + body only).
///
/// Labels are intentionally excluded: when the bot applies labels itself
/// it would otherwise see a hash mismatch on the next cycle and re-triage
/// the same issue forever (see `__triage_hash_v2_migrated` migration).
pub fn compute_issue_hash(title: &str, body: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update(b"\n");
    hasher.update(body.unwrap_or("").as_bytes());
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

    #[test]
    fn test_compute_issue_hash_ignores_labels() {
        let h1 = compute_issue_hash("title", Some("body"));
        let h2 = compute_issue_hash("title", Some("body"));
        assert_eq!(h1, h2);
        // The hash is now independent of any caller-side label set — the
        // function no longer accepts labels at all, so this test simply
        // pins the stability of the new (title, body) signature.
        let h3 = compute_issue_hash("title", Some("different"));
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_triage_hash_v2_migration_recomputes_existing() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Seed an issue and a triage row whose content_hash was computed
        // with the OLD (title+body+labels) formula — simulate by inserting
        // an arbitrary placeholder.
        conn.execute(
            "INSERT INTO issues (number, title, body, state, labels, created_at, updated_at)
             VALUES (1, 't', 'b', 'open', '[\"bug\"]', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO triage_results (issue_number, category, confidence, acted_at, content_hash)
             VALUES (1, 'bug', 0.9, '2026-01-01', 'OLD_HASH_PLACEHOLDER')",
            [],
        )
        .unwrap();

        // Clear the sentinel so the migration runs again on next call.
        conn.execute(
            "DELETE FROM sync_log WHERE table_name = '__triage_hash_v2_migrated'",
            [],
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        let stored: String = conn
            .query_row(
                "SELECT content_hash FROM triage_results WHERE issue_number = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, compute_issue_hash("t", Some("b")));
    }
}
