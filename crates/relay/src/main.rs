use std::path::PathBuf;

use anyhow::{Context as _, Result};
use anyplay_relay::config::{Config, TlsMode};
use clap::Parser;
use iroh_relay::server::Server;

/// An iroh relay that only admits endpoints holding a ticket from an AnyPlay cloud.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// The TOML settings file. Every setting can also come from an `ANYPLAY_RELAY_<NAME>`
    /// environment variable.
    #[arg(long, short, env = "ANYPLAY_RELAY_CONFIG")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // Both `ring` and `aws-lc-rs` can be linked in; name the one we use.
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("a TLS provider is already installed"))?;

    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    let verifier = config.verifier().await?;
    let server_config = config.server_config(verifier).await?;
    let mut server = Server::spawn(server_config)
        .await
        .context("cannot start the relay")?;
    tracing::info!(
        tls = ?config.tls,
        http = ?server.http_addr(),
        https = ?server.https_addr(),
        quic = ?server.quic_addr(),
        "relay running"
    );
    if config.tls == TlsMode::None {
        tracing::warn!("TLS is off: use this only for development");
    }

    tokio::select! {
        _ = shutdown_signal() => tracing::info!("shutting down"),
        result = server.join() => tracing::error!(?result, "the relay stopped"),
    }
    server.shutdown().await.context("shutting down")?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
