//! LLM usage accounting summary for the usage dashboard.
//!
//! Every successful LLM call is stored in `llm_invocations` (SQLite and
//! Postgres backends) via [`DatabaseBackend::record_llm_invocation`]. The
//! usage dashboard reads the counts back through
//! [`DatabaseBackend::usage_counts`].

use serde::Serialize;

/// Roll-up of LLM call counts for one repo over the three windows the
/// Pro dashboard surfaces. Counts are returned per-kind (`triage`,
/// `pr_analysis`, ...) so the UI can break them down without re-querying.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UsageCounts {
    pub total: u64,
    pub last_24h: u64,
    pub last_7d: u64,
    pub last_30d: u64,
    /// Per-kind breakdown for the same windows. Keys are the `kind`
    /// strings passed to `record_llm_invocation` (lower-case, snake_case).
    pub by_kind: Vec<UsageByKind>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct UsageByKind {
    pub kind: String,
    pub total: u64,
    pub last_24h: u64,
    pub last_7d: u64,
    pub last_30d: u64,
}

use anyhow::Result;
use rusqlite::{params, Connection};

/// Insert one LLM invocation row (SQLite backend).
pub fn record(conn: &Connection, kind: &str, model: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO llm_invocations (kind, model) VALUES (?1, ?2)",
        params![kind, model],
    )?;
    Ok(())
}

const WINDOWS_SQL: &str = "COUNT(*),
    SUM(CASE WHEN occurred_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 day') THEN 1 ELSE 0 END),
    SUM(CASE WHEN occurred_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-7 days') THEN 1 ELSE 0 END),
    SUM(CASE WHEN occurred_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-30 days') THEN 1 ELSE 0 END)";

/// Roll-up of `llm_invocations` (SQLite backend).
pub fn counts(conn: &Connection) -> Result<UsageCounts> {
    let n = |v: Option<i64>| v.unwrap_or(0).max(0) as u64;
    let (total, last_24h, last_7d, last_30d) = conn.query_row(
        &format!("SELECT {WINDOWS_SQL} FROM llm_invocations"),
        [],
        |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        },
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT kind, {WINDOWS_SQL} FROM llm_invocations GROUP BY kind ORDER BY COUNT(*) DESC"
    ))?;
    let by_kind = stmt
        .query_map([], |r| {
            Ok(UsageByKind {
                kind: r.get(0)?,
                total: n(r.get(1)?),
                last_24h: n(r.get(2)?),
                last_7d: n(r.get(3)?),
                last_30d: n(r.get(4)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(UsageCounts {
        total: n(total),
        last_24h: n(last_24h),
        last_7d: n(last_7d),
        last_30d: n(last_30d),
        by_kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_usage_counts_roll_up() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        record(&conn, "triage", None).unwrap();
        record(&conn, "triage", Some("m")).unwrap();
        record(&conn, "pr_analysis", None).unwrap();
        conn.execute(
            "INSERT INTO llm_invocations (kind, occurred_at) VALUES ('triage', '2000-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let c = counts(&conn).unwrap();
        assert_eq!(c.total, 4);
        assert_eq!(c.last_24h, 3);
        assert_eq!(c.last_30d, 3);
        assert_eq!(c.by_kind[0].kind, "triage");
        assert_eq!(c.by_kind[0].total, 3);
        assert_eq!(c.by_kind[0].last_7d, 2);
    }
}
