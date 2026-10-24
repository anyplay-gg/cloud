//! The shared state of a running cloud and the operations that touch memory and database together.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyplay_api::{Event, close};
use anyplay_ticket::{Signer, Verifier};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::config::{Config, Tuning};
use crate::error::AppError;
use crate::hub::{Conn, Memory, Push};
use crate::limits::RateLimiter;
use crate::mail::Mailer;
use crate::{db, util};

pub type AppState = Arc<Inner>;

/// A sign-in code waiting to be entered.
pub struct Challenge {
    pub email: String,
    pub code: String,
    pub tries: u8,
    pub deadline: Instant,
}

/// Permission to create an account with a verified address.
pub struct SignupToken {
    pub email: String,
    pub deadline: Instant,
}

#[derive(Default)]
pub struct Pending {
    pub challenges: HashMap<String, Challenge>,
    pub signups: HashMap<String, SignupToken>,
}

/// What every request checks: which devices are signed in and which accounts are banned.
#[derive(Default)]
pub struct AuthCache {
    /// Device to account.
    pub devices: HashMap<u64, u64>,
    /// Account to the end of the ban (`None`: for good).
    pub bans: HashMap<u64, Option<u64>>,
}

pub struct Inner {
    pub config: Config,
    pub tuning: Tuning,
    pub db: db::Pool,
    pub signer: Signer,
    pub verifier: Verifier,
    pub mailer: Arc<dyn Mailer>,
    pub limiter: RateLimiter,
    pub pending: Mutex<Pending>,
    pub cache: RwLock<AuthCache>,
    pub hub: tokio::sync::Mutex<Memory>,
    /// Serializes changes to friendships and blocks, which check limits before they write.
    pub social: tokio::sync::Mutex<()>,
}

impl Inner {
    /// Fails when the device is signed out or the account is banned.
    pub fn check_device(&self, device: u64, account: u64) -> Result<(), AppError> {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        if cache.devices.get(&device) != Some(&account) {
            return Err(AppError::unauthorized(
                "This device is signed out. Sign in again.",
            ));
        }
        if let Some(until) = cache.bans.get(&account).copied()
            && until.is_none_or(|t| t > util::now())
        {
            return Err(AppError::banned(until));
        }
        Ok(())
    }

    /// Some(until) while the account is banned.
    pub fn ban_of(&self, account: u64) -> Option<Option<u64>> {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        cache
            .bans
            .get(&account)
            .copied()
            .filter(|until| until.is_none_or(|t| t > util::now()))
    }

    pub fn cache_device(&self, device: u64, account: u64) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.devices.insert(device, account);
    }

    pub fn uncache_device(&self, device: u64) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.devices.remove(&device);
    }

    // Event connections

    /// Registers an event connection of a device, replacing an older one. The receiver yields the
    /// `welcome` event first.
    pub async fn connect(
        self: &Arc<Self>,
        account: u64,
        device: u64,
    ) -> (UnboundedReceiver<Push>, u64) {
        let (tx, rx) = unbounded_channel();
        let (serial, was_online) = {
            let mut hub = self.hub.lock().await;
            hub.next_serial += 1;
            let serial = hub.next_serial;
            if let Some(old) = hub.conns.remove(&device) {
                let _ = old.tx.send(Push::Close {
                    code: 1000,
                    reason: "replaced by a newer connection",
                });
            }
            hub.conns.insert(
                device,
                Conn {
                    account,
                    serial,
                    tx: tx.clone(),
                },
            );
            let mem = hub.accounts.entry(account).or_default();
            mem.devices.insert(device);
            mem.generation += 1;
            let was_online = mem.online;
            mem.online = true;
            match hub.lease(account, self.tuning.lease_ttl) {
                Some(address) => {
                    let _ = tx.send(Push::Event(Event::Welcome {
                        address: address.to_string(),
                        server_time: util::now(),
                    }));
                }
                None => {
                    tracing::error!("no virtual address is free");
                    let _ = tx.send(Push::Close {
                        code: 1013,
                        reason: "no virtual address is free",
                    });
                }
            }
            (serial, was_online)
        };
        if !was_online {
            self.refresh_presence([account]).await;
        }
        (rx, serial)
    }

    /// Called when an event connection ends. The account stays online for a grace period.
    pub async fn disconnect(self: &Arc<Self>, account: u64, device: u64, serial: u64) {
        let generation = {
            let mut hub = self.hub.lock().await;
            match hub.conns.get(&device) {
                Some(conn) if conn.serial == serial => {}
                _ => return,
            }
            hub.conns.remove(&device);
            let Some(mem) = hub.accounts.get_mut(&account) else {
                return;
            };
            mem.devices.remove(&device);
            if !mem.devices.is_empty() {
                return;
            }
            mem.generation += 1;
            mem.generation
        };
        self.schedule_grace(account, generation);
    }

    /// Starts the two timers after which an account without a connection goes offline and leaves
    /// its sessions. A new connection (or a later call) changes the generation and cancels them.
    pub fn schedule_grace(self: &Arc<Self>, account: u64, generation: u64) {
        let weak = Arc::downgrade(self);
        let offline = self.tuning.offline_grace;
        let sessions = self.tuning.session_grace;
        tokio::spawn(async move {
            tokio::time::sleep(offline).await;
            let Some(state) = weak.upgrade() else { return };
            state.go_offline(account, generation).await;
            drop(state);
            tokio::time::sleep(sessions.saturating_sub(offline)).await;
            let Some(state) = weak.upgrade() else { return };
            state.leave_sessions(account, generation).await;
        });
    }

    /// Makes sure an account that is in a session without a connection leaves it in time.
    pub fn arm_grace(self: &Arc<Self>, hub: &mut Memory, account: u64) {
        if hub.is_connected(account) {
            return;
        }
        let mem = hub.accounts.entry(account).or_default();
        mem.generation += 1;
        self.schedule_grace(account, mem.generation);
    }

    async fn go_offline(self: &Arc<Self>, account: u64, generation: u64) {
        {
            let mut hub = self.hub.lock().await;
            let due = hub
                .accounts
                .get(&account)
                .is_some_and(|m| m.generation == generation && m.devices.is_empty() && m.online);
            if !due {
                return;
            }
            hub.offline(account, self.tuning.lease_ttl);
        }
        self.refresh_presence([account]).await;
    }

    async fn leave_sessions(self: &Arc<Self>, account: u64, generation: u64) {
        let mut dirty = Vec::new();
        {
            let mut hub = self.hub.lock().await;
            let due = hub
                .accounts
                .get(&account)
                .is_some_and(|m| m.generation == generation && m.devices.is_empty());
            if !due {
                return;
            }
            hub.leave_everything(account, &mut dirty);
        }
        self.refresh_presence(dirty).await;
    }

    // Presence

    /// Tells the online friends of each account whose presence changed since they were last told.
    pub async fn refresh_presence(&self, accounts: impl IntoIterator<Item = u64>) {
        let mut seen = HashSet::new();
        for account in accounts {
            if !seen.insert(account) {
                continue;
            }
            let mut hub = self.hub.lock().await;
            let presence = hub.presence_of(account);
            let previous = hub
                .accounts
                .get(&account)
                .and_then(|m| m.sent.clone())
                .unwrap_or_default();
            if previous == presence {
                continue;
            }
            if let Some(mem) = hub.accounts.get_mut(&account) {
                mem.sent = Some(presence.clone());
            }
            let friends = match db::friend_ids(&self.db, account).await {
                Ok(friends) => friends,
                Err(error) => {
                    tracing::error!(%error, "cannot list friends for a presence change");
                    continue;
                }
            };
            for friend in friends {
                hub.push(
                    friend,
                    Event::Presence {
                        account: anyplay_api::AccountId(account),
                        presence: presence.clone(),
                    },
                );
            }
        }
    }

    // Devices, bans, deletion

    /// Signs a device out: forgets it and closes its event connection.
    pub async fn sign_out_device(&self, device: u64) -> Result<(), AppError> {
        db::delete_device(&self.db, device).await?;
        self.uncache_device(device);
        let hub = self.hub.lock().await;
        hub.close_device(
            device,
            Some(Event::SignedOut),
            close::SIGNED_OUT,
            "signed out",
        );
        Ok(())
    }

    /// Applies a ban to what is running: closes connections and takes the account out of its
    /// sessions. The database row is written by the caller.
    pub async fn apply_ban(&self, account: u64, until: Option<u64>) {
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.bans.insert(account, until);
        }
        let mut dirty = vec![account];
        {
            let mut hub = self.hub.lock().await;
            hub.close_account(account, Event::Banned { until }, close::BANNED, "banned");
            hub.leave_everything(account, &mut dirty);
            hub.offline(account, self.tuning.lease_ttl);
        }
        self.refresh_presence(dirty).await;
    }

    pub fn lift_ban(&self, account: u64) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.bans.remove(&account);
    }

    /// Deletes an account everywhere.
    pub async fn delete_account(&self, account: u64) -> Result<(), AppError> {
        let friends = db::friend_ids(&self.db, account).await?;
        let devices = db::devices_of(&self.db, account).await?;
        db::delete_account(&self.db, account).await?;
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            for device in &devices {
                cache.devices.remove(&device.id());
            }
        }
        let mut dirty = Vec::new();
        let mut hub = self.hub.lock().await;
        hub.close_account(
            account,
            Event::SignedOut,
            close::SIGNED_OUT,
            "account deleted",
        );
        hub.purge_account(account, &mut dirty);
        for friend in friends {
            hub.push(
                friend,
                Event::Presence {
                    account: anyplay_api::AccountId(account),
                    presence: anyplay_api::Presence::default(),
                },
            );
            hub.push(friend, Event::Friends);
        }
        drop(hub);
        self.refresh_presence(dirty).await;
        Ok(())
    }

    /// The address leased to an account, leasing one if needed.
    pub async fn address_of(&self, account: u64) -> Option<Ipv4Addr> {
        self.hub.lock().await.lease(account, self.tuning.lease_ttl)
    }

    /// Asks every event connection to close, for shutdown.
    pub async fn close_all(&self) {
        let hub = self.hub.lock().await;
        for conn in hub.conns.values() {
            let _ = conn.tx.send(Push::Close {
                code: 1001,
                reason: "going away",
            });
        }
    }

    /// Forgets what has expired. Runs on a timer.
    pub async fn sweep(&self) {
        let now = Instant::now();
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.challenges.retain(|_, c| c.deadline > now);
            pending.signups.retain(|_, s| s.deadline > now);
        }
        self.limiter.sweep();
        self.hub.lock().await.sweep(now);
    }
}

/// Runs [`Inner::sweep`] until the state is dropped.
pub fn start_sweeper(state: &AppState) {
    let weak = Arc::downgrade(state);
    let every: Duration = state.tuning.sweep_interval;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            let Some(state) = weak.upgrade() else { return };
            state.sweep().await;
        }
    });
}
