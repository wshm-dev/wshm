use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::ai::schemas::IssueClassification;
use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageResultRow {
    pub issue_number: u64,
    pub category: String,
    pub confidence: f64,
    pub priority: Option<String>,
    pub summary: Option<String>,
    pub is_simple_fix: bool,
    pub acted_at: String,
}

impl Database {
    pub fn upsert_triage_result(
        &self,
        result: &IssueClassification,
        issue_number: u64,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let suggested_labels = serde_json::to_string(&result.suggested_labels)?;
            let relevant_files = serde_json::to_string(&result.relevant_files)?;
            let now = chrono::Utc::now().to_rfc3339();

            conn.execute(
                "INSERT INTO triage_results (issue_number, category, confidence, priority, summary, suggested_labels, is_duplicate_of, is_simple_fix, relevant_files, acted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(issue_number) DO UPDATE SET
                    category = excluded.category,
                    confidence = excluded.confidence,
                    priority = excluded.priority,
                    summary = excluded.summary,
                    suggested_labels = excluded.suggested_labels,
                    is_duplicate_of = excluded.is_duplicate_of,
                    is_simple_fix = excluded.is_simple_fix,
                    relevant_files = excluded.relevant_files,
                    acted_at = excluded.acted_at",
                params![
                    issue_number,
                    result.category,
                    result.confidence,
                    result.priority,
                    result.summary,
                    suggested_labels,
                    result.is_duplicate_of,
                    result.is_simple_fix,
                    relevant_files,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_triage_result(&self, issue_number: u64) -> Result<Option<TriageResultRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT issue_number, category, confidence, priority, summary, is_simple_fix, acted_at
                 FROM triage_results WHERE issue_number = ?1",
            )?;

            let result = stmt.query_row(params![issue_number], |row| {
                Ok(TriageResultRow {
                    issue_number: row.get(0)?,
                    category: row.get(1)?,
                    confidence: row.get(2)?,
                    priority: row.get(3)?,
                    summary: row.get(4)?,
                    is_simple_fix: row.get(5)?,
                    acted_at: row.get(6)?,
                })
            });

            match result {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Get open issues whose triage result is older than `max_age_hours`.
    pub fn get_stale_triage_results(&self, max_age_hours: u32) -> Result<Vec<TriageResultRow>> {
        self.with_conn(|conn| {
            let cutoff = chrono::Utc::now() - chrono::Duration::hours(max_age_hours as i64);
            let cutoff_str = cutoff.to_rfc3339();

            let mut stmt = conn.prepare(
                "SELECT t.issue_number, t.category, t.confidence, t.priority, t.summary, t.is_simple_fix, t.acted_at
                 FROM triage_results t
                 JOIN issues i ON t.issue_number = i.number
                 WHERE i.state = 'open' AND t.acted_at < ?1
                 ORDER BY t.acted_at ASC",
            )?;

            let rows = stmt
                .query_map(rusqlite::params![cutoff_str], |row| {
                    Ok(TriageResultRow {
                        issue_number: row.get(0)?,
                        category: row.get(1)?,
                        confidence: row.get(2)?,
                        priority: row.get(3)?,
                        summary: row.get(4)?,
                        is_simple_fix: row.get(5)?,
                        acted_at: row.get(6)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(rows)
        })
    }

    pub fn is_triaged(&self, issue_number: u64) -> Result<bool> {
        self.with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM triage_results WHERE issue_number = ?1",
                params![issue_number],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }
}
