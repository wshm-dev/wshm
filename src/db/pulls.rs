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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrAnalysisRow {
    pub pr_number: u64,
    pub summary: String,
    pub risk_level: String,
    pub pr_type: String,
    pub review_notes: Option<String>,
    pub analyzed_at: String,
}

use crate::db::Database;

impl Database {
    pub fn get_pr_analysis(&self, pr_number: u64) -> Result<Option<PrAnalysisRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT pr_number, summary, risk_level, pr_type, review_notes, analyzed_at
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
                })
            });

            match result {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
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

    pub fn get_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        self.with_conn(|conn| get_pull(conn, number))
    }

    pub fn get_open_pulls(&self) -> Result<Vec<PullRequest>> {
        self.with_conn(get_open_pulls)
    }

    pub fn get_unanalyzed_pulls(&self) -> Result<Vec<PullRequest>> {
        self.with_conn(get_unanalyzed_pulls)
    }
}

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
    })
}

pub fn get_pull(conn: &Connection, number: u64) -> Result<Option<PullRequest>> {
    let mut stmt = conn.prepare(
        "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at
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
        "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at
         FROM pull_requests WHERE state = 'open' ORDER BY number DESC",
    )?;

    let pulls = stmt
        .query_map([], row_to_pull)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(pulls)
}

pub fn get_unanalyzed_pulls(conn: &Connection) -> Result<Vec<PullRequest>> {
    let mut stmt = conn.prepare(
        "SELECT p.number, p.title, p.body, p.state, p.labels, p.author, p.head_sha, p.base_sha, p.head_ref, p.base_ref, p.mergeable, p.ci_status, p.created_at, p.updated_at
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
