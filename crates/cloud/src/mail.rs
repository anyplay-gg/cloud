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
use serde::{Deserialize, Serialize};

use crate::config::{Config, MailMode, SmtpTls};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Why a mail could not be sent, in a few words an operator can search for. The raw reply of the
/// server is never kept: it often repeats the address it refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailClass {
    /// No connection: DNS, refused, unreachable or dropped.
    Connect,
    /// The TLS handshake failed, or the server insists on a TLS mode the settings do not use.
    Tls,
    /// The server refused the user name or password.
    Auth,
    /// The server refused the recipient (best guess from the reply).
    RejectedRecipient,
    /// The server refused the sender, `mail_from` (best guess from the reply).
    RejectedSender,
    /// The address to send to is not an address. Nothing was sent.
    InvalidRecipient,
    /// The server or the cloud ran out of time.
    Timeout,
    /// Anything else.
    Other,
}

impl MailClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Auth => "auth",
            Self::RejectedRecipient => "rejected_recipient",
            Self::RejectedSender => "rejected_sender",
            Self::InvalidRecipient => "invalid_recipient",
            Self::Timeout => "timeout",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for MailClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The mail could not be sent. Holds the class, the SMTP status and the server's reply with every
/// e-mail address taken out, nothing that names a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailError {
    pub class: MailClass,
    /// The three-digit SMTP reply code, if the server answered with one.
    pub status: Option<u16>,
    /// The error text of the transport or the server's reply, see [`redact`].
    pub detail: Option<String>,
}

impl MailError {
    pub fn new(class: MailClass, status: Option<u16>) -> Self {
        Self {
            class,
            status,
            detail: None,
        }
    }
}

/// Makes a server reply safe to log: every word that holds an `@` is replaced, since replies
/// often repeat the address they refused. Capped at 200 characters.
pub fn redact(text: &str) -> String {
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|w| if w.contains('@') { "<address>" } else { w })
        .collect();
    words.join(" ").chars().take(200).collect()
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "{} (SMTP {status})", self.class),
            None => write!(f, "{}", self.class),
        }
    }
}

impl std::error::Error for MailError {}

/// What `classify` looks at. Kept apart from the transport's error type so that tests can build it.
#[derive(Debug, Default, Clone)]
struct Facts {
    status: Option<u16>,
    timeout: bool,
    tls: bool,
    /// The error came from the client side or from reading a reply, not from the network.
    local: bool,
    /// The lower-cased text of the error. Read for hints, never stored or logged.
    text: String,
}

fn facts(e: &lettre::transport::smtp::Error) -> Facts {
    Facts {
        status: e.status().map(u16::from),
        timeout: e.is_timeout(),
        tls: e.is_tls(),
        local: e.is_client() || e.is_response() || e.is_transport_shutdown(),
        text: e.to_string().to_ascii_lowercase(),
    }
}

fn classify(facts: &Facts) -> MailError {
    let status = facts.status;
    let class = if facts.timeout {
        MailClass::Timeout
    } else if facts.tls || facts.text.contains("starttls") {
        MailClass::Tls
    } else {
        match status {
            Some(530 | 534 | 535 | 538 | 454) => MailClass::Auth,
            Some(550..=554) => {
                let t = &facts.text;
                if t.contains("sender") || t.contains("mail from") || t.contains("from address") {
                    MailClass::RejectedSender
                } else {
                    MailClass::RejectedRecipient
                }
            }
            Some(_) => MailClass::Other,
            None if facts.local => MailClass::Other,
            None => MailClass::Connect,
        }
    };
    MailError::new(class, status)
}

fn failure(e: &lettre::transport::smtp::Error) -> MailError {
    let mut error = classify(&facts(e));
    error.detail = Some(redact(&e.to_string()));
    error
}

/// Runs a send with a time limit; running out of time is a `timeout` failure.
pub async fn within<F>(limit: Duration, send: F) -> Result<(), MailError>
where
    F: Future<Output = Result<(), MailError>>,
{
    tokio::time::timeout(limit, send)
        .await
        .unwrap_or(Err(MailError::new(MailClass::Timeout, None)))
}

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

/// A warning when the TLS mode and the port do not usually go together.
fn tls_port_hint(tls: SmtpTls, port: Option<u16>) -> Option<&'static str> {
    match (tls, port) {
        (SmtpTls::Starttls, Some(465)) => Some(
            "smtp_port 465 expects TLS from the first byte; set smtp_tls = \"tls\" (or leave it out)",
        ),
        (SmtpTls::Tls, Some(587)) => {
            Some("smtp_port 587 expects STARTTLS; set smtp_tls = \"starttls\" (or leave it out)")
        }
        _ => None,
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
        let tls = config.smtp_tls();
        if let Some(hint) = tls_port_hint(tls, config.smtp_port) {
            tracing::warn!("{hint}");
        }
        let builder = match tls {
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
            let to: Mailbox = to
                .parse()
                .map_err(|_| MailError::new(MailClass::InvalidRecipient, None))?;
            let message = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body)
                .map_err(|_| MailError::new(MailClass::Other, None))?;
            self.transport
                .send(message)
                .await
                .map_err(|e| failure(&e))?;
            tracing::info!("sign-in code accepted by the mail server");
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
            let to: Mailbox = to
                .parse()
                .map_err(|_| MailError::new(MailClass::InvalidRecipient, None))?;
            let message = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body.to_owned())
                .map_err(|_| MailError::new(MailClass::Other, None))?;
            self.transport
                .send(message)
                .await
                .map_err(|e| failure(&e))?;
            tracing::info!("notice accepted by the mail server");
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

    fn facts_of(status: Option<u16>, text: &str) -> Facts {
        Facts {
            status,
            text: text.to_ascii_lowercase(),
            ..Facts::default()
        }
    }

    #[test]
    fn failures_get_a_class_and_keep_the_status() {
        let class = |f: Facts| classify(&f);
        assert_eq!(
            class(facts_of(None, "connection refused")),
            MailError::new(MailClass::Connect, None)
        );
        assert_eq!(
            class(Facts {
                timeout: true,
                ..facts_of(None, "timed out")
            })
            .class,
            MailClass::Timeout
        );
        assert_eq!(
            class(Facts {
                tls: true,
                ..facts_of(None, "handshake")
            })
            .class,
            MailClass::Tls
        );
        assert_eq!(
            class(facts_of(Some(530), "must issue a STARTTLS command first")).class,
            MailClass::Tls
        );
        assert_eq!(
            class(facts_of(Some(535), "authentication failed")),
            MailError::new(MailClass::Auth, Some(535))
        );
        assert_eq!(
            class(facts_of(Some(550), "5.1.1 user unknown")),
            MailError::new(MailClass::RejectedRecipient, Some(550))
        );
        assert_eq!(
            class(facts_of(Some(553), "5.7.1 sender address rejected")),
            MailError::new(MailClass::RejectedSender, Some(553))
        );
        assert_eq!(
            class(facts_of(Some(421), "service not available")),
            MailError::new(MailClass::Other, Some(421))
        );
        assert_eq!(
            class(Facts {
                local: true,
                ..facts_of(None, "malformed reply")
            })
            .class,
            MailClass::Other
        );
    }

    #[test]
    fn replies_lose_every_address() {
        let reply = redact("550 5.1.1 <Someone@Example.com> user unknown, sender noreply@x.org");
        assert!(!reply.contains('@') && reply.contains("user unknown"));
        assert!(redact(&"a ".repeat(500)).len() <= 200);
    }

    #[test]
    fn the_error_text_names_no_address() {
        let error = MailError::new(MailClass::RejectedRecipient, Some(550));
        assert_eq!(error.to_string(), "rejected_recipient (SMTP 550)");
        assert_eq!(
            MailError::new(MailClass::Connect, None).to_string(),
            "connect"
        );
    }

    #[tokio::test]
    async fn running_out_of_time_is_a_timeout() {
        let slow = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(())
        };
        let result = within(Duration::from_millis(10), slow).await;
        assert_eq!(result, Err(MailError::new(MailClass::Timeout, None)));
        assert_eq!(
            within(Duration::from_secs(1), async { Ok(()) }).await,
            Ok(())
        );
    }

    fn smtp_config(port: Option<u16>, tls: Option<SmtpTls>) -> Config {
        Config {
            mail: MailMode::Smtp,
            smtp_host: Some("smtp.example.com".into()),
            smtp_port: port,
            smtp_tls: tls,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn the_transport_builds_in_every_tls_mode() {
        for (port, tls) in [
            (Some(587), None),
            (Some(465), None),
            (None, Some(SmtpTls::Tls)),
            (Some(25), Some(SmtpTls::None)),
        ] {
            SmtpMailer::new(&smtp_config(port, tls)).unwrap();
        }
    }

    #[test]
    fn a_mode_that_does_not_fit_the_port_is_called_out() {
        assert!(tls_port_hint(SmtpTls::Starttls, Some(465)).is_some());
        assert!(tls_port_hint(SmtpTls::Tls, Some(587)).is_some());
        assert!(tls_port_hint(SmtpTls::Tls, Some(465)).is_none());
        assert!(tls_port_hint(SmtpTls::Starttls, Some(587)).is_none());
        assert!(tls_port_hint(SmtpTls::None, Some(25)).is_none());
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
