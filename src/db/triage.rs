use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::ai::schemas::IssueClassification;
use crate::db::Database;

/// Return the suffix of the first label whose name starts with `prefix`
/// (case-insensitive). Used to reconstruct category / priority from
/// already-applied labels like `category:bug`.
fn first_after_prefix(labels: &[String], prefix: &str) -> Option<String> {
    let p = prefix.to_ascii_lowercase();
    labels.iter().find_map(|l| {
        if !l.to_ascii_lowercase().starts_with(&p) {
            return None;
        }
        let suffix = l.get(prefix.len()..)?.trim().to_string();
        if suffix.is_empty() {
            None
        } else {
            Some(suffix)
        }
    })
}

/// Map a row from a `SELECT issue_number, category, confidence, priority,
/// summary, is_simple_fix, acted_at, content_hash` query into a
/// [`TriageResultRow`]. Shared by the three queries below so a column-order
/// change only needs editing in one place.
fn row_to_triage_result(row: &rusqlite::Row) -> rusqlite::Result<TriageResultRow> {
    Ok(TriageResultRow {
        issue_number: row.get(0)?,
        category: row.get(1)?,
        confidence: row.get(2)?,
        priority: row.get(3)?,
        summary: row.get(4)?,
        is_simple_fix: row.get(5)?,
        acted_at: row.get(6)?,
        content_hash: row.get(7)?,
        domains: super::parse_labels_json(&row.get::<_, String>(8)?),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageResultRow {
    pub issue_number: u64,
    pub category: String,
    pub confidence: f64,
    pub priority: Option<String>,
    pub summary: Option<String>,
    pub is_simple_fix: bool,
    pub acted_at: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    /// "Grand domains" the AI review tagged this issue with (codex, bun, …).
    #[serde(default)]
    pub domains: Vec<String>,
}

impl Database {
    pub fn upsert_triage_result(
        &self,
        result: &IssueClassification,
        issue_number: u64,
    ) -> Result<()> {
        self.upsert_triage_result_with_hash(result, issue_number, None)
    }

    pub fn upsert_triage_result_with_hash(
        &self,
        result: &IssueClassification,
        issue_number: u64,
        content_hash: Option<&str>,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let suggested_labels = serde_json::to_string(&result.suggested_labels)?;
            let relevant_files = serde_json::to_string(&result.relevant_files)?;
            let domains = serde_json::to_string(&result.domains)?;
            let now = chrono::Utc::now().to_rfc3339();

            conn.execute(
                "INSERT INTO triage_results (issue_number, category, confidence, priority, summary, suggested_labels, is_duplicate_of, is_simple_fix, relevant_files, acted_at, content_hash, domains)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(issue_number) DO UPDATE SET
                    category = excluded.category,
                    confidence = excluded.confidence,
                    priority = excluded.priority,
                    summary = excluded.summary,
                    suggested_labels = excluded.suggested_labels,
                    is_duplicate_of = excluded.is_duplicate_of,
                    is_simple_fix = excluded.is_simple_fix,
                    relevant_files = excluded.relevant_files,
                    acted_at = excluded.acted_at,
                    content_hash = excluded.content_hash,
                    domains = excluded.domains",
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
                    content_hash,
                    domains,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_triage_result(&self, issue_number: u64) -> Result<Option<TriageResultRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT issue_number, category, confidence, priority, summary, is_simple_fix, acted_at, content_hash, domains
                 FROM triage_results WHERE issue_number = ?1",
            )?;

            let result = stmt.query_row(params![issue_number], row_to_triage_result);

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
                "SELECT t.issue_number, t.category, t.confidence, t.priority, t.summary, t.is_simple_fix, t.acted_at, t.content_hash, t.domains
                 FROM triage_results t
                 JOIN issues i ON t.issue_number = i.number
                 WHERE i.state = 'open' AND t.acted_at < ?1
                 ORDER BY t.acted_at ASC",
            )?;

            let rows = stmt
                .query_map(rusqlite::params![cutoff_str], row_to_triage_result)?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(rows)
        })
    }

    /// Load every triage result in a single query, keyed by issue number.
    ///
    /// Batch loader used by the web handlers to avoid an N+1 pattern where
    /// `get_triage_result` was called once per open issue (each call took
    /// the connection mutex and prepared a fresh statement).
    pub fn get_all_triage_results(
        &self,
    ) -> Result<std::collections::HashMap<u64, TriageResultRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT issue_number, category, confidence, priority, summary, is_simple_fix, acted_at, content_hash, domains
                 FROM triage_results",
            )?;
            let rows = stmt
                .query_map([], row_to_triage_result)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|r| (r.issue_number, r)).collect())
        })
    }

    /// Get the labels that wshm previously applied to an issue (from suggested_labels in triage_results).
    pub fn get_wshm_applied_labels(&self, issue_number: u64) -> Result<Vec<String>> {
        self.with_conn(|conn| {
            let result: rusqlite::Result<String> = conn.query_row(
                "SELECT suggested_labels FROM triage_results WHERE issue_number = ?1",
                params![issue_number],
                |row| row.get(0),
            );
            match result {
                Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Sum the count of wshm-applied labels per issue in a single query.
    /// Returns a map of issue_number -> number of suggested_labels.
    ///
    /// Batch replacement for calling `get_wshm_applied_labels` once per open
    /// issue (used by the revert-preview handler).
    pub fn get_applied_label_counts(&self) -> Result<std::collections::HashMap<u64, usize>> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT issue_number, suggested_labels FROM triage_results")?;
            let rows = stmt
                .query_map([], |row| {
                    let issue_number: u64 = row.get(0)?;
                    let labels_json: String = row.get(1)?;
                    Ok((issue_number, labels_json))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut map = std::collections::HashMap::new();
            for (issue_number, labels_json) in rows {
                let count = serde_json::from_str::<Vec<String>>(&labels_json)
                    .map(|v| v.len())
                    .unwrap_or(0);
                map.insert(issue_number, count);
            }
            Ok(map)
        })
    }

    /// Get recent triage activity (last N entries, most recent first).
    pub fn recent_activity(&self, limit: usize) -> Result<Vec<TriageResultRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.issue_number, t.category, t.confidence, t.priority, t.summary, t.is_simple_fix, t.acted_at, t.content_hash, t.domains
                 FROM triage_results t
                 ORDER BY t.acted_at DESC
                 LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![limit], row_to_triage_result)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Seed `triage_results` stubs for open issues that already carry
    /// wshm-managed labels on the forge but have no local row — typical
    /// after a fresh install, a storage migration, or wiping the local
    /// state.db. Returns the number of stub rows inserted.
    ///
    /// An issue qualifies when ALL of:
    ///   * it is open,
    ///   * it has no row in `triage_results`,
    ///   * its `updated_at` is older than `now - grace_hours` (so we don't
    ///     race ongoing label edits; `grace_hours = 0` skips this check),
    ///   * at least one of its labels starts with one of `managed_label_prefixes`.
    ///
    /// Each stub row stores a content_hash so the next batch's normal
    /// "needs triage" check sees the issue as already-triaged and avoids
    /// the LLM call. `category` is reconstructed from the first
    /// `category:*` label (defaults to `"uncategorized"`), `priority` from
    /// the first `priority:*` label.
    pub fn seed_triage_stubs_from_labels(
        &self,
        managed_label_prefixes: &[String],
        grace_hours: u32,
    ) -> Result<u64> {
        use crate::db::schema::compute_issue_hash;

        if managed_label_prefixes.is_empty() {
            return Ok(0);
        }

        let cutoff = if grace_hours > 0 {
            Some(chrono::Utc::now() - chrono::Duration::hours(grace_hours as i64))
        } else {
            None
        };

        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT i.number, i.title, i.body, i.labels, i.updated_at
                 FROM issues i
                 LEFT JOIN triage_results t ON i.number = t.issue_number
                 WHERE i.state = 'open' AND t.issue_number IS NULL",
            )?;

            struct Candidate {
                number: u64,
                title: String,
                body: Option<String>,
                labels: Vec<String>,
                updated_at: String,
            }

            let rows = stmt.query_map([], |row| {
                let labels_json: String = row.get(3)?;
                Ok(Candidate {
                    number: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                    updated_at: row.get(4)?,
                })
            })?;

            struct Stub {
                number: u64,
                category: String,
                priority: Option<String>,
                content_hash: String,
                matched_labels: Vec<String>,
            }

            let mut stubs: Vec<Stub> = Vec::new();

            for row in rows {
                let c = row?;

                if let Some(cutoff_dt) = cutoff {
                    let stale = chrono::DateTime::parse_from_rfc3339(&c.updated_at)
                        .map(|dt| dt.with_timezone(&chrono::Utc) < cutoff_dt)
                        .unwrap_or(true);
                    if !stale {
                        continue;
                    }
                }

                let matched: Vec<String> = c
                    .labels
                    .iter()
                    .filter(|l| {
                        managed_label_prefixes
                            .iter()
                            .any(|p| l.to_ascii_lowercase().starts_with(&p.to_ascii_lowercase()))
                    })
                    .cloned()
                    .collect();
                if matched.is_empty() {
                    continue;
                }

                let category = first_after_prefix(&matched, "category:")
                    .unwrap_or_else(|| "uncategorized".to_string());
                let priority = first_after_prefix(&matched, "priority:");
                let content_hash = compute_issue_hash(&c.title, c.body.as_deref());

                stubs.push(Stub {
                    number: c.number,
                    category,
                    priority,
                    content_hash,
                    matched_labels: matched,
                });
            }

            let now = chrono::Utc::now().to_rfc3339();
            let mut inserted: u64 = 0;
            // Wrap the inserts in a single transaction so seeding K stubs
            // costs one fsync instead of K (each bare conn.execute would
            // auto-commit). On a fresh/wiped state.db this can be hundreds
            // of issues.
            let tx = conn.unchecked_transaction()?;
            for stub in &stubs {
                let suggested = serde_json::to_string(&stub.matched_labels)?;
                let n = tx.execute(
                    "INSERT INTO triage_results (issue_number, category, confidence, priority, summary, suggested_labels, is_duplicate_of, is_simple_fix, relevant_files, acted_at, content_hash)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, 0, '[]', ?6, ?7)
                     ON CONFLICT(issue_number) DO NOTHING",
                    params![
                        stub.number,
                        stub.category,
                        1.0_f64,
                        stub.priority,
                        suggested,
                        now,
                        stub.content_hash,
                    ],
                )?;
                inserted += n as u64;
            }
            tx.commit()?;

            Ok(inserted)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::issues::Issue;
    use crate::db::Database;

    fn open_issue(number: u64, labels: Vec<&str>) -> Issue {
        Issue {
            number,
            title: format!("Issue #{number}"),
            body: Some("body".to_string()),
            state: "open".to_string(),
            labels: labels.into_iter().map(String::from).collect(),
            author: Some("alice".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            reactions_plus1: 0,
            reactions_total: 0,
        }
    }

    #[test]
    fn first_after_prefix_extracts_value() {
        let labels = vec!["category:bug".to_string(), "priority:high".to_string()];
        assert_eq!(
            first_after_prefix(&labels, "category:"),
            Some("bug".to_string())
        );
        assert_eq!(
            first_after_prefix(&labels, "priority:"),
            Some("high".to_string())
        );
        assert_eq!(first_after_prefix(&labels, "nope:"), None);
    }

    #[test]
    fn first_after_prefix_is_case_insensitive_on_prefix() {
        let labels = vec!["Category:Bug".to_string()];
        assert_eq!(
            first_after_prefix(&labels, "category:"),
            Some("Bug".to_string())
        );
    }

    #[test]
    fn first_after_prefix_skips_empty_suffix() {
        let labels = vec!["category:".to_string(), "category:bug".to_string()];
        assert_eq!(
            first_after_prefix(&labels, "category:"),
            Some("bug".to_string())
        );
    }

    #[test]
    fn seed_stubs_inserts_for_labeled_issues_only() {
        let db = Database::open_memory().unwrap();
        db.upsert_issue(&open_issue(1, vec!["category:bug", "priority:high"]))
            .unwrap();
        db.upsert_issue(&open_issue(2, vec!["needs-info"])).unwrap();
        db.upsert_issue(&open_issue(3, vec!["category:docs"]))
            .unwrap();

        let prefixes = vec!["category:".to_string(), "priority:".to_string()];
        let inserted = db.seed_triage_stubs_from_labels(&prefixes, 0).unwrap();
        assert_eq!(inserted, 2);

        let row1 = db.get_triage_result(1).unwrap().expect("issue 1 stub");
        assert_eq!(row1.category, "bug");
        assert_eq!(row1.priority.as_deref(), Some("high"));
        assert!(row1.content_hash.is_some());

        let row3 = db.get_triage_result(3).unwrap().expect("issue 3 stub");
        assert_eq!(row3.category, "docs");
        assert_eq!(row3.priority, None);

        assert!(
            db.get_triage_result(2).unwrap().is_none(),
            "issue 2 not seeded"
        );
    }

    #[test]
    fn seed_stubs_is_idempotent() {
        let db = Database::open_memory().unwrap();
        db.upsert_issue(&open_issue(1, vec!["category:bug"]))
            .unwrap();
        let prefixes = vec!["category:".to_string()];

        assert_eq!(db.seed_triage_stubs_from_labels(&prefixes, 0).unwrap(), 1);
        assert_eq!(
            db.seed_triage_stubs_from_labels(&prefixes, 0).unwrap(),
            0,
            "second run must not re-insert"
        );
    }

    #[test]
    fn seed_stubs_skips_when_grace_window_not_elapsed() {
        let db = Database::open_memory().unwrap();
        let mut recent = open_issue(1, vec!["category:bug"]);
        recent.updated_at = chrono::Utc::now().to_rfc3339();
        db.upsert_issue(&recent).unwrap();

        let prefixes = vec!["category:".to_string()];
        assert_eq!(db.seed_triage_stubs_from_labels(&prefixes, 24).unwrap(), 0);
        assert!(db.get_triage_result(1).unwrap().is_none());
    }

    #[test]
    fn seed_stubs_no_op_when_prefixes_empty() {
        let db = Database::open_memory().unwrap();
        db.upsert_issue(&open_issue(1, vec!["category:bug"]))
            .unwrap();
        assert_eq!(db.seed_triage_stubs_from_labels(&[], 0).unwrap(), 0);
        assert!(db.get_triage_result(1).unwrap().is_none());
    }

    #[test]
    fn seeded_stub_marks_issue_as_already_triaged() {
        // Acceptance criteria from issue #100: after seeding, the next
        // `get_issues_needing_triage` call must yield 0 issues — exactly
        // what stops the LLM burn on a wiped state.db.
        let db = Database::open_memory().unwrap();
        for n in 1..=5 {
            db.upsert_issue(&open_issue(n, vec!["category:bug", "priority:low"]))
                .unwrap();
        }

        let prefixes = vec!["category:".to_string(), "priority:".to_string()];
        assert_eq!(db.seed_triage_stubs_from_labels(&prefixes, 0).unwrap(), 5);

        let needing = db.get_issues_needing_triage(10, &[], 0).unwrap();
        assert!(needing.is_empty(), "seeded issues must not need triage");
    }
}
