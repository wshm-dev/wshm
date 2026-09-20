use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::ai::schemas::InlineReviewResult;
use crate::db::Database;

/// One stored inline-code-review pass over a PR. `result` is the AI's
/// findings (comments + summary + stats); `posted_to_github` records
/// whether this pass actually wrote inline comments to the PR (gated
/// separately from computing the review at all — see `review_post_comments`
/// in `RepoFeatures`), so the dashboard can show "computed, not posted".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReviewRow {
    pub pr_number: u64,
    pub result: InlineReviewResult,
    pub posted_to_github: bool,
    #[serde(default)]
    pub content_hash: Option<String>,
    pub reviewed_at: String,
}

fn row_to_pr_review(row: &rusqlite::Row) -> rusqlite::Result<PrReviewRow> {
    let result_json: String = row.get(1)?;
    let result: InlineReviewResult =
        serde_json::from_str(&result_json).unwrap_or(InlineReviewResult {
            comments: Vec::new(),
            summary: String::new(),
            stats: Default::default(),
        });
    Ok(PrReviewRow {
        pr_number: row.get(0)?,
        result,
        posted_to_github: row.get::<_, i64>(2)? != 0,
        content_hash: row.get(3)?,
        reviewed_at: row.get(4)?,
    })
}

const SELECT_COLUMNS: &str = "pr_number, result_json, posted_to_github, content_hash, reviewed_at";

impl Database {
    pub fn get_pr_review(&self, pr_number: u64) -> Result<Option<PrReviewRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM pr_reviews WHERE pr_number = ?1"
            ))?;
            let result = stmt.query_row(params![pr_number], row_to_pr_review);
            match result {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Load every stored review in one query, keyed by PR number. Mirrors
    /// `get_all_pr_analyses` — used by the /api/v1/reviews web handler to
    /// avoid an N+1 per open PR.
    pub fn get_all_pr_reviews(&self) -> Result<std::collections::HashMap<u64, PrReviewRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!("SELECT {SELECT_COLUMNS} FROM pr_reviews"))?;
            let rows = stmt
                .query_map([], row_to_pr_review)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|r| (r.pr_number, r)).collect())
        })
    }

    pub fn upsert_pr_review(&self, row: &PrReviewRow) -> Result<()> {
        self.with_conn(|conn| {
            let result_json = serde_json::to_string(&row.result).unwrap_or_default();
            conn.execute(
                "INSERT INTO pr_reviews (pr_number, result_json, posted_to_github, content_hash, reviewed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(pr_number) DO UPDATE SET
                    result_json = excluded.result_json,
                    posted_to_github = excluded.posted_to_github,
                    content_hash = excluded.content_hash,
                    reviewed_at = excluded.reviewed_at",
                params![
                    row.pr_number,
                    result_json,
                    row.posted_to_github as i64,
                    row.content_hash,
                    row.reviewed_at,
                ],
            )?;
            Ok(())
        })
    }
}
