//! The cloud under test: nodes started in this process, or an external cloud at a URL, and where
//! the harness reads sign-in codes from.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use anyplay_cloud::limits::Rates;
use anyplay_cloud::mail::{BoxFuture, MailError, Mailer};
use anyplay_cloud::{AppState, Config, Tuning, open, serve, serve_metrics};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::opts::Options;

/// Keeps the latest code per address, and gives it up once. The cloud's own `MemoryMailer` keeps
/// every code in a list and searches it from the end, which would cost the harness more with every
/// player.
#[derive(Default)]
pub struct LoadMailer {
    codes: Mutex<HashMap<String, String>>,
}

impl Mailer for LoadMailer {
    fn send_code<'a>(&'a self, to: &'a str, code: &'a str) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            self.codes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(to.to_owned(), code.to_owned());
            Ok(())
        })
    }
}

/// Where the sign-in codes come from.
pub enum Codes {
    Memory(Arc<LoadMailer>),
    /// Read from the log of an external cloud.
    Log(Arc<Mutex<HashMap<String, String>>>),
}

impl Codes {
    /// The code sent to `email`, once it has been sent.
    pub async fn take(&self, email: &str) -> Option<String> {
        for _ in 0..2000 {
            let found = match self {
                Codes::Memory(mailer) => mailer.codes.lock(),
                Codes::Log(codes) => codes.lock(),
            }
            .unwrap_or_else(|e| e.into_inner())
            .remove(email);
            if found.is_some() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        None
    }
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `(address, code)` from a log line of the cloud's `LogMailer`.
pub fn parse_code_line(line: &str) -> Option<(String, String)> {
    let clean = strip_ansi(line);
    if !clean.contains("sign-in code") {
        return None;
    }
    let field = |key: &str| {
        let at = clean.find(key)?;
        let rest = &clean[at + key.len()..];
        Some(rest.split_whitespace().next()?.trim_matches('"').to_owned())
    };
    Some((field(" to=")?.to_lowercase(), field(" code=")?))
}

fn tail_codes(path: PathBuf, codes: Arc<Mutex<HashMap<String, String>>>) {
    std::thread::spawn(move || {
        let mut position = 0u64;
        let mut pending = String::new();
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let Ok(mut file) = std::fs::File::open(&path) else {
                continue;
            };
            let Ok(len) = file.metadata().map(|m| m.len()) else {
                continue;
            };
            if len < position {
                position = 0;
            }
            if len == position || file.seek(SeekFrom::Start(position)).is_err() {
                continue;
            }
            let mut bytes = Vec::new();
            if file.read_to_end(&mut bytes).is_err() {
                continue;
            }
            position += bytes.len() as u64;
            pending.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(end) = pending.find('\n') {
                let line: String = pending.drain(..=end).collect();
                if let Some((to, code)) = parse_code_line(&line) {
                    codes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(to, code);
                }
            }
        }
    });
}

struct InProc {
    runtime: Runtime,
    state: AppState,
    stop: Option<oneshot::Sender<()>>,
    server: JoinHandle<std::io::Result<()>>,
}

pub struct Node {
    pub index: usize,
    /// `host:port` of the API.
    pub base: String,
    /// `host:port` of `/metrics`, when there is one.
    pub metrics: Option<String>,
    alive: AtomicBool,
    inproc: Mutex<Option<InProc>>,
}

impl Node {
    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn set_alive(&self, alive: bool) {
        self.alive.store(alive, Ordering::Relaxed);
    }

    /// Store connections in use and the pool's size, for an in-process node.
    pub fn pool_busy(&self) -> Option<(u32, u32)> {
        let guard = self.inproc.lock().unwrap_or_else(|e| e.into_inner());
        let node = guard.as_ref()?;
        let size = node.state.db.size();
        let idle = node.state.db.num_idle() as u32;
        Some((size.saturating_sub(idle), size))
    }

    /// Stops an in-process node the way a deploy does: not ready, drain, close with 1001.
    pub async fn stop(&self) {
        self.set_alive(false);
        let taken = self.inproc.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut node) = taken {
            if let Some(stop) = node.stop.take() {
                let _ = stop.send(());
            }
            let _ = node.server.await;
            node.runtime.shutdown_background();
        }
    }

    /// Stops an in-process node at once, as a crash does: its tasks and sockets vanish.
    pub fn kill(&self) {
        self.set_alive(false);
        let taken = self.inproc.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(node) = taken {
            node.runtime.shutdown_background();
        }
    }
}

pub struct Target {
    pub nodes: Vec<Arc<Node>>,
    pub codes: Codes,
    /// The server and name of the database this run created, to drop at the end.
    created_database: Mutex<Option<(String, String)>>,
    _dir: Option<tempfile::TempDir>,
    pub store: &'static str,
}

fn strip_scheme(url: &str) -> String {
    url.trim()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned()
}

impl Target {
    pub async fn start(opts: &Options) -> Result<Self> {
        if opts.external() {
            return Ok(Self::external(opts));
        }
        let (dir, database_url, created_database) = match &opts.database_url {
            Some(server) => {
                let (base, _) = server
                    .rsplit_once('/')
                    .context("--database-url ends in a database name")?;
                let name = format!("anyplay_load_{:016x}", rand::random::<u64>());
                create_database(server, &name).await?;
                (
                    tempfile::tempdir()?,
                    Some(format!("{base}/{name}")),
                    Some((server.clone(), name)),
                )
            }
            None => (tempfile::tempdir()?, None, None),
        };
        let mailer = Arc::new(LoadMailer::default());
        let mut nodes = Vec::new();
        for index in 0..opts.nodes {
            let node = start_node(
                index,
                opts,
                dir.path().to_path_buf(),
                database_url.clone(),
                mailer.clone(),
            )
            .await
            .with_context(|| format!("cannot start node {}", index + 1))?;
            nodes.push(Arc::new(node));
        }
        Ok(Self {
            nodes,
            codes: Codes::Memory(mailer),
            created_database: Mutex::new(created_database),
            _dir: Some(dir),
            store: if database_url.is_some() {
                "postgres"
            } else {
                "sqlite"
            },
        })
    }

    fn external(opts: &Options) -> Self {
        let codes = Arc::new(Mutex::new(HashMap::new()));
        if let Some(path) = &opts.mail_log {
            tail_codes(path.clone(), codes.clone());
        }
        let nodes = opts
            .url
            .iter()
            .enumerate()
            .map(|(index, url)| {
                Arc::new(Node {
                    index,
                    base: strip_scheme(url),
                    metrics: opts.metrics_url.get(index).map(|u| strip_scheme(u)),
                    alive: AtomicBool::new(true),
                    inproc: Mutex::new(None),
                })
            })
            .collect();
        Self {
            nodes,
            codes: Codes::Log(codes),
            created_database: Mutex::new(None),
            _dir: None,
            store: "external",
        }
    }

    /// Stops every in-process node and drops the database this run created.
    pub async fn shutdown(&self) {
        for node in &self.nodes {
            node.kill();
        }
        let created = self
            .created_database
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((server, name)) = created {
            // The nodes' connections close with their runtimes; FORCE ends any that linger.
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Err(error) = drop_database(&server, &name).await {
                eprintln!("could not drop the database {name}: {error}");
            }
        }
    }
}

async fn create_database(server: &str, name: &str) -> Result<()> {
    use sqlx::{Connection as _, Executor as _};
    let mut admin = sqlx::PgConnection::connect(server)
        .await
        .context("cannot reach the PostgreSQL server of --database-url")?;
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await?;
    Ok(())
}

async fn drop_database(server: &str, name: &str) -> Result<()> {
    use sqlx::{Connection as _, Executor as _};
    let mut admin = sqlx::PgConnection::connect(server).await?;
    admin
        .execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
        )))
        .await?;
    Ok(())
}

async fn start_node(
    index: usize,
    opts: &Options,
    data_dir: PathBuf,
    database_url: Option<String>,
    mailer: Arc<LoadMailer>,
) -> Result<Node> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(opts.node_threads.max(1))
        .thread_name(format!("cloud-node-{}", index + 1))
        .enable_all()
        .build()?;
    let config = Config {
        data_dir,
        database_url,
        database_connections: opts.db_connections,
        nats_url: opts.nats_url.clone().filter(|_| opts.nodes > 1),
        name: "Load test cloud".into(),
        relays: vec!["https://relay.example".into()],
        drain_seconds: opts.drain_seconds,
        // The simulated players befriend each other seconds after they sign up.
        new_account_hours: 0,
        ..Config::default()
    };
    let mut tuning = Tuning {
        // The longest the harness runs, so that no access token expires under a scenario.
        access_ttl_secs: 6 * 60 * 60,
        ..Tuning::default()
    };
    if !opts.default_limits {
        tuning.rates = Rates {
            email_per_address: 1_000,
            email_per_ip: 100_000_000,
            code_per_ip: 100_000_000,
            refresh_per_device: 100_000_000,
            account: 100_000_000,
            admin_per_ip: 100_000_000,
            ..Rates::default()
        };
    }
    let (stop, stopped) = oneshot::channel::<()>();
    let started = runtime
        .handle()
        .spawn(async move {
            let state = open(config, tuning, mailer).await?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let metrics = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("127.0.0.1:{}", listener.local_addr()?.port());
            let metrics_base = format!("127.0.0.1:{}", metrics.local_addr()?.port());
            tokio::spawn(serve_metrics(metrics, state.clone()));
            let server = tokio::spawn(serve(listener, state.clone(), async move {
                let _ = stopped.await;
            }));
            anyhow::Ok((state, base, metrics_base, server))
        })
        .await??;
    let (state, base, metrics, server) = started;
    Ok(Node {
        index,
        base,
        metrics: Some(metrics),
        alive: AtomicBool::new(true),
        inproc: Mutex::new(Some(InProc {
            runtime,
            state,
            stop: Some(stop),
            server,
        })),
    })
}

/// What `/metrics` of a node says, reduced to what the report uses.
#[derive(Clone, Debug, Default)]
pub struct MetricsSnap {
    pub store_errors: u64,
    pub connections: u64,
    pub news_sent: u64,
    pub news_received: u64,
    pub sweep_failures: u64,
    /// (route, status) to (requests, seconds spent answering).
    pub routes: BTreeMap<(String, String), (u64, f64)>,
}

pub fn parse_metrics(text: &str) -> MetricsSnap {
    let mut snap = MetricsSnap::default();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let Some((name_labels, value)) = line.rsplit_once(' ') else {
            continue;
        };
        let value: f64 = value.parse().unwrap_or(0.0);
        let (name, labels) = match name_labels.split_once('{') {
            Some((n, l)) => (n, l.trim_end_matches('}')),
            None => (name_labels, ""),
        };
        let label = |key: &str| {
            labels.split(',').find_map(|part| {
                part.strip_prefix(key)
                    .and_then(|v| v.strip_prefix("=\""))
                    .map(|v| v.trim_end_matches('"').to_owned())
            })
        };
        match name {
            "anyplay_cloud_store_errors_total" => snap.store_errors = value as u64,
            "anyplay_cloud_event_connections" => snap.connections = value as u64,
            "anyplay_cloud_news_sent_total" => snap.news_sent = value as u64,
            "anyplay_cloud_news_received_total" => snap.news_received = value as u64,
            "anyplay_cloud_sweep_failures_total" => snap.sweep_failures = value as u64,
            "anyplay_cloud_http_requests_total" | "anyplay_cloud_http_request_seconds_total" => {
                let (Some(route), Some(status)) = (label("route"), label("status")) else {
                    continue;
                };
                let entry = snap.routes.entry((route, status)).or_default();
                if name.ends_with("requests_total") {
                    entry.0 = value as u64;
                } else {
                    entry.1 = value;
                }
            }
            _ => {}
        }
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_read_from_a_log_line_with_colors() {
        let line = "\u{1b}[2m2026-10-24T10:00:00Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m anyplay_cloud::mail: sign-in code (mail = log) \u{1b}[3mto\u{1b}[0m\u{1b}[2m=\u{1b}[0mA@Example.com \u{1b}[3mcode\u{1b}[0m\u{1b}[2m=\u{1b}[0m123456";
        assert_eq!(
            parse_code_line(line),
            Some(("a@example.com".into(), "123456".into()))
        );
        let plain = "INFO anyplay_cloud::mail: sign-in code (mail = log) to=b@x.io code=000111";
        assert_eq!(
            parse_code_line(plain),
            Some(("b@x.io".into(), "000111".into()))
        );
        assert_eq!(parse_code_line("INFO something else"), None);
    }

    #[test]
    fn metrics_are_parsed_by_route() {
        let text = "# TYPE x counter\n\
            anyplay_cloud_store_errors_total 3\n\
            anyplay_cloud_event_connections 7\n\
            anyplay_cloud_http_requests_total{route=\"/v1/me\",status=\"200\"} 4\n\
            anyplay_cloud_http_request_seconds_total{route=\"/v1/me\",status=\"200\"} 0.5\n";
        let snap = parse_metrics(text);
        assert_eq!(snap.store_errors, 3);
        assert_eq!(snap.connections, 7);
        assert_eq!(snap.routes[&("/v1/me".into(), "200".into())], (4, 0.5));
    }
}
