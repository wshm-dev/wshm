//! Pipeline run tracking (`pipeline_runs`, `pipeline_step_runs`), written by
//! the pipeline engine (`crate::pipelines::engine`) and read by
//! `GET /api/v1/repos/{slug}/runs`.
//!
//! One run per processed issue/PR event: the item, the template, what
//! triggered it, the PR head, the planned steps, then one row per step with
//! its status (done / skipped / failed), reason, timings, AI calls, a short
//! summary and the number of findings. Bounded by
//! [`DatabaseBackend::cleanup_pipeline_runs`](crate::db::backend::DatabaseBackend::cleanup_pipeline_runs)
//! (`[pipeline_runs]` retention_days / max_per_item).

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// A step as planned when the run started (the run graph).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedStep {
    pub id: String,
    pub uses: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
}

/// Columns written when a run starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewPipelineRun {
    /// `issue` or `pr`.
    pub item_kind: String,
    pub item_number: u64,
    pub template: String,
    /// e.g. `pull_request.synchronize`.
    pub trigger: String,
    pub event_id: Option<i64>,
    pub head_sha: Option<String>,
    /// JSON array of [`PlannedStep`].
    pub plan_json: String,
    pub started_at: String,
}

/// Columns written when a run ends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineRunFinish {
    pub id: i64,
    /// `done` or `failed`.
    pub status: String,
    pub error: Option<String>,
    pub head_sha: Option<String>,
    /// AI calls of the run's steps, added to the stored count (background
    /// calls may have been added already).
    pub ai_calls: u32,
    pub finished_at: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStepRunRow {
    pub run_id: i64,
    /// Position of the step in the run (0-based).
    pub seq: u32,
    pub step_id: String,
    pub uses: String,
    /// `done`, `skipped` or `failed`.
    pub status: String,
    /// Skip reason or error message.
    pub reason: Option<String>,
    pub summary: Option<String>,
    pub findings: u32,
    pub ai_calls: u32,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineRunRow {
    pub id: i64,
    pub item_kind: String,
    pub item_number: u64,
    pub template: String,
    pub trigger: String,
    pub event_id: Option<i64>,
    pub head_sha: Option<String>,
    /// `running`, `done` or `failed`. A run left `running` was interrupted
    /// (pod restart) and is removed by the retention like any other.
    pub status: String,
    pub error: Option<String>,
    pub plan: Vec<PlannedStep>,
    pub ai_calls: u32,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub steps: Vec<PipelineStepRunRow>,
}

/// Filter of [`list_pipeline_runs`].
#[derive(Debug, Clone, Default)]
pub struct PipelineRunFilter {
    pub item_kind: Option<String>,
    pub item_number: Option<u64>,
}

/// Upper bound of a listing.
pub const MAX_LIST: usize = 200;

pub const SQLITE_DDL: &str = "
    CREATE TABLE IF NOT EXISTS pipeline_runs (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        item_kind   TEXT NOT NULL,
        item_number INTEGER NOT NULL,
        template    TEXT NOT NULL,
        trigger     TEXT NOT NULL,
        event_id    INTEGER,
        head_sha    TEXT,
        status      TEXT NOT NULL,
        error       TEXT,
        plan_json   TEXT NOT NULL DEFAULT '[]',
        ai_calls    INTEGER NOT NULL DEFAULT 0,
        started_at  TEXT NOT NULL,
        finished_at TEXT,
        duration_ms INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_pipeline_runs_item
        ON pipeline_runs(item_kind, item_number, id DESC);
    CREATE INDEX IF NOT EXISTS idx_pipeline_runs_started ON pipeline_runs(started_at);

    CREATE TABLE IF NOT EXISTS pipeline_step_runs (
        run_id      INTEGER NOT NULL,
        seq         INTEGER NOT NULL,
        step_id     TEXT NOT NULL,
        uses        TEXT NOT NULL,
        status      TEXT NOT NULL,
        reason      TEXT,
        summary     TEXT,
        findings    INTEGER NOT NULL DEFAULT 0,
        ai_calls    INTEGER NOT NULL DEFAULT 0,
        started_at  TEXT NOT NULL,
        finished_at TEXT NOT NULL,
        duration_ms INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (run_id, seq)
    );
";

pub fn start_pipeline_run(conn: &Connection, r: &NewPipelineRun) -> Result<i64> {
    conn.execute(
        "INSERT INTO pipeline_runs (item_kind, item_number, template, trigger, event_id,
            head_sha, status, plan_json, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8)",
        params![
            r.item_kind,
            r.item_number as i64,
            r.template,
            r.trigger,
            r.event_id,
            r.head_sha,
            r.plan_json,
            r.started_at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn record_pipeline_step(conn: &Connection, s: &PipelineStepRunRow) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO pipeline_step_runs (run_id, seq, step_id, uses, status, reason,
            summary, findings, ai_calls, started_at, finished_at, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            s.run_id,
            s.seq,
            s.step_id,
            s.uses,
            s.status,
            s.reason,
            s.summary,
            s.findings,
            s.ai_calls,
            s.started_at,
            s.finished_at,
            s.duration_ms as i64
        ],
    )?;
    Ok(())
}

pub fn finish_pipeline_run(conn: &Connection, f: &PipelineRunFinish) -> Result<()> {
    conn.execute(
        "UPDATE pipeline_runs SET status = ?2, error = ?3,
            head_sha = COALESCE(?4, head_sha), ai_calls = ai_calls + ?5, finished_at = ?6,
            duration_ms = ?7
         WHERE id = ?1",
        params![
            f.id,
            f.status,
            f.error,
            f.head_sha,
            f.ai_calls,
            f.finished_at,
            f.duration_ms as i64
        ],
    )?;
    Ok(())
}

/// Add AI calls made in the background after a step returned. Additive on
/// both rows (as is [`finish_pipeline_run`] on the run), so the order of
/// the two writes does not matter; a job finishing before its step row is
/// written only counts on the run.
pub fn add_pipeline_ai_calls(conn: &Connection, run_id: i64, seq: u32, n: u32) -> Result<()> {
    conn.execute(
        "UPDATE pipeline_runs SET ai_calls = ai_calls + ?2 WHERE id = ?1",
        params![run_id, n],
    )?;
    conn.execute(
        "UPDATE pipeline_step_runs SET ai_calls = ai_calls + ?3 WHERE run_id = ?1 AND seq = ?2",
        params![run_id, seq, n],
    )?;
    Ok(())
}

/// Parse the stored plan; a corrupt value reads as an empty plan.
pub fn parse_plan(json: &str) -> Vec<PlannedStep> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn list_pipeline_runs(
    conn: &Connection,
    filter: &PipelineRunFilter,
    limit: usize,
) -> Result<Vec<PipelineRunRow>> {
    let limit = limit.clamp(1, MAX_LIST) as i64;
    let mut stmt = conn.prepare(
        "SELECT id, item_kind, item_number, template, trigger, event_id, head_sha, status,
                error, plan_json, ai_calls, started_at, finished_at, duration_ms
         FROM pipeline_runs
         WHERE (?1 IS NULL OR item_kind = ?1) AND (?2 IS NULL OR item_number = ?2)
         ORDER BY id DESC LIMIT ?3",
    )?;
    let mut runs = stmt
        .query_map(
            params![
                filter.item_kind,
                filter.item_number.map(|n| n as i64),
                limit
            ],
            |row| {
                Ok(PipelineRunRow {
                    id: row.get(0)?,
                    item_kind: row.get(1)?,
                    item_number: row.get::<_, i64>(2)?.max(0) as u64,
                    template: row.get(3)?,
                    trigger: row.get(4)?,
                    event_id: row.get(5)?,
                    head_sha: row.get(6)?,
                    status: row.get(7)?,
                    error: row.get(8)?,
                    plan: parse_plan(&row.get::<_, String>(9)?),
                    ai_calls: row.get(10)?,
                    started_at: row.get(11)?,
                    finished_at: row.get(12)?,
                    duration_ms: row.get::<_, Option<i64>>(13)?.map(|d| d.max(0) as u64),
                    steps: Vec::new(),
                })
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut steps = conn.prepare(
        "SELECT run_id, seq, step_id, uses, status, reason, summary, findings, ai_calls,
                started_at, finished_at, duration_ms
         FROM pipeline_step_runs WHERE run_id = ?1 ORDER BY seq",
    )?;
    for run in &mut runs {
        run.steps = steps
            .query_map(params![run.id], |row| {
                Ok(PipelineStepRunRow {
                    run_id: row.get(0)?,
                    seq: row.get(1)?,
                    step_id: row.get(2)?,
                    uses: row.get(3)?,
                    status: row.get(4)?,
                    reason: row.get(5)?,
                    summary: row.get(6)?,
                    findings: row.get(7)?,
                    ai_calls: row.get(8)?,
                    started_at: row.get(9)?,
                    finished_at: row.get(10)?,
                    duration_ms: row.get::<_, i64>(11)?.max(0) as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
    }
    Ok(runs)
}

/// Delete runs older than `retention_days` (0 = no age limit) and runs
/// beyond the `max_per_item` newest of each issue/PR (0 = no limit), with
/// their steps. Returns the number of runs deleted.
pub fn cleanup_pipeline_runs(
    conn: &Connection,
    retention_days: u32,
    max_per_item: u32,
) -> Result<u64> {
    let mut deleted = 0u64;
    if retention_days > 0 {
        let cutoff =
            (chrono::Utc::now() - chrono::Duration::days(retention_days as i64)).to_rfc3339();
        deleted += conn.execute(
            "DELETE FROM pipeline_runs WHERE started_at < ?1",
            params![cutoff],
        )? as u64;
    }
    if max_per_item > 0 {
        deleted += conn.execute(
            "DELETE FROM pipeline_runs WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (
                        PARTITION BY item_kind, item_number ORDER BY id DESC) AS rn
                    FROM pipeline_runs)
                WHERE rn > ?1)",
            params![max_per_item as i64],
        )? as u64;
    }
    if deleted > 0 {
        conn.execute(
            "DELETE FROM pipeline_step_runs WHERE run_id NOT IN (SELECT id FROM pipeline_runs)",
            [],
        )?;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::backend::DatabaseBackend;
    use crate::db::Database;

    fn new_run(kind: &str, n: u64, started_at: &str) -> NewPipelineRun {
        NewPipelineRun {
            item_kind: kind.into(),
            item_number: n,
            template: "standard".into(),
            trigger: "issues.opened".into(),
            event_id: Some(4),
            head_sha: None,
            plan_json: r#"[{"id":"sync","uses":"sync"}]"#.into(),
            started_at: started_at.into(),
        }
    }

    fn step(run_id: i64, seq: u32) -> PipelineStepRunRow {
        PipelineStepRunRow {
            run_id,
            seq,
            step_id: format!("s{seq}"),
            uses: "sync".into(),
            status: "done".into(),
            started_at: "t".into(),
            finished_at: "t".into(),
            ..Default::default()
        }
    }

    #[test]
    fn run_roundtrip() {
        let db = Database::open_memory().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let id = db
            .start_pipeline_run(&new_run("issue", 3, &now))
            .unwrap()
            .unwrap();
        let mut s = step(id, 0);
        s.findings = 2;
        s.ai_calls = 1;
        s.summary = Some("ok".into());
        db.record_pipeline_step(&s).unwrap();
        let mut s1 = step(id, 1);
        s1.status = "skipped".into();
        s1.reason = Some("triage_issues disabled".into());
        db.record_pipeline_step(&s1).unwrap();

        let listed = db
            .list_pipeline_runs(
                &PipelineRunFilter {
                    item_kind: None,
                    item_number: Some(3),
                },
                10,
            )
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, "running");
        assert_eq!(listed[0].plan[0].uses, "sync");
        assert_eq!(listed[0].steps, vec![s.clone(), s1.clone()]);

        db.finish_pipeline_run(&PipelineRunFinish {
            id,
            status: "done".into(),
            error: None,
            head_sha: Some("abc".into()),
            ai_calls: 1,
            finished_at: now.clone(),
            duration_ms: 12,
        })
        .unwrap();
        let r = &db
            .list_pipeline_runs(&PipelineRunFilter::default(), 10)
            .unwrap()[0];
        assert_eq!(r.status, "done");
        assert_eq!(r.head_sha.as_deref(), Some("abc"));
        assert_eq!(r.duration_ms, Some(12));
        assert_eq!(r.event_id, Some(4));
        // Kind filter.
        assert!(db
            .list_pipeline_runs(
                &PipelineRunFilter {
                    item_kind: Some("pr".into()),
                    item_number: Some(3),
                },
                10
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn retention_by_age_and_per_item() {
        let db = Database::open_memory().unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();
        let old_id = db
            .start_pipeline_run(&new_run("pr", 1, &old))
            .unwrap()
            .unwrap();
        db.record_pipeline_step(&step(old_id, 0)).unwrap();
        let mut ids = Vec::new();
        for _ in 0..4 {
            let id = db
                .start_pipeline_run(&new_run("pr", 2, &now))
                .unwrap()
                .unwrap();
            db.record_pipeline_step(&step(id, 0)).unwrap();
            ids.push(id);
        }
        db.start_pipeline_run(&new_run("issue", 2, &now)).unwrap();

        // 1 too old + 2 beyond the 2 newest of PR #2; issue #2 is another item.
        assert_eq!(db.cleanup_pipeline_runs(30, 2).unwrap(), 3);
        let left: Vec<i64> = db
            .list_pipeline_runs(
                &PipelineRunFilter {
                    item_kind: Some("pr".into()),
                    item_number: None,
                },
                10,
            )
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(left, vec![ids[3], ids[2]]);
        let orphan_steps: i64 = db
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM pipeline_step_runs WHERE run_id NOT IN (SELECT id FROM pipeline_runs)",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(orphan_steps, 0);
        // 0 / 0 disables both bounds.
        assert_eq!(db.cleanup_pipeline_runs(0, 0).unwrap(), 0);
    }
}
