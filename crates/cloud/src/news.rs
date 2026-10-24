//! News for event connections: who has to hear about a change, and the connections that carry it.
//!
//! A node holds the event connections that reached it. A change is first written to the store;
//! then the node that made it sends the news to every account that has to hear about it. In one
//! process the news goes straight to the connections here. In a cluster it also goes out over
//! NATS core, one subject per account (`anyplay.account.<id>`), which a node subscribes to while
//! it holds a connection of that account (`docs/adr/0004-nats-carries-news-not-state.md`).
//!
//! News may be lost: what a client must not miss, it reads again after `welcome`. A node never
//! hears its own news back from NATS, so each event reaches a connection once.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyplay_api::Event;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

/// What the event connection task should do next.
#[derive(Debug)]
pub enum Push {
    Event(Event),
    Close {
        code: u16,
        reason: Cow<'static, str>,
    },
}

/// A change to what every node caches about who may use the cloud. The node that makes the
/// change applies it itself; the others hear of it over NATS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "note", rename_all = "snake_case")]
pub enum AuthNote {
    DeviceGone { device: u64 },
    Banned { account: u64, until: Option<u64> },
    Unbanned { account: u64 },
}

/// What one node tells the nodes holding an account's connections.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "note", rename_all = "snake_case")]
enum Note {
    Event {
        event: Event,
    },
    CloseDevice {
        device: u64,
        event: Option<Event>,
        code: u16,
        reason: String,
    },
    CloseAccount {
        event: Event,
        code: u16,
        reason: String,
    },
}

const NODES: &str = "anyplay.nodes";

fn subject(account: u64) -> String {
    format!("anyplay.account.{account}")
}

struct Conn {
    account: u64,
    /// Tells a replaced connection from its replacement when the old one finishes.
    serial: u64,
    tx: UnboundedSender<Push>,
}

#[derive(Default)]
struct Local {
    /// By device: a device holds one event connection.
    conns: HashMap<u64, Conn>,
    devices: HashMap<u64, HashSet<u64>>,
    next_serial: u64,
}

impl Local {
    fn send(&self, device: u64, push: Push) {
        if let Some(conn) = self.conns.get(&device) {
            let _ = conn.tx.send(push);
        }
    }

    fn devices_of(&self, account: u64) -> Vec<u64> {
        self.devices
            .get(&account)
            .map(|d| d.iter().copied().collect())
            .unwrap_or_default()
    }

    fn event(&self, account: u64, event: &Event) {
        for device in self.devices_of(account) {
            self.send(device, Push::Event(event.clone()));
        }
    }

    fn close_device(
        &self,
        device: u64,
        event: Option<Event>,
        code: u16,
        reason: Cow<'static, str>,
    ) {
        if let Some(event) = event {
            self.send(device, Push::Event(event));
        }
        self.send(device, Push::Close { code, reason });
    }

    fn close_account(&self, account: u64, event: &Event, code: u16, reason: Cow<'static, str>) {
        for device in self.devices_of(account) {
            self.close_device(device, Some(event.clone()), code, reason.clone());
        }
    }

    /// Applies news another node sent for an account.
    fn apply(&self, account: u64, note: Note) {
        match note {
            Note::Event { event } => self.event(account, &event),
            Note::CloseDevice {
                device,
                event,
                code,
                reason,
            } => {
                if self
                    .conns
                    .get(&device)
                    .is_some_and(|c| c.account == account)
                {
                    self.close_device(device, event, code, reason.into());
                }
            }
            Note::CloseAccount {
                event,
                code,
                reason,
            } => self.close_account(account, &event, code, reason.into()),
        }
    }
}

struct Nats {
    client: async_nats::Client,
    /// Everything this node publishes, in order, for the one task that sends it.
    outbox: UnboundedSender<(String, Vec<u8>)>,
    /// Per account with a connection here: the task that listens on its subject.
    listening: tokio::sync::Mutex<HashMap<u64, JoinHandle<()>>>,
}

#[derive(Default)]
pub struct News {
    local: Arc<Mutex<Local>>,
    nats: Option<Nats>,
    sent: AtomicU64,
    received: Arc<AtomicU64>,
}

impl News {
    /// News between the nodes of a cluster over the NATS server at `url`. The receiver yields
    /// what other nodes change about devices and bans.
    pub async fn over_nats(
        url: &str,
        node: &str,
    ) -> anyhow::Result<(News, UnboundedReceiver<AuthNote>)> {
        let client = async_nats::ConnectOptions::new()
            .no_echo()
            .name(format!("anyplay-cloud {node}"))
            .connect(url)
            .await?;

        let (outbox, mut queued) = unbounded_channel::<(String, Vec<u8>)>();
        let publisher = client.clone();
        tokio::spawn(async move {
            while let Some((subject, payload)) = queued.recv().await {
                if let Err(error) = publisher.publish(subject, payload.into()).await {
                    tracing::warn!(%error, "news to other nodes is lost");
                }
            }
        });

        let mut nodes = client.subscribe(NODES).await?;
        let (notes, received) = unbounded_channel();
        tokio::spawn(async move {
            while let Some(message) = nodes.next().await {
                match serde_json::from_slice::<AuthNote>(&message.payload) {
                    Ok(note) => {
                        if notes.send(note).is_err() {
                            return;
                        }
                    }
                    Err(error) => tracing::warn!(%error, "a node sent a note this one cannot read"),
                }
            }
        });

        let news = News {
            nats: Some(Nats {
                client,
                outbox,
                listening: tokio::sync::Mutex::default(),
            }),
            ..News::default()
        };
        Ok((news, received))
    }

    fn local(&self) -> std::sync::MutexGuard<'_, Local> {
        self.local.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish<T: Serialize>(&self, subject: String, note: &T) {
        let Some(nats) = &self.nats else { return };
        match serde_json::to_vec(note) {
            Ok(payload) => {
                if nats.outbox.send((subject, payload)).is_ok() {
                    self.sent.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(error) => tracing::error!(%error, "cannot write news for other nodes"),
        }
    }

    /// Starts listening to an account's news from other nodes, unless this node does already.
    async fn listen(
        &self,
        nats: &Nats,
        listening: &mut HashMap<u64, JoinHandle<()>>,
        account: u64,
    ) {
        if listening.contains_key(&account) {
            return;
        }
        let mut news = match nats.client.subscribe(subject(account)).await {
            Ok(news) => news,
            Err(error) => {
                tracing::error!(%error, "cannot listen to news for an account");
                return;
            }
        };
        let local = self.local.clone();
        let received = self.received.clone();
        let task = tokio::spawn(async move {
            while let Some(message) = news.next().await {
                received.fetch_add(1, Ordering::Relaxed);
                match serde_json::from_slice::<Note>(&message.payload) {
                    Ok(note) => local
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .apply(account, note),
                    Err(error) => tracing::warn!(%error, "unreadable news from a node"),
                }
            }
        });
        listening.insert(account, task);
    }

    /// Event connections this node holds, and news it sent to and received from other nodes.
    pub fn counts(&self) -> (usize, u64, u64) {
        (
            self.local().conns.len(),
            self.sent.load(Ordering::Relaxed),
            self.received.load(Ordering::Relaxed),
        )
    }

    /// Whether the connection to NATS stands; `None` for a single process without one.
    pub fn nats_connected(&self) -> Option<bool> {
        self.nats
            .as_ref()
            .map(|nats| nats.client.connection_state() == async_nats::connection::State::Connected)
    }

    /// A new serial for a connection, unique on this node. The store records it before the
    /// connection is attached.
    pub fn next_serial(&self) -> u64 {
        let mut local = self.local();
        local.next_serial += 1;
        local.next_serial
    }

    /// Registers the event connection of a device, closing an older one of the same device. The
    /// receiver yields `first` before anything else; [`News::detach`] needs the same `serial`.
    /// In a cluster the node listens to the account's news before `first` goes out, so nothing
    /// sent after the welcome is missed.
    pub async fn attach(
        &self,
        account: u64,
        device: u64,
        serial: u64,
        first: Push,
    ) -> UnboundedReceiver<Push> {
        // Held to the end, so that a detach of the account's last connection cannot stop the
        // listening in between.
        let mut listening = match &self.nats {
            Some(nats) => {
                let mut listening = nats.listening.lock().await;
                self.listen(nats, &mut listening, account).await;
                Some(listening)
            }
            None => None,
        };
        let (tx, rx) = unbounded_channel();
        let _ = tx.send(first);
        {
            let mut local = self.local();
            if let Some(old) = local.conns.remove(&device) {
                let _ = old.tx.send(Push::Close {
                    code: 1000,
                    reason: "replaced by a newer connection".into(),
                });
                if let Some(devices) = local.devices.get_mut(&old.account) {
                    devices.remove(&device);
                }
            }
            local.conns.insert(
                device,
                Conn {
                    account,
                    serial,
                    tx,
                },
            );
            local.devices.entry(account).or_default().insert(device);
        }
        listening.take();
        rx
    }

    /// Forgets a connection that ended. False when a newer connection of the device replaced it
    /// already.
    pub async fn detach(&self, device: u64, serial: u64) -> bool {
        let mut listening = match &self.nats {
            Some(nats) => Some(nats.listening.lock().await),
            None => None,
        };
        let mut local = self.local();
        match local.conns.get(&device) {
            Some(conn) if conn.serial == serial => {}
            _ => return false,
        }
        let Some(conn) = local.conns.remove(&device) else {
            return false;
        };
        let last = match local.devices.get_mut(&conn.account) {
            Some(devices) => {
                devices.remove(&device);
                devices.is_empty()
            }
            None => true,
        };
        if last {
            local.devices.remove(&conn.account);
            // Dropping the subscription inside the task tells NATS this node stops listening.
            if let Some(task) = listening.as_mut().and_then(|l| l.remove(&conn.account)) {
                task.abort();
            }
        }
        true
    }

    /// Sends an event to every device of an account that holds an event connection, here or on
    /// another node.
    pub fn push(&self, account: u64, event: &Event) {
        self.local().event(account, event);
        self.publish(
            subject(account),
            &Note::Event {
                event: event.clone(),
            },
        );
    }

    /// Sends an event to one device of an account, if given, then closes its connection.
    pub fn close_device(
        &self,
        account: u64,
        device: u64,
        event: Option<Event>,
        code: u16,
        reason: &'static str,
    ) {
        self.local()
            .close_device(device, event.clone(), code, reason.into());
        self.publish(
            subject(account),
            &Note::CloseDevice {
                device,
                event,
                code,
                reason: reason.to_owned(),
            },
        );
    }

    /// Sends an event to every device of an account, then closes the connections.
    pub fn close_account(&self, account: u64, event: &Event, code: u16, reason: &'static str) {
        self.local()
            .close_account(account, event, code, reason.into());
        self.publish(
            subject(account),
            &Note::CloseAccount {
                event: event.clone(),
                code,
                reason: reason.to_owned(),
            },
        );
    }

    /// Tells the other nodes what changed about devices and bans.
    pub fn tell_nodes(&self, note: &AuthNote) {
        self.publish(NODES.to_owned(), note);
    }

    /// Closes every connection this node holds, for shutdown.
    pub fn close_all(&self, code: u16, reason: &'static str) {
        let local = self.local();
        for conn in local.conns.values() {
            let _ = conn.tx.send(Push::Close {
                code,
                reason: reason.into(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn welcome() -> Push {
        Push::Event(Event::Friends)
    }

    async fn attach(news: &News, account: u64, device: u64) -> (UnboundedReceiver<Push>, u64) {
        let serial = news.next_serial();
        (
            news.attach(account, device, serial, welcome()).await,
            serial,
        )
    }

    #[tokio::test]
    async fn a_new_connection_of_a_device_closes_the_old_one() {
        let news = News::default();
        let (mut old, old_serial) = attach(&news, 1, 10).await;
        let (mut new, new_serial) = attach(&news, 1, 10).await;
        assert!(matches!(old.try_recv(), Ok(Push::Event(_))));
        assert!(matches!(old.try_recv(), Ok(Push::Close { code: 1000, .. })));
        assert!(
            !news.detach(10, old_serial).await,
            "the old one was replaced"
        );
        news.push(1, &Event::Friends);
        assert!(matches!(new.try_recv(), Ok(Push::Event(_))), "first");
        assert!(matches!(new.try_recv(), Ok(Push::Event(Event::Friends))));
        assert!(news.detach(10, new_serial).await);
        news.push(1, &Event::Friends);
        assert!(
            new.try_recv().is_err(),
            "a detached connection hears nothing"
        );
    }

    #[tokio::test]
    async fn news_reaches_every_device_of_the_account_and_no_one_else() {
        let news = News::default();
        let (mut a1, _) = attach(&news, 1, 10).await;
        let (mut a2, _) = attach(&news, 1, 11).await;
        let (mut b, _) = attach(&news, 2, 20).await;
        for rx in [&mut a1, &mut a2, &mut b] {
            rx.try_recv().unwrap();
        }
        news.push(1, &Event::Friends);
        assert!(a1.try_recv().is_ok());
        assert!(a2.try_recv().is_ok());
        assert!(b.try_recv().is_err());
        news.close_account(2, &Event::SignedOut, 4004, "signed out");
        assert!(matches!(b.try_recv(), Ok(Push::Event(Event::SignedOut))));
        assert!(matches!(b.try_recv(), Ok(Push::Close { code: 4004, .. })));
    }

    #[test]
    fn notes_read_back_as_written() {
        let note = Note::CloseDevice {
            device: 7,
            event: Some(Event::SignedOut),
            code: 4004,
            reason: "signed out".into(),
        };
        let text = serde_json::to_string(&note).unwrap();
        assert!(matches!(
            serde_json::from_str::<Note>(&text).unwrap(),
            Note::CloseDevice {
                device: 7,
                code: 4004,
                ..
            }
        ));
        let auth = AuthNote::Banned {
            account: 3,
            until: None,
        };
        let text = serde_json::to_string(&auth).unwrap();
        assert_eq!(serde_json::from_str::<AuthNote>(&text).unwrap(), auth);
    }
}
