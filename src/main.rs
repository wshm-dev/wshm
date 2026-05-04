use anyhow::Result;
use clap::Parser;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let log_buffer = wshm_core::daemon::log_buffer::install_global();
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .with(wshm_core::daemon::log_buffer::LogLayer::new(log_buffer))
        .init();
    wshm_core::telemetry::maybe_ping();
    wshm_core::login::inject_credentials();
    let cli = wshm_core::Cli::parse();
    wshm_core::run_oss(cli).await
}
