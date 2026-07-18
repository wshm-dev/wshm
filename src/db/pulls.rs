use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub head_sha: Option<String>,
    pub base_sha: Option<String>,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub mergeable: Option<bool>,
    pub ci_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// GitHub review decision for open PRs: "approved" /
    /// "changes_requested" / "review_required" (None when unknown or not
    /// yet synced). Maintained by a dedicated sync pass — NOT written by
    /// the upsert path, so a regular PR sync never wipes it.
    #[serde(default)]
    pub review_decision: Option<String>,
    /// When the current `review_decision` value was first observed. Lets
    /// the UI detect "author pushed after changes were requested"
    /// (updated_at > review_decision_at) without per-review API calls.
    #[serde(default)]
    pub review_decision_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrAnalysisRow {
    pub pr_number: u64,
    pub summary: String,
    pub risk_level: String,
    pub pr_type: String,
    pub review_notes: Option<String>,
    pub analyzed_at: String,
    #[serde(default)]
    pub content_hash: Option<String>,
}

use crate::db::Database;

impl Database {
    pub fn get_pr_analysis(&self, pr_number: u64) -> Result<Option<PrAnalysisRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT pr_number, summary, risk_level, pr_type, review_notes, analyzed_at, content_hash
                 FROM pr_analyses WHERE pr_number = ?1",
            )?;

            let result = stmt.query_row(params![pr_number], |row| {
                Ok(PrAnalysisRow {
                    pr_number: row.get(0)?,
                    summary: row.get(1)?,
                    risk_level: row.get(2)?,
                    pr_type: row.get(3)?,
                    review_notes: row.get(4)?,
                    analyzed_at: row.get(5)?,
                    content_hash: row.get(6)?,
                })
            });

            match result {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Load every PR analysis in a single query, keyed by PR number.
    ///
    /// Batch loader used by the web handlers to avoid an N+1 pattern where
    /// `get_pr_analysis` was called once per open PR (each call took the
    /// connection mutex and prepared a fresh statement).
    pub fn get_all_pr_analyses(&self) -> Result<std::collections::HashMap<u64, PrAnalysisRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT pr_number, summary, risk_level, pr_type, review_notes, analyzed_at, content_hash
                 FROM pr_analyses",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(PrAnalysisRow {
                        pr_number: row.get(0)?,
                        summary: row.get(1)?,
                        risk_level: row.get(2)?,
                        pr_type: row.get(3)?,
                        review_notes: row.get(4)?,
                        analyzed_at: row.get(5)?,
                        content_hash: row.get(6)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|r| (r.pr_number, r)).collect())
        })
    }

    pub fn upsert_pull(&self, pr: &PullRequest) -> Result<()> {
        self.with_conn(|conn| {
            upsert_pull(conn, pr)?;
            Ok(())
        })
    }

    pub fn batch_upsert_pulls(&self, pulls: &[PullRequest]) -> Result<()> {
        if pulls.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            for pr in pulls {
                upsert_pull(&tx, pr)?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Apply freshly-synced GitHub review decisions to open PRs.
    ///
    /// `decisions` maps PR number → decision ("approved" /
    /// "changes_requested" / "review_required"); open PRs absent from the
    /// map get their decision cleared (no review activity bucket matched).
    /// `review_decision_at` is only rewritten when the decision VALUE
    /// changes, so the UI can detect "author pushed after the decision"
    /// via `updated_at > review_decision_at`. Returns the number of PRs
    /// whose decision changed.
    pub fn set_review_decisions(
        &self,
        decisions: &std::collections::HashMap<u64, Option<String>>,
    ) -> Result<u64> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let now = chrono::Utc::now().to_rfc3339();
            let mut changed = 0u64;
            {
                let mut current = tx.prepare(
                    "SELECT number, review_decision FROM pull_requests WHERE state = 'open'",
                )?;
                let rows: Vec<(u64, Option<String>)> = current
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?;
                let mut update = tx.prepare(
                    "UPDATE pull_requests SET review_decision = ?1, review_decision_at = ?2
                     WHERE number = ?3",
                )?;
                for (number, old) in rows {
                    let new = decisions.get(&number).cloned().unwrap_or(None);
                    if new != old {
                        update.execute(params![new, now, number])?;
                        changed += 1;
                    }
                }
            }
            tx.commit()?;
            Ok(changed)
        })
    }

    pub fn get_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        self.with_conn(|conn| get_pull(conn, number))
    }

    pub fn get_open_pulls(&self) -> Result<Vec<PullRequest>> {
        self.with_conn(get_open_pulls)
    }

    pub fn get_unanalyzed_pulls(&self) -> Result<Vec<PullRequest>> {
        self.with_conn(get_unanalyzed_pulls)
    }

    pub fn get_pulls_needing_analysis(&self) -> Result<Vec<PullRequest>> {
        self.with_conn(get_pulls_needing_analysis)
    }

    pub fn get_closed_pulls(&self, limit: usize) -> Result<Vec<PullRequest>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at, review_decision, review_decision_at
                 FROM pull_requests WHERE state = 'closed'
                 ORDER BY updated_at DESC LIMIT ?1",
            )?;
            let pulls = stmt
                .query_map(params![limit as i64], row_to_pull)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(pulls)
        })
    }

    /// Upsert one PR analysis row. Extracted from `pipelines::pr_analysis`
    /// so the pipeline can run against any `DatabaseBackend` impl rather
    /// than reaching into a SQLite-specific `with_conn`.
    pub fn upsert_pr_analysis(&self, row: &PrAnalysisRow) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO pr_analyses (pr_number, summary, risk_level, pr_type, review_notes, analyzed_at, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(pr_number) DO UPDATE SET
                    summary = excluded.summary,
                    risk_level = excluded.risk_level,
                    pr_type = excluded.pr_type,
                    review_notes = excluded.review_notes,
                    analyzed_at = excluded.analyzed_at,
                    content_hash = excluded.content_hash",
                params![
                    row.pr_number,
                    row.summary,
                    row.risk_level,
                    row.pr_type,
                    row.review_notes,
                    row.analyzed_at,
                    row.content_hash,
                ],
            )?;
            Ok(())
        })
    }
}

/// NOTE: review_decision / review_decision_at are deliberately NOT part of
/// this upsert - they are maintained by the dedicated review-decision sync
/// pass (set_review_decisions), so a regular PR sync never wipes them.
pub fn upsert_pull(conn: &Connection, pr: &PullRequest) -> Result<()> {
    let labels_json = serde_json::to_string(&pr.labels)?;
    conn.execute(
        "INSERT INTO pull_requests (number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(number) DO UPDATE SET
            title = excluded.title,
            body = excluded.body,
            state = excluded.state,
            labels = excluded.labels,
            author = excluded.author,
            head_sha = excluded.head_sha,
            base_sha = excluded.base_sha,
            head_ref = excluded.head_ref,
            base_ref = excluded.base_ref,
            mergeable = excluded.mergeable,
            ci_status = excluded.ci_status,
            updated_at = excluded.updated_at",
        params![
            pr.number,
            pr.title,
            pr.body,
            pr.state,
            labels_json,
            pr.author,
            pr.head_sha,
            pr.base_sha,
            pr.head_ref,
            pr.base_ref,
            pr.mergeable,
            pr.ci_status,
            pr.created_at,
            pr.updated_at,
        ],
    )?;
    Ok(())
}

fn row_to_pull(row: &rusqlite::Row) -> rusqlite::Result<PullRequest> {
    let labels_json: String = row.get(4)?;
    Ok(PullRequest {
        number: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        state: row.get(3)?,
        labels: super::parse_labels_json(&labels_json),
        author: row.get(5)?,
        head_sha: row.get(6)?,
        base_sha: row.get(7)?,
        head_ref: row.get(8)?,
        base_ref: row.get(9)?,
        mergeable: row.get(10)?,
        ci_status: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        review_decision: row.get(14)?,
        review_decision_at: row.get(15)?,
    })
}

pub fn get_pull(conn: &Connection, number: u64) -> Result<Option<PullRequest>> {
    let mut stmt = conn.prepare(
        "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at, review_decision, review_decision_at
         FROM pull_requests WHERE number = ?1",
    )?;

    let result = stmt.query_row(params![number], row_to_pull);

    match result {
        Ok(pr) => Ok(Some(pr)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn get_open_pulls(conn: &Connection) -> Result<Vec<PullRequest>> {
    let mut stmt = conn.prepare(
        "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at, review_decision, review_decision_at
         FROM pull_requests WHERE state = 'open' ORDER BY number DESC",
    )?;

    let pulls = stmt
        .query_map([], row_to_pull)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(pulls)
}

pub fn get_unanalyzed_pulls(conn: &Connection) -> Result<Vec<PullRequest>> {
    let mut stmt = conn.prepare(
        "SELECT p.number, p.title, p.body, p.state, p.labels, p.author, p.head_sha, p.base_sha, p.head_ref, p.base_ref, p.mergeable, p.ci_status, p.created_at, p.updated_at, p.review_decision, p.review_decision_at
         FROM pull_requests p
         LEFT JOIN pr_analyses a ON p.number = a.pr_number
         WHERE p.state = 'open' AND a.pr_number IS NULL
         ORDER BY p.number DESC",
    )?;

    let pulls = stmt
        .query_map([], row_to_pull)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(pulls)
}

/// Open PRs that need (re)analysis: never analyzed (NULL hash) OR whose
/// content changed since the last analysis (stored hash != current hash).
/// Mirrors `get_issues_needing_triage` so the scheduled batch only spends
/// AI credits when a PR actually changed.
pub fn get_pulls_needing_analysis(conn: &Connection) -> Result<Vec<PullRequest>> {
    use crate::db::schema::compute_pr_hash;

    let mut stmt = conn.prepare(
        "SELECT p.number, p.title, p.body, p.state, p.labels, p.author, p.head_sha, p.base_sha, p.head_ref, p.base_ref, p.mergeable, p.ci_status, p.created_at, p.updated_at, p.review_decision, p.review_decision_at,
                a.content_hash
         FROM pull_requests p
         LEFT JOIN pr_analyses a ON p.number = a.pr_number
         WHERE p.state = 'open'
         ORDER BY p.number DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        let pr = row_to_pull(row)?;
        let stored_hash: Option<String> = row.get(16)?;
        Ok((pr, stored_hash))
    })?;

    let mut pulls = Vec::new();
    for row in rows {
        let (pr, stored_hash) = row?;
        let current_hash = compute_pr_hash(
            &pr.title,
            pr.body.as_deref(),
            pr.head_sha.as_deref(),
            &pr.labels,
        );
        if stored_hash.is_none() || stored_hash.as_deref() != Some(current_hash.as_str()) {
            pulls.push(pr);
        }
    }
    Ok(pulls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::compute_pr_hash;
    use crate::db::Database;

    fn test_pull(number: u64, title: &str) -> PullRequest {
        PullRequest {
            number,
            title: title.to_string(),
            body: Some("Body".to_string()),
            state: "open".to_string(),
            labels: vec!["enhancement".to_string()],
            author: Some("user".to_string()),
            head_sha: Some("abc123".to_string()),
            base_sha: Some("def456".to_string()),
            head_ref: Some("feature".to_string()),
            base_ref: Some("main".to_string()),
            mergeable: Some(true),
            ci_status: Some("success".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            review_decision: None,
            review_decision_at: None,
        }
    }

    #[test]
    fn review_decisions_set_change_and_clear() {
        use std::collections::HashMap;
        let db = Database::open_memory().unwrap();
        db.upsert_pull(&test_pull(1, "a")).unwrap();
        db.upsert_pull(&test_pull(2, "b")).unwrap();

        // First sync: PR 1 approved, PR 2 absent from all buckets.
        let mut d: HashMap<u64, Option<String>> = HashMap::new();
        d.insert(1, Some("approved".to_string()));
        assert_eq!(db.set_review_decisions(&d).unwrap(), 1);
        let p1 = db.get_pull(1).unwrap().unwrap();
        assert_eq!(p1.review_decision.as_deref(), Some("approved"));
        let first_at = p1.review_decision_at.clone().expect("timestamp set");

        // Same decision again: no change, timestamp preserved.
        assert_eq!(db.set_review_decisions(&d).unwrap(), 0);
        let p1b = db.get_pull(1).unwrap().unwrap();
        assert_eq!(p1b.review_decision_at.as_deref(), Some(first_at.as_str()));

        // Decision changes: value + timestamp move.
        d.insert(1, Some("changes_requested".to_string()));
        assert_eq!(db.set_review_decisions(&d).unwrap(), 1);
        let p1c = db.get_pull(1).unwrap().unwrap();
        assert_eq!(p1c.review_decision.as_deref(), Some("changes_requested"));

        // PR dropped from the map entirely: decision cleared.
        d.remove(&1);
        assert_eq!(db.set_review_decisions(&d).unwrap(), 1);
        assert!(db.get_pull(1).unwrap().unwrap().review_decision.is_none());

        // A regular upsert must NOT wipe a stored decision.
        d.insert(2, Some("approved".to_string()));
        db.set_review_decisions(&d).unwrap();
        db.upsert_pull(&test_pull(2, "b updated")).unwrap();
        let p2 = db.get_pull(2).unwrap().unwrap();
        assert_eq!(p2.review_decision.as_deref(), Some("approved"));
    }

    /// Insert a pr_analyses row with the given content hash (mirrors the
    /// production insert in pipelines::pr_analysis).
    fn record_analysis(db: &Database, pr_number: u64, content_hash: &str) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO pr_analyses (pr_number, summary, risk_level, pr_type, review_notes, analyzed_at, content_hash)
                 VALUES (?1, 'summary', 'low', 'feature', '{}', '2026-01-01T00:00:00Z', ?2)",
                params![pr_number, content_hash],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_get_pulls_needing_analysis() {
        let db = Database::open_memory().unwrap();

        // PR 1: never analyzed
        db.upsert_pull(&test_pull(1, "Never analyzed")).unwrap();

        // PR 2: analyzed, content unchanged
        let pr2 = test_pull(2, "Already analyzed");
        db.upsert_pull(&pr2).unwrap();
        let hash2 = compute_pr_hash(
            &pr2.title,
            pr2.body.as_deref(),
            pr2.head_sha.as_deref(),
            &pr2.labels,
        );
        record_analysis(&db, 2, &hash2);

        // PR 3: analyzed, then new commits pushed (head_sha changed)
        let mut pr3 = test_pull(3, "New commits");
        db.upsert_pull(&pr3).unwrap();
        let hash3_old = compute_pr_hash(
            &pr3.title,
            pr3.body.as_deref(),
            pr3.head_sha.as_deref(),
            &pr3.labels,
        );
        record_analysis(&db, 3, &hash3_old);
        pr3.head_sha = Some("zzz999".to_string());
        db.upsert_pull(&pr3).unwrap();

        // PR 4: analyzed and unchanged, but closed — must be excluded
        let pr4 = test_pull(4, "Closed PR");
        let mut pr4_closed = pr4.clone();
        pr4_closed.state = "closed".to_string();
        db.upsert_pull(&pr4_closed).unwrap();
        let hash4 = compute_pr_hash(
            &pr4.title,
            pr4.body.as_deref(),
            pr4.head_sha.as_deref(),
            &pr4.labels,
        );
        record_analysis(&db, 4, &hash4);

        let needing = db.get_pulls_needing_analysis().unwrap();
        let numbers: Vec<u64> = needing.iter().map(|p| p.number).collect();

        assert!(numbers.contains(&1), "never-analyzed PR should be selected");
        assert!(
            numbers.contains(&3),
            "content-changed PR should be selected"
        );
        assert!(!numbers.contains(&2), "unchanged PR must be skipped");
        assert!(!numbers.contains(&4), "closed PR must be skipped");
        assert_eq!(numbers.len(), 2);
    }
}
