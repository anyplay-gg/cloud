//! The shared state of a running cloud and the operations that change live state and send the
//! news about it. Live state is in the store ([`live`]); this node keeps only its own event
//! connections ([`News`]) and caches it can fill again.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyplay_api::{Event, close};
use anyplay_ticket::{Signer, Verifier};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::config::{Config, Tuning};
use crate::error::AppError;
use crate::limits::RateLimiter;
use crate::live::{self, Gone, SessionState};
use crate::mail::Mailer;
use crate::news::{AuthNote, News, Push};
use crate::{db, util};

pub type AppState = Arc<Inner>;

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
    pub cache: RwLock<AuthCache>,
    pub news: News,
    /// This process in the store's `nodes` table, new at every start.
    pub node: String,
    /// Serializes changes to friendships and blocks, which check limits before they write.
    pub social: tokio::sync::Mutex<()>,
}

fn ms(duration: Duration) -> i64 {
    duration.as_millis() as i64
}

impl Inner {
    /// Fails when the device is signed out or the account is banned. A device this node has
    /// not seen yet, because it signed in through another node, is looked up in the store.
    pub async fn check_device(&self, device: u64, account: u64) -> Result<(), AppError> {
        let known = {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            cache.devices.get(&device).copied()
        };
        let owner = match known {
            Some(owner) => Some(owner),
            None => {
                let owner = db::device_owner(&self.db, device).await?;
                if let Some(owner) = owner {
                    self.cache_device(device, owner);
                }
                owner
            }
        };
        if owner != Some(account) {
            return Err(AppError::unauthorized(
                "This device is signed out. Sign in again.",
            ));
        }
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
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

    /// When the lease of an account that goes offline now runs out.
    fn lease_end(&self) -> i64 {
        util::now_ms() + ms(self.tuning.lease_ttl)
    }

    // Event connections

    /// Registers an event connection of a device, replacing an older one. The receiver yields
    /// the `welcome` event first, or a close when the cloud cannot take the connection.
    pub async fn connect(
        self: &Arc<Self>,
        account: u64,
        device: u64,
    ) -> (UnboundedReceiver<Push>, u64) {
        let serial = self.news.next_serial();
        let first = match self.open_connection(account, device, serial).await {
            Ok(Some(address)) => Push::Event(Event::Welcome {
                address: address.to_string(),
                server_time: util::now(),
            }),
            Ok(None) => {
                tracing::error!("no virtual address is free");
                Push::Close {
                    code: 1013,
                    reason: "no virtual address is free".into(),
                }
            }
            Err(error) => {
                tracing::error!(error = %error.body.message, "cannot record an event connection");
                Push::Close {
                    code: 1011,
                    reason: "the cloud cannot reach its store".into(),
                }
            }
        };
        let welcomed = matches!(first, Push::Event(_));
        let pushes = self.news.attach(account, device, serial, first).await;
        if welcomed {
            self.refresh_presence([account]).await;
        }
        (pushes, serial)
    }

    async fn open_connection(
        &self,
        account: u64,
        device: u64,
        serial: u64,
    ) -> Result<Option<Ipv4Addr>, AppError> {
        let now = util::now_ms();
        let mut conn = self.db.acquire().await?;
        live::open_connection(&mut conn, account, device, &self.node, serial, now).await?;
        Ok(live::lease(&mut conn, account, self.lease_end(), now).await?)
    }

    /// Called when an event connection ends. The account stays online for a grace period; the
    /// sweep takes it offline after that.
    pub async fn disconnect(&self, account: u64, device: u64, serial: u64) {
        if !self.news.detach(device, serial).await {
            return;
        }
        let closed = async {
            let mut conn = self.db.acquire().await?;
            live::close_connection(
                &mut conn,
                account,
                device,
                &self.node,
                serial,
                util::now_ms(),
            )
            .await
        };
        if let Err(error) = closed.await {
            // Once this node's heartbeat stops, another sweep closes what this one could not.
            tracing::error!(%error, "cannot forget an event connection");
        }
    }

    // Presence

    /// Tells the friends of each account whose presence changed since they were last told.
    pub async fn refresh_presence(&self, accounts: impl IntoIterator<Item = u64>) {
        let mut seen = HashSet::new();
        for account in accounts {
            if !seen.insert(account) {
                continue;
            }
            if let Err(error) = self.refresh_one(account).await {
                tracing::error!(error = %error.body.message, "cannot tell friends about a presence change");
            }
        }
    }

    async fn refresh_one(&self, account: u64) -> Result<(), AppError> {
        let presence = {
            let mut conn = self.db.acquire().await?;
            let mut presence = live::presence_of(&mut conn, account).await?;
            let Some(revision) = live::mark_sent(&mut conn, account, &presence).await? else {
                return Ok(());
            };
            presence.revision = revision;
            presence
        };
        let event = Event::Presence {
            account: anyplay_api::AccountId(account),
            presence,
        };
        for friend in db::friend_ids(&self.db, account).await? {
            self.news.push(friend, &event);
        }
        Ok(())
    }

    // Sessions and invitations

    /// Sends a session as it is now to all its members.
    pub fn announce(&self, session: &SessionState) {
        let event = Event::Session {
            session: session.view(),
        };
        for member in session.member_ids() {
            self.news.push(member, &event);
        }
    }

    /// Tells the recipients of invitations that ended.
    pub fn invitations_gone(&self, gone: &[Gone]) {
        for invitation in gone {
            self.news.push(
                invitation.to(),
                &Event::InvitationGone {
                    id: invitation.id.clone(),
                },
            );
        }
    }

    /// Takes a member out of a session. A leaving leader hands over to the longest-standing
    /// member; the last member closes the session. Returns the accounts whose presence may have
    /// changed.
    pub async fn remove_member(&self, id: &str, account: u64) -> Result<Vec<u64>, AppError> {
        let mut tx = self.db.begin().await?;
        let Some(mut session) = live::lock_session(&mut tx, id).await? else {
            return Ok(Vec::new());
        };
        if !session.is_member(account) {
            return Ok(Vec::new());
        }
        live::delete_member(&mut tx, id, account).await?;
        session.members.retain(|m| m.account_id as u64 != account);
        let gone = match session.members.first() {
            None => {
                let gone = live::take_of_session(&mut tx, id).await?;
                live::delete_session(&mut tx, id).await?;
                gone
            }
            Some(first) => {
                if session.leader() == account {
                    session.row.leader = first.account_id;
                    live::update_session(&mut tx, &session.row).await?;
                }
                Vec::new()
            }
        };
        tx.commit().await?;

        self.news
            .push(account, &Event::SessionGone { id: id.to_owned() });
        self.invitations_gone(&gone);
        let mut dirty = vec![account];
        if !session.members.is_empty() {
            self.announce(&session);
            dirty.extend(session.member_ids());
        }
        Ok(dirty)
    }

    /// Takes an account out of every session and drops the invitations it sent or holds.
    /// Returns the accounts whose presence may have changed.
    pub async fn leave_everything(&self, account: u64) -> Result<Vec<u64>, AppError> {
        let sessions = live::sessions_of(&mut *self.db.acquire().await?, account).await?;
        let mut dirty = Vec::new();
        for id in sessions {
            dirty.extend(self.remove_member(&id, account).await?);
        }
        let gone = live::take_of_account(&mut *self.db.acquire().await?, account).await?;
        self.invitations_gone(&gone);
        Ok(dirty)
    }

    // Devices, bans, deletion

    /// Signs a device out: forgets it and closes its event connection, wherever it is.
    pub async fn sign_out_device(&self, device: u64) -> Result<(), AppError> {
        let owner = db::device_owner(&self.db, device).await?;
        db::delete_device(&self.db, device).await?;
        self.uncache_device(device);
        self.news.tell_nodes(&AuthNote::DeviceGone { device });
        if let Some(account) = owner {
            self.news.close_device(
                account,
                device,
                Some(Event::SignedOut),
                close::SIGNED_OUT,
                "signed out",
            );
        }
        Ok(())
    }

    /// Applies what another node changed about devices and bans.
    pub fn apply_note(&self, note: AuthNote) {
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        match note {
            AuthNote::DeviceGone { device } => {
                cache.devices.remove(&device);
            }
            AuthNote::Banned { account, until } => {
                cache.bans.insert(account, until);
            }
            AuthNote::Unbanned { account } => {
                cache.bans.remove(&account);
            }
        }
    }

    /// Applies a ban to what is running: closes connections and takes the account out of its
    /// sessions. The database row is written by the caller.
    pub async fn apply_ban(&self, account: u64, until: Option<u64>) -> Result<(), AppError> {
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.bans.insert(account, until);
        }
        self.news.tell_nodes(&AuthNote::Banned { account, until });
        self.news
            .close_account(account, &Event::Banned { until }, close::BANNED, "banned");
        let mut dirty = self.leave_everything(account).await?;
        live::set_offline(
            &mut *self.db.acquire().await?,
            account,
            util::now_ms(),
            self.lease_end(),
        )
        .await?;
        dirty.push(account);
        self.refresh_presence(dirty).await;
        Ok(())
    }

    pub fn lift_ban(&self, account: u64) {
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.bans.remove(&account);
        }
        self.news.tell_nodes(&AuthNote::Unbanned { account });
    }

    /// Deletes an account everywhere.
    pub async fn delete_account(&self, account: u64) -> Result<(), AppError> {
        let friends = db::friend_ids(&self.db, account).await?;
        let devices = db::devices_of(&self.db, account).await?;
        // Sessions and invitations first, while their members can still be told.
        let dirty = self.leave_everything(account).await?;
        self.news.close_account(
            account,
            &Event::SignedOut,
            close::SIGNED_OUT,
            "account deleted",
        );
        db::delete_account(&self.db, account).await?;
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            for device in &devices {
                cache.devices.remove(&device.id());
            }
        }
        for device in &devices {
            self.news.tell_nodes(&AuthNote::DeviceGone {
                device: device.id(),
            });
        }
        let offline = Event::Presence {
            account: anyplay_api::AccountId(account),
            presence: anyplay_api::Presence::default(),
        };
        for friend in friends {
            self.news.push(friend, &offline);
            self.news.push(friend, &Event::Friends);
        }
        self.refresh_presence(dirty.into_iter().filter(|a| *a != account))
            .await;
        Ok(())
    }

    /// The address leased to an account, leasing one if needed.
    pub async fn address_of(&self, account: u64) -> Result<Option<Ipv4Addr>, AppError> {
        let mut conn = self.db.acquire().await?;
        Ok(live::lease(&mut conn, account, self.lease_end(), util::now_ms()).await?)
    }

    /// Asks every event connection of this node to close, for shutdown.
    pub fn close_all(&self) {
        self.news.close_all(1001, "going away");
    }

    /// Shows that this node is alive and carries out what is due: the connections of dead nodes,
    /// the grace periods, and invitations, challenges and leases that ran out. Every node runs it.
    pub async fn sweep(&self) -> Result<(), AppError> {
        self.limiter.sweep();
        self.reload_bans().await?;
        let now = util::now_ms();
        let (offline, gone, leaving) = {
            let mut conn = self.db.acquire().await?;
            live::heartbeat(&mut conn, &self.node, now).await?;
            live::sweep_dead_nodes(&mut conn, now - ms(self.tuning.node_timeout)).await?;
            let offline = live::sweep_offline(
                &mut conn,
                now - ms(self.tuning.offline_grace),
                self.lease_end(),
            )
            .await?;
            let gone = live::sweep_expired(&mut conn, now).await?;
            let leaving =
                live::due_to_leave(&mut conn, now - ms(self.tuning.session_grace)).await?;
            (offline, gone, leaving)
        };
        self.invitations_gone(&gone);
        let mut dirty = offline;
        for account in leaving {
            dirty.extend(self.leave_everything(account).await?);
        }
        self.refresh_presence(dirty).await;
        Ok(())
    }
}

impl Inner {
    /// Reads the bans that apply now, so that a ban another node did not manage to tell still
    /// holds here.
    async fn reload_bans(&self) -> Result<(), AppError> {
        let now = util::now();
        let bans: HashMap<u64, Option<u64>> = db::all_bans(&self.db)
            .await?
            .into_iter()
            .filter(|ban| ban.active(now))
            .map(|ban| (ban.account_id as u64, ban.until()))
            .collect();
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.bans = bans;
        Ok(())
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
            if let Err(error) = state.sweep().await {
                tracing::error!(error = %error.body.message, "the sweep failed; it runs again soon");
            }
        }
    });
}
