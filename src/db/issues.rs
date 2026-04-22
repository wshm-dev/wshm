use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::parse_labels_json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub reactions_plus1: u32,
    pub reactions_total: u32,
}

use crate::db::Database;

impl Database {
    pub fn upsert_issue(&self, issue: &Issue) -> Result<()> {
        self.with_conn(|conn| {
            upsert_issue(conn, issue)?;
            Ok(())
        })
    }

    pub fn batch_upsert_issues(&self, issues: &[Issue]) -> Result<()> {
        if issues.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            for issue in issues {
                upsert_issue(&tx, issue)?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn get_issue(&self, number: u64) -> Result<Option<Issue>> {
        self.with_conn(|conn| get_issue(conn, number))
    }

    pub fn get_open_issues(&self) -> Result<Vec<Issue>> {
        self.with_conn(get_open_issues)
    }

    pub fn get_untriaged_issues(&self) -> Result<Vec<Issue>> {
        self.with_conn(get_untriaged_issues)
    }

    /// Get issues that need triage: either never triaged OR content changed since last triage.
    /// Returns up to `limit` issues prioritized by reaction count.
    pub fn get_issues_needing_triage(&self, limit: usize) -> Result<Vec<Issue>> {
        self.with_conn(|conn| get_issues_needing_triage(conn, limit))
    }

    /// Merge new labels into the issue's existing labels in the DB cache (additive, no overwrite).
    pub fn merge_issue_labels(&self, number: u64, add: &[String], remove: &[String]) -> Result<()> {
        self.with_conn(|conn| {
            // Read current labels
            let current: String = conn
                .query_row(
                    "SELECT labels FROM issues WHERE number = ?1",
                    params![number],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| "[]".to_string());
            let mut labels: Vec<String> = serde_json::from_str(&current).unwrap_or_default();

            // Remove old wshm labels
            labels.retain(|l| !remove.iter().any(|r| r.eq_ignore_ascii_case(l)));

            // Add new labels (dedup)
            for label in add {
                if !labels.iter().any(|l| l.eq_ignore_ascii_case(label)) {
                    labels.push(label.clone());
                }
            }

            let labels_json = serde_json::to_string(&labels)?;
            conn.execute(
                "UPDATE issues SET labels = ?1 WHERE number = ?2",
                params![labels_json, number],
            )?;
            Ok(())
        })
    }
}

pub fn upsert_issue(conn: &Connection, issue: &Issue) -> Result<()> {
    let labels_json = serde_json::to_string(&issue.labels)?;
    conn.execute(
        "INSERT INTO issues (number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(number) DO UPDATE SET
            title = excluded.title,
            body = excluded.body,
            state = excluded.state,
            labels = excluded.labels,
            author = excluded.author,
            updated_at = excluded.updated_at,
            reactions_plus1 = excluded.reactions_plus1,
            reactions_total = excluded.reactions_total",
        params![
            issue.number,
            issue.title,
            issue.body,
            issue.state,
            labels_json,
            issue.author,
            issue.created_at,
            issue.updated_at,
            issue.reactions_plus1,
            issue.reactions_total,
        ],
    )?;
    Ok(())
}

fn row_to_issue(row: &rusqlite::Row) -> rusqlite::Result<Issue> {
    let labels_json: String = row.get(4)?;
    Ok(Issue {
        number: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        state: row.get(3)?,
        labels: parse_labels_json(&labels_json),
        author: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        reactions_plus1: row.get(8)?,
        reactions_total: row.get(9)?,
    })
}

pub fn get_issue(conn: &Connection, number: u64) -> Result<Option<Issue>> {
    let mut stmt = conn.prepare(
        "SELECT number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total
         FROM issues WHERE number = ?1",
    )?;

    let result = stmt.query_row(params![number], row_to_issue);

    match result {
        Ok(issue) => Ok(Some(issue)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn get_open_issues(conn: &Connection) -> Result<Vec<Issue>> {
    let mut stmt = conn.prepare(
        "SELECT number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total
         FROM issues WHERE state = 'open' ORDER BY number DESC",
    )?;

    let issues = stmt
        .query_map([], row_to_issue)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(issues)
}

pub fn get_untriaged_issues(conn: &Connection) -> Result<Vec<Issue>> {
    let mut stmt = conn.prepare(
        "SELECT i.number, i.title, i.body, i.state, i.labels, i.author, i.created_at, i.updated_at, i.reactions_plus1, i.reactions_total
         FROM issues i
         LEFT JOIN triage_results t ON i.number = t.issue_number
         WHERE i.state = 'open' AND t.issue_number IS NULL
         ORDER BY i.reactions_total DESC, i.number ASC
         LIMIT 20",
    )?;

    let issues = stmt
        .query_map([], row_to_issue)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(issues)
}

/// Get issues that need triage: either never triaged OR content changed since last triage.
/// Returns up to `limit` issues prioritized by reaction count, then issue number.
pub fn get_issues_needing_triage(conn: &Connection, limit: usize) -> Result<Vec<Issue>> {
    use crate::db::schema::compute_issue_hash;

    // First, get all open issues with their triage status
    let mut stmt = conn.prepare(
        "SELECT i.number, i.title, i.body, i.state, i.labels, i.author, i.created_at, i.updated_at, i.reactions_plus1, i.reactions_total,
                t.content_hash
         FROM issues i
         LEFT JOIN triage_results t ON i.number = t.issue_number
         WHERE i.state = 'open'
         ORDER BY i.reactions_total DESC, i.number ASC",
    )?;

    let mut issues_needing_triage = Vec::new();
    let rows = stmt.query_map([], |row| {
        let issue = Issue {
            number: row.get(0)?,
            title: row.get(1)?,
            body: row.get(2)?,
            state: row.get(3)?,
            labels: parse_labels_json(&row.get::<_, String>(4)?),
            author: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            reactions_plus1: row.get(8)?,
            reactions_total: row.get(9)?,
        };
        let stored_hash: Option<String> = row.get(10)?;
        Ok((issue, stored_hash))
    })?;

    for row in rows {
        if issues_needing_triage.len() >= limit {
            break;
        }

        let (issue, stored_hash) = row?;
        let current_hash = compute_issue_hash(&issue.title, issue.body.as_deref(), &issue.labels);

        // Need triage if: never triaged (NULL hash) OR content changed (hash mismatch)
        if stored_hash.is_none() || stored_hash.as_deref() != Some(current_hash.as_str()) {
            issues_needing_triage.push(issue);
        }
    }

    Ok(issues_needing_triage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn test_issue() -> Issue {
        Issue {
            number: 1,
            title: "Test issue".to_string(),
            body: Some("Description".to_string()),
            state: "open".to_string(),
            labels: vec!["bug".to_string()],
            author: Some("user".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            reactions_plus1: 0,
            reactions_total: 0,
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let db = Database::open_memory().unwrap();
        let issue = test_issue();
        db.upsert_issue(&issue).unwrap();

        let fetched = db.get_issue(1).unwrap().unwrap();
        assert_eq!(fetched.title, "Test issue");
        assert_eq!(fetched.labels, vec!["bug".to_string()]);
    }

    #[test]
    fn test_get_open_issues() {
        let db = Database::open_memory().unwrap();
        db.upsert_issue(&test_issue()).unwrap();

        let mut closed = test_issue();
        closed.number = 2;
        closed.state = "closed".to_string();
        db.upsert_issue(&closed).unwrap();

        let open = db.get_open_issues().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].number, 1);
    }

    #[test]
    fn test_get_untriaged_issues() {
        let db = Database::open_memory().unwrap();
        db.upsert_issue(&test_issue()).unwrap();

        let untriaged = db.get_untriaged_issues().unwrap();
        assert_eq!(untriaged.len(), 1);
    }

    #[test]
    fn test_get_issues_needing_triage() {
        use crate::ai::schemas::IssueClassification;
        use crate::db::schema::compute_issue_hash;

        let db = Database::open_memory().unwrap();

        // Issue 1: never triaged
        let mut issue1 = test_issue();
        issue1.number = 1;
        db.upsert_issue(&issue1).unwrap();

        // Issue 2: triaged, content unchanged
        let mut issue2 = test_issue();
        issue2.number = 2;
        issue2.title = "Already triaged".to_string();
        db.upsert_issue(&issue2).unwrap();
        let classification = IssueClassification {
            category: "bug".to_string(),
            confidence: 0.9,
            priority: Some("high".to_string()),
            summary: "Test summary".to_string(),
            suggested_labels: vec![],
            is_duplicate_of: None,
            is_simple_fix: false,
            relevant_files: vec![],
        };
        let hash2 = compute_issue_hash(&issue2.title, issue2.body.as_deref(), &issue2.labels);
        db.upsert_triage_result_with_hash(&classification, 2, Some(&hash2))
            .unwrap();

        // Issue 3: triaged, but content changed
        let mut issue3 = test_issue();
        issue3.number = 3;
        issue3.title = "Content changed".to_string();
        db.upsert_issue(&issue3).unwrap();
        let hash3_old = compute_issue_hash(&issue3.title, issue3.body.as_deref(), &issue3.labels);
        db.upsert_triage_result_with_hash(&classification, 3, Some(&hash3_old))
            .unwrap();

        // Now modify issue 3's title (simulates content change)
        issue3.title = "Content changed - UPDATED".to_string();
        db.upsert_issue(&issue3).unwrap();

        let needing_triage = db.get_issues_needing_triage(10).unwrap();

        // Should return issue 1 (never triaged) and issue 3 (content changed)
        assert_eq!(needing_triage.len(), 2);
        let numbers: Vec<u64> = needing_triage.iter().map(|i| i.number).collect();
        assert!(numbers.contains(&1));
        assert!(numbers.contains(&3));
        assert!(!numbers.contains(&2)); // Issue 2 unchanged, should not be returned
    }
}
