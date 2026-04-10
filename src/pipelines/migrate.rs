use anyhow::Result;

use crate::cli::MigrateArgs;

/// Sanitize a repo slug into a valid PostgreSQL schema name.
/// e.g. "rtk-ai/rtk" -> "wshm_rtk_ai_rtk"
#[cfg_attr(not(feature = "export-postgres"), allow(dead_code))]
fn sanitize_schema_name(slug: &str) -> String {
    let sanitized: String = slug
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    format!("wshm_{}", sanitized)
}

/// PostgreSQL DDL that mirrors the SQLite schema.
#[cfg_attr(not(feature = "export-postgres"), allow(dead_code))]
fn pg_create_tables_ddl(schema: &str) -> String {
    format!(
        r#"
CREATE SCHEMA IF NOT EXISTS {schema};

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
    issue_number     BIGINT PRIMARY KEY,
    category         TEXT NOT NULL,
    confidence       DOUBLE PRECISION NOT NULL,
    priority         TEXT,
    summary          TEXT,
    suggested_labels TEXT NOT NULL DEFAULT '[]',
    is_duplicate_of  BIGINT,
    is_simple_fix    BOOLEAN NOT NULL DEFAULT FALSE,
    relevant_files   TEXT NOT NULL DEFAULT '[]',
    acted_at         TEXT NOT NULL
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

CREATE INDEX IF NOT EXISTS {schema}_idx_issues_state ON {schema}.issues(state);
CREATE INDEX IF NOT EXISTS {schema}_idx_pulls_state ON {schema}.pull_requests(state);
CREATE INDEX IF NOT EXISTS {schema}_idx_comments_issue ON {schema}.comments(issue_number);
CREATE INDEX IF NOT EXISTS {schema}_idx_webhook_status ON {schema}.webhook_events(status);
CREATE INDEX IF NOT EXISTS {schema}_idx_triage_acted_at ON {schema}.triage_results(acted_at);
"#,
        schema = schema
    )
}

/// Migrate a single SQLite database to PostgreSQL.
#[cfg(feature = "export-postgres")]
async fn migrate_one(
    db: &crate::db::Database,
    slug: &str,
    pool: &sqlx::PgPool,
) -> Result<MigrationSummary> {
    use anyhow::Context;
    use sqlx::Executor;

    let schema = sanitize_schema_name(slug);
    let ddl = pg_create_tables_ddl(&schema);

    // Create schema and tables
    pool.execute(ddl.as_str())
        .await
        .with_context(|| format!("Failed to create schema '{}'", schema))?;

    let mut summary = MigrationSummary {
        slug: slug.to_string(),
        issues: 0,
        pull_requests: 0,
        comments: 0,
        labels: 0,
        triage_results: 0,
        pr_analyses: 0,
        sync_log: 0,
        webhook_events: 0,
    };

    // --- Issues ---
    let issues: Vec<(i64, String, Option<String>, String, String, Option<String>, String, String, i64, i64)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total FROM issues"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &issues {
        let q = format!(
            "INSERT INTO {schema}.issues (number, title, body, state, labels, author, created_at, updated_at, reactions_plus1, reactions_total)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (number) DO UPDATE SET
                title = EXCLUDED.title, body = EXCLUDED.body, state = EXCLUDED.state,
                labels = EXCLUDED.labels, author = EXCLUDED.author,
                updated_at = EXCLUDED.updated_at, reactions_plus1 = EXCLUDED.reactions_plus1,
                reactions_total = EXCLUDED.reactions_total"
        );
        sqlx::query(&q)
            .bind(row.0).bind(&row.1).bind(&row.2).bind(&row.3)
            .bind(&row.4).bind(&row.5).bind(&row.6).bind(&row.7)
            .bind(row.8 as i32).bind(row.9 as i32)
            .execute(pool)
            .await?;
    }
    summary.issues = issues.len();

    // --- Pull Requests ---
    let pulls: Vec<(i64, String, Option<String>, String, String, Option<String>,
                     Option<String>, Option<String>, Option<String>, Option<String>,
                     Option<bool>, Option<String>, String, String)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at FROM pull_requests"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<bool>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &pulls {
        let q = format!(
            "INSERT INTO {schema}.pull_requests (number, title, body, state, labels, author, head_sha, base_sha, head_ref, base_ref, mergeable, ci_status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (number) DO UPDATE SET
                title = EXCLUDED.title, body = EXCLUDED.body, state = EXCLUDED.state,
                labels = EXCLUDED.labels, author = EXCLUDED.author,
                head_sha = EXCLUDED.head_sha, base_sha = EXCLUDED.base_sha,
                head_ref = EXCLUDED.head_ref, base_ref = EXCLUDED.base_ref,
                mergeable = EXCLUDED.mergeable, ci_status = EXCLUDED.ci_status,
                updated_at = EXCLUDED.updated_at"
        );
        sqlx::query(&q)
            .bind(row.0).bind(&row.1).bind(&row.2).bind(&row.3)
            .bind(&row.4).bind(&row.5).bind(&row.6).bind(&row.7)
            .bind(&row.8).bind(&row.9).bind(row.10).bind(&row.11)
            .bind(&row.12).bind(&row.13)
            .execute(pool)
            .await?;
    }
    summary.pull_requests = pulls.len();

    // --- Comments ---
    let comments: Vec<(i64, i64, String, Option<String>, String)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, issue_number, body, author, created_at FROM comments"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &comments {
        let q = format!(
            "INSERT INTO {schema}.comments (id, issue_number, body, author, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                body = EXCLUDED.body, author = EXCLUDED.author"
        );
        sqlx::query(&q)
            .bind(row.0).bind(row.1).bind(&row.2).bind(&row.3).bind(&row.4)
            .execute(pool)
            .await?;
    }
    summary.comments = comments.len();

    // --- Labels ---
    let labels: Vec<(String, Option<String>, Option<String>)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name, color, description FROM labels"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &labels {
        let q = format!(
            "INSERT INTO {schema}.labels (name, color, description)
             VALUES ($1, $2, $3)
             ON CONFLICT (name) DO UPDATE SET
                color = EXCLUDED.color, description = EXCLUDED.description"
        );
        sqlx::query(&q)
            .bind(&row.0).bind(&row.1).bind(&row.2)
            .execute(pool)
            .await?;
    }
    summary.labels = labels.len();

    // --- Triage Results ---
    let triage: Vec<(i64, String, f64, Option<String>, Option<String>, String, Option<i64>, bool, String, String)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT issue_number, category, confidence, priority, summary, suggested_labels, is_duplicate_of, is_simple_fix, relevant_files, acted_at FROM triage_results"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &triage {
        let q = format!(
            "INSERT INTO {schema}.triage_results (issue_number, category, confidence, priority, summary, suggested_labels, is_duplicate_of, is_simple_fix, relevant_files, acted_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (issue_number) DO UPDATE SET
                category = EXCLUDED.category, confidence = EXCLUDED.confidence,
                priority = EXCLUDED.priority, summary = EXCLUDED.summary,
                suggested_labels = EXCLUDED.suggested_labels, is_duplicate_of = EXCLUDED.is_duplicate_of,
                is_simple_fix = EXCLUDED.is_simple_fix, relevant_files = EXCLUDED.relevant_files,
                acted_at = EXCLUDED.acted_at"
        );
        sqlx::query(&q)
            .bind(row.0).bind(&row.1).bind(row.2).bind(&row.3)
            .bind(&row.4).bind(&row.5).bind(row.6).bind(row.7)
            .bind(&row.8).bind(&row.9)
            .execute(pool)
            .await?;
    }
    summary.triage_results = triage.len();

    // --- PR Analyses ---
    let analyses: Vec<(i64, String, String, String, Option<String>, String)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT pr_number, summary, risk_level, pr_type, review_notes, analyzed_at FROM pr_analyses"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &analyses {
        let q = format!(
            "INSERT INTO {schema}.pr_analyses (pr_number, summary, risk_level, pr_type, review_notes, analyzed_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (pr_number) DO UPDATE SET
                summary = EXCLUDED.summary, risk_level = EXCLUDED.risk_level,
                pr_type = EXCLUDED.pr_type, review_notes = EXCLUDED.review_notes,
                analyzed_at = EXCLUDED.analyzed_at"
        );
        sqlx::query(&q)
            .bind(row.0).bind(&row.1).bind(&row.2).bind(&row.3)
            .bind(&row.4).bind(&row.5)
            .execute(pool)
            .await?;
    }
    summary.pr_analyses = analyses.len();

    // --- Sync Log ---
    let sync_entries: Vec<(String, String, Option<String>)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT table_name, last_synced_at, etag FROM sync_log"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &sync_entries {
        let q = format!(
            "INSERT INTO {schema}.sync_log (table_name, last_synced_at, etag)
             VALUES ($1, $2, $3)
             ON CONFLICT (table_name) DO UPDATE SET
                last_synced_at = EXCLUDED.last_synced_at, etag = EXCLUDED.etag"
        );
        sqlx::query(&q)
            .bind(&row.0).bind(&row.1).bind(&row.2)
            .execute(pool)
            .await?;
    }
    summary.sync_log = sync_entries.len();

    // --- Webhook Events ---
    let events: Vec<(i64, String, String, Option<i64>, String, String, Option<String>, String, Option<String>)> =
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, event_type, action, number, payload, status, error, received_at, processed_at FROM webhook_events"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

    for row in &events {
        let q = format!(
            "INSERT INTO {schema}.webhook_events (id, event_type, action, number, payload, status, error, received_at, processed_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO UPDATE SET
                status = EXCLUDED.status, error = EXCLUDED.error,
                processed_at = EXCLUDED.processed_at"
        );
        sqlx::query(&q)
            .bind(row.0).bind(&row.1).bind(&row.2).bind(row.3)
            .bind(&row.4).bind(&row.5).bind(&row.6).bind(&row.7)
            .bind(&row.8)
            .execute(pool)
            .await?;
    }
    summary.webhook_events = events.len();

    Ok(summary)
}

#[cfg(feature = "export-postgres")]
struct MigrationSummary {
    slug: String,
    issues: usize,
    pull_requests: usize,
    comments: usize,
    labels: usize,
    triage_results: usize,
    pr_analyses: usize,
    sync_log: usize,
    webhook_events: usize,
}

#[cfg(feature = "export-postgres")]
impl std::fmt::Display for MigrationSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Migrated {}: {} issues, {} pulls, {} triage results, {} PR analyses, {} comments, {} labels, {} sync entries, {} webhook events",
            self.slug,
            self.issues,
            self.pull_requests,
            self.triage_results,
            self.pr_analyses,
            self.comments,
            self.labels,
            self.sync_log,
            self.webhook_events,
        )
    }
}

/// Entry point for `wshm migrate`.
#[cfg(feature = "export-postgres")]
pub async fn run(args: &MigrateArgs, cli: &crate::cli::Cli) -> Result<()> {
    use anyhow::Context;
    use crate::config;
    use crate::db::Database;

    if args.to != "postgresql" {
        anyhow::bail!("Unsupported target '{}'. Currently only 'postgresql' is supported.", args.to);
    }

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&args.uri)
        .await
        .with_context(|| format!("Failed to connect to PostgreSQL at '{}'", args.uri))?;

    if args.all {
        // Multi-repo mode: read global config
        let config_path = args.config.clone().unwrap_or_else(config::GlobalConfig::default_path);
        let global = config::GlobalConfig::load(&config_path)
            .with_context(|| format!("Failed to load global config from {}", config_path.display()))?;

        let enabled_repos: Vec<_> = global.repos.iter().filter(|r| r.enabled).collect();
        if enabled_repos.is_empty() {
            anyhow::bail!("No enabled repos found in {}", config_path.display());
        }

        println!("Migrating {} repos to PostgreSQL...", enabled_repos.len());
        println!();

        for repo in &enabled_repos {
            let db_path = repo.path.join(".wshm").join("state.db");
            if !db_path.exists() {
                println!("  [skip] {}: no state.db at {}", repo.slug, db_path.display());
                continue;
            }

            let db = Database::open_path(&db_path)
                .with_context(|| format!("Failed to open SQLite at {}", db_path.display()))?;

            let summary = migrate_one(&db, &repo.slug, &pool).await
                .with_context(|| format!("Failed to migrate {}", repo.slug))?;

            println!("  {}", summary);
        }
    } else {
        // Single-repo mode: use current repo
        let config = config::Config::load(cli)?;
        let db = Database::open(&config)?;
        let slug = config.repo_slug();

        println!("Migrating {} to PostgreSQL...", slug);

        let summary = migrate_one(&db, &slug, &pool).await?;
        println!("{}", summary);
    }

    pool.close().await;
    println!();
    println!("Migration complete.");

    Ok(())
}

/// Stub when the export-postgres feature is not enabled.
#[cfg(not(feature = "export-postgres"))]
pub async fn run(args: &MigrateArgs, _cli: &crate::cli::Cli) -> Result<()> {
    let _ = args;
    anyhow::bail!(
        "PostgreSQL migration requires the 'export-postgres' feature.\n\
         Rebuild with: cargo build --features export-postgres"
    )
}

/// Migrate from a specific SQLite path (used by tests or scripts).
#[cfg(feature = "export-postgres")]
pub async fn migrate_from_path(db_path: &std::path::Path, slug: &str, pg_uri: &str) -> Result<()> {
    use anyhow::Context;
    use crate::db::Database;

    let db = Database::open_path(db_path)?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(pg_uri)
        .await
        .with_context(|| format!("Failed to connect to PostgreSQL at '{}'", pg_uri))?;

    let summary = migrate_one(&db, slug, &pool).await?;
    println!("{}", summary);

    pool.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_schema_name() {
        assert_eq!(sanitize_schema_name("rtk-ai/rtk"), "wshm_rtk_ai_rtk");
        assert_eq!(sanitize_schema_name("my-org/my-repo"), "wshm_my_org_my_repo");
        assert_eq!(sanitize_schema_name("simple"), "wshm_simple");
        assert_eq!(
            sanitize_schema_name("org/repo.with.dots"),
            "wshm_org_repo_with_dots"
        );
    }

    #[test]
    fn test_ddl_contains_all_tables() {
        let ddl = pg_create_tables_ddl("wshm_test");
        assert!(ddl.contains("CREATE SCHEMA IF NOT EXISTS wshm_test"));
        assert!(ddl.contains("wshm_test.issues"));
        assert!(ddl.contains("wshm_test.pull_requests"));
        assert!(ddl.contains("wshm_test.comments"));
        assert!(ddl.contains("wshm_test.labels"));
        assert!(ddl.contains("wshm_test.triage_results"));
        assert!(ddl.contains("wshm_test.pr_analyses"));
        assert!(ddl.contains("wshm_test.sync_log"));
        assert!(ddl.contains("wshm_test.webhook_events"));
    }
}
