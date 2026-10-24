//! Sending sign-in codes and plain notices. Behind a small trait so development and tests need no mail server.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, Result};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Message, Tokio1Executor};

use crate::config::{Config, MailMode, SmtpTls};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The mail could not be sent.
#[derive(Debug)]
pub struct MailError(pub String);

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MailError {}

/// Delivers a sign-in code to an address.
pub trait Mailer: Send + Sync + 'static {
    fn send_code<'a>(&'a self, to: &'a str, code: &'a str) -> BoxFuture<'a, Result<(), MailError>>;

    /// Delivers a notice about the account: plain text, from the same sender as the codes.
    fn send_notice<'a>(
        &'a self,
        to: &'a str,
        subject: &'a str,
        body: &'a str,
    ) -> BoxFuture<'a, Result<(), MailError>>;
}

/// How a player reaches the operator. A cloud has no contact address of its own, so the notice
/// names the cloud and its address.
pub fn contact_line(config: &Config) -> String {
    match config.web_origin() {
        Some(url) => format!("contact the operator of {} ({url})", config.name),
        None => format!("contact the operator of {}", config.name),
    }
}

/// The notice that goes to the old address when an account gets a new one. `new_email` is masked
/// (`n***@example.com`): the old address may no longer be the player's.
pub fn email_changed_message(cloud_name: &str, new_email: &str, contact: &str) -> (String, String) {
    let subject = format!("The e-mail address of your {cloud_name} account was changed");
    let body = format!(
        "The e-mail address of your {cloud_name} account was changed to {new_email}. \
         Every other browser and device was signed out.\n\n\
         If this was you, you need to do nothing.\n\n\
         If it was not you, someone else has access to your account, and this address no longer \
         opens it. Please {contact} and say that the address was changed without you.\n"
    );
    (subject, body)
}

/// The notice that goes to the account's address when its deletion is scheduled. `date` is the
/// day the account will be deleted.
pub fn deletion_scheduled_message(cloud_name: &str, date: &str, contact: &str) -> (String, String) {
    let subject = format!("Your {cloud_name} account is scheduled for deletion");
    let body = format!(
        "Your {cloud_name} account is scheduled for deletion on {date} (UTC). Every device and \
         browser was signed out.\n\n\
         If this was you, you need to do nothing.\n\n\
         If it was not you, sign in before {date} and cancel the deletion, or {contact}.\n"
    );
    (subject, body)
}

/// The subject and plain-text body of a code mail.
pub fn code_message(cloud_name: &str, code: &str) -> (String, String) {
    let subject = format!("Your {cloud_name} sign-in code");
    let body = format!(
        "Your {cloud_name} sign-in code is {code}.\n\n\
         It expires in 10 minutes. If you did not ask for it, you can ignore this message.\n"
    );
    (subject, body)
}

/// Writes codes to the log. For development.
pub struct LogMailer;

impl Mailer for LogMailer {
    fn send_code<'a>(&'a self, to: &'a str, code: &'a str) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            tracing::info!(%to, %code, "sign-in code (mail = log)");
            Ok(())
        })
    }

    fn send_notice<'a>(
        &'a self,
        to: &'a str,
        subject: &'a str,
        _body: &'a str,
    ) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            tracing::info!(%to, %subject, "notice (mail = log)");
            Ok(())
        })
    }
}

/// Keeps codes and notices in memory so a test can read them.
#[derive(Default)]
pub struct MemoryMailer {
    sent: Mutex<Vec<(String, String)>>,
    notices: Mutex<Vec<(String, String, String)>>,
}

impl MemoryMailer {
    /// The code most recently sent to `to`.
    pub fn last_code(&self, to: &str) -> Option<String> {
        let sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        sent.iter()
            .rev()
            .find(|(a, _)| a == to)
            .map(|(_, c)| c.clone())
    }

    /// How many notices were sent to `to`.
    pub fn notice_count(&self, to: &str) -> usize {
        let sent = self.notices.lock().unwrap_or_else(|e| e.into_inner());
        sent.iter().filter(|(a, _, _)| a == to).count()
    }

    /// The subject and body of the notice most recently sent to `to`.
    pub fn last_notice(&self, to: &str) -> Option<(String, String)> {
        let sent = self.notices.lock().unwrap_or_else(|e| e.into_inner());
        sent.iter()
            .rev()
            .find(|(a, _, _)| a == to)
            .map(|(_, s, b)| (s.clone(), b.clone()))
    }

    /// How many codes were sent to `to`.
    pub fn count(&self, to: &str) -> usize {
        let sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        sent.iter().filter(|(a, _)| a == to).count()
    }
}

impl Mailer for MemoryMailer {
    fn send_code<'a>(&'a self, to: &'a str, code: &'a str) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
            sent.push((to.to_owned(), code.to_owned()));
            Ok(())
        })
    }

    fn send_notice<'a>(
        &'a self,
        to: &'a str,
        subject: &'a str,
        body: &'a str,
    ) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            let mut sent = self.notices.lock().unwrap_or_else(|e| e.into_inner());
            sent.push((to.to_owned(), subject.to_owned(), body.to_owned()));
            Ok(())
        })
    }
}

/// Sends codes through an SMTP server.
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    cloud_name: String,
}

impl SmtpMailer {
    pub fn new(config: &Config) -> Result<Self> {
        let host = config
            .smtp_host
            .as_deref()
            .context("smtp_host is not set")?;
        let builder = match config.smtp_tls {
            SmtpTls::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
            SmtpTls::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
            SmtpTls::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                host,
            )),
        }
        .context("cannot set up the SMTP connection")?;
        let mut builder = builder.timeout(Some(Duration::from_secs(15)));
        if let Some(port) = config.smtp_port {
            builder = builder.port(port);
        }
        if let (Some(user), Some(password)) = (&config.smtp_username, &config.smtp_password) {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        let from: Mailbox = config
            .mail_from
            .parse()
            .context("mail_from is not an address")?;
        Ok(Self {
            transport: builder.build(),
            from,
            cloud_name: config.name.clone(),
        })
    }
}

impl Mailer for SmtpMailer {
    fn send_code<'a>(&'a self, to: &'a str, code: &'a str) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            let (subject, body) = code_message(&self.cloud_name, code);
            let to: Mailbox = to.parse().map_err(|_| MailError("bad recipient".into()))?;
            let message = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body)
                .map_err(|e| MailError(e.to_string()))?;
            self.transport
                .send(message)
                .await
                .map_err(|e| MailError(e.to_string()))?;
            tracing::debug!("sign-in code sent");
            Ok(())
        })
    }

    fn send_notice<'a>(
        &'a self,
        to: &'a str,
        subject: &'a str,
        body: &'a str,
    ) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            let to: Mailbox = to.parse().map_err(|_| MailError("bad recipient".into()))?;
            let message = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body.to_owned())
                .map_err(|e| MailError(e.to_string()))?;
            self.transport
                .send(message)
                .await
                .map_err(|e| MailError(e.to_string()))?;
            tracing::debug!("notice sent");
            Ok(())
        })
    }
}

/// The mailer the settings ask for.
pub fn from_config(config: &Config) -> Result<std::sync::Arc<dyn Mailer>> {
    Ok(match config.mail {
        MailMode::Log => {
            tracing::warn!("mail = log: sign-in codes are written to the log, not sent");
            std::sync::Arc::new(LogMailer)
        }
        MailMode::Smtp => std::sync::Arc::new(SmtpMailer::new(config)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mail_says_the_code_the_lifetime_and_that_it_can_be_ignored() {
        let (subject, body) = code_message("Test cloud", "123456");
        assert!(subject.contains("Test cloud"));
        assert!(!subject.contains("123456"));
        assert!(body.contains("123456"));
        assert!(body.contains("10 minutes"));
        assert!(body.contains("ignore"));
    }

    #[test]
    fn the_notices_say_what_happened_and_whom_to_ask() {
        let contact = "contact the operator of Test cloud (https://play.example.com)";
        let (subject, body) = email_changed_message("Test cloud", "n***@example.com", contact);
        assert!(subject.contains("Test cloud") && !subject.contains("example.com"));
        assert!(body.contains("n***@example.com"));
        assert!(body.contains("If it was not you") && body.contains(contact));
        assert!(
            !body.contains("sign in"),
            "the old address no longer opens the account"
        );
        let (subject, body) = deletion_scheduled_message("Test cloud", "2026-11-23", contact);
        assert!(subject.contains("scheduled for deletion"));
        assert!(body.contains("2026-11-23") && body.contains("cancel the deletion"));
        assert!(body.contains(contact));
    }

    #[test]
    fn the_contact_is_the_address_of_the_cloud() {
        let mut config = Config {
            name: "Test cloud".into(),
            ..Config::default()
        };
        assert_eq!(contact_line(&config), "contact the operator of Test cloud");
        config.public_url = Some("https://play.example.com".into());
        assert_eq!(
            contact_line(&config),
            "contact the operator of Test cloud (https://play.example.com)"
        );
    }

    #[tokio::test]
    async fn the_memory_mailer_keeps_the_latest_code() {
        let mailer = MemoryMailer::default();
        mailer.send_code("a@example.com", "111111").await.unwrap();
        mailer.send_code("a@example.com", "222222").await.unwrap();
        assert_eq!(mailer.last_code("a@example.com").as_deref(), Some("222222"));
        assert_eq!(mailer.count("a@example.com"), 2);
        assert_eq!(mailer.last_code("b@example.com"), None);
    }
}
