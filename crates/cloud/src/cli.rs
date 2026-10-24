//! The command line: `serve` (the default), `keygen` and the `admin` commands that call the admin
//! routes of a running cloud.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::routes::admin::{BAN, BANS, BanAnswer, BanRequest, REPORTS, ReportView};

#[derive(Parser, Debug)]
#[command(version, about = "The central services of the AnyPlay network")]
pub struct Cli {
    /// The TOML settings file. Every setting can also come from an `ANYPLAY_CLOUD_<NAME>`
    /// environment variable.
    #[arg(long, short, env = "ANYPLAY_CLOUD_CONFIG", global = true)]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the cloud. This is the default.
    Serve,
    /// Create the signing key if it does not exist, and print its public key for relays.
    Keygen,
    /// Exit with an error unless a running cloud answers. For container health checks.
    Healthcheck {
        /// Where the cloud listens. Default: the `listen` setting, on this machine.
        #[arg(long)]
        url: Option<String>,
    },
    /// Moderation, through the admin routes of a running cloud. Needs `admin_token`.
    Admin {
        /// Where the cloud listens. Default: the `listen` setting, on this machine.
        #[arg(long)]
        url: Option<String>,
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum AdminAction {
    /// Ban an account, and stop its e-mail address from signing up again.
    Ban {
        handle: String,
        /// Ban for this many days. Without it the ban is for good.
        #[arg(long)]
        days: Option<u64>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Lift a ban.
    Unban { handle: String },
    /// List the newest reports.
    Reports {
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

/// Runs an admin command and prints the result.
pub async fn admin(config: &Config, url: Option<String>, action: AdminAction) -> Result<()> {
    let token = config
        .admin_token
        .as_deref()
        .context("admin_token is not set; the admin routes are off")?;
    let base = url.unwrap_or_else(|| config.local_url());
    let base = base.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .user_agent(concat!("anyplay-cloud/", env!("CARGO_PKG_VERSION")))
        .build()?;

    match action {
        AdminAction::Ban {
            handle,
            days,
            reason,
        } => {
            let response = client
                .post(format!("{base}{BANS}"))
                .bearer_auth(token)
                .json(&BanRequest {
                    handle,
                    days,
                    reason,
                })
                .send()
                .await?;
            let answer: BanAnswer = read(response).await?;
            match answer.until {
                Some(until) => {
                    println!("Banned {} until Unix time {until}.", answer.account.handle)
                }
                None => println!("Banned {} for good.", answer.account.handle),
            }
        }
        AdminAction::Unban { handle } => {
            let response = client
                .delete(format!("{base}{BAN}").replace("{handle}", &handle))
                .bearer_auth(token)
                .send()
                .await?;
            check(response).await?;
            println!("Lifted the ban on {handle}.");
        }
        AdminAction::Reports { limit } => {
            let response = client
                .get(format!("{base}{REPORTS}?limit={limit}"))
                .bearer_auth(token)
                .send()
                .await?;
            let reports: Vec<ReportView> = read(response).await?;
            if reports.is_empty() {
                println!("No reports.");
            }
            for r in reports {
                println!(
                    "#{} at {}: {} reported {} ({}) {}",
                    r.id, r.created, r.reporter.handle, r.reported.handle, r.reason, r.details
                );
            }
        }
    }
    Ok(())
}

async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("the cloud answered {status}: {body}")
}

async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    Ok(check(response).await?.json().await?)
}

/// Asks a running cloud for `/v1/info`. For container health checks.
pub async fn healthcheck(config: &Config, url: Option<String>) -> Result<()> {
    let base = url.unwrap_or_else(|| config.local_url());
    let response = reqwest::Client::new()
        .get(format!(
            "{}{}",
            base.trim_end_matches('/'),
            crate::routes::HEALTH
        ))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("the cloud answered {}", response.status());
    }
    Ok(())
}
