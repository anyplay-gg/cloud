//! The players' side: HTTP calls that are timed and counted, sign-in, and the event connection
//! with the reconnect rules of the specification.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyplay_api::{
    CodeAnswer, CodeRequest, Device, EmailAnswer, EmailRequest, Event, Platform, SignedIn,
    SignupRequest, close,
};
use futures_util::StreamExt as _;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{oneshot, watch};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::opts::{Options, Reconnect};
use crate::stats::Recorder;
use crate::target::Target;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The value of the drop signal that ends every connection loop for good.
const STOP: u64 = u64::MAX;

/// What the harness knows about one player.
pub struct Shared {
    pub idx: usize,
    pub id: u64,
    pub token: String,
    /// The node this player's load balancer sends it to, while that node lives.
    pub home: AtomicUsize,
    /// The event connection is open.
    pub up: AtomicBool,
    pub friends: Mutex<Vec<usize>>,
}

/// Who must come back after a storm, and when each came back.
pub struct StormTrack {
    pub t0: Instant,
    pub expected: Vec<bool>,
    /// Microseconds after `t0` of the player's first welcome, or 0.
    pub done: Vec<AtomicU64>,
}

impl StormTrack {
    pub fn new(expected: Vec<bool>) -> Self {
        let done = expected.iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            t0: Instant::now(),
            expected,
            done,
        }
    }

    pub fn reconnected(&self, idx: usize) {
        if self.expected.get(idx) == Some(&true) {
            let micros = (self.t0.elapsed().as_micros() as u64).max(1);
            let _ =
                self.done[idx].compare_exchange(0, micros, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    pub fn expected_count(&self) -> usize {
        self.expected.iter().filter(|e| **e).count()
    }

    pub fn done_count(&self) -> usize {
        self.done
            .iter()
            .filter(|d| d.load(Ordering::Relaxed) != 0)
            .count()
    }
}

pub struct Ctx {
    pub opts: Options,
    pub http: reqwest::Client,
    pub target: Arc<Target>,
    pub epoch: Instant,
    /// Four hex digits that keep the addresses and handles of one run apart from another's.
    pub run: String,
    rec: RwLock<Arc<Recorder>>,
    pub drop_tx: watch::Sender<u64>,
    pub storm: RwLock<Option<Arc<StormTrack>>>,
    pub connected: AtomicUsize,
    pub players: RwLock<Vec<Arc<Shared>>>,
}

/// An answer, or status 0 when none arrived.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json<T: DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_slice(&self.body).ok()
    }
}

impl Ctx {
    pub fn new(opts: Options, target: Arc<Target>) -> Result<Arc<Self>, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(1024)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()?;
        Ok(Arc::new(Self {
            opts,
            http,
            target,
            epoch: Instant::now(),
            run: format!("{:04x}", rand::random::<u16>()),
            rec: RwLock::new(Arc::new(Recorder::new())),
            drop_tx: watch::channel(0).0,
            storm: RwLock::new(None),
            connected: AtomicUsize::new(0),
            players: RwLock::new(Vec::new()),
        }))
    }

    pub fn rec(&self) -> Arc<Recorder> {
        self.rec.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Starts a new recording for the next scenario and returns it.
    pub fn new_recorder(&self) -> Arc<Recorder> {
        let rec = Arc::new(Recorder::new());
        *self.rec.write().unwrap_or_else(|e| e.into_inner()) = rec.clone();
        rec
    }

    pub fn email(&self, idx: usize) -> String {
        format!("p{idx}-{}@load.test", self.run)
    }

    pub fn handle(&self, idx: usize) -> String {
        format!("l{}{idx:07}", self.run)
    }

    pub fn micros(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    /// The node a player with this home reaches: its home while that lives, else the next one.
    pub fn node_for(&self, home: usize) -> usize {
        let n = self.target.nodes.len();
        (0..n)
            .map(|k| (home + k) % n)
            .find(|i| self.target.nodes[*i].alive())
            .unwrap_or(home % n)
    }

    pub fn stopping(&self) -> bool {
        *self.drop_tx.borrow() == STOP
    }

    pub fn stop_all(&self) {
        let _ = self.drop_tx.send(STOP);
    }

    /// Breaks every event connection at once.
    pub fn drop_all(&self) {
        self.drop_tx.send_modify(|g| {
            if *g != STOP {
                *g += 1;
            }
        });
    }

    /// One timed request to a node.
    pub async fn call(
        &self,
        op: &'static str,
        node: usize,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let url = format!("http://{}{path}", self.target.nodes[node].base);
        let mut request = self.http.request(method, url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let started = Instant::now();
        let reply = match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                match response.bytes().await {
                    Ok(bytes) => Reply {
                        status,
                        body: bytes.to_vec(),
                    },
                    Err(error) => {
                        self.rec()
                            .count(&format!("http_error:{}", error_kind(&error)), 1);
                        Reply {
                            status: 0,
                            body: Vec::new(),
                        }
                    }
                }
            }
            Err(error) => {
                self.rec()
                    .count(&format!("http_error:{}", error_kind(&error)), 1);
                Reply {
                    status: 0,
                    body: Vec::new(),
                }
            }
        };
        self.rec().op(op, reply.status, started.elapsed());
        reply
    }

    /// A call by a player, to the node it is sent to.
    pub async fn call_as(
        &self,
        me: &Shared,
        op: &'static str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let node = self.node_for(me.home.load(Ordering::Relaxed));
        self.call(op, node, method, path, Some(&me.token), body)
            .await
    }
}

fn device() -> Device {
    Device {
        name: "load".into(),
        platform: Platform::Linux,
        // A load player has no device key: it is a legacy device, which a cloud accepts.
        key: None,
    }
}

/// Signs player `idx` in by e-mail code, creating the account when it does not exist yet.
/// Returns the account ID and the access token.
pub async fn login(ctx: &Ctx, idx: usize) -> Option<(u64, String)> {
    let node = ctx.node_for(idx % ctx.target.nodes.len());
    let email = ctx.email(idx);
    let reply = ctx
        .call(
            "auth_email",
            node,
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!(EmailRequest {
                email: email.clone()
            })),
        )
        .await;
    let challenge: EmailAnswer = reply.ok().then(|| reply.json()).flatten()?;
    let Some(code) = ctx.target.codes.take(&email).await else {
        ctx.rec().count("no_code_received", 1);
        return None;
    };
    let reply = ctx
        .call(
            "auth_code",
            node,
            Method::POST,
            "/v1/auth/code",
            None,
            Some(json!(CodeRequest {
                challenge: challenge.challenge,
                code,
                device: device(),
            })),
        )
        .await;
    let answer: CodeAnswer = reply.ok().then(|| reply.json()).flatten()?;
    let signed_in: SignedIn = match answer {
        CodeAnswer::SignedIn { tokens, me } => SignedIn { tokens, me },
        CodeAnswer::NewAccount { signup, .. } => {
            let reply = ctx
                .call(
                    "auth_signup",
                    node,
                    Method::POST,
                    "/v1/auth/signup",
                    None,
                    Some(json!(SignupRequest {
                        signup,
                        handle: ctx.handle(idx),
                        age_confirmed: true,
                        device: device(),
                    })),
                )
                .await;
            reply.ok().then(|| reply.json()).flatten()?
        }
    };
    Some((signed_in.me.id.0, signed_in.tokens.access))
}

/// Waits before a reconnect as the specification tells clients: half the pause plus a random part
/// of up to the whole pause.
fn spec_delay(pause: f64) -> Duration {
    Duration::from_secs_f64(pause / 2.0 + rand::random::<f64>() * pause)
}

enum End {
    Stop,
    /// The harness broke the connection.
    Dropped,
    Closed(Option<u16>),
    Broken,
}

/// Opens the event connection and reads up to the welcome. Errs with the HTTP status of a refused
/// upgrade, or 0.
async fn connect_ws(ctx: &Ctx, me: &Shared) -> Result<Ws, u16> {
    let node = ctx.node_for(me.home.load(Ordering::Relaxed));
    let mut request = format!("ws://{}/v1/events", ctx.target.nodes[node].base)
        .into_client_request()
        .map_err(|_| 0u16)?;
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", me.token).parse().map_err(|_| 0u16)?,
    );
    let attempt = async {
        let (mut ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| match e {
                WsError::Http(response) => response.status().as_u16(),
                _ => 0,
            })?;
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    if matches!(
                        serde_json::from_str(text.as_str()),
                        Ok(Event::Welcome { .. })
                    ) {
                        return Ok(ws);
                    }
                }
                Some(Ok(_)) => {}
                _ => return Err(0),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(15), attempt)
        .await
        .unwrap_or(Err(0))
}

fn on_text(ctx: &Ctx, text: &str) {
    let rec = ctx.rec();
    match serde_json::from_str::<Event>(text) {
        Ok(Event::Presence { presence, .. }) => {
            rec.event("presence");
            // The sender stamped its change into the version: `t<microseconds>`.
            let stamp = presence
                .activity
                .as_ref()
                .and_then(|a| a.version.strip_prefix('t'))
                .and_then(|n| n.parse::<u64>().ok());
            if let Some(sent) = stamp {
                let took = ctx.micros().saturating_sub(sent);
                // An older stamp comes back when the cloud repeats a presence for another reason.
                if took < 60_000_000 {
                    rec.delivered(took);
                }
            }
        }
        Ok(Event::Welcome { .. }) => rec.event("welcome"),
        Ok(Event::Friends) => rec.event("friends"),
        Ok(Event::Invitation { .. }) => rec.event("invitation"),
        Ok(Event::InvitationGone { .. }) => rec.event("invitation_gone"),
        Ok(Event::Session { .. }) => rec.event("session"),
        Ok(Event::SessionGone { .. }) => rec.event("session_gone"),
        Ok(Event::SignedOut) => rec.event("signed_out"),
        Ok(Event::Banned { .. }) => rec.event("banned"),
        Ok(Event::Party { .. }) => rec.event("party"),
        Ok(Event::PartyGone { .. }) => rec.event("party_gone"),
        Ok(Event::PartyInvitation { .. }) => rec.event("party_invitation"),
        Ok(Event::PartyInvitationGone { .. }) => rec.event("party_invitation_gone"),
        Ok(Event::Message { .. }) => rec.event("message"),
        Ok(Event::MessageRequest { .. }) => rec.event("message_request"),
        Ok(Event::Read { .. }) => rec.event("read"),
        Ok(Event::Delivered { .. }) => rec.event("delivered"),
        Ok(Event::Typing { .. }) => rec.event("typing"),
        Ok(Event::MessageUpdate { .. }) => rec.event("message_update"),
        Ok(Event::Conversation { .. }) => rec.event("conversation"),
        Ok(Event::ConversationGone { .. }) => rec.event("conversation_gone"),
        Ok(Event::Feed) => rec.event("feed"),
        Ok(Event::AppearOffline { .. }) => rec.event("appear_offline"),
        Ok(Event::Action { .. }) => rec.event("action"),
        Ok(Event::MissedCall { .. }) => rec.event("missed_call"),
        Ok(Event::MissedCallGone { .. }) => rec.event("missed_call_gone"),
        Ok(Event::CallEnded { .. }) => rec.event("call_ended"),
        Ok(Event::Unknown) | Err(_) => rec.event("unknown"),
    }
}

async fn read_loop(ctx: &Ctx, mut ws: Ws, gen_rx: &mut watch::Receiver<u64>) -> End {
    loop {
        tokio::select! {
            changed = gen_rx.changed() => {
                // Dropping the stream closes the socket with no close frame.
                return if changed.is_err() || *gen_rx.borrow() == STOP { End::Stop } else { End::Dropped };
            }
            frame = ws.next() => match frame {
                Some(Ok(Message::Text(text))) => on_text(ctx, text.as_str()),
                Some(Ok(Message::Close(frame))) => {
                    return End::Closed(frame.map(|f| u16::from(f.code)));
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return End::Broken,
            }
        }
    }
}

/// One player's event connection for as long as the run lasts: connect, read, and reconnect after
/// a break the way the specification tells clients to. `first` is told at the first welcome.
pub async fn conn_loop(ctx: Arc<Ctx>, me: Arc<Shared>, mut first: Option<oneshot::Sender<()>>) {
    let immediate = ctx.opts.reconnect == Reconnect::Immediate;
    let mut gen_rx = ctx.drop_tx.subscribe();
    let mut pause = 1.0f64;
    let mut delay: Option<Duration> = None;
    loop {
        if ctx.stopping() {
            return;
        }
        if let Some(wait) = delay.take().filter(|d| !d.is_zero()) {
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = gen_rx.changed() => {}
            }
            if ctx.stopping() {
                return;
            }
        }
        gen_rx.borrow_and_update();
        let started = Instant::now();
        match connect_ws(&ctx, &me).await {
            Err(status) => {
                ctx.rec().op("ws_connect", status, started.elapsed());
                delay = Some(if immediate {
                    Duration::from_millis(200)
                } else {
                    spec_delay(pause)
                });
                pause = (pause * 2.0).min(30.0);
            }
            Ok(ws) => {
                ctx.rec().op("ws_connect", 101, started.elapsed());
                ctx.rec().event("welcome");
                pause = 1.0;
                me.up.store(true, Ordering::Relaxed);
                ctx.connected.fetch_add(1, Ordering::Relaxed);
                let track = ctx.storm.read().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(track) = track {
                    track.reconnected(me.idx);
                }
                if let Some(first) = first.take() {
                    let _ = first.send(());
                }
                // A client fetches its lists after the welcome.
                {
                    let (ctx, me) = (ctx.clone(), me.clone());
                    tokio::spawn(async move {
                        ctx.call_as(&me, "friends_list", Method::GET, "/v1/friends", None)
                            .await;
                    });
                }
                let end = read_loop(&ctx, ws, &mut gen_rx).await;
                me.up.store(false, Ordering::Relaxed);
                ctx.connected.fetch_sub(1, Ordering::Relaxed);
                delay = match end {
                    End::Stop => return,
                    _ if immediate => None,
                    End::Closed(Some(close::GOING_AWAY | close::RESTARTING)) => {
                        Some(Duration::from_secs_f64(
                            rand::random::<f64>() * close::RECONNECT_SPREAD_SECS as f64,
                        ))
                    }
                    End::Dropped | End::Closed(_) | End::Broken => Some(spec_delay(1.0)),
                };
            }
        }
    }
}

/// A short name for why a request got no answer.
fn error_kind(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "timeout".into();
    }
    let mut text = String::new();
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(e) = source {
        text = e.to_string();
        source = e.source();
    }
    let kind = if error.is_connect() {
        "connect"
    } else {
        "other"
    };
    format!("{kind} ({})", text.chars().take(60).collect::<String>())
}
