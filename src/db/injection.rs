//! Items flagged as possible prompt injection (`injection_flags`, #235).
//! One row per (kind, number, pipeline); a clean re-scan by the same
//! pipeline deletes it. Read by the dashboard API.

use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::ai::untrusted::Finding;
use crate::db::Database;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionFlagRow {
    /// "issue" or "pull".
    pub kind: String,
    pub number: u64,
    /// Pipeline that flagged it ("triage", "pr_analysis", "review", ...).
    pub pipeline: String,
    pub score: u32,
    /// Public descriptions of the rules that matched.
    pub reasons: Vec<String>,
    /// Matches with excerpts (maintainers only, never posted).
    pub findings: Vec<Finding>,
    /// Automatic mutating actions were disabled.
    pub blocked: bool,
    pub detected_at: String,
}

pub const CREATE_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS injection_flags (
        kind        TEXT NOT NULL,
        number      INTEGER NOT NULL,
        pipeline    TEXT NOT NULL,
        score       INTEGER NOT NULL,
        reasons     TEXT NOT NULL,
        findings    TEXT NOT NULL,
        blocked     INTEGER NOT NULL DEFAULT 0,
        detected_at TEXT NOT NULL,
        PRIMARY KEY (kind, number, pipeline)
    );
";

const COLUMNS: &str = "kind, number, pipeline, score, reasons, findings, blocked, detected_at";

fn row_to_flag(row: &rusqlite::Row) -> rusqlite::Result<InjectionFlagRow> {
    let reasons: String = row.get(4)?;
    let findings: String = row.get(5)?;
    Ok(InjectionFlagRow {
        kind: row.get(0)?,
        number: row.get(1)?,
        pipeline: row.get(2)?,
        score: row.get::<_, i64>(3)?.max(0) as u32,
        reasons: serde_json::from_str(&reasons).unwrap_or_default(),
        findings: serde_json::from_str(&findings).unwrap_or_default(),
        blocked: row.get::<_, i64>(6)? != 0,
        detected_at: row.get(7)?,
    })
}

impl Database {
    pub fn upsert_injection_flag(&self, row: &InjectionFlagRow) -> Result<()> {
        let reasons = serde_json::to_string(&row.reasons)?;
        let findings = serde_json::to_string(&row.findings)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO injection_flags (kind, number, pipeline, score, reasons, findings, blocked, detected_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(kind, number, pipeline) DO UPDATE SET
                    score = excluded.score,
                    reasons = excluded.reasons,
                    findings = excluded.findings,
                    blocked = excluded.blocked,
                    detected_at = excluded.detected_at",
                params![
                    row.kind,
                    row.number,
                    row.pipeline,
                    row.score as i64,
                    reasons,
                    findings,
                    row.blocked as i64,
                    row.detected_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn delete_injection_flag(&self, kind: &str, number: u64, pipeline: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM injection_flags WHERE kind = ?1 AND number = ?2 AND pipeline = ?3",
                params![kind, number, pipeline],
            )?;
            Ok(())
        })
    }

    pub fn get_injection_flags(&self, kind: &str, number: u64) -> Result<Vec<InjectionFlagRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM injection_flags WHERE kind = ?1 AND number = ?2 ORDER BY pipeline"
            ))?;
            let rows = stmt
                .query_map(params![kind, number], row_to_flag)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn list_injection_flags(&self) -> Result<Vec<InjectionFlagRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM injection_flags ORDER BY detected_at DESC"
            ))?;
            let rows = stmt
                .query_map([], row_to_flag)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}
