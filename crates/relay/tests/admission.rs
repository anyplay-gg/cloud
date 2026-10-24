//! A relay running in this process admits a client only with a ticket for its own endpoint.

use std::net::Ipv4Addr;
use std::sync::Arc;

use anyplay_relay::TicketAccess;
use anyplay_ticket::{Claims, Signer};
use iroh_base::{RelayUrl, SecretKey};
use iroh_dns::dns::DnsResolver;
use iroh_relay::client::{ClientBuilder, ConnectError};
use iroh_relay::protos::handshake;
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use iroh_relay::tls::{CaTlsConfig, default_provider};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn hex(key: &SecretKey) -> String {
    key.public()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn start(signer: &Signer) -> (Server, RelayUrl) {
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.access = Arc::new(TicketAccess::new(signer.verifier()));
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url)
}

async fn connect(url: &RelayUrl, key: SecretKey, token: Option<&str>) -> Result<(), ConnectError> {
    let tls = CaTlsConfig::default()
        .client_config(default_provider())
        .unwrap();
    let mut builder =
        ClientBuilder::new(url.clone(), key, DnsResolver::new()).tls_client_config(tls);
    if let Some(token) = token {
        builder = builder.auth_token(token);
    }
    builder.connect().await.map(|_| ())
}

fn denied(result: Result<(), ConnectError>) -> bool {
    matches!(
        result,
        Err(ConnectError::Handshake {
            source: handshake::Error::ServerDeniedAuth { .. },
            ..
        })
    )
}

#[tokio::test]
async fn a_client_with_a_ticket_for_its_endpoint_is_admitted() {
    let signer = Signer::generate();
    let (_server, url) = start(&signer).await;
    let key = SecretKey::generate();
    let ticket = signer.issue(&Claims::relay(1, hex(&key), now(), now() + 3600));
    connect(&url, key, Some(&ticket)).await.expect("admitted");
}

#[tokio::test]
async fn clients_without_a_good_ticket_are_denied() {
    let signer = Signer::generate();
    let (_server, url) = start(&signer).await;

    let key = SecretKey::generate();
    assert!(denied(connect(&url, key.clone(), None).await), "no ticket");

    assert!(
        denied(connect(&url, key.clone(), Some("v1.garbage.garbage")).await),
        "garbage"
    );

    let expired = signer.issue(&Claims::relay(
        1,
        hex(&key),
        now().saturating_sub(10),
        now().saturating_sub(5),
    ));
    assert!(
        denied(connect(&url, key.clone(), Some(&expired)).await),
        "expired"
    );

    let foreign = Signer::generate().issue(&Claims::relay(1, hex(&key), now(), now() + 3600));
    assert!(
        denied(connect(&url, key.clone(), Some(&foreign)).await),
        "signed by another cloud"
    );

    let other_key = SecretKey::generate();
    let borrowed = signer.issue(&Claims::relay(1, hex(&other_key), now(), now() + 3600));
    assert!(
        denied(connect(&url, key.clone(), Some(&borrowed)).await),
        "a ticket for somebody else's endpoint"
    );

    let access = signer.issue(&Claims::access(1, 2, now(), now() + 3600));
    assert!(
        denied(connect(&url, key, Some(&access)).await),
        "an access token is not a relay ticket"
    );
}

#[tokio::test]
async fn a_relay_with_a_certificate_from_files_serves_https_and_address_discovery() {
    use anyplay_relay::config::{Config, TlsMode};

    // Make a self-signed certificate; a reloading relay and a manual one read the same files.
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("relay.crt");
    let key = dir.path().join("relay.key");
    std::fs::write(&cert, certified.cert.pem()).unwrap();
    std::fs::write(&key, certified.signing_key.serialize_pem()).unwrap();

    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    for tls in [TlsMode::Manual, TlsMode::Reloading] {
        let config = Config {
            tls,
            cert_file: Some(cert.clone()),
            key_file: Some(key.clone()),
            http_bind: ([127, 0, 0, 1], 0).into(),
            https_bind: ([127, 0, 0, 1], 0).into(),
            quic_bind: ([127, 0, 0, 1], 0).into(),
            ..Config::default()
        };
        let signer = Signer::generate();
        let server_config = config.server_config(signer.verifier()).await.unwrap();
        let server = Server::spawn(server_config).await.unwrap();
        assert!(server.https_addr().is_some(), "{tls:?}: HTTPS");
        assert!(server.quic_addr().is_some(), "{tls:?}: address discovery");
    }
}

#[tokio::test]
async fn a_missing_certificate_is_an_error_after_the_wait() {
    use anyplay_relay::config::{Config, TlsMode};
    let config = Config {
        tls: TlsMode::Reloading,
        cert_file: Some("/nonexistent/relay.crt".into()),
        key_file: Some("/nonexistent/relay.key".into()),
        cert_wait: 0,
        ..Config::default()
    };
    let error = config
        .server_config(Signer::generate().verifier())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("did not appear"), "{error}");
}

/// The caps over a real connection: a second connection of an account is turned away at its
/// limit, and a place comes free when the first one ends. The metrics show what happened.
#[tokio::test]
async fn an_account_at_its_limit_is_turned_away_until_a_connection_ends() {
    use std::time::Duration;

    use anyplay_relay::config::{Config, TlsMode};
    use anyplay_relay::metrics;

    async fn open(
        url: &RelayUrl,
        signer: &Signer,
        account: u64,
    ) -> Result<iroh_relay::client::Client, ConnectError> {
        let key = SecretKey::generate();
        let ticket = signer.issue(&Claims::relay(account, hex(&key), now(), now() + 3600));
        let tls = CaTlsConfig::default()
            .client_config(default_provider())
            .unwrap();
        ClientBuilder::new(url.clone(), key, DnsResolver::new())
            .tls_client_config(tls)
            .auth_token(&ticket)
            .connect()
            .await
    }

    let signer = Signer::generate();
    let config = Config {
        tls: TlsMode::None,
        http_bind: (Ipv4Addr::LOCALHOST, 0).into(),
        max_connections_per_account: 1,
        ..Config::default()
    };
    let access = config.access(signer.verifier());
    let server = Server::spawn(config.server_config_with(access.clone()).await.unwrap())
        .await
        .unwrap();
    let url: RelayUrl = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    let sources = metrics::Sources {
        admission: access.admission().clone(),
        stats: access.stats().clone(),
        iroh: server.metrics().server.clone(),
    };

    let first = open(&url, &signer, 5).await.expect("the first is admitted");
    let second = open(&url, &signer, 5).await;
    assert!(denied(second.map(|_| ())), "the second is turned away");
    open(&url, &signer, 6)
        .await
        .expect("another account is not affected");

    let text = metrics::render(&sources);
    assert!(text.contains("anyplay_relay_refusals_total{reason=\"account_limit\"} 1"));
    assert!(text.contains("anyplay_relay_ticket_checks_total{result=\"ok\"} 3"));

    drop(first);
    let mut admitted = false;
    for _ in 0..50 {
        if open(&url, &signer, 5).await.is_ok() {
            admitted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        admitted,
        "the place came free when the first connection ended"
    );
}
