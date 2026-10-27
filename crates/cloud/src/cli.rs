//! The command line: `serve` (the default), `keygen` and the `admin` commands that call the admin
//! routes of a running cloud.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::Config;
use anyplay_api::{
    ActionKind, ActionView, AppealDecision, AppealOutcome, AppealView, AuditEntry, Dismissal,
    GrantModerator, Moderator, ReportView, Rule, path,
};

use crate::routes::admin::{
    ACTIONS, ActionByHandle, BAN, BANS, BanAnswer, BanRequest, MAIL_TEST, MailTestAnswer,
    MailTestRequest, REPORTS,
};

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
    /// Make a VAPID key pair for Web Push and print both halves. Nothing is stored: put the
    /// private key into `ANYPLAY_CLOUD_VAPID_PRIVATE_KEY`.
    VapidKeys,
    /// Exit with an error unless a running cloud answers. For container health checks.
    Healthcheck {
        /// Where the cloud listens. Default: the `listen` setting, on this machine.
        #[arg(long)]
        url: Option<String>,
    },
    /// After a restore: delete again every account that was deleted since the backup. Opens the
    /// database directly, so run it while the cloud is stopped (deploy/restore.sh does). Reads
    /// the deletion ledger of the restored database and, with --ledger, the ledger of the
    /// database it replaced. Exits non-zero when an account could not be deleted.
    ReplayDeletions {
        /// A file with one `<account id> <deleted ms>` pair per line, from the replaced database.
        #[arg(long)]
        ledger: Option<PathBuf>,
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
    /// Ban an account, and stop its e-mail address from signing up again. The player reads the
    /// reason.
    Ban {
        handle: String,
        /// Ban for this many days. Without it the ban is for good.
        #[arg(long)]
        days: Option<u64>,
        #[arg(long)]
        reason: String,
        /// cheating, harassment, spam, name, illegal or other.
        #[arg(long)]
        rule: Option<String>,
    },
    /// Lift a ban.
    Unban { handle: String },
    /// Warn an account. The player reads the reason.
    Warn {
        handle: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        rule: Option<String>,
        /// The report this decides.
        #[arg(long)]
        report: Option<u64>,
    },
    /// Mute an account: no friend requests and no messages for some days.
    Mute {
        handle: String,
        #[arg(long)]
        days: u64,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        report: Option<u64>,
    },
    /// List the newest reports.
    Reports {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// open, decided or all.
        #[arg(long, default_value = "open")]
        status: String,
    },
    /// Decide a report without an action.
    Dismiss {
        id: u64,
        #[arg(long)]
        reason: String,
    },
    /// Give an account the moderator role.
    GrantModerator { handle: String },
    /// Take the moderator role back.
    RevokeModerator { handle: String },
    /// List the moderators.
    Moderators,
    /// List the open appeals.
    Appeals,
    /// Decide an appeal: the action stands, or with --reverse it is lifted.
    DecideAppeal {
        id: u64,
        #[arg(long)]
        reverse: bool,
        #[arg(long)]
        reason: String,
    },
    /// Send one test mail through the mail settings of the running cloud, and print `sent` or
    /// the error class and SMTP status.
    MailTest {
        /// The address to send to.
        address: String,
    },
    /// Show the newest lines of the audit log.
    Log {
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
    let get = |path: String| client.get(format!("{base}{path}")).bearer_auth(token);
    let post = |path: String| client.post(format!("{base}{path}")).bearer_auth(token);

    match action {
        AdminAction::Ban {
            handle,
            days,
            reason,
            rule: name,
        } => {
            let response = post(BANS.to_owned())
                .json(&BanRequest {
                    handle,
                    days,
                    reason: Some(reason),
                    rule: rule(name)?,
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
        AdminAction::Warn {
            handle,
            reason,
            rule: name,
            report,
        } => {
            let response = post(ACTIONS.to_owned())
                .json(&ActionByHandle {
                    handle,
                    kind: ActionKind::Warning,
                    rule: rule(name)?,
                    reason,
                    days: None,
                    report,
                })
                .send()
                .await?;
            let view: ActionView = read(response).await?;
            println!(
                "Warned {} (action {}).",
                view.account.handle, view.statement.id
            );
        }
        AdminAction::Mute {
            handle,
            days,
            reason,
            rule: name,
            report,
        } => {
            let response = post(ACTIONS.to_owned())
                .json(&ActionByHandle {
                    handle,
                    kind: ActionKind::Mute,
                    rule: rule(name)?,
                    reason,
                    days: Some(days),
                    report,
                })
                .send()
                .await?;
            let view: ActionView = read(response).await?;
            println!(
                "Muted {} (action {}).",
                view.account.handle, view.statement.id
            );
        }
        AdminAction::Reports { limit, status } => {
            let response = get(format!("{REPORTS}?limit={limit}&status={status}"))
                .send()
                .await?;
            let reports: Vec<ReportView> = read(response).await?;
            if reports.is_empty() {
                println!("No reports.");
            }
            for r in reports {
                println!(
                    "#{} at {} [{:?}]: {} reported {} ({:?}) {}",
                    r.id,
                    r.created,
                    r.status,
                    r.reporter.map_or("(deleted)".into(), |a| a.handle),
                    r.reported.handle,
                    r.reason,
                    r.details
                );
            }
        }
        AdminAction::Dismiss { id, reason } => {
            let response = post(path::fill(path::MOD_REPORT_DISMISS, id))
                .json(&Dismissal { reason })
                .send()
                .await?;
            let _: ReportView = read(response).await?;
            println!("Dismissed report {id}.");
        }
        AdminAction::GrantModerator { handle } => {
            let response = post(path::MOD_MODERATORS.to_owned())
                .json(&GrantModerator { handle })
                .send()
                .await?;
            let moderator: Moderator = read(response).await?;
            println!("{} is a moderator.", moderator.account.handle);
        }
        AdminAction::RevokeModerator { handle } => {
            let response = get(path::MOD_MODERATORS.to_owned()).send().await?;
            let moderators: Vec<Moderator> = read(response).await?;
            let found = moderators
                .iter()
                .find(|m| m.account.handle.eq_ignore_ascii_case(&handle))
                .context("this account is not a moderator")?;
            let response = client
                .delete(format!(
                    "{base}{}",
                    path::fill(path::MOD_MODERATOR, found.account.id)
                ))
                .bearer_auth(token)
                .send()
                .await?;
            check(response).await?;
            println!("{handle} is no longer a moderator.");
        }
        AdminAction::Moderators => {
            let response = get(path::MOD_MODERATORS.to_owned()).send().await?;
            let moderators: Vec<Moderator> = read(response).await?;
            if moderators.is_empty() {
                println!("No moderators.");
            }
            for m in moderators {
                println!("{} (since Unix time {})", m.account.handle, m.since);
            }
        }
        AdminAction::Appeals => {
            let response = get(format!("{}?status=open", path::MOD_APPEALS))
                .send()
                .await?;
            let appeals: Vec<AppealView> = read(response).await?;
            if appeals.is_empty() {
                println!("No open appeals.");
            }
            for a in appeals {
                let s = &a.action.statement;
                println!(
                    "#{} at {}: {} against the {:?} {} ({}): {}",
                    a.id,
                    a.created,
                    a.action.account.handle,
                    s.kind,
                    s.id,
                    s.reason,
                    s.appeal.text.clone().unwrap_or_default()
                );
            }
        }
        AdminAction::DecideAppeal {
            id,
            reverse,
            reason,
        } => {
            let outcome = if reverse {
                AppealOutcome::Reverse
            } else {
                AppealOutcome::Uphold
            };
            let response = post(path::fill(path::MOD_APPEAL_DECISION, id))
                .json(&AppealDecision { outcome, reason })
                .send()
                .await?;
            let _: AppealView = read(response).await?;
            println!("Decided appeal {id}.");
        }
        AdminAction::MailTest { address } => {
            let response = post(MAIL_TEST.to_owned())
                .json(&MailTestRequest { to: address })
                .send()
                .await?;
            let answer: MailTestAnswer = read(response).await?;
            println!("{}", mail_test_line(&answer));
            if !answer.sent {
                std::process::exit(1);
            }
        }
        AdminAction::Log { limit } => {
            let response = get(format!("{}?limit={limit}", path::MOD_LOG))
                .send()
                .await?;
            let entries: Vec<AuditEntry> = read(response).await?;
            for e in entries {
                println!(
                    "#{} at {}: {} {:?} {} {}",
                    e.id,
                    e.time,
                    e.actor.handle,
                    e.action,
                    e.target.map_or(String::new(), |t| t.handle),
                    e.reason
                );
            }
        }
    }
    Ok(())
}

/// What `admin mail-test` prints.
fn mail_test_line(answer: &MailTestAnswer) -> String {
    if answer.sent {
        return match answer.mode.as_str() {
            "log" => "sent (mail = log: written to the log, not delivered)".into(),
            _ => "sent".into(),
        };
    }
    let class = answer.class.map_or("other", |c| c.as_str());
    let line = match answer.status {
        Some(status) => format!("failed: {class} (SMTP {status})"),
        None => format!("failed: {class}"),
    };
    match &answer.reply {
        Some(reply) => format!("{line}\nserver said: {reply}"),
        None => line,
    }
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

fn rule(name: Option<String>) -> Result<Option<Rule>> {
    name.map(|n| {
        Rule::from_name(&n.to_ascii_lowercase()).with_context(|| {
            format!("{n:?} is not a rule: cheating, harassment, spam, name, illegal or other")
        })
    })
    .transpose()
}

/// Reads a ledger file: one `<account id> <deleted ms>` pair per line. Blank lines are skipped.
pub fn read_ledger(text: &str) -> Result<Vec<(u64, i64)>> {
    let mut entries = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let entry = match (parts.next(), parts.next(), parts.next()) {
            (Some(id), Some(at), None) => id.parse().ok().zip(at.parse().ok()),
            _ => None,
        };
        entries.push(
            entry.with_context(|| format!("ledger line {} is not \"<id> <ms>\"", number + 1))?,
        );
    }
    Ok(entries)
}

/// `replay-deletions`: deletes again the accounts the deletion ledger lists that exist.
pub async fn replay_deletions(config: Config, ledger: Option<PathBuf>) -> Result<()> {
    let extra = match ledger {
        Some(path) => read_ledger(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?,
        )?,
        None => Vec::new(),
    };
    let mailer = crate::mail::from_config(&config)?;
    let state = crate::open(config, crate::Tuning::default(), mailer).await?;
    let (deleted, failed) = state
        .replay_deletions(&extra)
        .await
        .map_err(|e| anyhow::anyhow!(e.body.message))?;
    println!(
        "replayed deletions: {deleted} deleted, {failed} failed, {} from the file",
        extra.len()
    );
    if failed > 0 {
        bail!("{failed} accounts could not be deleted; run the command again");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::MailClass;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("anyplay-cloud").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn mail_test_takes_an_address() {
        let cli = parse(&["admin", "mail-test", "someone@example.com"]);
        match cli.command {
            Some(Command::Admin {
                url: None,
                action: AdminAction::MailTest { address },
            }) => assert_eq!(address, "someone@example.com"),
            other => panic!("unexpected {other:?}"),
        }
        let with_url = parse(&["admin", "--url", "http://cloud:8080", "mail-test", "a@b.c"]);
        assert!(matches!(
            with_url.command,
            Some(Command::Admin { url: Some(_), .. })
        ));
        assert!(Cli::try_parse_from(["anyplay-cloud", "admin", "mail-test"]).is_err());
    }

    #[test]
    fn mail_test_prints_sent_or_class_and_status() {
        let answer = |sent, class, status, mode: &str| MailTestAnswer {
            mode: mode.into(),
            sent,
            class,
            status,
            reply: None,
        };
        assert_eq!(mail_test_line(&answer(true, None, None, "smtp")), "sent");
        assert!(mail_test_line(&answer(true, None, None, "log")).contains("not delivered"));
        assert_eq!(
            mail_test_line(&answer(false, Some(MailClass::Auth), Some(535), "smtp")),
            "failed: auth (SMTP 535)"
        );
        assert_eq!(
            mail_test_line(&answer(false, Some(MailClass::Connect), None, "smtp")),
            "failed: connect"
        );
        let mut refused = answer(false, Some(MailClass::RejectedSender), Some(403), "smtp");
        refused.reply = Some("domain is not verified".into());
        assert_eq!(
            mail_test_line(&refused),
            "failed: rejected_sender (SMTP 403)\nserver said: domain is not verified"
        );
    }

    #[test]
    fn replay_deletions_reads_a_ledger_file() {
        let cli = parse(&["replay-deletions", "--ledger", "/ledger/old.txt"]);
        assert!(matches!(
            cli.command,
            Some(Command::ReplayDeletions { ledger: Some(_) })
        ));
        assert_eq!(
            read_ledger(
                "12 1000

  7 2000  
"
            )
            .unwrap(),
            [(12, 1000), (7, 2000)]
        );
        assert!(
            read_ledger(
                "12
"
            )
            .is_err()
        );
        assert!(
            read_ledger(
                "a b
"
            )
            .is_err()
        );
        assert!(
            read_ledger(
                "1 2 3
"
            )
            .is_err()
        );
    }
}
