use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod ai;
mod cli;
mod config;
mod daemon;
mod db;
mod github;
mod icm;
mod login;
mod pipelines;
mod update;

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    // Binary integrity check (skip for --help/--version which exit early)
    match update::verify_binary_integrity() {
        Ok(true) => tracing::debug!("Binary integrity check passed"),
        Ok(false) => {
            eprintln!("⚠️  WARNING: Binary integrity check FAILED — the wshm binary may have been tampered with.");
            eprintln!("   Run `wshm update --apply` to reinstall from a verified release.");
        }
        Err(_) => {} // No hash stored — first run or manual install, skip silently
    }

    // Inject stored credentials from .wshm/credentials into env
    login::inject_credentials();

    match &cli.command {
        Some(Command::Sync) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;
            github::sync::full_sync(&gh, &db).await?;
            println!("Sync complete.");
        }
        Some(Command::Triage(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "issues").await?;
            }

            pipelines::triage::run(&config, &db, &gh, args, cli.json).await?;
        }
        Some(Command::Pr(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            pipelines::pr_analysis::run(&config, &db, &gh, args, cli.json).await?;
        }
        Some(Command::Queue(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            pipelines::merge_queue::run(&config, &db, &gh, args, cli.json).await?;
        }
        Some(Command::Conflicts(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            pipelines::conflict_resolution::run(&config, &db, &gh, args, cli.json).await?;
        }
        Some(Command::Run(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "issues").await?;
                github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            let triage_args = cli::TriageArgs {
                issue: None,
                apply: args.apply,
            };
            pipelines::triage::run(&config, &db, &gh, &triage_args, cli.json).await?;

            let pr_args = cli::PrArgs {
                pr: None,
                apply: args.apply,
            };
            pipelines::pr_analysis::run(&config, &db, &gh, &pr_args, cli.json).await?;

            let queue_args = cli::QueueArgs { apply: args.apply };
            pipelines::merge_queue::run(&config, &db, &gh, &queue_args, cli.json).await?;

            let conflict_args = cli::ConflictArgs { apply: args.apply };
            pipelines::conflict_resolution::run(&config, &db, &gh, &conflict_args, cli.json)
                .await?;

            if !cli.json {
                println!("Full cycle complete.");
            }
        }
        Some(Command::Review(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            pipelines::review::run(&config, &db, &gh, args, cli.json).await?;
        }
        Some(Command::Health(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;

            if !cli.offline {
                let gh = github::Client::new(&config)?;
                github::sync::incremental_sync(&gh, &db, "pulls").await?;
            }

            pipelines::pr_health::run(&db, args, cli.json)?;
        }
        Some(Command::Fix(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let gh = github::Client::new(&config)?;

            if !cli.offline {
                github::sync::incremental_sync(&gh, &db, "issues").await?;
            }

            pipelines::autogen::run(&config, &db, &gh, args).await?;
        }
        Some(Command::Report(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            let slug = config.repo_slug();
            pipelines::report::run(&db, args, &slug)?;
        }
        Some(Command::Changelog(args)) => {
            let config = config::Config::load(&cli)?;
            let gh = github::Client::new(&config)?;
            pipelines::changelog::run(&gh, args).await?;
        }
        Some(Command::Dashboard(args)) => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            pipelines::dashboard::run(&db, args)?;
        }
        Some(Command::Model(model_cmd)) => match model_cmd {
            cli::ModelCommand::Pull { name } => {
                ai::local::pull_model(name)?;
            }
            cli::ModelCommand::List => {
                let models = ai::local::list_models()?;
                if models.is_empty() {
                    println!("No models available.");
                } else {
                    println!("{:<20} {:<10} {}", "MODEL", "SIZE", "STATUS");
                    println!("{}", "-".repeat(45));
                    for (name, size, downloaded) in &models {
                        let size_str = format!("{:.0} MB", *size as f64 / 1_000_000.0);
                        let status = if *downloaded {
                            "downloaded"
                        } else {
                            "available"
                        };
                        println!("{:<20} {:<10} {}", name, size_str, status);
                    }
                }
            }
            cli::ModelCommand::Remove { name } => {
                let spec = ai::local::KNOWN_MODELS.iter().find(|m| m.name == name);
                let filename = spec.map(|s| s.filename).unwrap_or(name.as_str());
                let path = ai::local::models_dir().join(filename);
                if path.exists() {
                    std::fs::remove_file(&path)?;
                    println!("Removed model: {name}");
                } else {
                    println!("Model '{name}' not found locally.");
                }
            }
        },
        Some(Command::Login(args)) => {
            login::run(args)?;
        }
        Some(Command::Update(args)) => {
            update::check_and_update(args.apply, cli.json).await?;
        }
        Some(Command::Daemon(args)) => {
            if args.install {
                daemon::systemd::install(args)?;
                return Ok(());
            }
            if args.uninstall {
                daemon::systemd::uninstall()?;
                return Ok(());
            }
            let config = config::Config::load(&cli)?;
            daemon::run(config, args.clone()).await?;
        }
        Some(Command::Config(config_cmd)) => match config_cmd {
            cli::ConfigCommand::Init => {
                config::Config::init_template()?;
                println!("Created .wshm/config.toml template.");
            }
        },
        None => {
            let config = config::Config::load(&cli)?;
            let db = db::Database::open(&config)?;
            pipelines::status::show(&db, cli.json)?;
        }
    }

    Ok(())
}
