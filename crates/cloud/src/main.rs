use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use anyplay_cloud::cli::{Cli, Command};
use anyplay_cloud::{
    Config, Tuning, config, mail, open, serve, serve_metrics, shutdown_signal, signing_key,
};
use clap::Parser as _;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,sqlx=warn".into());
    match config.log_format {
        config::LogFormat::Text => tracing_subscriber::fmt().with_env_filter(filter).init(),
        config::LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init(),
    }
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => run(config).await,
        Command::Keygen => {
            let (signer, created) = signing_key(&config)?;
            if let Some(file) = &config.signing_key_file {
                eprintln!(
                    "Read the signing key in {}; nothing was written.",
                    file.display()
                );
                println!("{}", signer.verifier().to_base64());
                return Ok(());
            }
            if created {
                eprintln!(
                    "Created {}/{}.",
                    config.data_dir.display(),
                    anyplay_cloud::KEY_FILE
                );
            } else {
                eprintln!("The signing key exists already; nothing changed.");
            }
            eprintln!(
                "Relays need this public key (or the file {}/{}):",
                config.data_dir.display(),
                anyplay_cloud::PUBLIC_KEY_FILE
            );
            println!("{}", signer.verifier().to_base64());
            Ok(())
        }
        Command::Healthcheck { url } => anyplay_cloud::cli::healthcheck(&config, url).await,
        Command::Admin { url, action } => anyplay_cloud::cli::admin(&config, url, action).await,
    }
}

async fn run(config: Config) -> Result<()> {
    let listen = config.listen;
    let mailer = mail::from_config(&config)?;
    if config.mail == config::MailMode::Smtp {
        tracing::info!("sign-in codes go out by SMTP");
    }
    if config.admin_token.is_none() {
        tracing::info!("admin_token is not set: the admin routes are off");
    }
    let metrics_listen = config.metrics_listen;
    let drain = Duration::from_secs(config.drain_seconds);
    let state = open(config, Tuning::default(), Arc::clone(&mailer)).await?;
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    if let Some(address) = metrics_listen {
        let metrics = TcpListener::bind(address)
            .await
            .with_context(|| format!("cannot listen on {address} for metrics"))?;
        tokio::spawn(serve_metrics(metrics, state.clone()));
        tracing::info!(%address, "metrics served");
    }
    tracing::info!(%listen, name = %state.config.name, "cloud running");

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(serve(listener, state, async move {
        let _ = stopped.await;
    }));
    tokio::select! {
        result = &mut server => {
            result.context("the server task failed")??;
            return Ok(());
        }
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
    let _ = stop.send(());
    match tokio::time::timeout(drain + Duration::from_secs(10), server).await {
        Ok(result) => result.context("the server task failed")??,
        Err(_) => tracing::warn!("some connections did not close in time; stopping anyway"),
    }
    tracing::info!("stopped");
    Ok(())
}
