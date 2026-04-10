//! PostgreSQL database backend using sqlx.
//!
//! Mirrors all operations from the SQLite `Database` implementation.
//! Each repo gets its own PostgreSQL schema (derived from the repo slug).
//!
//! Enabled via `[database] provider = "postgresql"` in config.toml.

#[cfg(feature = "database-postgres")]
mod inner {
    use anyhow::{Context, Result};
    use sqlx::postgres::PgPoolOptions;
    use sqlx::{PgPool, Row};

    use crate::ai::schemas::IssueClassification;
    use crate::config::Config;
    use crate::db::backend::DatabaseBackend;
    use crate::db::events::WebhookEventRow;
    use crate::db::issues::Issue;
    use crate::db::parse_labels_json;
    use crate::db::pulls::{PrAnalysisRow, PullRequest};
    use crate::db::sync::SyncEntry;
    use crate::db::triage::TriageResultRow;

    /// PostgreSQL backend. Wraps a connection pool scoped to a per-repo schema.
    pub struct PostgresDb {
        pool: PgPool,
        schema: String,
    }

    /// Derive a PostgreSQL schema name from a repo slug.
    /// Replaces `/` and `-` with `_`, prepends `wshm_`.
    /// Example: "rtk-ai/rtk" -> "wshm_rtk_ai_rtk"
    pub fn schema_from_slug(slug: &str) -> String {
        let sanitized: String = slug
            .chars()
            .map(|c| match c {
                '/' | '-' | '.' => '_',
                c if c.is_ascii_alphanumeric() || c == '_' => c,
                _ => '_',
            })
            .collect();
        format!("wshm_{sanitized}")
    }

    impl PostgresDb {
        /// Connect to PostgreSQL and set up the repo schema.
        pub async fn connect(config: &Config) -> Result<Self> {
            let db_config = config
                .database
                .as_ref()
                .context("Missing [database] config for PostgreSQL backend")?;

            let uri = db_config
                .uri
                .as_deref()
                .unwrap_or("postgres://localhost/wshm");

            let slug = config.repo_slug();
            let schema = schema_from_slug(&slug);

            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(uri)
                .await
                .with_context(|| format!("Failed to connect to PostgreSQL: {uri}"))?;

            let db = Self { pool, schema };
            db.migrate().await?;
            Ok(db)
        }

        /// Create the schema and all tables if they do not exist.
        async fn migrate(&self) -> Result<()> {
            // Create schema
            let create_schema = format!("CREATE SCHEMA IF NOT EXISTS {}", self.schema);
            sqlx::query(&create_schema).execute(&self.pool).await?;

            // Create tables within the schema
            let sql = format!(
                r#"
                CREATE TABLE IF NOT EXISTS {schema}.issues (
                    number          BIGINT PRIMARY KEY,
                    title           TEXT NOT NULL,
                    body            TEXT,
                    state           TEXT NOT NULL DEFAULT 'open',
                    labels          TEXT NOT NULL DEFAULT '[]',
                    author          TEXT,
                    created_at      TEXT NOT NULL,
                    updated_at      TEXT NOT NULL,
                    reactions_plus1 INTEGER NOT NULL DEFAULT 0,
                    reactions_total INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS {schema}.pull_requests (
                    number      BIGINT PRIMARY KEY,
                    title       TEXT NOT NULL,
                    body        TEXT,
                    state       TEXT NOT NULL DEFAULT 'open',
                    labels      TEXT NOT NULL DEFAULT '[]',
                    author      TEXT,
                    head_sha    TEXT,
                    base_sha    TEXT,
                    head_ref    TEXT,
                    base_ref    TEXT,
                    mergeable   BOOLEAN,
                    ci_status   TEXT,
                    created_at  TEXT NOT NULL,
                    updated_at  TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS {schema}.comments (
                    id            BIGINT PRIMARY KEY,
                    issue_number  BIGINT NOT NULL,
                    body          TEXT NOT NULL,
                    author        TEXT,
                    created_at    TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS {schema}.labels (
                    name        TEXT PRIMARY KEY,
                    color       TEXT,
                    description TEXT
                );

                CREATE TABLE IF NOT EXISTS {schema}.triage_results (
                    issue_number    BIGINT PRIMARY KEY,
                    category        TEXT NOT NULL,
                    confidence      DOUBLE PRECISION NOT NULL,
                    priority        TEXT,
                    summary         TEXT,
                    suggested_labels TEXT NOT NULL DEFAULT '[]',
                    is_duplicate_of BIGINT,
                    is_simple_fix   BOOLEAN NOT NULL DEFAULT FALSE,
                    relevant_files  TEXT NOT NULL DEFAULT '[]',
                    acted_at        TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS {schema}.pr_analyses (
                    pr_number     BIGINT PRIMARY KEY,
                    summary       TEXT NOT NULL,
                    risk_level    TEXT NOT NULL,
                    pr_type       TEXT NOT NULL,
                    review_notes  TEXT,
                    analyzed_at   TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS {schema}.sync_log (
                    table_name     TEXT PRIMARY KEY,
                    last_synced_at TEXT NOT NULL,
                    etag           TEXT
                );

                CREATE TABLE IF NOT EXISTS {schema}.webhook_events (
                    id           BIGSERIAL PRIMARY KEY,
                    event_type   TEXT NOT NULL,
                    action       TEXT NOT NULL,
                    number       BIGINT,
                    payload      TEXT NOT NULL,
                    status       TEXT NOT NULL DEFAULT 'pending',
                    error        TEXT,
                    received_at  TEXT NOT NULL,
                    processed_at TEXT
                );
                "#,
                schema = self.schema
            );
            sqlx::query(&sql).execute(&self.pool).await?;

            // Create indexes
            let indexes = format!(
                r#"
                CREATE INDEX IF NOT EXISTS idx_issues_state ON {schema}.issues(state);
                CREATE INDEX IF NOT EXISTS idx_pulls_state ON {schema}.pull_requests(state);
                CREATE INDEX IF NOT EXISTS idx_comments_issue ON {schema}.comments(issue_number);
                CREATE INDEX IF NOT EXISTS idx_webhook_status ON {schema}.webhook_events(status);
                CREATE INDEX IF NOT EXISTS idx_triage_acted_at ON {schema}.triage_results(acted_at);
                "#,
                schema = self.schema
            );
            sqlx::query(&indexes).execute(&self.pool).await?;

            Ok(())
        }

        /// Helper: run a blocking closure on the tokio runtime.
        /// Since DatabaseBackend methods are synchronous, we block on async sqlx calls.
        fn block_on<F, T>(&self, f: F) -> Result<T>
        where
            F: std::future::Future<Output = Result<T>>,
        {
            tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
        }
    }

    impl DatabaseBackend for PostgresDb {
        // ── Issues ──────────────────────────────────────────────

        fn upsert_issue(&self, issue: &Issue) -> Result<()> {
            self.block_on(async {
                let labels_json = serde_json::to_string(&issue.labels)?;
                let sql = format!(
                    "INSERT INTO {schema}.issues
                        (number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                     ON CONFLICT (number) DO UPDATE SET
                        title = EXCLUDED.title,
                        body = EXCLUDED.body,
                        state = EXCLUDED.state,
                        labels = EXCLUDED.labels,
                        author = EXCLUDED.author,
                        updated_at = EXCLUDED.updated_at,
                        reactions_plus1 = EXCLUDED.reactions_plus1,
                        reactions_total = EXCLUDED.reactions_total",
                    schema = self.schema
                );
                sqlx::query(&sql)
                    .bind(issue.number as i64)
                    .bind(&issue.title)
                    .bind(&issue.body)
                    .bind(&issue.state)
                    .bind(&labels_json)
                    .bind(&issue.author)
                    .bind(&issue.created_at)
                    .bind(&issue.updated_at)
                    .bind(issue.reactions_plus1 as i32)
                    .bind(issue.reactions_total as i32)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
        }

        fn batch_upsert_issues(&self, issues: &[Issue]) -> Result<()> {
            if issues.is_empty() {
                return Ok(());
            }
            self.block_on(async {
                let mut tx = self.pool.begin().await?;
                let sql = format!(
                    "INSERT INTO {schema}.issues
                        (number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                     ON CONFLICT (number) DO UPDATE SET
                        title = EXCLUDED.title,
                        body = EXCLUDED.body,
                        state = EXCLUDED.state,
                        labels = EXCLUDED.labels,
                        author = EXCLUDED.author,
                        updated_at = EXCLUDED.updated_at,
                        reactions_plus1 = EXCLUDED.reactions_plus1,
                        reactions_total = EXCLUDED.reactions_total",
                    schema = self.schema
                );
                for issue in issues {
                    let labels_json = serde_json::to_string(&issue.labels)?;
                    sqlx::query(&sql)
                        .bind(issue.number as i64)
                        .bind(&issue.title)
                        .bind(&issue.body)
                        .bind(&issue.state)
                        .bind(&labels_json)
                        .bind(&issue.author)
                        .bind(&issue.created_at)
                        .bind(&issue.updated_at)
                        .bind(issue.reactions_plus1 as i32)
                        .bind(issue.reactions_total as i32)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(())
            })
        }

        fn get_issue(&self, number: u64) -> Result<Option<Issue>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total
                     FROM {schema}.issues WHERE number = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(number as i64)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.map(|r| row_to_issue(&r)))
            })
        }

        fn get_open_issues(&self) -> Result<Vec<Issue>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total
                     FROM {schema}.issues WHERE state = 'open' ORDER BY number DESC",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
                Ok(rows.iter().map(row_to_issue).collect())
            })
        }

        fn get_untriaged_issues(&self) -> Result<Vec<Issue>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT i.number, i.title, i.body, i.state, i.labels, i.author, i.created_at, i.updated_at, i.reactions_plus1, i.reactions_total
                     FROM {schema}.issues i
                     LEFT JOIN {schema}.triage_results t ON i.number = t.issue_number
                     WHERE i.state = 'open' AND t.issue_number IS NULL
                     ORDER BY i.reactions_total DESC, i.number ASC
                     LIMIT 20",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
                Ok(rows.iter().map(row_to_issue).collect())
            })
        }

        fn get_issues_needing_triage(&self, limit: usize) -> Result<Vec<Issue>> {
            self.block_on(async {
                use crate::db::schema::compute_issue_hash;

                let sql = format!(
                    "SELECT i.number, i.title, i.body, i.state, i.labels, i.author, i.created_at, i.updated_at, i.reactions_plus1, i.reactions_total,
                            t.content_hash
                     FROM {schema}.issues i
                     LEFT JOIN {schema}.triage_results t ON i.number = t.issue_number
                     WHERE i.state = 'open'
                     ORDER BY i.reactions_total DESC, i.number ASC",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;

                let mut issues_needing_triage = Vec::new();
                for row in rows {
                    if issues_needing_triage.len() >= limit {
                        break;
                    }

                    let issue = Issue {
                        number: row.get::<i64, _>("number") as u64,
                        title: row.get("title"),
                        body: row.get("body"),
                        state: row.get("state"),
                        labels: parse_labels_json(&row.get::<String, _>("labels")),
                        author: row.get("author"),
                        created_at: row.get("created_at"),
                        updated_at: row.get("updated_at"),
                        reactions_plus1: row.get::<i32, _>("reactions_plus1") as u32,
                        reactions_total: row.get::<i32, _>("reactions_total") as u32,
                    };
                    let stored_hash: Option<String> = row.get("content_hash");
                    let current_hash = compute_issue_hash(&issue.title, issue.body.as_deref(), &issue.labels);

                    // Need triage if: never triaged (NULL hash) OR content changed (hash mismatch)
                    if stored_hash.is_none() || stored_hash.as_deref() != Some(current_hash.as_str()) {
                        issues_needing_triage.push(issue);
                    }
                }

                Ok(issues_needing_triage)
            })
        }

        fn merge_issue_labels(&self, number: u64, add: &[String], remove: &[String]) -> Result<()> {
            self.block_on(async {
                let sql = format!(
                    "SELECT labels FROM {schema}.issues WHERE number = $1",
                    schema = self.schema
                );
                let current: String = sqlx::query(&sql)
                    .bind(number as i64)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(|r| r.get::<String, _>("labels"))
                    .unwrap_or_else(|| "[]".to_string());

                let mut labels: Vec<String> = serde_json::from_str(&current).unwrap_or_default();
                labels.retain(|l| !remove.iter().any(|r| r.eq_ignore_ascii_case(l)));
                for label in add {
                    if !labels.iter().any(|l| l.eq_ignore_ascii_case(label)) {
                        labels.push(label.clone());
                    }
                }
                let labels_json = serde_json::to_string(&labels)?;

                let update_sql = format!(
                    "UPDATE {schema}.issues SET labels = $1 WHERE number = $2",
                    schema = self.schema
                );
                sqlx::query(&update_sql)
                    .bind(&labels_json)
                    .bind(number as i64)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
        }

        // ── Pull Requests ───────────────────────────────────────

        fn upsert_pull(&self, pr: &PullRequest) -> Result<()> {
            self.block_on(async {
                let labels_json = serde_json::to_string(&pr.labels)?;
                let sql = format!(
                    "INSERT INTO {schema}.pull_requests
                        (number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                     ON CONFLICT (number) DO UPDATE SET
                        title = EXCLUDED.title,
                        body = EXCLUDED.body,
                        state = EXCLUDED.state,
                        labels = EXCLUDED.labels,
                        author = EXCLUDED.author,
                        head_sha = EXCLUDED.head_sha,
                        base_sha = EXCLUDED.base_sha,
                        head_ref = EXCLUDED.head_ref,
                        base_ref = EXCLUDED.base_ref,
                        mergeable = EXCLUDED.mergeable,
                        ci_status = EXCLUDED.ci_status,
                        updated_at = EXCLUDED.updated_at",
                    schema = self.schema
                );
                sqlx::query(&sql)
                    .bind(pr.number as i64)
                    .bind(&pr.title)
                    .bind(&pr.body)
                    .bind(&pr.state)
                    .bind(&labels_json)
                    .bind(&pr.author)
                    .bind(&pr.head_sha)
                    .bind(&pr.base_sha)
                    .bind(&pr.head_ref)
                    .bind(&pr.base_ref)
                    .bind(pr.mergeable)
                    .bind(&pr.ci_status)
                    .bind(&pr.created_at)
                    .bind(&pr.updated_at)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
        }

        fn batch_upsert_pulls(&self, pulls: &[PullRequest]) -> Result<()> {
            if pulls.is_empty() {
                return Ok(());
            }
            self.block_on(async {
                let mut tx = self.pool.begin().await?;
                let sql = format!(
                    "INSERT INTO {schema}.pull_requests
                        (number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                     ON CONFLICT (number) DO UPDATE SET
                        title = EXCLUDED.title,
                        body = EXCLUDED.body,
                        state = EXCLUDED.state,
                        labels = EXCLUDED.labels,
                        author = EXCLUDED.author,
                        head_sha = EXCLUDED.head_sha,
                        base_sha = EXCLUDED.base_sha,
                        head_ref = EXCLUDED.head_ref,
                        base_ref = EXCLUDED.base_ref,
                        mergeable = EXCLUDED.mergeable,
                        ci_status = EXCLUDED.ci_status,
                        updated_at = EXCLUDED.updated_at",
                    schema = self.schema
                );
                for pr in pulls {
                    let labels_json = serde_json::to_string(&pr.labels)?;
                    sqlx::query(&sql)
                        .bind(pr.number as i64)
                        .bind(&pr.title)
                        .bind(&pr.body)
                        .bind(&pr.state)
                        .bind(&labels_json)
                        .bind(&pr.author)
                        .bind(&pr.head_sha)
                        .bind(&pr.base_sha)
                        .bind(&pr.head_ref)
                        .bind(&pr.base_ref)
                        .bind(pr.mergeable)
                        .bind(&pr.ci_status)
                        .bind(&pr.created_at)
                        .bind(&pr.updated_at)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(())
            })
        }

        fn get_pull(&self, number: u64) -> Result<Option<PullRequest>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at
                     FROM {schema}.pull_requests WHERE number = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(number as i64)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.map(|r| row_to_pull(&r)))
            })
        }

        fn get_open_pulls(&self) -> Result<Vec<PullRequest>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at
                     FROM {schema}.pull_requests WHERE state = 'open' ORDER BY number DESC",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
                Ok(rows.iter().map(row_to_pull).collect())
            })
        }

        fn get_unanalyzed_pulls(&self) -> Result<Vec<PullRequest>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT p.number, p.title, p.body, p.state, p.labels, p.author, p.head_sha, p.base_sha, p.head_ref, p.base_ref, p.mergeable, p.ci_status, p.created_at, p.updated_at
                     FROM {schema}.pull_requests p
                     LEFT JOIN {schema}.pr_analyses a ON p.number = a.pr_number
                     WHERE p.state = 'open' AND a.pr_number IS NULL
                     ORDER BY p.number DESC",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
                Ok(rows.iter().map(row_to_pull).collect())
            })
        }

        fn get_pr_analysis(&self, pr_number: u64) -> Result<Option<PrAnalysisRow>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT pr_number, summary, risk_level, pr_type, review_notes, analyzed_at
                     FROM {schema}.pr_analyses WHERE pr_number = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(pr_number as i64)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.map(|r| PrAnalysisRow {
                    pr_number: r.get::<i64, _>("pr_number") as u64,
                    summary: r.get("summary"),
                    risk_level: r.get("risk_level"),
                    pr_type: r.get("pr_type"),
                    review_notes: r.get("review_notes"),
                    analyzed_at: r.get("analyzed_at"),
                }))
            })
        }

        // ── Triage ──────────────────────────────────────────────

        fn upsert_triage_result(
            &self,
            result: &IssueClassification,
            issue_number: u64,
        ) -> Result<()> {
            self.block_on(async {
                let suggested_labels = serde_json::to_string(&result.suggested_labels)?;
                let relevant_files = serde_json::to_string(&result.relevant_files)?;
                let now = chrono::Utc::now().to_rfc3339();
                let sql = format!(
                    "INSERT INTO {schema}.triage_results
                        (issue_number, category, confidence, priority, summary, suggested_labels, is_duplicate_of, is_simple_fix, relevant_files, acted_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                     ON CONFLICT (issue_number) DO UPDATE SET
                        category = EXCLUDED.category,
                        confidence = EXCLUDED.confidence,
                        priority = EXCLUDED.priority,
                        summary = EXCLUDED.summary,
                        suggested_labels = EXCLUDED.suggested_labels,
                        is_duplicate_of = EXCLUDED.is_duplicate_of,
                        is_simple_fix = EXCLUDED.is_simple_fix,
                        relevant_files = EXCLUDED.relevant_files,
                        acted_at = EXCLUDED.acted_at",
                    schema = self.schema
                );
                sqlx::query(&sql)
                    .bind(issue_number as i64)
                    .bind(&result.category)
                    .bind(result.confidence)
                    .bind(&result.priority)
                    .bind(&result.summary)
                    .bind(&suggested_labels)
                    .bind(result.is_duplicate_of.map(|n| n as i64))
                    .bind(result.is_simple_fix)
                    .bind(&relevant_files)
                    .bind(&now)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
        }

        fn get_triage_result(&self, issue_number: u64) -> Result<Option<TriageResultRow>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT issue_number, category, confidence, priority, summary, is_simple_fix, acted_at
                     FROM {schema}.triage_results WHERE issue_number = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(issue_number as i64)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.map(|r| row_to_triage(&r)))
            })
        }

        fn get_stale_triage_results(&self, max_age_hours: u32) -> Result<Vec<TriageResultRow>> {
            self.block_on(async {
                let cutoff = chrono::Utc::now() - chrono::Duration::hours(max_age_hours as i64);
                let cutoff_str = cutoff.to_rfc3339();
                let sql = format!(
                    "SELECT t.issue_number, t.category, t.confidence, t.priority, t.summary, t.is_simple_fix, t.acted_at
                     FROM {schema}.triage_results t
                     JOIN {schema}.issues i ON t.issue_number = i.number
                     WHERE i.state = 'open' AND t.acted_at < $1
                     ORDER BY t.acted_at ASC",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql)
                    .bind(&cutoff_str)
                    .fetch_all(&self.pool)
                    .await?;
                Ok(rows.iter().map(row_to_triage).collect())
            })
        }

        fn get_wshm_applied_labels(&self, issue_number: u64) -> Result<Vec<String>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT suggested_labels FROM {schema}.triage_results WHERE issue_number = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(issue_number as i64)
                    .fetch_optional(&self.pool)
                    .await?;
                match row {
                    Some(r) => {
                        let json: String = r.get("suggested_labels");
                        Ok(serde_json::from_str(&json).unwrap_or_default())
                    }
                    None => Ok(Vec::new()),
                }
            })
        }

        fn recent_activity(&self, limit: usize) -> Result<Vec<TriageResultRow>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT t.issue_number, t.category, t.confidence, t.priority, t.summary, t.is_simple_fix, t.acted_at
                     FROM {schema}.triage_results t
                     ORDER BY t.acted_at DESC
                     LIMIT $1",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql)
                    .bind(limit as i64)
                    .fetch_all(&self.pool)
                    .await?;
                Ok(rows.iter().map(row_to_triage).collect())
            })
        }

        fn is_triaged(&self, issue_number: u64) -> Result<bool> {
            self.block_on(async {
                let sql = format!(
                    "SELECT COUNT(*) as cnt FROM {schema}.triage_results WHERE issue_number = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(issue_number as i64)
                    .fetch_one(&self.pool)
                    .await?;
                let count: i64 = row.get("cnt");
                Ok(count > 0)
            })
        }

        // ── Sync Log ────────────────────────────────────────────

        fn get_sync_entry(&self, table_name: &str) -> Result<Option<SyncEntry>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT table_name, last_synced_at, etag FROM {schema}.sync_log WHERE table_name = $1",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(table_name)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.map(|r| SyncEntry {
                    table_name: r.get("table_name"),
                    last_synced_at: r.get("last_synced_at"),
                    etag: r.get("etag"),
                }))
            })
        }

        fn update_sync_entry(
            &self,
            table_name: &str,
            last_synced_at: &str,
            etag: Option<&str>,
        ) -> Result<()> {
            self.block_on(async {
                let sql = format!(
                    "INSERT INTO {schema}.sync_log (table_name, last_synced_at, etag)
                     VALUES ($1, $2, $3)
                     ON CONFLICT (table_name) DO UPDATE SET
                        last_synced_at = EXCLUDED.last_synced_at,
                        etag = EXCLUDED.etag",
                    schema = self.schema
                );
                sqlx::query(&sql)
                    .bind(table_name)
                    .bind(last_synced_at)
                    .bind(etag)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
        }

        // ── Webhook Events ──────────────────────────────────────

        fn insert_webhook_event(
            &self,
            event_type: &str,
            action: &str,
            number: Option<u64>,
            payload: &str,
        ) -> Result<i64> {
            self.block_on(async {
                let now = chrono::Utc::now().to_rfc3339();
                let sql = format!(
                    "INSERT INTO {schema}.webhook_events (event_type, action, number, payload, status, received_at)
                     VALUES ($1, $2, $3, $4, 'pending', $5)
                     RETURNING id",
                    schema = self.schema
                );
                let row = sqlx::query(&sql)
                    .bind(event_type)
                    .bind(action)
                    .bind(number.map(|n| n as i64))
                    .bind(payload)
                    .bind(&now)
                    .fetch_one(&self.pool)
                    .await?;
                let id: i64 = row.get("id");
                Ok(id)
            })
        }

        fn update_event_status(&self, id: i64, status: &str, error: Option<&str>) -> Result<()> {
            self.block_on(async {
                let now = chrono::Utc::now().to_rfc3339();
                let sql = format!(
                    "UPDATE {schema}.webhook_events SET status = $1, error = $2, processed_at = $3 WHERE id = $4",
                    schema = self.schema
                );
                sqlx::query(&sql)
                    .bind(status)
                    .bind(error)
                    .bind(&now)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            })
        }

        fn pending_event_count(&self) -> Result<u64> {
            self.block_on(async {
                let sql = format!(
                    "SELECT COUNT(*) as cnt FROM {schema}.webhook_events WHERE status = 'pending'",
                    schema = self.schema
                );
                let row = sqlx::query(&sql).fetch_one(&self.pool).await?;
                let count: i64 = row.get("cnt");
                Ok(count as u64)
            })
        }

        fn cleanup_old_events(&self, days: u32) -> Result<u64> {
            self.block_on(async {
                let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
                let cutoff_str = cutoff.to_rfc3339();
                let sql = format!(
                    "DELETE FROM {schema}.webhook_events WHERE status IN ('done', 'failed') AND received_at < $1",
                    schema = self.schema
                );
                let result = sqlx::query(&sql)
                    .bind(&cutoff_str)
                    .execute(&self.pool)
                    .await?;
                Ok(result.rows_affected())
            })
        }

        fn get_pending_events(&self) -> Result<Vec<WebhookEventRow>> {
            self.block_on(async {
                let sql = format!(
                    "SELECT id, event_type, action, number, payload, status, error, received_at, processed_at
                     FROM {schema}.webhook_events WHERE status = 'pending' ORDER BY id ASC",
                    schema = self.schema
                );
                let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
                Ok(rows
                    .iter()
                    .map(|r| WebhookEventRow {
                        id: r.get("id"),
                        event_type: r.get("event_type"),
                        action: r.get("action"),
                        number: r.get::<Option<i64>, _>("number").map(|n| n as u64),
                        payload: r.get("payload"),
                        status: r.get("status"),
                        error: r.get("error"),
                        received_at: r.get("received_at"),
                        processed_at: r.get("processed_at"),
                    })
                    .collect())
            })
        }
    }

    // ── Row conversion helpers ──────────────────────────────────

    fn row_to_issue(row: &sqlx::postgres::PgRow) -> Issue {
        let labels_json: String = row.get("labels");
        Issue {
            number: row.get::<i64, _>("number") as u64,
            title: row.get("title"),
            body: row.get("body"),
            state: row.get("state"),
            labels: parse_labels_json(&labels_json),
            author: row.get("author"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            reactions_plus1: row.get::<i32, _>("reactions_plus1") as u32,
            reactions_total: row.get::<i32, _>("reactions_total") as u32,
        }
    }

    fn row_to_pull(row: &sqlx::postgres::PgRow) -> PullRequest {
        let labels_json: String = row.get("labels");
        PullRequest {
            number: row.get::<i64, _>("number") as u64,
            title: row.get("title"),
            body: row.get("body"),
            state: row.get("state"),
            labels: parse_labels_json(&labels_json),
            author: row.get("author"),
            head_sha: row.get("head_sha"),
            base_sha: row.get("base_sha"),
            head_ref: row.get("head_ref"),
            base_ref: row.get("base_ref"),
            mergeable: row.get("mergeable"),
            ci_status: row.get("ci_status"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }

    fn row_to_triage(row: &sqlx::postgres::PgRow) -> TriageResultRow {
        TriageResultRow {
            issue_number: row.get::<i64, _>("issue_number") as u64,
            category: row.get("category"),
            confidence: row.get("confidence"),
            priority: row.get("priority"),
            summary: row.get("summary"),
            is_simple_fix: row.get("is_simple_fix"),
            acted_at: row.get("acted_at"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_schema_from_slug() {
            assert_eq!(schema_from_slug("rtk-ai/rtk"), "wshm_rtk_ai_rtk");
            assert_eq!(schema_from_slug("wshm-dev/wshm"), "wshm_wshm_dev_wshm");
            assert_eq!(
                schema_from_slug("my.org/repo-name"),
                "wshm_my_org_repo_name"
            );
        }
    }
}

#[cfg(feature = "database-postgres")]
pub use inner::*;
