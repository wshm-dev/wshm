use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    wshm_core::telemetry::maybe_ping();
    wshm_core::login::inject_credentials();
    let cli = wshm_core::Cli::parse();
    wshm_core::run_oss(cli).await
}
