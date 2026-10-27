//! Two players on one machine: a cloud, a relay that checks tickets, two net services driven
//! through the control protocol as a launcher would, and two fake games on the bridge.
//!
//! They sign up, become friends, see each other play, and one joins the other's session from
//! the launcher. The games then exchange datagrams through the tunnel. Two more runs put one
//! player on relay only, from the start and while playing, and check that the tunnel to them goes
//! through the relay.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyplay_bridge_protocol::{ActivateResult, Message, PeerState};
use anyplay_cloud::mail::MemoryMailer;
use anyplay_cloud::{Config, Tuning, open, serve};
use anyplay_control::anyplay_api::{AccountId, FriendLists};
use anyplay_control::{
    Answer, Cloud, Command, Event, Failure, GameStarted, Link, Output, command_line, parse_output,
};
use anyplay_fake_game::{Bridge, session_id};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::sync::{mpsc, oneshot};

const TITLE: &str = "TEST00001";
const LIMIT: Duration = Duration::from_secs(30);

type Waiters = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

/// Plays the launcher's side of the control protocol for one net service.
struct Launcher {
    write: WriteHalf<DuplexStream>,
    waiters: Waiters,
    events: mpsc::UnboundedReceiver<Event>,
    next: u64,
    _data: tempfile::TempDir,
    /// Whose launcher this is, and where every event it shows is noted.
    name: &'static str,
    journal: Journal,
}

impl Launcher {
    async fn start(name: &'static str, relay_only: bool, journal: Journal) -> Launcher {
        let data = tempfile::tempdir().unwrap();
        let (service_end, launcher_end) = tokio::io::duplex(1 << 20);
        let (service_read, service_write) = tokio::io::split(service_end);
        let config = anyplay_net::Config {
            data: data.path().to_path_buf(),
            file_credentials: true,
            relay_roots: Vec::new(),
            bind: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
            relay_only,
            device_name: "Test PC".into(),
            audio: anyplay_net::voice::Audio::system(),
            workers: anyplay_net::screen::WorkerSet::none(),
            discord: anyplay_discord::Backend::Off,
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
            _data: data,
            name,
            journal,
        }
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &mut self,
        command: Command,
    ) -> Result<T, Failure> {
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

    /// Waits for an event that `pick` accepts, skipping the others.
    async fn wait<T>(&mut self, what: &str, pick: impl Fn(&Event) -> Option<T>) -> T {
        tokio::time::timeout(LIMIT, async {
            loop {
                let event = self.events.recv().await.expect("events ended");
                self.note(&event);
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {what} in time:\n{}", self.journal.lines()))
    }

    /// Notes an event in the journal, shortened: a friend list or session can be long.
    fn note(&self, event: &Event) {
        let mut shown = format!("{event:?}");
        if shown.len() > 600 {
            let mut end = 600;
            while !shown.is_char_boundary(end) {
                end -= 1;
            }
            shown.truncate(end);
            shown.push_str(" ...");
        }
        self.journal
            .note(format!("{}'s launcher: {shown}", self.name));
    }

    async fn friends(&mut self, what: &str, test: impl Fn(&FriendLists) -> bool) -> FriendLists {
        self.wait(what, |event| match event {
            Event::Friends(lists) if test(lists) => Some(lists.clone()),
            _ => None,
        })
        .await
    }

    /// Signs up a new account with the code the cloud mailed.
    async fn sign_up(
        &mut self,
        cloud: &str,
        mail: &MemoryMailer,
        email: &str,
        handle: &str,
    ) -> AccountId {
        self.ok(Command::Hello {
            launcher: "test".into(),
            cloud: cloud.into(),
        })
        .await;
        self.ok(Command::Email {
            email: email.into(),
        })
        .await;
        let code = mail.last_code(email).expect("no code mailed");
        let answer = self.ok(Command::Code { code }).await;
        assert_eq!(answer["result"], "new_account");
        self.ok(Command::SignUp {
            handle: handle.into(),
            age_confirmed: true,
        })
        .await;
        self.wait("sign-in", |event| match event {
            Event::State {
                cloud: Cloud::Online,
                account: Some(account),
                ..
            } => Some(account.id),
            _ => None,
        })
        .await
    }

    /// Play Together: starts the game with a `lan` session of its own. Returns the game and the
    /// session the net service opened for it.
    async fn start_hosted_game(&mut self, host: anyplay_control::Host) -> (Bridge, String) {
        let started: GameStarted = self
            .call(Command::GameStart {
                title_id: TITLE.into(),
                title: "Test Game".into(),
                version: "01.000".into(),
                build: "0.1.3".into(),
                join: None,
                host: Some(host),
                version_rule: Default::default(),
                override_version: false,
            })
            .await
            .expect("hosted game start");
        let session = started.session.clone().expect("the start opened a session");
        let bridge = Bridge::connect(
            &started.env["ANYPS5_NET_ENDPOINT"],
            &started.env["ANYPS5_NET_TOKEN"],
        )
        .await
        .expect("bridge connects");
        bridge
            .wait_state(LIMIT, |state| state.signed_in)
            .await
            .expect("the game sees the player signed in");
        (bridge, session)
    }

    async fn start_game(&mut self, join: Option<String>) -> Bridge {
        let started: GameStarted = self
            .call(Command::GameStart {
                title_id: TITLE.into(),
                title: "Test Game".into(),
                version: "01.000".into(),
                build: "0.1.3".into(),
                join,
                host: None,
                version_rule: Default::default(),
                override_version: false,
            })
            .await
            .expect("game start");
        let bridge = Bridge::connect(
            &started.env["ANYPS5_NET_ENDPOINT"],
            &started.env["ANYPS5_NET_TOKEN"],
        )
        .await
        .expect("bridge connects");
        bridge
            .wait_state(LIMIT, |state| state.signed_in)
            .await
            .expect("the game sees the player signed in");
        bridge
    }
}

async fn relay(signer: &anyplay_ticket::Signer) -> (iroh_relay::server::Server, String, Journal) {
    let journal = Journal::default();
    let mut relay = iroh_relay::server::RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.access = Arc::new(Recorded {
        access: anyplay_relay::TicketAccess::new(signer.verifier()),
        journal: journal.clone(),
    });
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    let server = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap());
    (server, url, journal)
}

/// What happened, with the time since the test began, for a failure message.
#[derive(Clone, Debug)]
struct Journal(Arc<Mutex<Vec<String>>>, std::time::Instant);

impl Default for Journal {
    fn default() -> Self {
        Journal(Arc::default(), std::time::Instant::now())
    }
}

impl Journal {
    fn note(&self, what: String) {
        let at = self.1.elapsed().as_millis();
        self.0.lock().unwrap().push(format!("{at:>6} ms  {what}"));
    }

    fn lines(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}

/// The relay's ticket check, noting every connection it admits or refuses and every one that ends.
#[derive(Debug)]
struct Recorded {
    access: anyplay_relay::TicketAccess,
    journal: Journal,
}

impl iroh_relay::server::AccessControl for Recorded {
    async fn on_connect(
        &self,
        request: &iroh_relay::server::ClientRequest,
    ) -> iroh_relay::server::Access {
        let access = self.access.on_connect(request).await;
        self.journal.note(format!(
            "relay: {} connects: {access:?}",
            request.endpoint_id().fmt_short()
        ));
        access
    }

    fn on_disconnect(
        &self,
        endpoint_id: iroh_base::EndpointId,
        connection_id: iroh_relay::server::ConnectionId,
    ) {
        self.journal
            .note(format!("relay: {} gone", endpoint_id.fmt_short()));
        iroh_relay::server::AccessControl::on_disconnect(&self.access, endpoint_id, connection_id);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_players_meet_and_play_through_the_tunnel() {
    assert_eq!(meet_and_play(AliceRelay::Never).await, Link::Direct);
}

/// A player on relay only never hands the other an address, so the tunnel to them stays on the
/// relay where it would otherwise go direct.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_player_on_relay_only_is_reached_through_the_relay() {
    assert_eq!(meet_and_play(AliceRelay::FromStart).await, Link::Relay);
}

/// Turned on while playing, relay only moves a direct tunnel to the relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_only_turned_on_moves_a_direct_tunnel_to_the_relay() {
    assert_eq!(meet_and_play(AliceRelay::TurnedOn).await, Link::Relay);
}

/// When Alice's net service keeps to the relay.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AliceRelay {
    Never,
    FromStart,
    TurnedOn,
}

/// Alice and Bobby meet and play. Returns the tunnel to Alice as Bobby's launcher shows it, once
/// it has had a few seconds to go direct; with [`AliceRelay::TurnedOn`], once Alice turned relay
/// only on after that.
async fn meet_and_play(alice_relay: AliceRelay) -> Link {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cloud_dir = tempfile::tempdir().unwrap();
    let (signer, _) = anyplay_cloud::load_signing_key(cloud_dir.path()).unwrap();
    let (_relay, relay_url, journal) = relay(&signer).await;
    let mail = Arc::new(MemoryMailer::default());
    let config = Config {
        data_dir: cloud_dir.path().to_path_buf(),
        name: "End-to-end cloud".into(),
        relays: vec![relay_url],
        // The players sign up a moment before they befriend each other.
        new_account_hours: 0,
        ..Config::default()
    };
    let state = open(config, Tuning::default(), mail.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let cloud_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, state, std::future::pending()));

    let mut alice = Launcher::start(
        "Alice",
        alice_relay == AliceRelay::FromStart,
        journal.clone(),
    )
    .await;
    let mut bobby = Launcher::start("Bobby", false, journal.clone()).await;
    let alice_id = alice
        .sign_up(&cloud_url, &mail, "alice@example.org", "Alice")
        .await;
    let bobby_id = bobby
        .sign_up(&cloud_url, &mail, "bobby@example.org", "Bobby")
        .await;

    let answer = alice
        .ok(Command::AddFriend {
            handle: "bobby".into(),
        })
        .await;
    assert_eq!(answer["result"], "requested");
    bobby
        .friends("the request", |l| {
            l.incoming.iter().any(|r| r.account.id == alice_id)
        })
        .await;
    bobby.ok(Command::Accept { account: alice_id }).await;
    alice
        .friends("the new friend", |l| {
            l.friends.iter().any(|f| f.account.id == bobby_id)
        })
        .await;

    // Alice plays and opens a session the way a game does.
    let game_a = alice.start_game(None).await;
    let reply = game_a
        .web(
            "anyplay",
            "POST",
            "/v1/sessions",
            br#"{"max_members":4,"visibility":"friends"}"#,
        )
        .await
        .unwrap();
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let session = session_id(&reply).expect("session id");
    assert_eq!(
        game_a.activate(&session).await.unwrap(),
        ActivateResult::Done
    );

    // A request in a format the network does not know is refused, never answered with a guess.
    let unknown = game_a
        .web("someGroup", "GET", "/v1/things", b"")
        .await
        .unwrap();
    assert_eq!(unknown.status, 0);

    // Bobby sees Alice playing in a session he may join, and joins it from his launcher.
    let lists = bobby
        .friends("Alice's session", |l| {
            l.friends.iter().any(|f| {
                f.account.id == alice_id
                    && f.presence
                        .activity
                        .as_ref()
                        .and_then(|a| a.session.as_ref())
                        .is_some_and(|s| s.joinable)
            })
        })
        .await;
    let seen = lists
        .friends
        .iter()
        .find(|f| f.account.id == alice_id)
        .unwrap();
    let activity = seen.presence.activity.clone().unwrap();
    assert_eq!(activity.title_id, TITLE);
    assert_eq!(activity.session.unwrap().id, session);

    let mismatch = bobby
        .call::<serde_json::Value>(Command::CheckJoin {
            session: session.clone(),
            title_id: TITLE.into(),
            version: "01.050".into(),
            build: "0.1.3".into(),
            version_rule: Default::default(),
            override_version: false,
        })
        .await
        .unwrap_err();
    assert_eq!(mismatch.code, "version_mismatch");
    assert_eq!(mismatch.expected.unwrap().version, "01.000");
    // The launcher's table can let a patch through, and the player can try anyway; the build still counts.
    for (rule, over) in [
        (anyplay_control::anyplay_api::VersionRule::Major, false),
        (anyplay_control::anyplay_api::VersionRule::Exact, true),
    ] {
        bobby
            .ok(Command::CheckJoin {
                session: session.clone(),
                title_id: TITLE.into(),
                version: "01.050".into(),
                build: "0.1.3".into(),
                version_rule: rule,
                override_version: over,
            })
            .await;
    }
    let wrong_build = bobby
        .call::<serde_json::Value>(Command::CheckJoin {
            session: session.clone(),
            title_id: TITLE.into(),
            version: "01.050".into(),
            build: "9.9.9".into(),
            version_rule: anyplay_control::anyplay_api::VersionRule::Any,
            override_version: true,
        })
        .await
        .unwrap_err();
    assert_eq!(wrong_build.code, "version_mismatch");
    bobby
        .ok(Command::CheckJoin {
            session: session.clone(),
            title_id: TITLE.into(),
            version: "01.000".into(),
            build: "0.1.3".into(),
            version_rule: Default::default(),
            override_version: false,
        })
        .await;

    let game_b = bobby.start_game(Some(session.clone())).await;
    let intent = game_b
        .wait_event(LIMIT, |m| match m {
            Message::Intent(intent) => Some(intent),
            _ => None,
        })
        .await
        .expect("join intent");
    assert_eq!(intent.intent_type, "joinSession");
    assert_eq!(
        intent.properties,
        vec![("sessionId".to_owned(), session.clone())]
    );
    assert_eq!(
        game_b.activate(&session).await.unwrap(),
        ActivateResult::Done
    );

    let connected = |m: Message| match m {
        Message::Peer(peer) if peer.peer_state == PeerState::Connected => Some(peer),
        _ => None,
    };
    let alice_seen_by_b = game_b
        .wait_event(LIMIT, connected)
        .await
        .expect("tunnel at B");
    let bobby_seen_by_a = game_a
        .wait_event(LIMIT, connected)
        .await
        .expect("tunnel at A");
    assert_eq!(alice_seen_by_b.account_id, alice_id.0);
    assert_eq!(bobby_seen_by_a.account_id, bobby_id.0);
    assert_eq!(alice_seen_by_b.address, game_a.state().address);
    assert_eq!(bobby_seen_by_a.address, game_b.state().address);

    let socket_a = game_a.socket(3658).await.unwrap();
    let socket_b = game_b.socket(3658).await.unwrap();
    let mut answered = false;
    for round in 0u32..50 {
        socket_b
            .send_to(alice_seen_by_b.address, 3658, &round.to_le_bytes())
            .await
            .unwrap();
        let Ok(Ok((from, port, bytes))) =
            tokio::time::timeout(Duration::from_millis(300), socket_a.recv()).await
        else {
            continue;
        };
        assert_eq!((from, port), (bobby_seen_by_a.address, 3658));
        socket_a.send_to(from, port, &bytes).await.unwrap();
        let (back_from, _, back) = tokio::time::timeout(Duration::from_secs(5), socket_b.recv())
            .await
            .expect("no answer back")
            .unwrap();
        assert_eq!(back_from, alice_seen_by_b.address);
        assert_eq!(back, round.to_le_bytes());
        answered = true;
        break;
    }
    assert!(answered, "no datagram crossed the tunnel");

    // Both launchers show the session with a live tunnel to the other player.
    let links = bobby
        .wait("the tunnel in the session", |event| match event {
            Event::Session {
                session: Some(shown),
            } if shown.members.len() == 2
                && shown
                    .members
                    .iter()
                    .any(|m| m.link.is_some_and(|l| l != Link::Connecting)) =>
            {
                Some(shown.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(links.id, session);
    let first_link = links
        .members
        .iter()
        .find(|m| m.account.id == alice_id)
        .and_then(|m| m.link)
        .expect("Bobby's launcher shows the tunnel to Alice");
    // A tunnel often starts on the relay and goes direct a moment later; on loopback it always
    // can, unless Alice keeps to the relay. A busy CI machine can take a while to get there, so
    // where a direct tunnel is expected the wait is the full limit (it ends as soon as the tunnel
    // goes direct); where it is not, a few seconds show that the tunnel stays on the relay.
    let patience = if alice_relay == AliceRelay::FromStart {
        Duration::from_secs(5)
    } else {
        LIMIT
    };
    let went_direct = first_link == Link::Direct
        || tokio::time::timeout(
            patience,
            bobby.wait("a direct tunnel to Alice", |event| match event {
                Event::Session {
                    session: Some(shown),
                } if shown
                    .members
                    .iter()
                    .any(|m| m.account.id == alice_id && m.link == Some(Link::Direct)) =>
                {
                    Some(())
                }
                _ => None,
            }),
        )
        .await
        .is_ok();
    let alice_link = if went_direct {
        Link::Direct
    } else {
        first_link
    };
    if alice_relay == AliceRelay::TurnedOn {
        assert_eq!(alice_link, Link::Direct);
        journal.note("Alice turns relay only on".into());
        alice.ok(Command::RelayOnly { on: true }).await;
        journal.note("Alice's net service answered".into());
        // Every tunnel state Bobby's launcher shows for Alice goes into the journal, so that a
        // failure tells how far the move to the relay got.
        let moved = tokio::time::timeout(LIMIT, async {
            loop {
                let event = bobby.events.recv().await.expect("events ended");
                if let Event::Session {
                    session: Some(shown),
                } = &event
                {
                    let link = shown
                        .members
                        .iter()
                        .find(|m| m.account.id == alice_id)
                        .map(|m| m.link);
                    journal.note(format!("Bobby's launcher: tunnel to Alice {link:?}"));
                    if link == Some(Some(Link::Relay)) {
                        return Link::Relay;
                    }
                }
            }
        })
        .await;
        return moved.unwrap_or_else(|_| {
            panic!(
                "the tunnel to Alice did not move to the relay in time:\n{}",
                journal.lines()
            )
        });
    }

    // Bobby's game ends: he leaves, and Alice's game hears that he is gone.
    bobby.ok(Command::GameEnd).await;
    drop(socket_b);
    drop(game_b);
    let gone = game_a
        .wait_event(LIMIT, |m| match m {
            Message::Peer(peer) if peer.peer_state == PeerState::Disconnected => Some(peer),
            _ => None,
        })
        .await
        .expect("Alice hears that Bobby left");
    assert_eq!(gone.account_id, bobby_id.0);
    alice_link
}

/// Play Together: Alice starts the game with a session of her own, Bobby joins it from his
/// presence, and the games find each other by broadcast, as a LAN game does, without either
/// calling a session function. Then Alice's game ends and Bobby's launcher is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friends_play_together_by_broadcast_and_the_session_ends_with_its_host() {
    use anyplay_control::anyplay_api::{SessionKind, Visibility};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let cloud_dir = tempfile::tempdir().unwrap();
    let (signer, _) = anyplay_cloud::load_signing_key(cloud_dir.path()).unwrap();
    let (_relay, relay_url, journal) = relay(&signer).await;
    let mail = Arc::new(MemoryMailer::default());
    let config = Config {
        data_dir: cloud_dir.path().to_path_buf(),
        name: "End-to-end cloud".into(),
        relays: vec![relay_url],
        new_account_hours: 0,
        ..Config::default()
    };
    let state = open(config, Tuning::default(), mail.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let cloud_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, state, std::future::pending()));

    let mut alice = Launcher::start("Alice", false, journal.clone()).await;
    let mut bobby = Launcher::start("Bobby", false, journal.clone()).await;
    let alice_id = alice
        .sign_up(&cloud_url, &mail, "alice@example.org", "Alice")
        .await;
    let bobby_id = bobby
        .sign_up(&cloud_url, &mail, "bobby@example.org", "Bobby")
        .await;
    alice
        .ok(Command::AddFriend {
            handle: "bobby".into(),
        })
        .await;
    bobby
        .friends("the request", |l| {
            l.incoming.iter().any(|r| r.account.id == alice_id)
        })
        .await;
    bobby.ok(Command::Accept { account: alice_id }).await;
    alice
        .friends("the new friend", |l| {
            l.friends.iter().any(|f| f.account.id == bobby_id)
        })
        .await;

    // Alice presses Play Together. Her game never calls a session function.
    let (game_a, session) = alice
        .start_hosted_game(anyplay_control::Host {
            max_members: 4,
            visibility: Visibility::Friends,
            party: false,
        })
        .await;
    let shown = alice
        .wait("her own LAN session", |event| match event {
            Event::Session { session: Some(s) } if s.id == session => Some(s.clone()),
            _ => None,
        })
        .await;
    assert_eq!(shown.kind, SessionKind::Lan);

    // Bobby sees her playing together, and his Join starts his game in the session.
    let lists = bobby
        .friends("Alice playing together", |l| {
            l.friends.iter().any(|f| {
                f.account.id == alice_id
                    && f.presence
                        .activity
                        .as_ref()
                        .and_then(|a| a.session.as_ref())
                        .is_some_and(|s| s.joinable && s.kind == SessionKind::Lan)
            })
        })
        .await;
    let reference = lists
        .friends
        .iter()
        .find(|f| f.account.id == alice_id)
        .and_then(|f| f.presence.activity.clone())
        .and_then(|a| a.session)
        .unwrap();
    assert_eq!(reference.id, session);
    assert_eq!((reference.members, reference.max_members), (1, 4));
    bobby
        .ok(Command::CheckJoin {
            session: session.clone(),
            title_id: TITLE.into(),
            version: "01.000".into(),
            build: "0.1.3".into(),
            version_rule: Default::default(),
            override_version: false,
        })
        .await;
    let game_b = bobby.start_game(Some(session.clone())).await;
    let joined = alice
        .wait("Bobby in the session", |event| match event {
            Event::Session { session: Some(s) } if s.members.len() == 2 => Some(s.clone()),
            _ => None,
        })
        .await;
    assert_eq!(joined.kind, SessionKind::Lan);

    // The games listen on the LAN discovery port and broadcast to it, as Minecraft does. A copy
    // reaches the other member, from the sender's virtual address.
    let socket_a = game_a.socket(19132).await.unwrap();
    let socket_b = game_b.socket(19132).await.unwrap();
    let ping = b"\x01looking for a world";
    let mut heard = None;
    for _ in 0..60 {
        socket_b
            .send_to(Ipv4Addr::BROADCAST, 19132, ping)
            .await
            .unwrap();
        if let Ok(Ok(got)) = tokio::time::timeout(Duration::from_millis(300), socket_a.recv()).await
        {
            heard = Some(got);
            break;
        }
    }
    let (from, from_port, bytes) = heard.expect("Alice's game never heard Bobby's broadcast");
    assert_eq!(
        (from, from_port, bytes.as_slice()),
        (game_b.state().address, 19132, &ping[..])
    );
    // Alice's world answers by unicast and Bobby hears it. Her own broadcast reaches him as well.
    socket_a.send_to(from, from_port, b"a world").await.unwrap();
    let (answer_from, _, answer) = tokio::time::timeout(Duration::from_secs(5), socket_b.recv())
        .await
        .expect("no answer")
        .unwrap();
    assert_eq!(answer_from, game_a.state().address);
    assert_eq!(answer, b"a world");
    socket_a
        .send_to(Ipv4Addr::new(198, 19, 255, 255), 19132, b"hosting")
        .await
        .unwrap();
    let (_, _, hosting) = tokio::time::timeout(Duration::from_secs(5), socket_b.recv())
        .await
        .expect("Bobby never heard the world announce itself")
        .unwrap();
    assert_eq!(hosting, b"hosting");

    // Alice's game ends: the world goes with her, and Bobby's launcher says the session ended.
    alice.ok(Command::GameEnd).await;
    drop(socket_a);
    drop(game_a);
    let ended = bobby
        .wait("the end of the session", |event| match event {
            Event::SessionEnded {
                session: id, host, ..
            } if *id == session => Some(host.clone()),
            _ => None,
        })
        .await;
    assert_eq!(ended.id, alice_id);
    assert_eq!(ended.handle, "Alice");
    bobby
        .wait("no session in Bobby's launcher", |event| match event {
            Event::Session { session: None } => Some(()),
            _ => None,
        })
        .await;
}

/// A running cloud with a relay, and the mail it sends.
struct Running {
    url: String,
    mail: Arc<MemoryMailer>,
    journal: Journal,
    _relay: iroh_relay::server::Server,
    _data: tempfile::TempDir,
}

/// Starts a cloud. Share play is off in a cloud's default setup; only a caller that asks for it
/// gets it.
async fn start_cloud(share_enabled: bool) -> Running {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let data = tempfile::tempdir().unwrap();
    let (signer, _) = anyplay_cloud::load_signing_key(data.path()).unwrap();
    let (relay, relay_url, journal) = relay(&signer).await;
    let mail = Arc::new(MemoryMailer::default());
    let config = Config {
        data_dir: data.path().to_path_buf(),
        name: "End-to-end cloud".into(),
        relays: vec![relay_url],
        new_account_hours: 0,
        share_enabled,
        ..Config::default()
    };
    let state = open(config, Tuning::default(), mail.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, state, std::future::pending()));
    Running {
        url,
        mail,
        journal,
        _relay: relay,
        _data: data,
    }
}

/// Signs an existing account in on one more device, as a plain API client, and returns its access
/// token. The net service keeps its own token to itself.
async fn api_token(cloud: &Running, email: &str) -> String {
    use anyplay_control::anyplay_api::{
        CodeAnswer, CodeRequest, Device, EmailAnswer, EmailRequest, Platform,
    };
    let http = reqwest::Client::new();
    let asked: EmailAnswer = http
        .post(format!("{}/v1/auth/email", cloud.url))
        .json(&EmailRequest {
            email: email.into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = cloud.mail.last_code(email).expect("no code mailed");
    let answer: CodeAnswer = http
        .post(format!("{}/v1/auth/code", cloud.url))
        .json(&CodeRequest {
            challenge: asked.challenge,
            code,
            device: Device {
                name: "API client".into(),
                platform: Platform::Linux,
                key: None,
            },
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    match answer {
        CodeAnswer::SignedIn { tokens, .. } => tokens.access,
        other => panic!("expected a sign-in for {email}, got {other:?}"),
    }
}

/// One call to the cloud's API as the holder of `token`: the status and the JSON body (null when
/// there is none).
async fn api(
    cloud: &Running,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}{path}", cloud.url))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let reply = request.send().await.unwrap();
    let status = reply.status().as_u16();
    let bytes = reply.bytes().await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Share play from the invitation to its end: two party members, the host's invitation, the
/// guest's acceptance, the mode changing and the host stopping. Both net services follow what
/// the cloud tells them; the calls a launcher would make with a share worker are made as a plain
/// API client, because the test has no worker to encode or decode a picture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_share_runs_from_invitation_to_end() {
    use anyplay_control::anyplay_api::{Share, ShareMode, ShareState};
    use anyplay_control::{SharePhase, ShareRole};
    use reqwest::Method;

    let cloud = start_cloud(true).await;
    let mut alice = Launcher::start("Alice", false, cloud.journal.clone()).await;
    let mut bobby = Launcher::start("Bobby", false, cloud.journal.clone()).await;
    let alice_id = alice
        .sign_up(&cloud.url, &cloud.mail, "alice@example.org", "Alice")
        .await;
    let bobby_id = bobby
        .sign_up(&cloud.url, &cloud.mail, "bobby@example.org", "Bobby")
        .await;

    // They are friends, and Bobby is in Alice's party.
    alice
        .ok(Command::AddFriend {
            handle: "bobby".into(),
        })
        .await;
    bobby
        .friends("the request", |l| {
            l.incoming.iter().any(|r| r.account.id == alice_id)
        })
        .await;
    bobby.ok(Command::Accept { account: alice_id }).await;
    alice
        .friends("the new friend", |l| {
            l.friends.iter().any(|f| f.account.id == bobby_id)
        })
        .await;
    alice
        .ok(Command::PartyInvite {
            accounts: vec![bobby_id],
            call: false,
        })
        .await;
    let party_invitation = bobby
        .wait("the party invitation", |event| match event {
            Event::PartyInvitation { invitation } => Some(invitation.id.clone()),
            _ => None,
        })
        .await;
    bobby
        .ok(Command::PartyAnswer {
            id: party_invitation,
            accept: true,
        })
        .await;
    // Both net services know the party before the share begins: a share with someone outside
    // the party they hold would end at once.
    for launcher in [&mut alice, &mut bobby] {
        launcher
            .wait("the party of two", |event| match event {
                Event::Party { party: Some(party) } if party.members.len() == 2 => Some(()),
                _ => None,
            })
            .await;
    }

    // Alice plays, and Bobby sees it: the cloud shares only a game that is running.
    let _game = alice.start_game(None).await;
    bobby
        .friends("Alice playing", |l| {
            l.friends
                .iter()
                .any(|f| f.account.id == alice_id && f.presence.activity.is_some())
        })
        .await;

    let alice_token = api_token(&cloud, "alice@example.org").await;
    let bobby_token = api_token(&cloud, "bobby@example.org").await;
    let host_endpoint = "01".repeat(32);
    let guest_endpoint = "02".repeat(32);

    // The host invites the guest. The guest's launcher shows the invitation, the host's shows
    // that the guest has not answered.
    let (status, body) = api(
        &cloud,
        Method::POST,
        "/v1/shares",
        &alice_token,
        Some(serde_json::json!({
            "to": bobby_id.0,
            "mode": "watch",
            "endpoint": host_endpoint,
        })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let invited: Share = serde_json::from_value(body).unwrap();
    assert_eq!(invited.state, ShareState::Invited);
    assert_eq!(invited.mode, ShareMode::Watch);
    assert_eq!(invited.host.id, alice_id);
    assert_eq!(invited.guest.id, bobby_id);
    assert_eq!(invited.title_id, TITLE);

    let invitation = bobby
        .wait("the share invitation", |event| match event {
            Event::ShareInvitation { invitation } => Some(invitation.clone()),
            _ => None,
        })
        .await;
    assert_eq!(invitation.share, invited.id);
    assert_eq!(invitation.from.id, alice_id);
    assert_eq!(invitation.mode, ShareMode::Watch);
    let waiting = alice
        .wait("the invited share", |event| match event {
            Event::Share {
                share: Some(view),
                ..
            } if view.role == ShareRole::Host && view.phase == SharePhase::Invited => {
                Some(view.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(waiting.id, invited.id);
    assert_eq!(waiting.peer.id, bobby_id);

    // The guest accepts and names its own endpoint. Its launcher shows the share.
    let (status, body) = api(
        &cloud,
        Method::POST,
        "/v1/share/accept",
        &bobby_token,
        Some(serde_json::json!({
            "endpoint": guest_endpoint,
            "relay": "https://relay.example.com",
        })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let active: Share = serde_json::from_value(body).unwrap();
    assert_eq!(active.id, invited.id);
    assert_eq!(active.state, ShareState::Active);
    assert_eq!(active.host_endpoint, host_endpoint);
    assert_eq!(active.guest_endpoint, guest_endpoint);
    let guest_view = bobby
        .wait("the share on the guest's side", |event| match event {
            Event::Share {
                share: Some(view),
                ..
            } if view.role == ShareRole::Guest => Some(view.clone()),
            _ => None,
        })
        .await;
    assert_eq!(guest_view.id, invited.id);
    assert_eq!(guest_view.peer.id, alice_id);

    // Only the host changes the mode, and the guest sees the change.
    let (status, _) = api(
        &cloud,
        Method::PATCH,
        "/v1/share",
        &bobby_token,
        Some(serde_json::json!({"mode": "together"})),
    )
    .await;
    assert_eq!(status, 403);
    let mut revision = active.revision;
    for (name, mode) in [
        ("hand_over", ShareMode::HandOver),
        ("together", ShareMode::Together),
    ] {
        let (status, body) = api(
            &cloud,
            Method::PATCH,
            "/v1/share",
            &alice_token,
            Some(serde_json::json!({"mode": name})),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let changed: Share = serde_json::from_value(body).unwrap();
        assert_eq!(changed.mode, mode);
        assert!(changed.revision > revision, "the revision did not rise");
        revision = changed.revision;
        let (status, body) = api(&cloud, Method::GET, "/v1/share", &bobby_token, None).await;
        assert_eq!(status, 200, "{body}");
        let seen: Share = serde_json::from_value(body).unwrap();
        assert_eq!(seen.mode, mode);
        assert_eq!(seen.state, ShareState::Active);
    }

    // The host stops: the guest is told, and the cloud holds no share for either of them.
    let (status, _) = api(&cloud, Method::DELETE, "/v1/share", &alice_token, None).await;
    assert_eq!(status, 204);
    bobby
        .wait("the end of the share", |event| match event {
            Event::Notice { code, .. } if code == "share_ended" => Some(()),
            _ => None,
        })
        .await;
    for token in [&alice_token, &bobby_token] {
        let (status, _) = api(&cloud, Method::GET, "/v1/share", token, None).await;
        assert_eq!(status, 404);
    }
}

/// A cloud that did not switch share play on refuses every call for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cloud_without_share_play_refuses_a_share() {
    use reqwest::Method;

    let cloud = start_cloud(false).await;
    let mut alice = Launcher::start("Alice", false, cloud.journal.clone()).await;
    alice
        .sign_up(&cloud.url, &cloud.mail, "alice@example.org", "Alice")
        .await;
    let token = api_token(&cloud, "alice@example.org").await;
    let (status, _) = api(
        &cloud,
        Method::POST,
        "/v1/shares",
        &token,
        Some(serde_json::json!({
            "to": 1,
            "mode": "watch",
            "endpoint": "01".repeat(32),
        })),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = api(&cloud, Method::GET, "/v1/share", &token, None).await;
    assert_eq!(status, 403);
}
