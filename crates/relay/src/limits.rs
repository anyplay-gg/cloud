//! What a relay holds a client to besides its ticket: how many clients it carries, how many
//! connections one account may hold, how long a ticket may live. And the counters that say how
//! often each check passed or refused.
//!
//! The rate at which one client may send is iroh-relay's own limiter, set in [`crate::config`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use iroh_relay::server::ConnectionId;

/// The limits of [`Admission`]. `0` turns a limit off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionLimits {
    /// Connections this relay carries at once.
    pub max_clients: usize,
    /// Connections one account may hold at once, over all its devices.
    pub max_connections_per_account: usize,
    /// The longest time a ticket may still be valid for, in seconds from now. The cloud issues
    /// tickets for 24 hours; a ticket for much longer did not come from a cloud that follows the
    /// specification.
    pub max_ticket_lifetime_secs: u64,
}

impl AdmissionLimits {
    pub const DEFAULT_MAX_CLIENTS: usize = 10_000;
    pub const DEFAULT_MAX_CONNECTIONS_PER_ACCOUNT: usize = 16;
    /// 24 hours and one hour of slack.
    pub const DEFAULT_MAX_TICKET_LIFETIME_SECS: u64 = 25 * 60 * 60;
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            max_clients: Self::DEFAULT_MAX_CLIENTS,
            max_connections_per_account: Self::DEFAULT_MAX_CONNECTIONS_PER_ACCOUNT,
            max_ticket_lifetime_secs: Self::DEFAULT_MAX_TICKET_LIFETIME_SECS,
        }
    }
}

/// What the ticket check made of a connection attempt. One is counted per attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketResult {
    Ok,
    /// The client sent no ticket.
    Missing,
    Malformed,
    /// Forged, altered or signed by another cloud.
    Signature,
    /// An access token, not a relay ticket.
    Kind,
    Expired,
    /// A good ticket for another endpoint.
    Endpoint,
    /// A good ticket that is valid for longer than this relay accepts.
    Lifetime,
}

impl TicketResult {
    pub const ALL: [TicketResult; 8] = [
        TicketResult::Ok,
        TicketResult::Missing,
        TicketResult::Malformed,
        TicketResult::Signature,
        TicketResult::Kind,
        TicketResult::Expired,
        TicketResult::Endpoint,
        TicketResult::Lifetime,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TicketResult::Ok => "ok",
            TicketResult::Missing => "missing",
            TicketResult::Malformed => "malformed",
            TicketResult::Signature => "signature",
            TicketResult::Kind => "kind",
            TicketResult::Expired => "expired",
            TicketResult::Endpoint => "endpoint",
            TicketResult::Lifetime => "lifetime",
        }
    }
}

/// Why a connection was turned away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The ticket was missing or bad; [`TicketResult`] says how.
    Ticket,
    /// The account holds as many connections as it may.
    AccountLimit,
    /// The relay carries as many clients as it may.
    RelayFull,
}

impl Refusal {
    pub const ALL: [Refusal; 3] = [Refusal::Ticket, Refusal::AccountLimit, Refusal::RelayFull];

    pub fn name(self) -> &'static str {
        match self {
            Refusal::Ticket => "ticket",
            Refusal::AccountLimit => "account_limit",
            Refusal::RelayFull => "relay_full",
        }
    }
}

/// Counters of one relay. They carry no account or endpoint IDs.
#[derive(Debug, Default)]
pub struct Stats {
    checks: [AtomicU64; TicketResult::ALL.len()],
    refusals: [AtomicU64; Refusal::ALL.len()],
}

impl Stats {
    pub fn check(&self, result: TicketResult) {
        self.checks[result as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn refuse(&self, refusal: Refusal) {
        self.refusals[refusal as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn checks(&self, result: TicketResult) -> u64 {
        self.checks[result as usize].load(Ordering::Relaxed)
    }

    pub fn refusals(&self, refusal: Refusal) -> u64 {
        self.refusals[refusal as usize].load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct Held {
    /// The account of each admitted connection, until it ends.
    connections: HashMap<ConnectionId, u64>,
    per_account: HashMap<u64, usize>,
}

/// Counts the admitted connections, in total and per account, and refuses past the limits.
#[derive(Debug)]
pub struct Admission {
    limits: AdmissionLimits,
    held: Mutex<Held>,
}

impl Admission {
    pub fn new(limits: AdmissionLimits) -> Self {
        Self {
            limits,
            held: Mutex::default(),
        }
    }

    pub fn limits(&self) -> AdmissionLimits {
        self.limits
    }

    /// Takes a place for a connection of `account`, or says why there is none. A connection that
    /// was admitted gives its place back with [`Admission::release`].
    pub fn admit(&self, connection: ConnectionId, account: u64) -> Result<(), Refusal> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let max = self.limits.max_clients;
        if max != 0 && held.connections.len() >= max {
            return Err(Refusal::RelayFull);
        }
        let max = self.limits.max_connections_per_account;
        let count = held.per_account.get(&account).copied().unwrap_or(0);
        if max != 0 && count >= max {
            return Err(Refusal::AccountLimit);
        }
        if held.connections.insert(connection, account).is_none() {
            *held.per_account.entry(account).or_insert(0) += 1;
        }
        Ok(())
    }

    /// Gives back the place of a connection that ended.
    pub fn release(&self, connection: ConnectionId) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let Some(account) = held.connections.remove(&connection) else {
            return;
        };
        if let Some(count) = held.per_account.get_mut(&account) {
            *count -= 1;
            if *count == 0 {
                held.per_account.remove(&account);
            }
        }
    }

    /// Connections the relay carries now.
    pub fn clients(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .connections
            .len()
    }

    /// Accounts that hold a connection now.
    pub fn accounts(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .per_account
            .len()
    }
}
