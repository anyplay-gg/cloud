//! The relay's settings: a TOML file, each value overridable by an `ANYPLAY_RELAY_<NAME>`
//! environment variable. An empty variable counts as not set.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use anyplay_ticket::Verifier;
use iroh_relay::server::{
    AcmeConfig, CertConfig, ClientRateLimit, Limits, QuicConfig, RelayConfig, ServerConfig,
    TlsConfig,
};
use serde::{Deserialize, Serialize};

use crate::{AdmissionLimits, TicketAccess};

/// How the relay gets its TLS certificate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Plain HTTP only, no address discovery. For development.
    None,
    /// A certificate and key from files.
    Manual,
    /// Like `manual`, and the files are read again every hour, so a renewed certificate (for
    /// example one that Caddy manages) is used without a restart.
    Reloading,
    /// A certificate from Let's Encrypt for `hostname`.
    #[default]
    Letsencrypt,
}

impl std::str::FromStr for TlsMode {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(TlsMode::None),
            "manual" => Ok(TlsMode::Manual),
            "reloading" => Ok(TlsMode::Reloading),
            "letsencrypt" => Ok(TlsMode::Letsencrypt),
            other => bail!("tls must be none, manual, reloading or letsencrypt, not {other:?}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The cloud's public key as base64 text, as printed by `anyplay-cloud keygen`.
    pub cloud_public_key: Option<String>,
    /// A file that holds the same text. The cloud writes one next to its signing key.
    pub cloud_public_key_file: Option<PathBuf>,
    /// How long to wait for that file to appear, in seconds.
    pub cloud_public_key_wait: u64,
    pub http_bind: SocketAddr,
    pub https_bind: SocketAddr,
    /// The UDP address for QUIC address discovery. Needs TLS.
    pub quic_bind: SocketAddr,
    pub tls: TlsMode,
    pub cert_file: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    /// How long to wait for the certificate files to appear, in seconds.
    pub cert_wait: u64,
    /// The host name the certificate is for, with `tls = "letsencrypt"`.
    pub hostname: Option<String>,
    /// An e-mail address Let's Encrypt can reach the operator at.
    pub contact: Option<String>,
    /// Where Let's Encrypt account data and certificates are kept.
    pub cert_dir: PathBuf,
    /// Use Let's Encrypt's staging service, whose certificates no client trusts.
    pub letsencrypt_staging: bool,
    /// Limit what one client may send, in bytes per second. `0` is no limit.
    pub client_bytes_per_second: Option<u32>,
    /// How much one client may send at once before the limit above holds it back, in bytes. `0`
    /// leaves it to iroh-relay (a tenth of a second of the rate).
    pub client_burst_bytes: Option<u32>,
    /// Connections this relay carries at once. `0` is no limit.
    pub max_clients: usize,
    /// Connections one account may hold at once. `0` is no limit.
    pub max_connections_per_account: usize,
    /// The longest a ticket may still be valid for when a client presents it, in seconds. `0` is
    /// no limit.
    pub max_ticket_lifetime_secs: u64,
    /// Where Prometheus metrics are served. Without it there are none.
    pub metrics_bind: Option<SocketAddr>,
}

/// One mebibyte per second is about eight megabits: far more than a game needs through a relay.
const DEFAULT_CLIENT_BYTES_PER_SECOND: u32 = 1024 * 1024;
const DEFAULT_CLIENT_BURST_BYTES: u32 = 2 * 1024 * 1024;

impl Default for Config {
    fn default() -> Self {
        Self {
            cloud_public_key: None,
            cloud_public_key_file: None,
            cloud_public_key_wait: 120,
            http_bind: ([0, 0, 0, 0], 80).into(),
            https_bind: ([0, 0, 0, 0], 443).into(),
            quic_bind: ([0, 0, 0, 0], 7842).into(),
            tls: TlsMode::default(),
            cert_file: None,
            key_file: None,
            cert_wait: 180,
            hostname: None,
            contact: None,
            cert_dir: PathBuf::from("certs"),
            letsencrypt_staging: false,
            client_bytes_per_second: Some(DEFAULT_CLIENT_BYTES_PER_SECOND),
            client_burst_bytes: Some(DEFAULT_CLIENT_BURST_BYTES),
            max_clients: AdmissionLimits::DEFAULT_MAX_CLIENTS,
            max_connections_per_account: AdmissionLimits::DEFAULT_MAX_CONNECTIONS_PER_ACCOUNT,
            max_ticket_lifetime_secs: AdmissionLimits::DEFAULT_MAX_TICKET_LIFETIME_SECS,
            metrics_bind: None,
        }
    }
}

impl Config {
    /// Reads the file, if any, then applies the environment.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config = match path {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?
            }
            None => Config::default(),
        };
        config.apply_env(|name| std::env::var(format!("ANYPLAY_RELAY_{name}")).ok())?;
        Ok(config)
    }

    /// Applies overrides. `get` receives the upper-case field name, such as `HTTP_BIND`.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<()> {
        let get = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        fn parse<T: std::str::FromStr>(name: &str, text: &str) -> Result<T>
        where
            T::Err: std::fmt::Display,
        {
            text.trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("{name}: {e}"))
        }
        if let Some(v) = get("CLOUD_PUBLIC_KEY") {
            self.cloud_public_key = Some(v);
        }
        if let Some(v) = get("CLOUD_PUBLIC_KEY_FILE") {
            self.cloud_public_key_file = Some(v.into());
        }
        if let Some(v) = get("CLOUD_PUBLIC_KEY_WAIT") {
            self.cloud_public_key_wait = parse("CLOUD_PUBLIC_KEY_WAIT", &v)?;
        }
        if let Some(v) = get("HTTP_BIND") {
            self.http_bind = parse("HTTP_BIND", &v)?;
        }
        if let Some(v) = get("HTTPS_BIND") {
            self.https_bind = parse("HTTPS_BIND", &v)?;
        }
        if let Some(v) = get("QUIC_BIND") {
            self.quic_bind = parse("QUIC_BIND", &v)?;
        }
        if let Some(v) = get("TLS") {
            self.tls = parse("TLS", &v)?;
        }
        if let Some(v) = get("CERT_FILE") {
            self.cert_file = Some(v.into());
        }
        if let Some(v) = get("KEY_FILE") {
            self.key_file = Some(v.into());
        }
        if let Some(v) = get("CERT_WAIT") {
            self.cert_wait = parse("CERT_WAIT", &v)?;
        }
        if let Some(v) = get("HOSTNAME") {
            self.hostname = Some(v);
        }
        if let Some(v) = get("CONTACT") {
            self.contact = Some(v);
        }
        if let Some(v) = get("CERT_DIR") {
            self.cert_dir = v.into();
        }
        if let Some(v) = get("LETSENCRYPT_STAGING") {
            self.letsencrypt_staging = parse("LETSENCRYPT_STAGING", &v)?;
        }
        if let Some(v) = get("CLIENT_BYTES_PER_SECOND") {
            self.client_bytes_per_second = Some(parse("CLIENT_BYTES_PER_SECOND", &v)?);
        }
        if let Some(v) = get("CLIENT_BURST_BYTES") {
            self.client_burst_bytes = Some(parse("CLIENT_BURST_BYTES", &v)?);
        }
        if let Some(v) = get("MAX_CLIENTS") {
            self.max_clients = parse("MAX_CLIENTS", &v)?;
        }
        if let Some(v) = get("MAX_CONNECTIONS_PER_ACCOUNT") {
            self.max_connections_per_account = parse("MAX_CONNECTIONS_PER_ACCOUNT", &v)?;
        }
        if let Some(v) = get("MAX_TICKET_LIFETIME_SECS") {
            self.max_ticket_lifetime_secs = parse("MAX_TICKET_LIFETIME_SECS", &v)?;
        }
        if let Some(v) = get("METRICS_BIND") {
            self.metrics_bind = Some(parse("METRICS_BIND", &v)?);
        }
        Ok(())
    }

    /// The cloud's public key, from the setting or from the file. With a file, waits for it to
    /// appear, because the cloud writes it on its first start.
    pub async fn verifier(&self) -> Result<Verifier> {
        if let Some(text) = &self.cloud_public_key {
            return Verifier::from_base64(text).context("cloud_public_key is not a public key");
        }
        let Some(path) = &self.cloud_public_key_file else {
            bail!("set cloud_public_key or cloud_public_key_file");
        };
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.cloud_public_key_wait);
        loop {
            match tokio::fs::read_to_string(path).await {
                Ok(text) => {
                    return Verifier::from_base64(&text)
                        .with_context(|| format!("{} does not hold a public key", path.display()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if tokio::time::Instant::now() >= deadline {
                        bail!("{} did not appear; has the cloud started?", path.display());
                    }
                    tracing::info!(path = %path.display(), "waiting for the cloud's public key");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
            }
        }
    }

    /// The admission check with this configuration's limits.
    pub fn access(&self, verifier: Verifier) -> Arc<TicketAccess> {
        Arc::new(TicketAccess::with_limits(
            verifier,
            AdmissionLimits {
                max_clients: self.max_clients,
                max_connections_per_account: self.max_connections_per_account,
                max_ticket_lifetime_secs: self.max_ticket_lifetime_secs,
            },
        ))
    }

    /// The per-client receive limit, if there is one.
    pub fn client_rate_limit(&self) -> Option<ClientRateLimit> {
        let rate = self.client_bytes_per_second.and_then(NonZeroU32::new)?;
        let mut limit = ClientRateLimit::new(rate);
        limit.max_burst_bytes = self.client_burst_bytes.and_then(NonZeroU32::new);
        Some(limit)
    }

    /// Everything `iroh-relay` needs to start.
    pub async fn server_config(&self, verifier: Verifier) -> Result<ServerConfig> {
        self.server_config_with(self.access(verifier)).await
    }

    /// Like [`Config::server_config`], for a caller that keeps the `access` to read its counters.
    pub async fn server_config_with(&self, access: Arc<TicketAccess>) -> Result<ServerConfig> {
        let mut relay = RelayConfig::new(self.http_bind);
        relay.access = access;
        relay.key_cache_capacity = Some(1024);
        let mut limits = Limits::default();
        limits.client_rx = self.client_rate_limit();
        relay.limits = limits;

        let mut quic = None;
        if self.tls != TlsMode::None {
            let cert = self.cert_config().await?;
            relay.tls = Some(TlsConfig::new(self.https_bind, cert));
            quic = Some(QuicConfig::new(self.quic_bind));
        }

        let mut server = ServerConfig::default();
        server.relay = Some(relay);
        server.quic = quic;
        Ok(server)
    }

    async fn cert_config(&self) -> Result<CertConfig> {
        let builder = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .context("TLS protocol versions")?
        .with_no_client_auth();
        match self.tls {
            TlsMode::None => bail!("no certificate is needed without TLS"),
            TlsMode::Manual | TlsMode::Reloading => {
                let cert = self
                    .cert_file
                    .clone()
                    .context("tls = manual or reloading needs cert_file")?;
                let key = self
                    .key_file
                    .clone()
                    .context("tls = manual or reloading needs key_file")?;
                wait_for_files(&[&cert, &key], Duration::from_secs(self.cert_wait)).await?;
                if self.tls == TlsMode::Reloading {
                    let resolver = iroh_relay::server::reloading_resolver(
                        builder.crypto_provider(),
                        cert,
                        key,
                        RELOAD_INTERVAL,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("cannot load the certificate: {e}"))?;
                    let server_config = builder.with_cert_resolver(resolver);
                    return Ok(CertConfig::Manual { server_config });
                }
                let (certs, key) = tokio::task::spawn_blocking(move || load_pem(&cert, &key))
                    .await
                    .context("reading certificate files")??;
                let server_config = builder
                    .with_single_cert(certs, key)
                    .context("certificate and key do not fit")?;
                Ok(CertConfig::Manual { server_config })
            }
            TlsMode::Letsencrypt => {
                let hostname = self
                    .hostname
                    .clone()
                    .context("tls = letsencrypt needs hostname")?;
                let contact = self
                    .contact
                    .clone()
                    .context("tls = letsencrypt needs contact")?;
                let acme_config = AcmeConfig::letsencrypt(!self.letsencrypt_staging)
                    .domains(vec![hostname])
                    .contact(vec![format!("mailto:{contact}")])
                    .cache_path(self.cert_dir.clone());
                Ok(CertConfig::LetsEncrypt {
                    acme_config,
                    server_config_builder: builder,
                })
            }
        }
    }
}

/// How often a `reloading` certificate is read again.
const RELOAD_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Waits until all files exist, for a certificate that another program is still fetching.
async fn wait_for_files(files: &[&Path], wait: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let missing = files.iter().find(|f| !f.exists());
        let Some(missing) = missing else {
            return Ok(());
        };
        if tokio::time::Instant::now() >= deadline {
            bail!("{} did not appear in time", missing.display());
        }
        tracing::info!(path = %missing.display(), "waiting for a certificate file");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
type Pem = (
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
);

fn load_pem(cert: &Path, key: &Path) -> Result<Pem> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs = CertificateDer::pem_file_iter(cert)
        .with_context(|| format!("cannot open {}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("cannot read certificates from {}", cert.display()))?;
    if certs.is_empty() {
        bail!("{} holds no certificate", cert.display());
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .with_context(|| format!("cannot read the key from {}", key.display()))?;
    Ok((certs, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_and_the_environment_combine_with_the_environment_winning() {
        let mut config: Config = toml::from_str(
            r#"
            tls = "none"
            http_bind = "127.0.0.1:3340"
            hostname = "relay.example"
            "#,
        )
        .unwrap();
        config
            .apply_env(|name| match name {
                "HTTP_BIND" => Some("127.0.0.1:9999".into()),
                "HOSTNAME" => Some("  ".into()),
                "CLIENT_BYTES_PER_SECOND" => Some("5000".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(config.http_bind, "127.0.0.1:9999".parse().unwrap());
        assert_eq!(config.hostname.as_deref(), Some("relay.example"));
        assert_eq!(config.client_bytes_per_second, Some(5000));
        assert_eq!(config.tls, TlsMode::None);
    }

    #[test]
    fn the_limits_are_on_by_default_and_settable() {
        let config = Config::default();
        let rate = config.client_rate_limit().expect("a rate limit by default");
        assert_eq!(rate.bytes_per_second.get(), 1024 * 1024);
        assert_eq!(rate.max_burst_bytes.map(|b| b.get()), Some(2 * 1024 * 1024));
        assert_eq!(config.max_clients, 10_000);
        assert_eq!(config.max_connections_per_account, 16);
        assert_eq!(config.max_ticket_lifetime_secs, 25 * 60 * 60);

        let mut config = config;
        config
            .apply_env(|name| {
                Some(
                    match name {
                        "CLIENT_BYTES_PER_SECOND" => "2000",
                        "CLIENT_BURST_BYTES" => "0",
                        "MAX_CLIENTS" => "5",
                        "MAX_CONNECTIONS_PER_ACCOUNT" => "2",
                        "MAX_TICKET_LIFETIME_SECS" => "60",
                        "METRICS_BIND" => "127.0.0.1:9187",
                        _ => return None,
                    }
                    .into(),
                )
            })
            .unwrap();
        let rate = config.client_rate_limit().unwrap();
        assert_eq!(rate.bytes_per_second.get(), 2000);
        assert_eq!(rate.max_burst_bytes, None, "0 leaves the burst to iroh");
        assert_eq!(config.max_clients, 5);
        assert_eq!(config.max_connections_per_account, 2);
        assert_eq!(config.max_ticket_lifetime_secs, 60);
        assert_eq!(config.metrics_bind, Some("127.0.0.1:9187".parse().unwrap()));

        config.client_bytes_per_second = Some(0);
        assert!(config.client_rate_limit().is_none(), "0 turns it off");
    }

    #[test]
    fn unknown_settings_and_bad_values_are_errors() {
        assert!(toml::from_str::<Config>("colour = 1").is_err());
        let mut config = Config::default();
        assert!(
            config
                .apply_env(|n| (n == "TLS").then(|| "maybe".into()))
                .is_err()
        );
    }

    #[tokio::test]
    async fn the_rate_limit_reaches_the_relay_server() {
        let verifier = anyplay_ticket::Signer::generate().verifier();
        let config = Config {
            tls: TlsMode::None,
            ..Config::default()
        };
        let server = config.server_config(verifier.clone()).await.unwrap();
        let limit = server
            .relay
            .unwrap()
            .limits
            .client_rx
            .expect("on by default");
        assert_eq!(limit.bytes_per_second.get(), 1024 * 1024);
        let off = Config {
            tls: TlsMode::None,
            client_bytes_per_second: Some(0),
            ..Config::default()
        };
        let server = off.server_config(verifier).await.unwrap();
        assert!(server.relay.unwrap().limits.client_rx.is_none());
    }

    #[tokio::test]
    async fn the_public_key_comes_from_the_setting() {
        let signer = anyplay_ticket::Signer::generate();
        let config = Config {
            cloud_public_key: Some(signer.verifier().to_base64()),
            ..Config::default()
        };
        assert_eq!(config.verifier().await.unwrap(), signer.verifier());
        assert!(Config::default().verifier().await.is_err());
    }
}
