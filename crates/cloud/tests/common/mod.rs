//! A cloud running in this process on a random port with a temporary database and a mail sink,
//! driven through its real HTTP API and event WebSocket.
//!
//! The database is a SQLite file, or, when `ANYPLAY_TEST_DATABASE_URL` names a PostgreSQL
//! database such as `postgres://postgres:secret@127.0.0.1:5432/postgres`, a new database on that
//! server for each cloud. CI runs every test both ways. With `ANYPLAY_TEST_NATS_URL` as well,
//! [`TestCloud::cluster`] starts several nodes of one cloud.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use anyplay_api::{
    ApiError, CodeAnswer, CodeRequest, Device, EmailAnswer, EmailRequest, Event, Platform,
    SignedIn, SignupRequest, Tokens,
};
use anyplay_cloud::limits::Rates;
use anyplay_cloud::mail::MemoryMailer;
use anyplay_cloud::{AppState, Config, Tuning, open, serve};
use futures_util::StreamExt as _;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub struct TestCloud {
    pub base: String,
    pub state: AppState,
    pub mail: Arc<MemoryMailer>,
    pub http: reqwest::Client,
    _dir: Arc<tempfile::TempDir>,
    _server: JoinHandle<std::io::Result<()>>,
}

/// An answer of the cloud.
pub struct Reply {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}

impl Reply {
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_value(self.body.clone()).unwrap_or_else(|e| {
            panic!(
                "unexpected body {} (status {}): {e}",
                self.body, self.status
            )
        })
    }

    pub fn error(&self) -> ApiError {
        self.json()
    }

    /// Asserts the status and returns the reply.
    pub fn ok(self, status: u16) -> Self {
        assert_eq!(self.status, status, "body: {}", self.body);
        self
    }

    /// Asserts an error answer with this code and status.
    pub fn is_error(&self, code: &str, status: u16) {
        assert_eq!(self.status, status, "body: {}", self.body);
        assert_eq!(self.body["error"], code, "body: {}", self.body);
    }
}

/// An HTTP/1.1 answer with a `content-length` body, once all of it has arrived.
fn read_answer(bytes: &[u8]) -> Option<Reply> {
    let end = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = std::str::from_utf8(&bytes[..end]).expect("the head is text");
    let mut lines = head.lines();
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let mut headers = reqwest::header::HeaderMap::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':')?;
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.trim().as_bytes()).ok()?,
            value.trim().parse().ok()?,
        );
    }
    let len: usize = headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let body = bytes.get(end..end + len)?;
    Some(Reply {
        status,
        headers,
        body: serde_json::from_slice(body).unwrap_or(Value::Null),
    })
}

/// A new, empty database on the server of `ANYPLAY_TEST_DATABASE_URL`, or `None` without it.
pub async fn postgres_database() -> Option<String> {
    use sqlx::{Connection as _, Executor as _};
    let server = std::env::var("ANYPLAY_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())?;
    let (base, _) = server
        .rsplit_once('/')
        .expect("ANYPLAY_TEST_DATABASE_URL ends in a database name");
    let name = format!("anyplay_test_{:016x}", rand::random::<u64>());
    let mut admin = sqlx::PgConnection::connect(&server)
        .await
        .expect("the test database server answers");
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .unwrap();
    Some(format!("{base}/{name}"))
}

/// Timings short enough for tests and limits high enough not to get in the way.
pub fn fast_tuning() -> Tuning {
    Tuning {
        offline_grace: Duration::from_millis(300),
        // Long, so that a test whose players hold no event connection keeps its sessions however
        // slowly it runs; the tests of the grace itself shorten it.
        session_grace: Duration::from_secs(30),
        lease_ttl: Duration::from_secs(30),
        sweep_interval: Duration::from_millis(100),
        rates: Rates {
            email_per_address: 100,
            email_per_ip: 10_000,
            code_per_ip: 10_000,
            refresh_per_device: 10_000,
            account: 1_000_000,
            ..Rates::default()
        },
        ..Tuning::default()
    }
}

impl TestCloud {
    pub async fn start() -> Self {
        Self::with(|_| {}, |_| {}).await
    }

    pub async fn with(config: impl FnOnce(&mut Config), tuning: impl FnOnce(&mut Tuning)) -> Self {
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let database_url = postgres_database().await;
        Self::node(dir, database_url, None, config, tuning).await
    }

    /// `n` nodes of one cloud: one database, one NATS server and one signing key, each node on
    /// its own port. `None` unless both `ANYPLAY_TEST_DATABASE_URL` and `ANYPLAY_TEST_NATS_URL`
    /// are set.
    pub async fn cluster(n: usize, config: impl Fn(&mut Config)) -> Option<Vec<Self>> {
        let nats = std::env::var("ANYPLAY_TEST_NATS_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())?;
        let database_url = postgres_database().await?;
        // The nodes share the data folder, so the first creates the signing key and the others
        // read it.
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let mut nodes = Vec::new();
        for _ in 0..n {
            nodes.push(
                Self::node(
                    dir.clone(),
                    Some(database_url.clone()),
                    Some(nats.clone()),
                    &config,
                    |_| {},
                )
                .await,
            );
        }
        Some(nodes)
    }

    async fn node(
        dir: Arc<tempfile::TempDir>,
        database_url: Option<String>,
        nats_url: Option<String>,
        config: impl FnOnce(&mut Config),
        tuning: impl FnOnce(&mut Tuning),
    ) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut cfg = Config {
            data_dir: dir.path().to_path_buf(),
            database_url,
            // Many test clouds share one database server.
            database_connections: 4,
            nats_url,
            name: "Test cloud".into(),
            relays: vec![
                "https://relay-a.example".into(),
                "https://relay-b.example".into(),
            ],
            ..Config::default()
        };
        config(&mut cfg);
        let mut tun = fast_tuning();
        tuning(&mut tun);
        let mail = Arc::new(MemoryMailer::default());
        let state = open(cfg, tun, mail.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(serve(listener, state.clone(), std::future::pending()));
        Self {
            base,
            state,
            mail,
            http: reqwest::Client::new(),
            _dir: dir,
            _server: server,
        }
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut request = self
            .http
            .request(method, format!("http://{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap();
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    /// Writes `request` as it is on a new connection and reads the answer: for requests that a
    /// client library would not send the same way.
    pub async fn raw(&self, request: &[u8]) -> Reply {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut stream = TcpStream::connect(&self.base).await.unwrap();
        stream.write_all(request).await.unwrap();
        let mut answer = Vec::new();
        let mut buf = [0; 4096];
        loop {
            if let Some(reply) = read_answer(&answer) {
                return reply;
            }
            let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
                .await
                .expect("no answer in time")
                .unwrap();
            assert!(
                read > 0,
                "the connection closed before the answer: {:?}",
                String::from_utf8_lossy(&answer)
            );
            answer.extend_from_slice(&buf[..read]);
        }
    }

    pub async fn get(&self, path: &str, token: &str) -> Reply {
        self.request(Method::GET, path, Some(token), None).await
    }

    pub async fn post(&self, path: &str, token: &str, body: Value) -> Reply {
        self.request(Method::POST, path, Some(token), Some(body))
            .await
    }

    pub async fn post_empty(&self, path: &str, token: &str) -> Reply {
        self.request(Method::POST, path, Some(token), None).await
    }

    pub async fn put(&self, path: &str, token: &str, body: Option<Value>) -> Reply {
        self.request(Method::PUT, path, Some(token), body).await
    }

    pub async fn patch(&self, path: &str, token: &str, body: Value) -> Reply {
        self.request(Method::PATCH, path, Some(token), Some(body))
            .await
    }

    pub async fn delete(&self, path: &str, token: &str) -> Reply {
        self.request(Method::DELETE, path, Some(token), None).await
    }

    /// Asks for a sign-in code and returns the challenge and the code from the mail sink.
    pub async fn ask_for_code(&self, email: &str) -> (EmailAnswer, String) {
        let reply = self
            .request(
                Method::POST,
                "/v1/auth/email",
                None,
                Some(json!(EmailRequest {
                    email: email.into()
                })),
            )
            .await
            .ok(200);
        let answer: EmailAnswer = reply.json();
        let normalized = email.trim().to_lowercase();
        let code = self.mail.last_code(&normalized).expect("a mail was sent");
        (answer, code)
    }

    pub fn device(name: &str) -> Device {
        Device {
            name: name.into(),
            platform: Platform::Linux,
        }
    }

    /// Enters a code. Returns the raw reply.
    pub async fn enter_code(&self, challenge: &str, code: &str, device: &str) -> Reply {
        self.request(
            Method::POST,
            "/v1/auth/code",
            None,
            Some(json!(CodeRequest {
                challenge: challenge.into(),
                code: code.into(),
                device: Self::device(device),
            })),
        )
        .await
    }

    pub async fn try_sign_up(&self, signup: &str, handle: &str, age_confirmed: bool) -> Reply {
        self.request(
            Method::POST,
            "/v1/auth/signup",
            None,
            Some(json!(SignupRequest {
                signup: signup.into(),
                handle: handle.into(),
                age_confirmed,
                device: Self::device("test pc"),
            })),
        )
        .await
    }

    /// Signs a new account up by e-mail code.
    pub async fn sign_up(&self, handle: &str) -> User {
        let email = format!("{}@example.com", handle.to_lowercase());
        let (challenge, code) = self.ask_for_code(&email).await;
        let answer: CodeAnswer = self
            .enter_code(&challenge.challenge, &code, "first pc")
            .await
            .ok(200)
            .json();
        let CodeAnswer::NewAccount { signup, .. } = answer else {
            panic!("expected a new account for {email}");
        };
        let signed_in: SignedIn = self.try_sign_up(&signup, handle, true).await.ok(200).json();
        User {
            email,
            id: signed_in.me.id.0,
            handle: signed_in.me.handle,
            tokens: signed_in.tokens,
        }
    }

    /// Signs an existing account in on another device.
    pub async fn sign_in(&self, email: &str, device: &str) -> SignedIn {
        let (challenge, code) = self.ask_for_code(email).await;
        let answer: CodeAnswer = self
            .enter_code(&challenge.challenge, &code, device)
            .await
            .ok(200)
            .json();
        match answer {
            CodeAnswer::SignedIn { tokens, me } => SignedIn { tokens, me },
            other => panic!("expected a sign-in, got {other:?}"),
        }
    }

    pub async fn events(&self, token: &str) -> Events {
        Events::connect(&self.base, token).await.expect("upgrade")
    }

    /// Two friends, both with an open event connection (welcome already read).
    pub async fn friends(&self, a: &str, b: &str) -> (User, User) {
        let a = self.sign_up(a).await;
        let b = self.sign_up(b).await;
        self.post(
            "/v1/friends/requests",
            a.token(),
            json!({"handle": b.handle}),
        )
        .await
        .ok(200);
        self.post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
            .await
            .ok(204);
        (a, b)
    }
}

pub struct User {
    pub email: String,
    pub id: u64,
    pub handle: String,
    pub tokens: Tokens,
}

impl User {
    pub fn token(&self) -> &str {
        &self.tokens.access
    }
}

pub struct Events {
    /// The virtual address from the welcome event.
    pub address: String,
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

const WAIT: Duration = Duration::from_secs(5);

impl Events {
    /// Connects and reads the welcome event. Errs with the HTTP status when the upgrade is refused.
    pub async fn connect(base: &str, token: &str) -> Result<Events, u16> {
        let mut events = Self::connect_raw(base, token).await?;
        match events.next().await {
            Event::Welcome { address, .. } => {
                events.address = address;
                Ok(events)
            }
            other => panic!("expected welcome first, got {other:?}"),
        }
    }

    /// Connects without reading anything.
    pub async fn connect_raw(base: &str, token: &str) -> Result<Events, u16> {
        let mut request = format!("ws://{base}/v1/events")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        match tokio_tungstenite::connect_async(request).await {
            Ok((ws, _)) => Ok(Events {
                address: String::new(),
                ws,
            }),
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                Err(response.status().as_u16())
            }
            Err(other) => panic!("cannot connect: {other}"),
        }
    }

    /// The next event; `None` when nothing came within `wait` or the connection ended.
    pub async fn next_within(&mut self, wait: Duration) -> Option<Event> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = tokio::time::timeout(left, next_frame(&mut self.ws))
                .await
                .ok()??;
            match frame {
                Message::Text(text) => {
                    return Some(serde_json::from_str(text.as_str()).expect("an event"));
                }
                Message::Close(_) => return None,
                _ => {}
            }
        }
    }

    pub async fn next(&mut self) -> Event {
        self.next_within(WAIT)
            .await
            .expect("an event within 5 seconds")
    }

    /// Reads events until one matches, skipping the others.
    pub async fn until(&mut self, mut wanted: impl FnMut(&Event) -> bool) -> Event {
        loop {
            let event = self.next().await;
            if wanted(&event) {
                return event;
            }
        }
    }

    /// Asserts that no event comes within `wait`.
    pub async fn quiet(&mut self, wait: Duration) {
        if let Some(event) = self.next_within(wait).await {
            panic!("unexpected event {event:?}");
        }
    }

    /// Reads until the connection closes and returns the close code, skipping events.
    pub async fn close_code(&mut self) -> Option<u16> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, next_frame(&mut self.ws))
                .await
                .expect("the connection closes within 5 seconds")
            {
                Some(Message::Close(frame)) => return frame.map(|f| u16::from(f.code)),
                None => return None,
                Some(_) => {}
            }
        }
    }

    /// True when the connection ends within `wait`, however it ends.
    pub async fn ends_within(&mut self, wait: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, next_frame(&mut self.ws)).await {
                Err(_) => return false,
                Ok(None) | Ok(Some(Message::Close(_))) => return true,
                Ok(Some(_)) => {}
            }
        }
    }
}

async fn next_frame(ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Option<Message> {
    match ws.next().await {
        Some(Ok(message)) => Some(message),
        _ => None,
    }
}
