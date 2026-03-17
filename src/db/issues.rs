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

    pub fn get_issue(&self, number: u64) -> Result<Option<Issue>> {
        self.with_conn(|conn| get_issue(conn, number))
    }

    pub fn get_open_issues(&self) -> Result<Vec<Issue>> {
        self.with_conn(|conn| get_open_issues(conn))
    }

    pub fn get_untriaged_issues(&self) -> Result<Vec<Issue>> {
        self.with_conn(|conn| get_untriaged_issues(conn))
    }

    pub fn update_issue_labels(&self, number: u64, labels: &[String]) -> Result<()> {
        self.with_conn(|conn| {
            let labels_json = serde_json::to_string(labels)?;
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

pub fn get_issue(conn: &Connection, number: u64) -> Result<Option<Issue>> {
    let mut stmt = conn.prepare(
        "SELECT number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total
         FROM issues WHERE number = ?1",
    )?;

    let result = stmt.query_row(params![number], |row| {
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
    });

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
        .query_map([], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(issues)
}

pub fn get_untriaged_issues(conn: &Connection) -> Result<Vec<Issue>> {
    let mut stmt = conn.prepare(
        "SELECT i.number, i.title, i.body, i.state, i.labels, i.author, i.created_at, i.updated_at, i.reactions_plus1, i.reactions_total
         FROM issues i
         LEFT JOIN triage_results t ON i.number = t.issue_number
         WHERE i.state = 'open' AND t.issue_number IS NULL
         ORDER BY i.number DESC",
    )?;

    let issues = stmt
        .query_map([], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(issues)
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
}
