//! Stored results of automatic bug-report reproduction (#229).
//!
//! `repro_runs` is append-only: one row per attempt that reached the AI.
//! The latest row of an issue is what the dashboard shows and what dedupe
//! compares against; the row count over the last 24h backs
//! `[triage.repro] max_runs_per_day`.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Database;

/// A file the AI generated for a reproduction (test or script).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReproFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReproRunRow {
    /// Row id (0 before insertion).
    #[serde(default)]
    pub id: i64,
    pub issue_number: u64,
    /// `confirmed` | `not-reproduced` | `needs-info` | `error`.
    pub outcome: String,
    /// Short human explanation of the outcome.
    #[serde(default)]
    pub reason: String,
    /// sha256(issue content hash + default-branch head). Same key = no re-run.
    pub dedupe_key: String,
    pub issue_hash: String,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub files: Vec<ReproFile>,
    #[serde(default)]
    pub expected_failure: Option<String>,
    /// Trimmed sandbox output.
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub missing_info: Vec<String>,
    #[serde(default)]
    pub comment_posted: bool,
    #[serde(default)]
    pub test_pr: Option<u64>,
    /// RFC 3339 UTC, second precision (`2026-01-02T03:04:05Z`) so string
    /// comparison orders correctly in both SQLite and Postgres.
    pub created_at: String,
}

pub(crate) const SELECT_COLUMNS: &str =
    "id, issue_number, outcome, reason, dedupe_key, issue_hash, \
     head_sha, default_branch, command, files_json, expected_failure, output, exit_code, \
     missing_info, comment_posted, test_pr, created_at";

fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<ReproRunRow> {
    let files_json: String = row.get(9)?;
    let missing_json: String = row.get(13)?;
    Ok(ReproRunRow {
        id: row.get(0)?,
        issue_number: row.get::<_, i64>(1)? as u64,
        outcome: row.get(2)?,
        reason: row.get(3)?,
        dedupe_key: row.get(4)?,
        issue_hash: row.get(5)?,
        head_sha: row.get(6)?,
        default_branch: row.get(7)?,
        command: row.get(8)?,
        files: serde_json::from_str(&files_json).unwrap_or_default(),
        expected_failure: row.get(10)?,
        output: row.get(11)?,
        exit_code: row.get(12)?,
        missing_info: serde_json::from_str(&missing_json).unwrap_or_default(),
        comment_posted: row.get::<_, i64>(14)? != 0,
        test_pr: row.get::<_, Option<i64>>(15)?.map(|n| n as u64),
        created_at: row.get(16)?,
    })
}

impl Database {
    pub fn insert_repro_run(&self, run: &ReproRunRow) -> Result<i64> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO repro_runs (issue_number, outcome, reason, dedupe_key, issue_hash,
                    head_sha, default_branch, command, files_json, expected_failure, output,
                    exit_code, missing_info, comment_posted, test_pr, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    run.issue_number as i64,
                    run.outcome,
                    run.reason,
                    run.dedupe_key,
                    run.issue_hash,
                    run.head_sha,
                    run.default_branch,
                    run.command,
                    serde_json::to_string(&run.files)?,
                    run.expected_failure,
                    run.output,
                    run.exit_code,
                    serde_json::to_string(&run.missing_info)?,
                    run.comment_posted as i64,
                    run.test_pr.map(|n| n as i64),
                    run.created_at,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn latest_repro_run(&self, issue_number: u64) -> Result<Option<ReproRunRow>> {
        self.with_conn(|conn| {
            Ok(conn
                .query_row(
                    &format!(
                        "SELECT {SELECT_COLUMNS} FROM repro_runs WHERE issue_number = ?1
                         ORDER BY id DESC LIMIT 1"
                    ),
                    params![issue_number as i64],
                    row_to_run,
                )
                .optional()?)
        })
    }

    pub fn count_repro_runs_since(&self, since: &str) -> Result<u64> {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM repro_runs WHERE created_at >= ?1",
                params![since],
                |row| row.get(0),
            )?;
            Ok(n as u64)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(issue: u64, outcome: &str, at: &str) -> ReproRunRow {
        ReproRunRow {
            issue_number: issue,
            outcome: outcome.into(),
            dedupe_key: format!("k{issue}{outcome}"),
            issue_hash: "h".into(),
            files: vec![ReproFile {
                path: "t.sh".into(),
                content: "exit 1".into(),
            }],
            missing_info: vec!["version".into()],
            test_pr: Some(9),
            exit_code: Some(1),
            created_at: at.into(),
            ..Default::default()
        }
    }

    #[test]
    fn latest_and_count_since() {
        let dir = std::env::temp_dir().join(format!("wshm-repro-db-{}", std::process::id()));
        let db = Database::open_path(&dir.join("state.db")).unwrap();
        assert!(db.latest_repro_run(1).unwrap().is_none());
        db.insert_repro_run(&run(1, "error", "2026-01-01T00:00:00Z"))
            .unwrap();
        let id = db
            .insert_repro_run(&run(1, "confirmed", "2026-01-02T00:00:00Z"))
            .unwrap();
        db.insert_repro_run(&run(2, "needs-info", "2026-01-03T00:00:00Z"))
            .unwrap();
        let latest = db.latest_repro_run(1).unwrap().unwrap();
        assert_eq!(latest.id, id);
        assert_eq!(latest.outcome, "confirmed");
        assert_eq!(latest.files[0].path, "t.sh");
        assert_eq!(latest.missing_info, vec!["version"]);
        assert_eq!(latest.test_pr, Some(9));
        assert_eq!(
            db.count_repro_runs_since("2026-01-02T00:00:00Z").unwrap(),
            2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
