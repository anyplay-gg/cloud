//! Everything the cloud keeps in memory: event connections, presence, sessions, invitations and
//! virtual address leases. One lock guards it all, so a change and the events it causes are
//! never seen half done. Durable data lives in the database (see `db`).

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use anyplay_api::{
    Account, AccountId, Activity, Event, Invitation, Member, Presence, Session, SessionRef,
    Visibility,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::util::sha256;

/// What the event connection task should do next.
#[derive(Debug)]
pub enum Push {
    Event(Event),
    Close { code: u16, reason: &'static str },
}

pub struct Conn {
    pub account: u64,
    /// Tells a replaced connection from its replacement when the old one finishes.
    pub serial: u64,
    pub tx: UnboundedSender<Push>,
}

#[derive(Default)]
pub struct AccountMem {
    /// Devices that hold an event connection.
    pub devices: HashSet<u64>,
    pub online: bool,
    /// Changes whenever a connection opens or the last one closes; timers compare it.
    pub generation: u64,
    pub activity: Option<Activity>,
    /// The presence friends were last told about.
    pub sent: Option<Presence>,
    pub slot: Option<u32>,
    pub lease_expires: Option<Instant>,
    pub sessions: Vec<String>,
}

pub struct MemberMem {
    pub account: u64,
    pub handle: String,
    pub endpoint: String,
    pub relay: Option<String>,
    pub address: Ipv4Addr,
    pub joined: u64,
}

pub struct SessionMem {
    pub id: String,
    pub title_id: String,
    pub title: String,
    pub version: String,
    pub build: String,
    pub leader: u64,
    pub max_members: u16,
    pub visibility: Visibility,
    pub properties: String,
    /// In order of joining, so the first is the longest-standing member.
    pub members: Vec<MemberMem>,
    pub created: u64,
}

pub struct InvitationMem {
    pub id: String,
    pub from: u64,
    pub from_handle: String,
    pub to: u64,
    pub session: String,
    pub title_id: String,
    pub title: String,
    pub version: String,
    pub expires: u64,
    pub deadline: Instant,
}

/// The size of `198.18.0.0/15`.
const ADDRESS_SPACE: u32 = 1 << 17;
const ADDRESS_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

fn slot_address(slot: u32) -> Ipv4Addr {
    Ipv4Addr::from(ADDRESS_BASE + slot)
}

/// Addresses ending in .0 or .255 look like network and broadcast addresses to some games.
fn usable(slot: u32) -> bool {
    let last = slot & 0xff;
    last != 0 && last != 0xff
}

fn preferred_slot(account: u64) -> u32 {
    let hash = sha256(&account.to_be_bytes());
    let n = u64::from_be_bytes([
        hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7],
    ]);
    (n % u64::from(ADDRESS_SPACE)) as u32
}

#[derive(Default)]
pub struct Memory {
    pub accounts: HashMap<u64, AccountMem>,
    pub conns: HashMap<u64, Conn>,
    pub sessions: HashMap<String, SessionMem>,
    pub invitations: HashMap<String, InvitationMem>,
    slots: HashMap<u32, u64>,
    pub next_serial: u64,
}

impl Memory {
    /// Sends an event to every device of an account that is connected.
    pub fn push(&self, account: u64, event: Event) {
        let Some(mem) = self.accounts.get(&account) else {
            return;
        };
        for device in &mem.devices {
            if let Some(conn) = self.conns.get(device) {
                let _ = conn.tx.send(Push::Event(event.clone()));
            }
        }
    }

    pub fn is_connected(&self, account: u64) -> bool {
        self.accounts
            .get(&account)
            .is_some_and(|a| !a.devices.is_empty())
    }

    /// Sends an event to one device, then closes its connection.
    pub fn close_device(&self, device: u64, event: Option<Event>, code: u16, reason: &'static str) {
        if let Some(conn) = self.conns.get(&device) {
            if let Some(event) = event {
                let _ = conn.tx.send(Push::Event(event));
            }
            let _ = conn.tx.send(Push::Close { code, reason });
        }
    }

    /// Sends an event to every device of an account, then closes the connections.
    pub fn close_account(&self, account: u64, event: Event, code: u16, reason: &'static str) {
        let Some(mem) = self.accounts.get(&account) else {
            return;
        };
        for device in &mem.devices {
            self.close_device(*device, Some(event.clone()), code, reason);
        }
    }

    // Presence

    pub fn presence_of(&self, account: u64) -> Presence {
        let Some(mem) = self.accounts.get(&account) else {
            return Presence::default();
        };
        if !mem.online {
            return Presence::default();
        }
        let activity = mem.activity.clone().map(|mut activity| {
            activity.session = self.joinable_session(account, &activity.title_id);
            activity
        });
        Presence {
            online: true,
            activity,
        }
    }

    /// The session of this title that friends of the account may join.
    fn joinable_session(&self, account: u64, title_id: &str) -> Option<SessionRef> {
        let mem = self.accounts.get(&account)?;
        mem.sessions
            .iter()
            .filter_map(|id| self.sessions.get(id))
            .filter(|s| s.title_id == title_id && s.visibility == Visibility::Friends)
            .min_by_key(|s| s.created)
            .map(|s| SessionRef {
                id: s.id.clone(),
                joinable: s.members.len() < usize::from(s.max_members),
                members: s.members.len() as u16,
                max_members: s.max_members,
            })
    }

    // Virtual addresses

    /// The account's address, leasing one when it has none. Keeps the lease alive: for good while
    /// the account is connected, for `ttl` after its last sign of life otherwise.
    pub fn lease(&mut self, account: u64, ttl: Duration) -> Option<Ipv4Addr> {
        let now = Instant::now();
        let connected = self.is_connected(account);
        let held = self
            .accounts
            .get(&account)
            .and_then(|a| a.slot)
            .filter(|slot| self.slots.get(slot) == Some(&account));
        let slot = match held {
            Some(slot) => slot,
            None => self.find_slot(account, now)?,
        };
        self.slots.insert(slot, account);
        let mem = self.accounts.entry(account).or_default();
        mem.slot = Some(slot);
        mem.lease_expires = if connected { None } else { Some(now + ttl) };
        Some(slot_address(slot))
    }

    fn find_slot(&mut self, account: u64, now: Instant) -> Option<u32> {
        let start = preferred_slot(account);
        for step in 0..ADDRESS_SPACE {
            let slot = (start + step) % ADDRESS_SPACE;
            if !usable(slot) {
                continue;
            }
            match self.slots.get(&slot).copied() {
                None => return Some(slot),
                Some(owner) if owner == account => return Some(slot),
                Some(owner) => {
                    let free = self.accounts.get(&owner).is_none_or(|m| {
                        m.devices.is_empty()
                            && m.sessions.is_empty()
                            && m.lease_expires.is_some_and(|t| t <= now)
                    });
                    if free {
                        if let Some(m) = self.accounts.get_mut(&owner) {
                            m.slot = None;
                        }
                        self.slots.remove(&slot);
                        return Some(slot);
                    }
                }
            }
        }
        None
    }

    fn release_lease(&mut self, account: u64) {
        if let Some(slot) = self.accounts.get_mut(&account).and_then(|a| a.slot.take())
            && self.slots.get(&slot) == Some(&account)
        {
            self.slots.remove(&slot);
        }
    }

    // Sessions

    pub fn session_view(&self, id: &str) -> Option<Session> {
        let s = self.sessions.get(id)?;
        Some(Session {
            id: s.id.clone(),
            title_id: s.title_id.clone(),
            title: s.title.clone(),
            version: s.version.clone(),
            build: s.build.clone(),
            leader: AccountId(s.leader),
            max_members: s.max_members,
            visibility: s.visibility,
            properties: s.properties.clone(),
            members: s
                .members
                .iter()
                .map(|m| Member {
                    account: Account {
                        id: AccountId(m.account),
                        handle: m.handle.clone(),
                    },
                    endpoint: m.endpoint.clone(),
                    relay: m.relay.clone(),
                    address: m.address.to_string(),
                    joined: m.joined,
                })
                .collect(),
            created: s.created,
        })
    }

    /// Sends the session as it is now to all its members.
    pub fn announce_session(&self, id: &str) {
        let (Some(view), Some(session)) = (self.session_view(id), self.sessions.get(id)) else {
            return;
        };
        for member in &session.members {
            self.push(
                member.account,
                Event::Session {
                    session: view.clone(),
                },
            );
        }
    }

    pub fn is_member(&self, id: &str, account: u64) -> bool {
        self.sessions
            .get(id)
            .is_some_and(|s| s.members.iter().any(|m| m.account == account))
    }

    /// Takes a member out of a session. A leaving leader hands over to the longest-standing
    /// member; the last member closes the session. Accounts whose presence may have changed are
    /// added to `dirty`.
    pub fn remove_member(&mut self, id: &str, account: u64, dirty: &mut Vec<u64>) {
        let Some(session) = self.sessions.get_mut(id) else {
            return;
        };
        let Some(position) = session.members.iter().position(|m| m.account == account) else {
            return;
        };
        session.members.remove(position);
        let empty = session.members.is_empty();
        if session.leader == account
            && let Some(first) = session.members.first()
        {
            session.leader = first.account;
        }
        if let Some(mem) = self.accounts.get_mut(&account) {
            mem.sessions.retain(|s| s != id);
        }
        dirty.push(account);
        self.push(account, Event::SessionGone { id: id.to_owned() });
        if empty {
            self.sessions.remove(id);
            self.drop_session_invitations(id);
        } else {
            if let Some(session) = self.sessions.get(id) {
                dirty.extend(session.members.iter().map(|m| m.account));
            }
            self.announce_session(id);
        }
    }

    fn drop_session_invitations(&mut self, session: &str) {
        let gone: Vec<String> = self
            .invitations
            .values()
            .filter(|i| i.session == session)
            .map(|i| i.id.clone())
            .collect();
        for id in gone {
            self.remove_invitation(&id);
        }
    }

    /// Forgets an invitation and tells its recipient.
    pub fn remove_invitation(&mut self, id: &str) -> Option<InvitationMem> {
        let invitation = self.invitations.remove(id)?;
        self.push(
            invitation.to,
            Event::InvitationGone {
                id: invitation.id.clone(),
            },
        );
        Some(invitation)
    }

    pub fn invitation_view(&self, invitation: &InvitationMem) -> Invitation {
        Invitation {
            id: invitation.id.clone(),
            from: Account {
                id: AccountId(invitation.from),
                handle: invitation.from_handle.clone(),
            },
            session: invitation.session.clone(),
            title_id: invitation.title_id.clone(),
            title: invitation.title.clone(),
            version: invitation.version.clone(),
            expires: invitation.expires,
        }
    }

    /// Removes the invitations between two accounts, in both directions.
    pub fn remove_invitations_between(&mut self, a: u64, b: u64) {
        let gone: Vec<String> = self
            .invitations
            .values()
            .filter(|i| (i.from == a && i.to == b) || (i.from == b && i.to == a))
            .map(|i| i.id.clone())
            .collect();
        for id in gone {
            self.remove_invitation(&id);
        }
    }

    /// Takes an account out of every session and drops its invitations.
    pub fn leave_everything(&mut self, account: u64, dirty: &mut Vec<u64>) {
        let sessions = self
            .accounts
            .get(&account)
            .map(|a| a.sessions.clone())
            .unwrap_or_default();
        for id in sessions {
            self.remove_member(&id, account, dirty);
        }
        let gone: Vec<String> = self
            .invitations
            .values()
            .filter(|i| i.from == account || i.to == account)
            .map(|i| i.id.clone())
            .collect();
        for id in gone {
            self.remove_invitation(&id);
        }
    }

    /// Forgets everything about an account that is gone for good.
    pub fn purge_account(&mut self, account: u64, dirty: &mut Vec<u64>) {
        self.leave_everything(account, dirty);
        self.release_lease(account);
        self.accounts.remove(&account);
    }

    /// Drops what has expired or is no longer needed.
    pub fn sweep(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .invitations
            .values()
            .filter(|i| i.deadline <= now)
            .map(|i| i.id.clone())
            .collect();
        for id in expired {
            self.remove_invitation(&id);
        }
        let idle: Vec<u64> = self
            .accounts
            .iter()
            .filter(|(_, m)| {
                !m.online
                    && m.devices.is_empty()
                    && m.sessions.is_empty()
                    && m.lease_expires.is_none_or(|t| t <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in idle {
            self.release_lease(id);
            self.accounts.remove(&id);
        }
    }

    pub fn offline(&mut self, account: u64, lease_ttl: Duration) {
        if let Some(mem) = self.accounts.get_mut(&account) {
            mem.online = false;
            mem.activity = None;
            mem.lease_expires = Some(Instant::now() + lease_ttl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: Duration = Duration::from_secs(600);

    #[test]
    fn addresses_come_from_the_virtual_range_and_avoid_the_ends_of_a_block() {
        let mut memory = Memory::default();
        for account in 1..2000u64 {
            let address = memory.lease(account, TTL).unwrap();
            assert!(anyplay_api::is_virtual(address));
            let last = address.octets()[3];
            assert!(last != 0 && last != 255, "{address}");
        }
    }

    #[test]
    fn an_account_keeps_its_address_and_collisions_probe_on() {
        let mut memory = Memory::default();
        let first = memory.lease(7, TTL).unwrap();
        assert_eq!(memory.lease(7, TTL), Some(first));
        // Take the preferred slot of another account by hand and see it move on.
        let wanted = preferred_slot(8);
        memory.slots.insert(wanted, 999);
        memory
            .accounts
            .entry(999)
            .or_default()
            .sessions
            .push("x".into());
        let moved = memory.lease(8, TTL).unwrap();
        assert_ne!(moved, slot_address(wanted));
    }

    #[test]
    fn an_expired_lease_is_given_to_the_next_account_that_wants_the_slot() {
        let mut memory = Memory::default();
        let wanted = preferred_slot(8);
        memory.slots.insert(wanted, 999);
        let mem = memory.accounts.entry(999).or_default();
        mem.slot = Some(wanted);
        mem.lease_expires = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(memory.lease(8, TTL), Some(slot_address(wanted)));
        assert_eq!(memory.accounts[&999].slot, None);
    }

    #[test]
    fn a_connected_account_never_loses_its_lease() {
        let mut memory = Memory::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        memory.conns.insert(
            1,
            Conn {
                account: 5,
                serial: 1,
                tx,
            },
        );
        memory.accounts.entry(5).or_default().devices.insert(1);
        memory.lease(5, TTL).unwrap();
        assert_eq!(memory.accounts[&5].lease_expires, None);
        memory.sweep(Instant::now() + Duration::from_secs(3600));
        assert!(memory.accounts.contains_key(&5));
    }
}
