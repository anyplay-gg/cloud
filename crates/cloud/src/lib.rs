//! The AnyPlay cloud: accounts, friends, presence, sessions, invitations and relay tickets.
//!
//! It implements the API in `spec/cloud-api.md` of anyplay-gg/network, using the types of the
//! `anyplay-api` crate for every request, answer and event. Durable data is in SQLite
//! ([`db`]); everything else lives in memory ([`hub`]).

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context as _, Result};
use anyplay_ticket::Signer;
use tokio::net::TcpListener;

pub mod cli;
pub mod config;
pub mod db;
pub mod http;
pub mod hub;
pub mod limits;
pub mod mail;
pub mod routes;
pub mod state;
pub mod util;

mod error;

pub use config::{Config, Tuning};
pub use error::AppError;
pub use state::{AppState, Inner};

/// The file in the data directory that holds the secret signing key.
pub const KEY_FILE: &str = "signing.key";
/// The file that holds the matching public key. Relays read it.
pub const PUBLIC_KEY_FILE: &str = "signing.pub";
const DATABASE_FILE: &str = "anyplay-cloud.db";

/// The signing key of a data directory, created on first use. The public key is written next to it
/// every time, so a relay that shares the directory always finds the current one.
pub fn load_signing_key(data_dir: &std::path::Path) -> Result<(Signer, bool)> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;
    let path = data_dir.join(KEY_FILE);
    let (signer, created) = Signer::load_or_create(&path)
        .with_context(|| format!("cannot use the signing key {}", path.display()))?;
    let public: PathBuf = data_dir.join(PUBLIC_KEY_FILE);
    std::fs::write(&public, format!("{}\n", signer.verifier().to_base64()))
        .with_context(|| format!("cannot write {}", public.display()))?;
    Ok((signer, created))
}

/// Opens the data directory (database and signing key), loads who is signed in and who is banned,
/// and starts the background sweeper.
pub async fn open(
    config: Config,
    tuning: Tuning,
    mailer: Arc<dyn mail::Mailer>,
) -> Result<AppState> {
    let (signer, created) = load_signing_key(&config.data_dir)?;
    if created {
        tracing::info!("created a new signing key");
    }
    let pool = db::open(&config.data_dir.join(DATABASE_FILE))
        .await
        .context("cannot open the database")?;

    let mut cache = state::AuthCache::default();
    for (device, account) in db::all_devices(&pool).await? {
        cache.devices.insert(device, account);
    }
    let now = util::now();
    for ban in db::all_bans(&pool).await? {
        if ban.active(now) {
            cache.bans.insert(ban.account_id as u64, ban.until());
        }
    }

    let state = Arc::new(Inner {
        verifier: signer.verifier(),
        signer,
        config,
        tuning,
        db: pool,
        mailer,
        limiter: limits::RateLimiter::default(),
        pending: Mutex::new(state::Pending::default()),
        cache: RwLock::new(cache),
        hub: tokio::sync::Mutex::new(hub::Memory::default()),
        social: tokio::sync::Mutex::new(()),
    });
    state::start_sweeper(&state);
    Ok(state)
}

/// Serves the API on the listener until `shutdown` completes, then closes event connections and
/// stops.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = routes::router(state.clone());
    let closing = state.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        // Open event connections would keep a graceful shutdown waiting.
        closing.close_all().await;
    })
    .await
}

/// Completes on Ctrl-C or, on Unix, SIGTERM.
pub async fn shutdown_signal() {
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
