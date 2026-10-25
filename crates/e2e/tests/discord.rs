//! Discord's Social SDK end to end, with the fake SDK: a real cloud, a real net service driven
//! through the control protocol as a launcher would, and a fake Discord token endpoint.
//!
//! A player connects: the (fake) SDK reports a code, the net service sends it to the cloud, the
//! cloud trades it at the fake Discord with its secret and the player's PKCE verifier, and the
//! access token goes back into the SDK, which reports connected. The refresh token is kept in
//! the net service and used for the next connect, without asking the player again.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyplay_cloud::config::LinkEndpoints;
use anyplay_cloud::mail::MemoryMailer;
use anyplay_cloud::{Config, Tuning, open, serve};
use anyplay_control::{
    Answer, Cloud, Command, DiscordInfo, DiscordState, Event, Failure, Output, command_line,
    parse_output,
};
use anyplay_discord::{Backend, FakeScript};
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::sync::{mpsc, oneshot};

const CLIENT_ID: &str = "1563714917998133288";
const CLIENT_SECRET: &str = "e2e-secret";
const LIMIT: Duration = Duration::from_secs(30);

// The fake Discord

#[derive(Default)]
struct FakeDiscord {
    refresh: HashMap<String, ()>,
    requests: Vec<HashMap<String, String>>,
    counter: u32,
}

type Shared = Arc<Mutex<FakeDiscord>>;

async fn token(State(fake): State<Shared>, body: Bytes) -> axum::response::Response {
    let fields: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let mut fake = fake.lock().unwrap();
    fake.requests.push(fields.clone());
    if fields.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match fields.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {}
        Some("refresh_token")
            if fake
                .refresh
                .remove(fields.get("refresh_token").map_or("", String::as_str))
                .is_some() => {}
        _ => return (StatusCode::BAD_REQUEST, "invalid_grant").into_response(),
    }
    fake.counter += 1;
    let refresh = format!("refresh-{}", fake.counter);
    fake.refresh.insert(refresh.clone(), ());
    axum::Json(json!({
        "access_token": format!("access-{}", fake.counter),
        "token_type": "Bearer",
        "expires_in": 604_800,
        "refresh_token": refresh,
        "scope": "identify openid sdk.social_layer_presence",
    }))
    .into_response()
}

// The launcher

type Waiters = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

struct Launcher {
    write: WriteHalf<DuplexStream>,
    waiters: Waiters,
    events: mpsc::UnboundedReceiver<Event>,
    next: u64,
    script: FakeScript,
}

impl Launcher {
    async fn start(data: &Path, discord: Backend) -> Launcher {
        let (service_end, launcher_end) = tokio::io::duplex(1 << 20);
        let (service_read, service_write) = tokio::io::split(service_end);
        let script = match &discord {
            Backend::Fake(script) => script.clone(),
            _ => FakeScript::default(),
        };
        let config = anyplay_net::Config {
            data: data.to_path_buf(),
            file_credentials: true,
            relay_roots: Vec::new(),
            bind: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
            relay_only: false,
            device_name: "Test PC".into(),
            audio: anyplay_net::voice::Audio::system(),
            discord,
            workers: anyplay_net::screen::WorkerSet::none(),
        };
        tokio::spawn(anyplay_net::run(
            config,
            service_read,
            service_write,
            std::future::pending(),
        ));
        let (read, write) = tokio::io::split(launcher_end);
        let waiters: Waiters = Arc::default();
        let (events_tx, events) = mpsc::unbounded_channel();
        {
            let waiters = waiters.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    match parse_output(&line) {
                        Some(Output::Answer(answer)) => {
                            let waiter = waiters.lock().unwrap().remove(&answer.id);
                            if let Some(waiter) = waiter {
                                let _ = waiter.send(answer);
                            }
                        }
                        Some(Output::Event(event)) => {
                            let _ = events_tx.send(event);
                        }
                        None => panic!("unreadable line from the net service: {line}"),
                    }
                }
            });
        }
        Launcher {
            write,
            waiters,
            events,
            next: 1,
            script,
        }
    }

    async fn call(&mut self, command: Command) -> Result<serde_json::Value, Failure> {
        let id = self.next;
        self.next += 1;
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(id, tx);
        self.write
            .write_all(command_line(id, &command).as_bytes())
            .await
            .unwrap();
        tokio::time::timeout(LIMIT, rx)
            .await
            .expect("no answer in time")
            .expect("the net service went away")
            .into_result()
    }

    async fn ok(&mut self, command: Command) -> serde_json::Value {
        let shown = format!("{command:?}");
        self.call(command)
            .await
            .unwrap_or_else(|failure| panic!("{shown} failed: {failure:?}"))
    }

    async fn wait<T>(&mut self, what: &str, pick: impl Fn(&Event) -> Option<T>) -> T {
        tokio::time::timeout(LIMIT, async {
            loop {
                let event = self.events.recv().await.expect("events ended");
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {what} in time"))
    }

    async fn discord(&mut self, what: &str, state: DiscordState) -> DiscordInfo {
        self.wait(what, |event| match event {
            Event::Discord(info) if info.state == state => Some(info.clone()),
            _ => None,
        })
        .await
    }
}

async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen");
}

fn discord_files(data: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("discord-"))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_player_connects_discord_through_the_cloud_and_connects_again_from_the_kept_token() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Discord
    let fake: Shared = Arc::default();
    let app = Router::new()
        .route("/api/oauth2/token", post(token))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let discord_url = format!("http://{}/api", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // The cloud
    let cloud_dir = tempfile::tempdir().unwrap();
    let mail = Arc::new(MemoryMailer::default());
    let config = Config {
        data_dir: cloud_dir.path().to_path_buf(),
        discord_client_id: Some(CLIENT_ID.into()),
        discord_client_secret: Some(CLIENT_SECRET.into()),
        ..Config::default()
    };
    let tuning = Tuning {
        links: LinkEndpoints {
            discord_api: discord_url,
            ..LinkEndpoints::default()
        },
        ..Tuning::default()
    };
    let state = open(config, tuning, mail.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let cloud_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, state, std::future::pending()));

    // The player signs up.
    let data = tempfile::tempdir().unwrap();
    let mut launcher = Launcher::start(data.path(), Backend::Fake(FakeScript::default())).await;
    launcher
        .ok(Command::Hello {
            launcher: "test".into(),
            cloud: cloud_url.clone(),
        })
        .await;
    launcher
        .ok(Command::Email {
            email: "d@example.com".into(),
        })
        .await;
    let code = mail.last_code("d@example.com").unwrap();
    launcher.ok(Command::Code { code }).await;
    launcher
        .ok(Command::SignUp {
            handle: "Discorder".into(),
            age_confirmed: true,
        })
        .await;
    launcher
        .wait("sign-in", |event| match event {
            Event::State {
                cloud: Cloud::Online,
                account: Some(_),
                ..
            } => Some(()),
            _ => None,
        })
        .await;

    // Before connecting, Discord is there and signed out.
    let status = launcher.ok(Command::DiscordStatus).await;
    assert_eq!(status["state"], "signed_out");

    // Connect: the SDK authorises, the cloud trades the code, the SDK connects.
    launcher.ok(Command::DiscordConnect).await;
    let connected = launcher
        .discord("the connected state", DiscordState::Connected)
        .await;
    let user = connected.user.expect("the player");
    assert_eq!(user.name, "Test Player");
    assert_eq!(connected.friends, Some(3));
    {
        let fake = fake.lock().unwrap();
        let sent = &fake.requests[0];
        assert_eq!(sent["grant_type"], "authorization_code");
        assert_eq!(
            sent["code"], "fake-code-1",
            "the SDK's code reached Discord"
        );
        assert!(
            sent["code_verifier"].starts_with("fake-verifier-1-"),
            "the SDK's verifier reached Discord"
        );
        assert_eq!(sent["redirect_uri"], "http://127.0.0.1/callback");
        assert_eq!(
            sent["client_secret"], CLIENT_SECRET,
            "the cloud adds the secret"
        );
    }
    assert_eq!(launcher.script.seen.tokens(), ["access-1"]);
    let status = launcher.ok(Command::DiscordStatus).await;
    assert_eq!(status["state"], "connected");
    assert_eq!(status["user"]["id"], user.id);

    // The refresh token is kept, in a file of its own, for this account.
    eventually("the refresh token is kept", || {
        discord_files(data.path()).len() == 1
    })
    .await;
    let kept = std::fs::read_to_string(&discord_files(data.path())[0]).unwrap();
    assert!(kept.contains("refresh-1"), "{kept}");

    // Disconnecting ends the connection and forgets the token.
    launcher.ok(Command::DiscordDisconnect).await;
    launcher
        .discord("the signed-out state", DiscordState::SignedOut)
        .await;
    eventually("the refresh token is forgotten", || {
        discord_files(data.path()).is_empty()
    })
    .await;

    // Connect, restart the service, connect again: the kept token is used, nobody is asked.
    launcher.ok(Command::DiscordConnect).await;
    launcher
        .discord("the second connection", DiscordState::Connected)
        .await;
    eventually("the second token is kept", || {
        discord_files(data.path()).len() == 1
    })
    .await;
    drop(launcher);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut again = Launcher::start(data.path(), Backend::Fake(FakeScript::default())).await;
    again
        .ok(Command::Hello {
            launcher: "test".into(),
            cloud: cloud_url,
        })
        .await;
    again
        .wait("the resumed sign-in", |event| match event {
            Event::State {
                cloud: Cloud::Online,
                account: Some(_),
                ..
            } => Some(()),
            _ => None,
        })
        .await;
    let before = fake.lock().unwrap().requests.len();
    again.ok(Command::DiscordConnect).await;
    again
        .discord(
            "the connection from the kept token",
            DiscordState::Connected,
        )
        .await;
    assert_eq!(
        again.script.seen.authorizations(),
        0,
        "the player was not asked again"
    );
    let fake = fake.lock().unwrap();
    assert_eq!(fake.requests.len(), before + 1);
    assert_eq!(fake.requests.last().unwrap()["grant_type"], "refresh_token");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_sdk_discord_is_unavailable_and_nothing_else_changes() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let data = tempfile::tempdir().unwrap();
    let mut launcher = Launcher::start(data.path(), Backend::Off).await;
    launcher
        .ok(Command::Hello {
            launcher: "test".into(),
            cloud: "http://127.0.0.1:9".into(),
        })
        .await;
    launcher
        .discord("the unavailable state", DiscordState::Unavailable)
        .await;
    let status = launcher.ok(Command::DiscordStatus).await;
    assert_eq!(status["state"], "unavailable");
    let failure = launcher.call(Command::DiscordConnect).await.unwrap_err();
    // Not signed in is checked first; either way nothing was started.
    assert!(
        failure.code == "not_signed_in" || failure.code == "bad_state",
        "{failure:?}"
    );
    assert!(discord_files(data.path()).is_empty());
}
