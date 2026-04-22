//! Shared OSS command dispatcher.
//!
//! This module contains the full dispatch logic for the OSS subcommand set.
//! It is used by both the standalone `wshm` binary (shipped with the
//! `wshm-core` crate) and the Pro `wshm-pro` binary, which falls back to
//! this dispatcher for non-Pro commands. Keeping a single source of truth
//! here ensures the OSS surface stays in sync across both releases.

use anyhow::Result;

use crate::cli::{Cli, Command};
use crate::pipelines::triage::OutputFormat;

/// Convert global CLI flags into the triage pipeline output format.
pub fn triage_format(cli: &Cli) -> OutputFormat {
    if cli.csv {
        OutputFormat::Csv
    } else if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Text
    }
}

/// Load the per-repo config, open the database, and build a GitHub client.
pub fn init_core(cli: &Cli) -> Result<(crate::Config, crate::Database, crate::Client)> {
    let config = crate::Config::load(cli)?;
    let db = crate::Database::open(&config)?;
    let gh = crate::Client::new(&config)?;
    Ok((config, db, gh))
}

/// Same as [`init_core`] but also builds an optional export manager.
pub fn init_full(
    cli: &Cli,
) -> Result<(
    crate::Config,
    crate::Database,
    crate::Client,
    Option<crate::export::ExportManager>,
)> {
    let (config, db, gh) = init_core(cli)?;
    let exporter = crate::export::ExportManager::from_config(&config.export)?;
    Ok((config, db, gh, exporter))
}

/// Dispatch an OSS subcommand.
///
/// This is the authoritative OSS command surface: any subcommand in
/// [`crate::cli::Command`] is handled here. Pro binaries reuse this
/// function for their non-Pro subcommands.
pub async fn run_oss(cli: Cli) -> Result<()> {
    match &cli.command {
        Some(Command::Sync) => {
            let (config, db, gh, exporter) = init_full(&cli)?;
            crate::github::sync::full_sync(&gh, &db).await?;
            if let Some(ref em) = exporter {
                em.emit(&crate::export::ExportEvent {
                    kind: crate::export::EventKind::SyncCompleted,
                    repo: config.repo_slug(),
                    timestamp: chrono::Utc::now(),
                    data: serde_json::json!({"sync_type": "full"}),
                })
                .await?;
            }
            println!("Sync complete.");
        }
        Some(Command::Triage(args)) => {
            let (config, db, gh, exporter) = init_full(&cli)?;
            if !cli.offline {
                crate::github::sync::incremental_sync(&gh, &db, "issues").await?;
            }
            crate::pipelines::triage::run(
                &config,
                &db,
                &gh,
                args,
                triage_format(&cli),
                exporter.as_ref(),
            )
            .await?;
        }
        Some(Command::Pr(args)) => {
            let (config, db, gh, exporter) = init_full(&cli)?;
            if !cli.offline {
                crate::github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }
            crate::pipelines::pr_analysis::run(
                &config,
                &db,
                &gh,
                args,
                cli.json,
                exporter.as_ref(),
            )
            .await?;
        }
        Some(Command::Queue(args)) => {
            let (config, db, gh, exporter) = init_full(&cli)?;
            if !cli.offline {
                crate::github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }
            crate::pipelines::merge_queue::run(
                &config,
                &db,
                &gh,
                args,
                cli.json,
                exporter.as_ref(),
            )
            .await?;
        }
        Some(Command::Run(args)) => {
            let (config, db, gh, exporter) = init_full(&cli)?;
            if !cli.offline {
                crate::github::sync::incremental_sync(&gh, &db, "issues").await?;
                crate::github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            let triage_args = crate::cli::TriageArgs {
                issue: None,
                apply: args.apply,
                retriage: false,
            };
            crate::pipelines::triage::run(
                &config,
                &db,
                &gh,
                &triage_args,
                triage_format(&cli),
                exporter.as_ref(),
            )
            .await?;

            let pr_args = crate::cli::PrArgs {
                pr: None,
                apply: args.apply,
            };
            crate::pipelines::pr_analysis::run(
                &config,
                &db,
                &gh,
                &pr_args,
                cli.json,
                exporter.as_ref(),
            )
            .await?;

            let queue_args = crate::cli::QueueArgs { apply: args.apply };
            crate::pipelines::merge_queue::run(
                &config,
                &db,
                &gh,
                &queue_args,
                cli.json,
                exporter.as_ref(),
            )
            .await?;

            // NOTE: Pro-only conflict resolution is invoked from the
            // wshm-pro `run_oss_command` wrapper (which calls this
            // function, then runs conflict_resolution separately).
            // The OSS binary does not include conflict resolution.

            if !cli.json {
                println!("Full cycle complete.");
            }
        }
        Some(Command::Health(args)) => {
            let config = crate::Config::load(&cli)?;
            let db = crate::Database::open(&config)?;
            if !cli.offline {
                let gh = crate::Client::new(&config)?;
                crate::github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }
            crate::pipelines::pr_health::run(&db, args, cli.json)?;
        }
        Some(Command::Context) => {
            let config = crate::Config::load(&cli)?;
            let db = crate::Database::open(&config)?;
            let slug = config.repo_slug();
            crate::pipelines::context::run(&db, &slug)?;
        }
)
        Some(Command::Login(args)) => {
            if args.license {
                crate::license::login()?;
            }
            if !args.license {
                crate::login::run(args)?;
            }
        }
        Some(Command::Update(args)) => {
            crate::update::check_and_update(args.apply, cli.json).await?;
        }
        Some(Command::Config(config_cmd)) => match config_cmd {
            crate::cli::ConfigCommand::Init => {
                crate::config::Config::init_template()?;
                println!("Created .wshm/config.toml template.");
            }
        },
        Some(Command::Migrate(args)) => {
            crate::pipelines::migrate::run(args, &cli).await?;
        }
        Some(Command::Revert(args)) => {
            let (_config, db, gh, _) = init_full(&cli)?;
            if !cli.offline {
                crate::github::sync::full_sync(&gh, &db).await?;
            }
            crate::pipelines::revert::run(&db, &gh, args.apply).await?;
        }
        Some(Command::Backup(args)) => {
            crate::pipelines::backup::backup(args)?;
        }
        Some(Command::Restore(args)) => {
            crate::pipelines::backup::restore(args)?;
        }
        Some(Command::Telemetry(args)) => {
            handle_telemetry_command(args)?;
        }
        Some(Command::Summary) => {
            let config = crate::Config::load(&cli)?;
            let db = crate::Database::open(&config)?;
            if !cli.offline {
                let gh = crate::Client::new(&config)?;
                crate::github::sync::incremental_sync(&gh, &db, "issues").await?;
                crate::github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }
            crate::pipelines::status::show_summary(&config, &db, cli.json)?;
        }
        Some(Command::Tui) => match crate::Config::load(&cli) {
            Ok(config) => {
                let db = crate::Database::open(&config)?;
                crate::tui::run(&config, &db).await?;
            }
            Err(_) => {
                let global_path = crate::config::GlobalConfig::default_path();
                if !global_path.exists() {
                    anyhow::bail!("Not in a git repo and no ~/.wshm/global.toml found.");
                }
                let global = crate::config::GlobalConfig::load(&global_path)?;
                let first = global
                    .repos
                    .iter()
                    .find(|r| r.enabled)
                    .ok_or_else(|| anyhow::anyhow!("No enabled repos in global.toml"))?;
                let config = crate::config::Config::load_for_repo(&first.path, &first.slug)?;
                let db = crate::Database::open(&config)?;
                crate::tui::run(&config, &db).await?;
            }
        },
        Some(Command::Daemon(args)) => {
            // Handle systemd install/uninstall first
            if args.install {
                return crate::daemon::systemd::install(args);
            }
            if args.uninstall {
                return crate::daemon::systemd::uninstall();
            }

            // Multi-repo mode: load global config if provided or if present at default path
            let global_path = args.config.clone().unwrap_or_else(|| {
                crate::config::GlobalConfig::default_path()
            });

            if global_path.exists() {
                let global = crate::config::GlobalConfig::load(&global_path)?;
                crate::daemon::run_multi(global, args.clone()).await?;
            } else {
                // Single-repo mode: load per-repo config
                let config = crate::Config::load(&cli)?;
                crate::daemon::run(config, args.clone()).await?;
            }
        }
        None => {
            let config = crate::Config::load(&cli)?;
            let db = crate::Database::open(&config)?;
            crate::pipelines::status::show(&db, cli.json)?;
        }
    }

    Ok(())
}

/// Handle the `wshm telemetry` subcommand (GDPR consent management).
fn handle_telemetry_command(args: &crate::cli::TelemetryArgs) -> Result<()> {
    use crate::telemetry::{consent_state, set_consent, ConsentState};

    if args.accept {
        set_consent(true)?;
        println!("Telemetry: ACCEPTED");
        println!("Anonymous pings will be sent occasionally (see `wshm telemetry --status`).");
        println!("Revoke anytime with `wshm telemetry --decline`.");
    } else if args.decline {
        set_consent(false)?;
        println!("Telemetry: DECLINED");
        println!("No telemetry will be sent.");
    } else {
        // Default: show status
        let state = consent_state();
        let status = match state {
            ConsentState::Accepted => "ACCEPTED",
            ConsentState::Declined => "DECLINED",
            ConsentState::Unknown => "NOT SET (default: disabled)",
        };
        println!("Telemetry consent: {status}");
        println!();
        println!("Manage:");
        println!("  wshm telemetry --accept    # enable anonymous telemetry");
        println!("  wshm telemetry --decline   # disable (default)");
        println!();
        println!("Env override: WSHM_TELEMETRY_DISABLED=1");
        println!();
        println!("What we collect (ONLY if accepted):");
        println!("  - Anonymous device hash (SHA256, not reversible)");
        println!("  - wshm version, OS, architecture");
        println!("  - Number of configured repos (count only)");
        println!("  - Install method (brew/cargo/manual)");
        println!();
        println!("What we NEVER collect:");
        println!("  - Repo names, URLs, or content");
        println!("  - Issue or PR content");
        println!("  - API tokens or credentials");
    }
    Ok(())
}
