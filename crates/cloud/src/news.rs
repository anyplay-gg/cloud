//! News for event connections: who has to hear about a change, and the connections that carry it.
//!
//! A node holds the event connections that reached it. A change is first written to the store;
//! then the node that made it sends the news to every account that has to hear about it. In one
//! process the news goes straight to the connections here. In a cluster it travels between nodes
//! over NATS (`docs/adr/0004-nats-carries-news-not-state.md`); this is the seam where that adapter
//! goes. News may be lost: what a client must not miss, it reads again after `welcome`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use anyplay_api::Event;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What the event connection task should do next.
#[derive(Debug)]
pub enum Push {
    Event(Event),
    Close { code: u16, reason: &'static str },
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
}

#[derive(Default)]
pub struct News {
    local: Mutex<Local>,
}

impl News {
    fn local(&self) -> std::sync::MutexGuard<'_, Local> {
        self.local.lock().unwrap_or_else(|e| e.into_inner())
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
    pub fn attach(
        &self,
        account: u64,
        device: u64,
        serial: u64,
        first: Push,
    ) -> UnboundedReceiver<Push> {
        let (tx, rx) = unbounded_channel();
        let _ = tx.send(first);
        let mut local = self.local();
        if let Some(old) = local.conns.remove(&device) {
            let _ = old.tx.send(Push::Close {
                code: 1000,
                reason: "replaced by a newer connection",
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
        rx
    }

    /// Forgets a connection that ended. False when a newer connection of the device replaced it
    /// already.
    pub fn detach(&self, device: u64, serial: u64) -> bool {
        let mut local = self.local();
        match local.conns.get(&device) {
            Some(conn) if conn.serial == serial => {}
            _ => return false,
        }
        if let Some(conn) = local.conns.remove(&device)
            && let Some(devices) = local.devices.get_mut(&conn.account)
        {
            devices.remove(&device);
            if devices.is_empty() {
                local.devices.remove(&conn.account);
            }
        }
        true
    }

    /// Sends an event to every device of an account that holds an event connection.
    pub fn push(&self, account: u64, event: &Event) {
        let local = self.local();
        for device in local.devices_of(account) {
            local.send(device, Push::Event(event.clone()));
        }
    }

    /// Sends an event to one device, if given, then closes its connection.
    pub fn close_device(&self, device: u64, event: Option<Event>, code: u16, reason: &'static str) {
        let local = self.local();
        if let Some(event) = event {
            local.send(device, Push::Event(event));
        }
        local.send(device, Push::Close { code, reason });
    }

    /// Sends an event to every device of an account, then closes the connections.
    pub fn close_account(&self, account: u64, event: &Event, code: u16, reason: &'static str) {
        let local = self.local();
        for device in local.devices_of(account) {
            local.send(device, Push::Event(event.clone()));
            local.send(device, Push::Close { code, reason });
        }
    }

    /// Closes every connection this node holds, for shutdown.
    pub fn close_all(&self, code: u16, reason: &'static str) {
        let local = self.local();
        for conn in local.conns.values() {
            let _ = conn.tx.send(Push::Close { code, reason });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn welcome() -> Push {
        Push::Event(Event::Friends)
    }

    fn attach(news: &News, account: u64, device: u64) -> (UnboundedReceiver<Push>, u64) {
        let serial = news.next_serial();
        (news.attach(account, device, serial, welcome()), serial)
    }

    #[test]
    fn a_new_connection_of_a_device_closes_the_old_one() {
        let news = News::default();
        let (mut old, old_serial) = attach(&news, 1, 10);
        let (mut new, new_serial) = attach(&news, 1, 10);
        assert!(matches!(old.try_recv(), Ok(Push::Event(_))));
        assert!(matches!(old.try_recv(), Ok(Push::Close { code: 1000, .. })));
        assert!(!news.detach(10, old_serial), "the old one was replaced");
        news.push(1, &Event::Friends);
        assert!(matches!(new.try_recv(), Ok(Push::Event(_))), "first");
        assert!(matches!(new.try_recv(), Ok(Push::Event(Event::Friends))));
        assert!(news.detach(10, new_serial));
        news.push(1, &Event::Friends);
        assert!(
            new.try_recv().is_err(),
            "a detached connection hears nothing"
        );
    }

    #[test]
    fn news_reaches_every_device_of_the_account_and_no_one_else() {
        let news = News::default();
        let (mut a1, _) = attach(&news, 1, 10);
        let (mut a2, _) = attach(&news, 1, 11);
        let (mut b, _) = attach(&news, 2, 20);
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
}
