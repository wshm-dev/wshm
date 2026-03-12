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

    Ok(())
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
