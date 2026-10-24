//! The AnyPlay cloud: accounts, friends, presence, sessions, invitations and relay tickets.
//!
//! It implements the API in `spec/cloud-api.md` of anyplay-gg/network, using the types of the
//! `anyplay-api` crate for every request, answer and event. Everything that more than one node
//! needs is in the store: durable data ([`db`]) and live state ([`live`]), in SQLite or
//! PostgreSQL. A node keeps only its own event connections ([`news`]).

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, Result};
use anyplay_ticket::Signer;
use tokio::net::TcpListener;

pub mod cli;
pub mod config;
pub mod db;
pub mod http;
pub mod limits;
pub mod live;
pub mod mail;
pub mod news;
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
    let file = config.data_dir.join(DATABASE_FILE);
    let location = match &config.database_url {
        Some(url) => db::Location::Postgres(url),
        None => db::Location::File(&file),
    };
    let pool = db::open(location, config.database_connections)
        .await
        .context("cannot open the database")?;

    let node = util::random_token(12);
    let (news, notes) = match &config.nats_url {
        Some(url) => {
            let (news, notes) = news::News::over_nats(url, &node)
                .await
                .with_context(|| format!("cannot reach NATS at {url}"))?;
            (news, Some(notes))
        }
        None => (news::News::default(), None),
    };

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
        cache: RwLock::new(cache),
        news,
        node,
        social: tokio::sync::Mutex::new(()),
    });
    // The first heartbeat, so that other nodes count this one as alive from the start.
    state
        .sweep()
        .await
        .map_err(|e| anyhow::anyhow!(e.body.message))
        .context("cannot write to the store")?;
    state::start_sweeper(&state);
    if let Some(mut notes) = notes {
        let weak = Arc::downgrade(&state);
        tokio::spawn(async move {
            while let Some(note) = notes.recv().await {
                let Some(state) = weak.upgrade() else { return };
                state.apply_note(note);
            }
        });
    }
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
        closing.close_all();
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
