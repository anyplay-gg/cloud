//! The cloud's settings: a TOML file, each value overridable by an `ANYPLAY_CLOUD_<NAME>`
//! environment variable. An empty variable counts as not set. `docs/configuration.md` lists them.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use anyplay_api::Signup;
use serde::{Deserialize, Serialize};

use crate::limits::Rates;

/// How sign-in codes leave the cloud.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailMode {
    /// Write the code to the log. For development.
    #[default]
    Log,
    /// Send it by SMTP.
    Smtp,
}

/// How the log is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Lines for people.
    #[default]
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// How the connection to the SMTP server is secured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// Start in plain text and switch to TLS (usually port 587).
    Starttls,
    /// TLS from the first byte (usually port 465). Also written `implicit`.
    #[serde(alias = "implicit")]
    Tls,
    /// No encryption. Only for a server on the same machine.
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    /// Where `/metrics` is served, apart from the API. Without it there are no metrics.
    pub metrics_listen: Option<SocketAddr>,
    /// How long a node that is told to stop keeps its connections while it reports itself not
    /// ready.
    pub drain_seconds: u64,
    /// `text` or `json`.
    pub log_format: LogFormat,
    pub data_dir: PathBuf,
    /// A file that holds the signing key. The cloud only reads it: it never creates it and never
    /// writes `signing.pub` next to it, so it can be a read-only secret mount. Without it the key
    /// lives in `data_dir`.
    pub signing_key_file: Option<PathBuf>,
    /// A `postgres://` URL. Without it the store is a SQLite file in `data_dir`.
    pub database_url: Option<String>,
    /// The most connections the store keeps open.
    pub database_connections: u32,
    /// A `nats://` URL. With it the cloud runs as one node of a cluster and tells the other
    /// nodes about changes over NATS.
    pub nats_url: Option<String>,
    pub name: String,
    pub relays: Vec<String>,
    pub mail: MailMode,
    pub mail_from: String,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    /// Absent: derived from the port (`tls` on 465, `starttls` otherwise).
    pub smtp_tls: Option<SmtpTls>,
    pub signup: Signup,
    pub privacy_url: Option<String>,
    pub terms_url: Option<String>,
    pub trust_proxy: bool,
    pub admin_token: Option<String>,
    /// How many days messages are kept before the sweeps delete them.
    pub message_retention_days: u64,
    /// How many days feed items are kept before the sweeps delete them, with their reactions.
    pub feed_retention_days: u64,
    // Moderation (docs/moderation.md)
    /// A secret for the keyed hash of banned e-mail addresses. Without it the cloud makes one in
    /// `data_dir/ban.key`.
    pub ban_secret: Option<String>,
    /// Hours an account counts as new: it may not write to people who are not its friends, and
    /// it may send only `new_account_friend_requests_per_day` friend requests. 0 turns the rule
    /// off.
    pub new_account_hours: u64,
    /// Friend requests a new account may send while it is new.
    pub new_account_friend_requests_per_day: u32,
    /// Days after an action in which the player may appeal.
    pub appeal_days: u64,
    /// Reports one account may file per day.
    pub report_limit_per_day: u32,
    /// Days to keep an ended or lifted ban, an ended or lifted action, a warning and their
    /// appeals, counted from the end.
    pub retention_bans_days: u64,
    /// Days to keep a decided report, counted from the decision.
    pub retention_reports_days: u64,
    /// Days after which a report nobody decided is deleted.
    pub retention_open_reports_days: u64,
    /// Days to keep a line of the audit log.
    pub retention_audit_days: u64,
    /// The address players reach this cloud at, such as `https://play.example.com`: the origin of
    /// the account web and the relying party of passkeys. Without it there is no account web and
    /// no passkeys.
    pub public_url: Option<String>,
    /// The version of the terms in force. A player who accepted another one is asked again at the
    /// next sign-in on the web. Empty or absent when there are no terms.
    pub terms_version: Option<String>,
    /// For how many days a device may sign in without a device key, and so refresh without a
    /// proof. 0 makes the key mandatory ([ADR 0016 of the network]).
    pub legacy_device_days: u64,
    /// The folder with the built web portal (a single-page app: `index.html` and its assets).
    /// With it the cloud serves the portal from its own origin. Needs `public_url`.
    pub portal_dir: Option<PathBuf>,
    /// The account web is served by another service on the cloud's origin (for example
    /// anyplay-gg/web behind Caddy, `deploy/compose.web.yaml`). The cloud then sends the browser
    /// to `/sign-in`, `/create` and `/account` but serves none of those paths itself. Needs
    /// `public_url`; not together with `portal_dir`.
    pub portal_external: bool,
    // Connections: linked accounts of other services (network spec/portal-api.md)
    /// The Client ID of the cloud's Discord application. With the secret it turns on the Discord
    /// connection. Needs `public_url`: Discord sends the browser back to it.
    pub discord_client_id: Option<String>,
    /// The Client Secret of that application.
    pub discord_client_secret: Option<String>,
    /// A Steam Web API key. Without it a Steam connection holds the SteamID64 only; with it also
    /// the persona name and picture.
    pub steam_api_key: Option<String>,
    // Web Push (network spec/portal-api.md, "Notifications (Web Push)")
    /// The cloud's VAPID private key: 32 bytes in unpadded base64url. A secret. Make one with
    /// `anyplay-cloud vapid-keys`. Without it the cloud sends no notifications.
    pub vapid_private_key: Option<String>,
    /// The contact the push services may use (`mailto:` or `https:`). Set with the key.
    pub vapid_subject: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([0, 0, 0, 0], 8080).into(),
            metrics_listen: None,
            drain_seconds: 5,
            log_format: LogFormat::Text,
            data_dir: PathBuf::from("data"),
            signing_key_file: None,
            database_url: None,
            database_connections: 16,
            nats_url: None,
            name: "AnyPlay cloud".into(),
            relays: Vec::new(),
            mail: MailMode::Log,
            mail_from: "AnyPlay cloud <noreply@localhost>".into(),
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_tls: None,
            signup: Signup::Open,
            privacy_url: None,
            terms_url: None,
            trust_proxy: false,
            admin_token: None,
            message_retention_days: 30,
            feed_retention_days: 90,
            ban_secret: None,
            new_account_hours: 24,
            new_account_friend_requests_per_day: 10,
            appeal_days: 183,
            report_limit_per_day: 20,
            retention_bans_days: 183,
            retention_reports_days: 183,
            retention_open_reports_days: 365,
            retention_audit_days: 1095,
            public_url: None,
            terms_version: None,
            legacy_device_days: 30,
            portal_dir: None,
            portal_external: false,
            discord_client_id: None,
            discord_client_secret: None,
            steam_api_key: None,
            vapid_private_key: None,
            vapid_subject: None,
        }
    }
}

fn parse<T: std::str::FromStr>(name: &str, text: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    text.trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("ANYPLAY_CLOUD_{name}: {e}"))
}

fn parse_word<T: for<'de> Deserialize<'de>>(name: &str, text: &str) -> Result<T> {
    let word = serde_json::Value::String(text.trim().to_ascii_lowercase());
    serde_json::from_value(word).map_err(|e| anyhow::anyhow!("ANYPLAY_CLOUD_{name}: {e}"))
}

impl SmtpTls {
    /// The mode to use: the one asked for, else implicit TLS on port 465 and STARTTLS on any
    /// other port.
    pub fn resolve(asked: Option<SmtpTls>, port: Option<u16>) -> SmtpTls {
        asked.unwrap_or(match port {
            Some(465) => SmtpTls::Tls,
            _ => SmtpTls::Starttls,
        })
    }
}

impl Config {
    /// How to secure the connection to the SMTP server.
    pub fn smtp_tls(&self) -> SmtpTls {
        SmtpTls::resolve(self.smtp_tls, self.smtp_port)
    }

    /// Reads the file, if any, then applies the environment and checks the result.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config = match path {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?
            }
            None => Config::default(),
        };
        config.apply_env(|name| std::env::var(format!("ANYPLAY_CLOUD_{name}")).ok())?;
        config.validate()?;
        Ok(config)
    }

    /// Applies overrides. `get` receives the upper-case field name, such as `LISTEN`.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<()> {
        let get = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        if let Some(v) = get("LISTEN") {
            self.listen = parse("LISTEN", &v)?;
        }
        if let Some(v) = get("METRICS_LISTEN") {
            self.metrics_listen = Some(parse("METRICS_LISTEN", &v)?);
        }
        if let Some(v) = get("DRAIN_SECONDS") {
            self.drain_seconds = parse("DRAIN_SECONDS", &v)?;
        }
        if let Some(v) = get("LOG_FORMAT") {
            self.log_format = parse_word("LOG_FORMAT", &v)?;
        }
        if let Some(v) = get("DATA_DIR") {
            self.data_dir = v.into();
        }
        if let Some(v) = get("SIGNING_KEY_FILE") {
            self.signing_key_file = Some(v.into());
        }
        if let Some(v) = get("DATABASE_URL") {
            self.database_url = Some(v);
        }
        if let Some(v) = get("DATABASE_CONNECTIONS") {
            self.database_connections = parse("DATABASE_CONNECTIONS", &v)?;
        }
        if let Some(v) = get("NATS_URL") {
            self.nats_url = Some(v);
        }
        if let Some(v) = get("NAME") {
            self.name = v;
        }
        if let Some(v) = get("RELAYS") {
            self.relays = v
                .split(',')
                .map(|url| url.trim().to_owned())
                .filter(|url| !url.is_empty())
                .collect();
        }
        if let Some(v) = get("MAIL") {
            self.mail = parse_word("MAIL", &v)?;
        }
        if let Some(v) = get("MAIL_FROM") {
            self.mail_from = v;
        }
        if let Some(v) = get("SMTP_HOST") {
            self.smtp_host = Some(v);
        }
        if let Some(v) = get("SMTP_PORT") {
            self.smtp_port = Some(parse("SMTP_PORT", &v)?);
        }
        if let Some(v) = get("SMTP_USERNAME") {
            self.smtp_username = Some(v);
        }
        if let Some(v) = get("SMTP_PASSWORD") {
            self.smtp_password = Some(v);
        }
        if let Some(v) = get("SMTP_TLS") {
            self.smtp_tls = Some(parse_word("SMTP_TLS", &v)?);
        }
        if let Some(v) = get("SIGNUP") {
            self.signup = parse_word("SIGNUP", &v)?;
        }
        if let Some(v) = get("PRIVACY_URL") {
            self.privacy_url = Some(v);
        }
        if let Some(v) = get("TERMS_URL") {
            self.terms_url = Some(v);
        }
        if let Some(v) = get("TRUST_PROXY") {
            self.trust_proxy = parse("TRUST_PROXY", &v)?;
        }
        if let Some(v) = get("ADMIN_TOKEN") {
            self.admin_token = Some(v);
        }
        if let Some(v) = get("MESSAGE_RETENTION_DAYS") {
            self.message_retention_days = parse("MESSAGE_RETENTION_DAYS", &v)?;
        }
        if let Some(v) = get("FEED_RETENTION_DAYS") {
            self.feed_retention_days = parse("FEED_RETENTION_DAYS", &v)?;
        }
        if let Some(v) = get("BAN_SECRET") {
            self.ban_secret = Some(v);
        }
        if let Some(v) = get("NEW_ACCOUNT_HOURS") {
            self.new_account_hours = parse("NEW_ACCOUNT_HOURS", &v)?;
        }
        if let Some(v) = get("NEW_ACCOUNT_FRIEND_REQUESTS_PER_DAY") {
            self.new_account_friend_requests_per_day =
                parse("NEW_ACCOUNT_FRIEND_REQUESTS_PER_DAY", &v)?;
        }
        if let Some(v) = get("APPEAL_DAYS") {
            self.appeal_days = parse("APPEAL_DAYS", &v)?;
        }
        if let Some(v) = get("REPORT_LIMIT_PER_DAY") {
            self.report_limit_per_day = parse("REPORT_LIMIT_PER_DAY", &v)?;
        }
        if let Some(v) = get("RETENTION_BANS_DAYS") {
            self.retention_bans_days = parse("RETENTION_BANS_DAYS", &v)?;
        }
        if let Some(v) = get("RETENTION_REPORTS_DAYS") {
            self.retention_reports_days = parse("RETENTION_REPORTS_DAYS", &v)?;
        }
        if let Some(v) = get("RETENTION_OPEN_REPORTS_DAYS") {
            self.retention_open_reports_days = parse("RETENTION_OPEN_REPORTS_DAYS", &v)?;
        }
        if let Some(v) = get("RETENTION_AUDIT_DAYS") {
            self.retention_audit_days = parse("RETENTION_AUDIT_DAYS", &v)?;
        }
        if let Some(v) = get("PUBLIC_URL") {
            self.public_url = Some(v);
        }
        if let Some(v) = get("TERMS_VERSION") {
            self.terms_version = Some(v);
        }
        if let Some(v) = get("LEGACY_DEVICE_DAYS") {
            self.legacy_device_days = parse("LEGACY_DEVICE_DAYS", &v)?;
        }
        if let Some(v) = get("PORTAL_DIR") {
            self.portal_dir = Some(v.into());
        }
        if let Some(v) = get("PORTAL_EXTERNAL") {
            self.portal_external = parse("PORTAL_EXTERNAL", &v)?;
        }
        if let Some(v) = get("DISCORD_CLIENT_ID") {
            self.discord_client_id = Some(v.trim().to_owned());
        }
        if let Some(v) = get("DISCORD_CLIENT_SECRET") {
            self.discord_client_secret = Some(v.trim().to_owned());
        }
        if let Some(v) = get("STEAM_API_KEY") {
            self.steam_api_key = Some(v.trim().to_owned());
        }
        if let Some(v) = get("VAPID_PRIVATE_KEY") {
            self.vapid_private_key = Some(v.trim().to_owned());
        }
        if let Some(v) = get("VAPID_SUBJECT") {
            self.vapid_subject = Some(v.trim().to_owned());
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.message_retention_days == 0 {
            bail!("message_retention_days must be at least 1");
        }
        if self.feed_retention_days == 0 {
            bail!("feed_retention_days must be at least 1");
        }
        if let Some(url) = &self.database_url
            && !(url.starts_with("postgres://") || url.starts_with("postgresql://"))
        {
            bail!(
                "database_url must be a postgres:// URL; without it the store is a file in data_dir"
            );
        }
        if let Some(file) = &self.signing_key_file
            && (file.file_name().is_none() || file.is_dir())
        {
            bail!("signing_key_file must name a file, not a folder");
        }
        if self.database_connections == 0 {
            bail!("database_connections must be at least 1");
        }
        if let Some(url) = &self.nats_url {
            if !(url.starts_with("nats://") || url.starts_with("tls://")) {
                bail!("nats_url must be a nats:// or tls:// URL");
            }
            if self.database_url.is_none() {
                bail!(
                    "nats_url needs database_url: the nodes of a cluster share one PostgreSQL database"
                );
            }
        }
        for url in &self.relays {
            if !crate::util::is_http_url(url) {
                bail!("relays: {url:?} is not an http or https URL");
            }
        }
        if self.mail == MailMode::Smtp {
            if self.smtp_host.is_none() {
                bail!("mail = \"smtp\" needs smtp_host");
            }
            if self.smtp_username.is_some() != self.smtp_password.is_some() {
                bail!("smtp_username and smtp_password go together");
            }
        }
        if let Some(token) = &self.admin_token
            && token.len() < 16
        {
            bail!("admin_token must have at least 16 characters");
        }
        if let Some(secret) = &self.ban_secret
            && secret.len() < 16
        {
            bail!("ban_secret must have at least 16 characters");
        }
        if self.appeal_days == 0 {
            bail!("appeal_days must be at least 1");
        }
        if self.report_limit_per_day == 0 {
            bail!("report_limit_per_day must be at least 1");
        }
        for (name, days) in [
            ("retention_bans_days", self.retention_bans_days),
            ("retention_reports_days", self.retention_reports_days),
            (
                "retention_open_reports_days",
                self.retention_open_reports_days,
            ),
            ("retention_audit_days", self.retention_audit_days),
        ] {
            if days == 0 {
                bail!("{name} must be at least 1");
            }
        }
        if self.retention_bans_days < self.appeal_days {
            bail!(
                "retention_bans_days must not be shorter than appeal_days: a warning would go before its appeal time ends"
            );
        }
        if self.name.trim().is_empty() || self.name.len() > 64 {
            bail!("name must have 1 to 64 characters");
        }
        if let Some(url) = &self.public_url {
            check_public_url(url)?;
        }
        if let Some(version) = &self.terms_version
            && (version.len() > 64 || version.chars().any(char::is_control))
        {
            bail!("terms_version must be at most 64 printable characters");
        }
        if self.legacy_device_days > 365 {
            bail!("legacy_device_days must be at most 365");
        }
        if self.portal_external {
            if self.portal_dir.is_some() {
                bail!("portal_dir and portal_external exclude each other");
            }
            if self.web_origin().is_none() {
                bail!(
                    "portal_external needs public_url: the account web is served from the origin of the cloud"
                );
            }
        }
        if let Some(dir) = &self.portal_dir {
            if self.web_origin().is_none() {
                bail!(
                    "portal_dir needs public_url: the portal is served from the origin of the account web"
                );
            }
            if !dir.join("index.html").is_file() {
                bail!("portal_dir {} has no index.html", dir.display());
            }
        }
        let discord_id = self.discord_client_id.as_deref().filter(|v| !v.is_empty());
        let discord_secret = self
            .discord_client_secret
            .as_deref()
            .filter(|v| !v.is_empty());
        if discord_id.is_some() != discord_secret.is_some() {
            bail!("discord_client_id and discord_client_secret go together");
        }
        if let Some(id) = discord_id {
            if self.web_origin().is_none() {
                bail!(
                    "the Discord connection needs public_url: Discord sends the browser back to it"
                );
            }
            if id.len() > 32 || !id.bytes().all(|b| b.is_ascii_digit()) {
                bail!("discord_client_id must be the numeric Client ID of the application");
            }
        }
        if let Some(key) = &self.steam_api_key
            && !key.is_empty()
            && (key.len() > 64 || !key.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            bail!("steam_api_key must be the key as Steam shows it (letters and digits)");
        }
        let key = self.vapid_private_key.as_deref().filter(|v| !v.is_empty());
        let subject = self.vapid_subject.as_deref().filter(|v| !v.is_empty());
        if key.is_some() != subject.is_some() {
            bail!("vapid_private_key and vapid_subject go together");
        }
        if let (Some(key), Some(subject)) = (key, subject) {
            if !(subject.starts_with("mailto:") || subject.starts_with("https://")) {
                bail!("vapid_subject must start with mailto: or https://");
            }
            crate::push::Vapid::new(key, subject).context("vapid_private_key")?;
        }
        Ok(())
    }

    /// The Discord application, when the cloud has one: its Client ID and secret.
    pub fn discord(&self) -> Option<(&str, &str)> {
        let id = self
            .discord_client_id
            .as_deref()
            .filter(|v| !v.is_empty())?;
        let secret = self
            .discord_client_secret
            .as_deref()
            .filter(|v| !v.is_empty())?;
        Some((id, secret))
    }

    /// The VAPID key and contact for Web Push, when the cloud has them.
    pub fn vapid(&self) -> Option<(&str, &str)> {
        let key = self
            .vapid_private_key
            .as_deref()
            .filter(|v| !v.is_empty())?;
        let subject = self.vapid_subject.as_deref().filter(|v| !v.is_empty())?;
        Some((key, subject))
    }

    /// The Steam Web API key, if the cloud has one.
    pub fn steam_key(&self) -> Option<&str> {
        self.steam_api_key.as_deref().filter(|v| !v.is_empty())
    }

    /// Whether the player signs in on the portal's paths (`/sign-in`, `/create`) and not on the
    /// cloud's HTML pages: the cloud serves the portal, or another service does.
    pub fn portal_landing(&self) -> bool {
        self.portal_dir.is_some() || self.portal_external
    }

    /// The origin of the account web, without a trailing slash, when this cloud has one.
    pub fn web_origin(&self) -> Option<String> {
        self.public_url
            .as_deref()
            .and_then(|url| check_public_url(url).ok())
    }

    /// The version of the terms in force, if there are any.
    pub fn terms(&self) -> Option<&str> {
        self.terms_version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }

    /// The URL the admin commands use to reach this cloud on the same machine.
    pub fn local_url(&self) -> String {
        let ip = if self.listen.ip().is_unspecified() {
            if self.listen.is_ipv6() {
                "[::1]".to_owned()
            } else {
                "127.0.0.1".to_owned()
            }
        } else if self.listen.is_ipv6() {
            format!("[{}]", self.listen.ip())
        } else {
            self.listen.ip().to_string()
        };
        format!("http://{ip}:{}", self.listen.port())
    }
}

/// Checks `public_url` and returns it as an origin: scheme, host and port, no trailing slash. A
/// passkey needs a domain name (not an IP address) and a secure origin, so `http` is accepted
/// for `localhost` only.
pub fn check_public_url(text: &str) -> Result<String> {
    let url = reqwest::Url::parse(text.trim())
        .map_err(|e| anyhow::anyhow!("public_url: {text:?} is not a URL ({e})"))?;
    let plain_path = matches!(url.path(), "" | "/");
    if !plain_path
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!(
            "public_url must be an origin such as https://play.example.com, without path, query or user"
        );
    }
    let Some(host) = url.domain() else {
        bail!("public_url needs a domain name, not an IP address: passkeys are tied to one");
    };
    match url.scheme() {
        "https" => {}
        "http" if host == "localhost" => {}
        _ => bail!("public_url must be https (http only for localhost)"),
    }
    Ok(url.origin().ascii_serialization())
}

/// Timings and limits that tests shorten. Not part of the settings file.
#[derive(Clone, Debug)]
pub struct Tuning {
    pub code_ttl: Duration,
    pub code_tries: u8,
    pub signup_ttl: Duration,
    pub access_ttl_secs: u64,
    pub relay_ttl_secs: u64,
    /// How long an account stays online after its last event connection closed.
    pub offline_grace: Duration,
    /// How long a member may stay disconnected before it leaves its sessions.
    pub session_grace: Duration,
    pub invitation_ttl: Duration,
    pub ping_interval: Duration,
    pub sweep_interval: Duration,
    /// How long a node may miss its heartbeat before its connections count as closed.
    pub node_timeout: Duration,
    pub rates: Rates,
    /// The most feed items one account gets per UTC day; later events are not recorded.
    pub feed_items_per_day: u32,
    /// How long a party member may stay disconnected before it leaves the party.
    pub party_grace: Duration,
    /// How long a browser session on the account web may sit unused.
    pub web_idle: Duration,
    /// The longest a browser session lives.
    pub web_max: Duration,
    /// How long a one-time code from the browser sign-in or an account link lives.
    pub link_ttl: Duration,
    /// How long a passkey ceremony may take, from the browser asking to the authenticator
    /// answering.
    pub ceremony_ttl: Duration,
    /// A browser session younger than this may do sensitive things (remove a passkey, sign out
    /// everywhere, delete the account); an older one signs in again first.
    pub fresh_for: Duration,
    /// How long after the player asks to delete the account it is deleted, and they can cancel.
    pub deletion_grace: Duration,
    /// How long a started connection waits for the provider to send the browser back.
    pub link_pending_ttl: Duration,
    /// Where the providers of connections are. Tests point these at a fake server.
    pub links: LinkEndpoints,
    /// Whether a push subscription may point at any address (tests run a push service of their
    /// own on this machine). Off: only the push services of the browsers.
    pub push_any_host: bool,
}

/// The addresses of the services behind the connections.
#[derive(Clone, Debug)]
pub struct LinkEndpoints {
    /// Where the player agrees (`/oauth2/authorize`).
    pub discord_web: String,
    /// The API (`/oauth2/token`, `/users/@me`).
    pub discord_api: String,
    /// Where avatars are.
    pub discord_cdn: String,
    /// The OpenID endpoint of Steam (`/login`; the claimed ID lives under `/id/`).
    pub steam_openid: String,
    /// The Steam Web API.
    pub steam_api: String,
}

impl Default for LinkEndpoints {
    fn default() -> Self {
        Self {
            discord_web: "https://discord.com".into(),
            discord_api: "https://discord.com/api".into(),
            discord_cdn: "https://cdn.discordapp.com".into(),
            steam_openid: "https://steamcommunity.com/openid".into(),
            steam_api: "https://api.steampowered.com".into(),
        }
    }
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            code_ttl: Duration::from_secs(10 * 60),
            code_tries: 5,
            signup_ttl: Duration::from_secs(15 * 60),
            access_ttl_secs: 15 * 60,
            relay_ttl_secs: 24 * 60 * 60,
            offline_grace: Duration::from_secs(10),
            session_grace: Duration::from_secs(30),
            invitation_ttl: Duration::from_secs(15 * 60),
            ping_interval: Duration::from_secs(20),
            sweep_interval: Duration::from_secs(5),
            node_timeout: Duration::from_secs(30),
            rates: Rates::default(),
            feed_items_per_day: 100,
            party_grace: Duration::from_secs(15 * 60),
            web_idle: Duration::from_secs(30 * 60),
            web_max: Duration::from_secs(4 * 60 * 60),
            link_ttl: Duration::from_secs(60),
            ceremony_ttl: Duration::from_secs(5 * 60),
            fresh_for: Duration::from_secs(15 * 60),
            deletion_grace: Duration::from_secs(30 * 24 * 60 * 60),
            link_pending_ttl: Duration::from_secs(5 * 60),
            links: LinkEndpoints::default(),
            push_any_host: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tls_mode_follows_the_port_unless_it_is_set() {
        let mut config = Config::default();
        assert_eq!(config.smtp_tls(), SmtpTls::Starttls);
        config.smtp_port = Some(587);
        assert_eq!(config.smtp_tls(), SmtpTls::Starttls);
        config.smtp_port = Some(465);
        assert_eq!(config.smtp_tls(), SmtpTls::Tls);
        config.smtp_tls = Some(SmtpTls::Starttls);
        assert_eq!(
            config.smtp_tls(),
            SmtpTls::Starttls,
            "an explicit mode wins"
        );
        config.smtp_port = None;
        config.smtp_tls = Some(SmtpTls::None);
        assert_eq!(config.smtp_tls(), SmtpTls::None);
    }

    #[test]
    fn implicit_is_another_word_for_tls() {
        let mut config = Config::default();
        config
            .apply_env(|name| (name == "SMTP_TLS").then(|| "Implicit".into()))
            .unwrap();
        assert_eq!(config.smtp_tls, Some(SmtpTls::Tls));
        let file: Config = toml::from_str("smtp_tls = \"implicit\"").unwrap();
        assert_eq!(file.smtp_tls(), SmtpTls::Tls);
        assert!(toml::from_str::<Config>("smtp_tls = \"ssl\"").is_err());
    }

    #[test]
    fn the_environment_beats_the_file() {
        let mut config: Config = toml::from_str(
            r#"
            name = "From file"
            relays = ["https://relay.example"]
            mail = "log"
            "#,
        )
        .unwrap();
        config
            .apply_env(|name| match name {
                "NAME" => Some("From env".into()),
                "RELAYS" => Some("https://a.example, https://b.example".into()),
                "MAIL" => Some("SMTP".into()),
                "TRUST_PROXY" => Some("true".into()),
                "SIGNUP" => Some("closed".into()),
                "ADMIN_TOKEN" => Some("".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(config.name, "From env");
        assert_eq!(config.relays, ["https://a.example", "https://b.example"]);
        assert_eq!(config.mail, MailMode::Smtp);
        assert!(config.trust_proxy);
        assert_eq!(config.signup, Signup::Closed);
        assert_eq!(config.admin_token, None);
    }

    #[test]
    fn feed_items_are_kept_90_days_unless_set() {
        let mut config = Config::default();
        assert_eq!(config.feed_retention_days, 90);
        config
            .apply_env(|name| match name {
                "FEED_RETENTION_DAYS" => Some("7".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(config.feed_retention_days, 7);
        config.validate().unwrap();
        config.feed_retention_days = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn messages_are_kept_30_days_and_new_accounts_wait_a_day_by_default() {
        let config = Config::default();
        assert_eq!(config.message_retention_days, 30);
        assert_eq!(config.new_account_hours, 24);
        assert_eq!(config.new_account_friend_requests_per_day, 10);
        let mut config = Config::default();
        config
            .apply_env(|name| match name {
                "MESSAGE_RETENTION_DAYS" => Some("7".into()),
                "NEW_ACCOUNT_HOURS" => Some("0".into()),
                "NEW_ACCOUNT_FRIEND_REQUESTS_PER_DAY" => Some("3".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(config.message_retention_days, 7);
        assert_eq!(config.new_account_hours, 0);
        assert_eq!(config.new_account_friend_requests_per_day, 3);
        config.validate().unwrap();
        config.message_retention_days = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn mistakes_are_refused() {
        assert!(toml::from_str::<Config>("colour = 1").is_err());
        let mut config = Config::default();
        assert!(
            config
                .apply_env(|n| (n == "LISTEN").then(|| "nope".into()))
                .is_err()
        );
        assert!(
            config
                .apply_env(|n| (n == "MAIL").then(|| "pigeon".into()))
                .is_err()
        );

        let smtp = Config {
            mail: MailMode::Smtp,
            ..Config::default()
        };
        assert!(smtp.validate().is_err(), "smtp needs a host");
        let short = Config {
            admin_token: Some("short".into()),
            ..Config::default()
        };
        assert!(short.validate().is_err());
        let relay = Config {
            relays: vec!["relay.example".into()],
            ..Config::default()
        };
        assert!(relay.validate().is_err());
        let mysql = Config {
            database_url: Some("mysql://db.example/cloud".into()),
            ..Config::default()
        };
        assert!(
            mysql.validate().is_err(),
            "only Postgres stands in for the file"
        );
        let postgres = Config {
            database_url: Some("postgres://cloud@db.example/cloud".into()),
            ..Config::default()
        };
        assert!(postgres.validate().is_ok());
        let lonely = Config {
            nats_url: Some("nats://nats.internal:4222".into()),
            ..Config::default()
        };
        assert!(lonely.validate().is_err(), "a cluster shares its database");
        let cluster = Config {
            nats_url: Some("nats://nats.internal:4222".into()),
            ..postgres
        };
        assert!(cluster.validate().is_ok());
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn the_signing_key_file_comes_from_the_environment_and_must_be_a_file() {
        let mut config = Config::default();
        assert_eq!(config.signing_key_file, None);
        config
            .apply_env(|n| (n == "SIGNING_KEY_FILE").then(|| "/keys/signing.key".into()))
            .unwrap();
        assert_eq!(config.signing_key_file, Some("/keys/signing.key".into()));
        assert!(config.validate().is_ok());
        let folder = Config {
            signing_key_file: Some(std::env::temp_dir()),
            ..Config::default()
        };
        assert!(folder.validate().is_err());
        let file: Config = toml::from_str(r#"signing_key_file = "/keys/signing.key""#).unwrap();
        assert_eq!(file.signing_key_file, Some("/keys/signing.key".into()));
    }

    #[test]
    fn the_local_url_replaces_a_wildcard_address() {
        assert_eq!(Config::default().local_url(), "http://127.0.0.1:8080");
    }

    #[test]
    fn the_public_url_is_an_origin_with_a_domain_name() {
        for (text, origin) in [
            ("https://play.example.com", "https://play.example.com"),
            ("https://play.example.com/", "https://play.example.com"),
            (
                "https://Play.Example.com:8443",
                "https://play.example.com:8443",
            ),
            ("http://localhost:8080", "http://localhost:8080"),
        ] {
            assert_eq!(check_public_url(text).unwrap(), origin, "{text}");
        }
        for bad in [
            "",
            "play.example.com",
            "http://play.example.com",
            "https://127.0.0.1",
            "https://[::1]",
            "https://play.example.com/web",
            "https://play.example.com/?x=1",
            "https://play.example.com/#x",
            "https://user@play.example.com",
            "ftp://play.example.com",
        ] {
            assert!(check_public_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_portal_folder_is_read_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config
            .apply_env(|name| match name {
                "PORTAL_DIR" => Some(dir.path().display().to_string()),
                "PUBLIC_URL" => Some("https://play.example.com".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(config.portal_dir.as_deref(), Some(dir.path()));
        assert!(config.validate().is_err(), "a folder without index.html");
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        config.validate().unwrap();
        config.public_url = None;
        assert!(
            config.validate().is_err(),
            "the portal is served from the origin of the account web"
        );
        assert_eq!(Config::default().portal_dir, None);
    }

    #[test]
    fn an_external_portal_is_read_and_checked() {
        let mut config = Config::default();
        assert!(!config.portal_external);
        config
            .apply_env(|name| match name {
                "PORTAL_EXTERNAL" => Some("true".into()),
                "PUBLIC_URL" => Some("https://play.example.com".into()),
                _ => None,
            })
            .unwrap();
        assert!(config.portal_external);
        assert!(config.portal_landing());
        config.validate().unwrap();
        config.public_url = None;
        assert!(config.validate().is_err(), "it needs public_url");
        config.public_url = Some("https://play.example.com".into());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        config.portal_dir = Some(dir.path().into());
        assert!(config.validate().is_err(), "not together with portal_dir");
        assert!(
            Config::default()
                .apply_env(|n| (n == "PORTAL_EXTERNAL").then(|| "maybe".into()))
                .is_err()
        );
    }

    #[test]
    fn the_account_web_settings_are_read_and_checked() {
        let mut config = Config::default();
        config
            .apply_env(|name| match name {
                "PUBLIC_URL" => Some("https://play.example.com".into()),
                "TERMS_VERSION" => Some(" 2026-10 ".into()),
                "LEGACY_DEVICE_DAYS" => Some("0".into()),
                _ => None,
            })
            .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.web_origin().as_deref(),
            Some("https://play.example.com")
        );
        assert_eq!(config.terms(), Some("2026-10"));
        assert_eq!(config.legacy_device_days, 0);

        config.terms_version = Some("  ".into());
        assert_eq!(config.terms(), None, "an empty version is no version");
        config.public_url = Some("http://example.com".into());
        assert!(config.validate().is_err());
        config.public_url = None;
        config.legacy_device_days = 400;
        assert!(config.validate().is_err());
        assert_eq!(Config::default().web_origin(), None);
    }

    #[test]
    fn the_connection_settings_are_read_and_checked() {
        let mut config = Config::default();
        assert!(config.discord().is_none() && config.steam_key().is_none());
        config
            .apply_env(|name| match name {
                "PUBLIC_URL" => Some("https://play.example.com".into()),
                "DISCORD_CLIENT_ID" => Some(" 1234567890 ".into()),
                "DISCORD_CLIENT_SECRET" => Some("shh".into()),
                "STEAM_API_KEY" => Some("ABCDEF0123456789".into()),
                _ => None,
            })
            .unwrap();
        config.validate().unwrap();
        assert_eq!(config.discord(), Some(("1234567890", "shh")));
        assert_eq!(config.steam_key(), Some("ABCDEF0123456789"));

        // The ID and the secret go together, and Discord needs a public address to come back to.
        let mut half = config.clone();
        half.discord_client_secret = None;
        assert!(half.validate().is_err());
        let mut lonely = config.clone();
        lonely.public_url = None;
        assert!(lonely.validate().is_err());
        let mut name = config.clone();
        name.discord_client_id = Some("my-app".into());
        assert!(name.validate().is_err(), "the Client ID is a number");
        let mut key = config.clone();
        key.steam_api_key = Some("not a key".into());
        assert!(key.validate().is_err());
        // An empty value counts as not set.
        let empty = Config {
            discord_client_id: Some(String::new()),
            discord_client_secret: Some(String::new()),
            ..Config::default()
        };
        empty.validate().unwrap();
        assert!(empty.discord().is_none());
    }
}
