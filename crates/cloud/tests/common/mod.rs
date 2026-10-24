//! A cloud running in this process on a random port with a temporary database and a mail sink,
//! driven through its real HTTP API and event WebSocket.
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
    _dir: tempfile::TempDir,
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

/// Timings short enough for tests and limits high enough not to get in the way.
pub fn fast_tuning() -> Tuning {
    Tuning {
        offline_grace: Duration::from_millis(300),
        session_grace: Duration::from_millis(900),
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
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            data_dir: dir.path().to_path_buf(),
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
