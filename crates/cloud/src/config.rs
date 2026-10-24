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

/// How the connection to the SMTP server is secured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// Start in plain text and switch to TLS (usually port 587).
    #[default]
    Starttls,
    /// TLS from the first byte (usually port 465).
    Tls,
    /// No encryption. Only for a server on the same machine.
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    /// A `postgres://` URL. Without it the store is a SQLite file in `data_dir`.
    pub database_url: Option<String>,
    /// The most connections the store keeps open.
    pub database_connections: u32,
    pub name: String,
    pub relays: Vec<String>,
    pub mail: MailMode,
    pub mail_from: String,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_tls: SmtpTls,
    pub signup: Signup,
    pub privacy_url: Option<String>,
    pub terms_url: Option<String>,
    pub trust_proxy: bool,
    pub admin_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([0, 0, 0, 0], 8080).into(),
            data_dir: PathBuf::from("data"),
            database_url: None,
            database_connections: 16,
            name: "AnyPlay cloud".into(),
            relays: Vec::new(),
            mail: MailMode::Log,
            mail_from: "AnyPlay cloud <noreply@localhost>".into(),
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_tls: SmtpTls::Starttls,
            signup: Signup::Open,
            privacy_url: None,
            terms_url: None,
            trust_proxy: false,
            admin_token: None,
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

impl Config {
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
        if let Some(v) = get("DATA_DIR") {
            self.data_dir = v.into();
        }
        if let Some(v) = get("DATABASE_URL") {
            self.database_url = Some(v);
        }
        if let Some(v) = get("DATABASE_CONNECTIONS") {
            self.database_connections = parse("DATABASE_CONNECTIONS", &v)?;
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
            self.smtp_tls = parse_word("SMTP_TLS", &v)?;
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
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(url) = &self.database_url
            && !(url.starts_with("postgres://") || url.starts_with("postgresql://"))
        {
            bail!(
                "database_url must be a postgres:// URL; without it the store is a file in data_dir"
            );
        }
        if self.database_connections == 0 {
            bail!("database_connections must be at least 1");
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
        if self.name.trim().is_empty() || self.name.len() > 64 {
            bail!("name must have 1 to 64 characters");
        }
        Ok(())
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
    /// How long an address is kept for an account that went offline.
    pub lease_ttl: Duration,
    pub invitation_ttl: Duration,
    pub ping_interval: Duration,
    pub sweep_interval: Duration,
    /// How long a node may miss its heartbeat before its connections count as closed.
    pub node_timeout: Duration,
    pub rates: Rates,
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
            lease_ttl: Duration::from_secs(10 * 60),
            invitation_ttl: Duration::from_secs(15 * 60),
            ping_interval: Duration::from_secs(20),
            sweep_interval: Duration::from_secs(5),
            node_timeout: Duration::from_secs(30),
            rates: Rates::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn the_local_url_replaces_a_wildcard_address() {
        assert_eq!(Config::default().local_url(), "http://127.0.0.1:8080");
    }
}
