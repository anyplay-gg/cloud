//! The browser sign-in end to end: a launcher asks the net service for a sign-in, a "browser"
//! (an HTTP client with a cookie jar) goes through the cloud's pages, and the cloud sends it to
//! the net service's loopback address. The net service trades the code for the tokens of its
//! device, bound to a key it made, and later refreshes them with a proof.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyplay_cloud::mail::MemoryMailer;
use anyplay_cloud::{Config, Tuning, open, serve};
use anyplay_control::{
    Answer, Cloud, Command, Event, Failure, Output, Pending, command_line, parse_output,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::sync::{mpsc, oneshot};

const LIMIT: Duration = Duration::from_secs(30);

type Waiters = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

struct Launcher {
    write: WriteHalf<DuplexStream>,
    waiters: Waiters,
    events: mpsc::UnboundedReceiver<Event>,
    next: u64,
    data: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Launcher {
    async fn start() -> Launcher {
        let dir = tempfile::tempdir().unwrap();
        let (service_end, launcher_end) = tokio::io::duplex(1 << 20);
        let (service_read, service_write) = tokio::io::split(service_end);
        let config = anyplay_net::Config {
            data: dir.path().to_path_buf(),
            file_credentials: true,
            relay_roots: Vec::new(),
            bind: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
            relay_only: false,
            device_name: "Test PC".into(),
            audio: anyplay_net::voice::Audio::system(),
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
            data: dir.path().to_path_buf(),
            _dir: dir,
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
}

/// A browser with a cookie jar that follows nothing by itself.
struct Browser {
    client: reqwest::Client,
    cookies: Mutex<HashMap<String, String>>,
}

struct Page {
    status: u16,
    location: Option<String>,
    body: String,
}

impl Browser {
    fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cookies: Mutex::new(HashMap::new()),
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Page {
        let cookie = self
            .cookies
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let response = request.header("cookie", cookie).send().await.unwrap();
        for value in response.headers().get_all("set-cookie") {
            let line = value.to_str().unwrap();
            let (pair, _) = line.split_once(';').unwrap_or((line, ""));
            let (name, value) = pair.split_once('=').unwrap();
            self.cookies
                .lock()
                .unwrap()
                .insert(name.to_owned(), value.to_owned());
        }
        Page {
            status: response.status().as_u16(),
            location: response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            body: response.text().await.unwrap(),
        }
    }

    async fn get(&self, url: &str) -> Page {
        self.send(self.client.get(url)).await
    }

    async fn post(&self, url: &str, fields: &[(&str, &str)]) -> Page {
        self.send(
            self.client
                .post(url)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(
                    fields
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("&"),
                ),
        )
        .await
    }
}

fn csrf(page: &Page) -> String {
    let marker = "name=\"csrf\" value=\"";
    let start = page.body.find(marker).expect("a csrf field") + marker.len();
    page.body[start..].split('"').next().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_net_service_signs_in_through_the_browser_and_refreshes_with_a_proof() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let mail = Arc::new(MemoryMailer::default());
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Config {
        data_dir: dir.path().to_path_buf(),
        name: "End-to-end cloud".into(),
        public_url: Some(format!("http://localhost:{port}")),
        // Every device must come with a key: the net service has to bind itself.
        legacy_device_days: 0,
        ..Config::default()
    };
    // Access tokens that run out at once make the next request refresh, with a proof.
    let tuning = Tuning {
        access_ttl_secs: 1,
        ..Tuning::default()
    };
    let state = open(config, tuning, mail.clone()).await.unwrap();
    tokio::spawn(serve(listener, state.clone(), std::future::pending()));
    let cloud_url = format!("http://127.0.0.1:{port}");

    let mut launcher = Launcher::start().await;
    launcher
        .ok(Command::Hello {
            launcher: "test".into(),
            cloud: cloud_url.clone(),
        })
        .await;

    // The launcher asks for a browser sign-in and gets the page to open.
    let started = launcher.ok(Command::BrowserSignIn { create: true }).await;
    let url = started["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with(&format!("http://localhost:{port}/web/authorize?")));
    assert!(url.contains("code_challenge_method=S256"));
    launcher
        .wait("the pending browser sign-in", |event| match event {
            Event::State {
                pending: Some(Pending::Browser),
                ..
            } => Some(()),
            _ => None,
        })
        .await;

    // The player creates an account in the browser.
    let browser = Browser::new();
    let local = url.replace("localhost", "127.0.0.1");
    let page = browser.get(&local).await;
    assert_eq!(page.status, 303, "{}", page.body);
    let create = browser
        .get(&format!("{cloud_url}{}", page.location.unwrap()))
        .await;
    let sent = browser
        .post(
            &format!("{cloud_url}/web/email"),
            &[
                ("csrf", &csrf(&create)),
                ("email", "e2e%40example.com"),
                ("intent", "create"),
            ],
        )
        .await;
    assert_eq!(sent.status, 303, "{}", sent.body);
    let code = mail.last_code("e2e@example.com").unwrap();
    let entry = browser.get(&format!("{cloud_url}/web/code")).await;
    let verified = browser
        .post(
            &format!("{cloud_url}/web/code"),
            &[("csrf", &csrf(&entry)), ("code", &code)],
        )
        .await;
    assert_eq!(verified.location.as_deref(), Some("/web/details"));
    let details = browser.get(&format!("{cloud_url}/web/details")).await;
    let created = browser
        .post(
            &format!("{cloud_url}/web/details"),
            &[
                ("csrf", &csrf(&details)),
                ("handle", "E2E_Player"),
                ("age", "yes"),
            ],
        )
        .await;
    assert_eq!(
        created.location.as_deref(),
        Some("/web/next"),
        "{}",
        created.body
    );
    let back = browser.get(&format!("{cloud_url}/web/next")).await;
    assert_eq!(back.status, 200);
    let text = back.body.replace("&amp;", "&");
    let start = text.find("http://127.0.0.1:").unwrap();
    let redirect = text[start..].split('"').next().unwrap().to_owned();

    // The browser follows the redirect to the net service, which finishes the sign-in.
    let loopback = browser.get(&redirect).await;
    assert_eq!(loopback.status, 200, "{}", loopback.body);
    assert!(loopback.body.contains("Signed in"));
    let handle = launcher
        .wait("the signed-in account", |event| match event {
            Event::State {
                cloud: Cloud::Online,
                account: Some(account),
                pending: None,
            } => Some(account.handle.clone()),
            _ => None,
        })
        .await;
    assert_eq!(handle, "E2E_Player");

    // Another visit to the loopback address is nothing: the listener is gone.
    assert!(
        reqwest::Client::new()
            .get(&redirect)
            .send()
            .await
            .is_err_and(|_| true)
    );

    // The device key and the tokens are kept together, bound.
    let kept = std::fs::read_dir(&launcher.data)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("signin-"))
        .expect("the sign-in is kept in a file");
    let saved = std::fs::read_to_string(kept.path()).unwrap();
    assert!(
        saved.contains("\"key\""),
        "the device key is kept with the token"
    );

    // The access token lives one second. The next request refreshes, signing the request
    // with the key; the cloud only accepts that from the key it was told at sign-in.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let privacy = launcher.ok(Command::Privacy).await;
    assert!(privacy.get("trophies").is_some(), "{privacy}");

    // The launcher can open the account page: a one-time link, asked for with a proof.
    let opened = launcher.ok(Command::AccountPage).await;
    let link = opened["url"].as_str().unwrap();
    assert!(link.starts_with(&format!("http://localhost:{port}/web/link?c=")));
    let page = browser.get(&link.replace("localhost", "127.0.0.1")).await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("E2E_Player"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cloud_without_an_account_web_says_so() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let mail = Arc::new(MemoryMailer::default());
    let config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    let state = open(config, Tuning::default(), mail).await.unwrap();
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let cloud_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, state, std::future::pending()));

    let mut launcher = Launcher::start().await;
    launcher
        .ok(Command::Hello {
            launcher: "test".into(),
            cloud: cloud_url,
        })
        .await;
    let failure = launcher
        .call::<serde_json::Value>(Command::BrowserSignIn { create: false })
        .await
        .unwrap_err();
    assert_eq!(failure.code, "not_found");
    assert!(failure.message.contains("no account web"));
}
