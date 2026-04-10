//! Standalone OSS `wshm` binary.
//!
//! This is the community release of wshm. It exposes exactly the OSS
//! command surface defined in [`wshm_core::cli::Cli`] and dispatches via
//! [`wshm_core::run_oss`]. Pro subcommands (`review`, `fix`, `improve`,
//! `conflicts`, `report`) are not compiled into this binary; they live in
//! the separate `wshm-pro` crate.

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use wshm_core::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Telemetry ping (no-op if disabled).
    wshm_core::telemetry::maybe_ping();

    // Binary integrity check (best-effort; failures are logged only).
    match wshm_core::update::verify_binary_integrity() {
        Ok(true) => tracing::debug!("Binary integrity check passed"),
        Ok(false) => {
            eprintln!("⚠️  WARNING: Binary integrity check FAILED.");
            eprintln!("   Run `wshm update --apply` to reinstall from a verified release.");
        }
        Err(_) => {}
    }

    // Inject any stored credentials into the environment.
    wshm_core::login::inject_credentials();

    let cli = Cli::parse();
    wshm_core::run_oss(cli).await
}
