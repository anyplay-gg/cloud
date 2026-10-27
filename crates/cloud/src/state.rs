//! The shared state of a running cloud and the operations that change live state and send the
//! news about it. Live state is in the store ([`live`]); this node keeps only its own event
//! connections ([`News`]) and caches it can fill again.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyplay_api::{Event, close};
use anyplay_ticket::{Signer, Verifier};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::addresses;
use crate::config::{Config, Tuning};
use crate::error::AppError;
use crate::limits::RateLimiter;
use crate::live::{self, Gone, SessionState};
use crate::mail::Mailer;
use crate::metrics::Metrics;
use crate::news::{AuthNote, News, Push};
use crate::{db, util};

pub use crate::db::WriteTx;

pub type AppState = Arc<Inner>;

/// How often a node deletes moderation data past its retention.
const RETENTION_EVERY_SECS: u64 = 60 * 60;

/// What every request checks: which devices are signed in and which accounts are banned.
#[derive(Default)]
pub struct AuthCache {
    /// Device to account.
    pub devices: HashMap<u64, u64>,
    /// Account to the end of the ban (`None`: for good).
    pub bans: HashMap<u64, Option<u64>>,
    /// Account to the time before which its access tokens are refused.
    pub valid_from: HashMap<u64, u64>,
}

/// What the rarity of a title's trophies counted to: the players and, by trophy number, how many
/// earned it.
pub type RarityCounts = Arc<(i64, Vec<(i64, i64)>)>;

/// The most titles whose rarity a node holds.
const RARITY_TITLES: usize = 256;

/// The rarity counts of titles, each kept for a short time so that a popular title is counted
/// once in a while and not once per request. Other nodes are not told: the time alone keeps it
/// right.
#[derive(Default)]
pub struct RarityCache {
    titles: std::sync::Mutex<HashMap<String, (Instant, RarityCounts)>>,
}

impl RarityCache {
    /// The counts for `title` that are younger than `ttl`.
    pub fn get(&self, title: &str, ttl: Duration) -> Option<RarityCounts> {
        let titles = self.titles.lock().unwrap_or_else(|e| e.into_inner());
        let (at, counts) = titles.get(title)?;
        (at.elapsed() < ttl).then(|| Arc::clone(counts))
    }

    /// Keeps the counts for `title`. Past [`RARITY_TITLES`] titles the stale ones go, and when
    /// none is stale the table starts again.
    pub fn put(&self, title: &str, ttl: Duration, counts: RarityCounts) {
        let mut titles = self.titles.lock().unwrap_or_else(|e| e.into_inner());
        if titles.len() >= RARITY_TITLES && !titles.contains_key(title) {
            titles.retain(|_, (at, _)| at.elapsed() < ttl);
            if titles.len() >= RARITY_TITLES {
                titles.clear();
            }
        }
        titles.insert(title.to_owned(), (Instant::now(), counts));
    }
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
    /// Trophy rarity per title, for `tuning.rarity_ttl`.
    pub rarity: RarityCache,
    pub news: News,
    /// The account web, when the cloud has a public address.
    pub web: Option<crate::web::Web>,
    /// This process in the store's `nodes` table, new at every start.
    pub node: String,
    pub metrics: Metrics,
    /// Set when the node is shutting down: it reports itself not ready, so that the load
    /// balancer stops sending players, before it closes their connections.
    pub draining: AtomicBool,
    /// The key of the hash of banned e-mail addresses.
    pub ban_key: crate::moderation::banhash::BanKey,
    /// When this node last ran the retention sweep of moderation data, in Unix seconds.
    pub retention_at: std::sync::atomic::AtomicU64,
    /// Starts the transactions of [`Inner::begin_write`].
    pub writer: db::Writer,
    /// Web Push, when the cloud has a VAPID key.
    pub push: Option<crate::push::Pusher>,
}

fn ms(duration: Duration) -> i64 {
    duration.as_millis() as i64
}

impl Inner {
    /// Starts a transaction that will write; see [`db::Writer`].
    pub async fn begin_write(&self) -> sqlx::Result<WriteTx> {
        self.writer.begin(&self.db).await
    }

    /// Fails when the device is signed out or the account is banned. A device this node has
    /// not seen yet, because it signed in through another node, is looked up in the store.
    pub async fn check_device(
        &self,
        device: u64,
        account: u64,
        issued: u64,
    ) -> Result<(), AppError> {
        self.check_device_owner(device, account, issued).await?;
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        if let Some(until) = cache.bans.get(&account).copied()
            && until.is_none_or(|t| t > util::now())
        {
            return Err(AppError::banned(until));
        }
        Ok(())
    }

    /// Fails when the device is signed out, belongs to another account or holds a token from
    /// before the account signed out everywhere. A ban does not matter here: a banned person may
    /// still read their statements and appeal.
    pub async fn check_device_owner(
        &self,
        device: u64,
        account: u64,
        issued: u64,
    ) -> Result<(), AppError> {
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
        if cache
            .valid_from
            .get(&account)
            .is_some_and(|from| issued < *from)
        {
            return Err(AppError::unauthorized(
                "This token was issued before the account signed out everywhere. Sign in again.",
            ));
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

    /// Registers an event connection of a device, replacing an older one. The receiver yields
    /// the `welcome` event first, or a close when the cloud cannot take the connection.
    pub async fn connect(
        self: &Arc<Self>,
        account: u64,
        device: u64,
    ) -> (UnboundedReceiver<Push>, u64) {
        let serial = self.news.next_serial();
        let first = match self.open_connection(account, device, serial).await {
            Ok(()) => Push::Event(Event::Welcome {
                address: addresses::preferred(account).to_string(),
                server_time: util::now(),
            }),
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
    ) -> Result<(), AppError> {
        let mut conn = self.db.acquire().await?;
        live::open_connection(
            &mut conn,
            account,
            device,
            &self.node,
            serial,
            util::now_ms(),
        )
        .await?;
        Ok(())
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

    /// Sets whether `account` appears offline, tells its friends their view changed and its own
    /// devices the new setting.
    pub async fn set_appear_offline(&self, account: u64, on: bool) -> Result<(), AppError> {
        if !db::set_appear_offline(&self.db, account, on).await? {
            return Err(AppError::not_found());
        }
        self.refresh_presence([account]).await;
        self.news
            .push(account, &Event::AppearOffline { appear_offline: on });
        Ok(())
    }

    /// Reads what the friends of `account` see and marks it sent in one write transaction that
    /// holds the account's presence: two refreshes at once (the game sets its activity while its
    /// session opens) then run one after the other, and the later one reads what the earlier one
    /// wrote. Read and marked apart, a refresh that read before a change could mark its older
    /// presence after one that read after it, and the friends kept the older one with the higher
    /// revision.
    ///
    /// An account without a presence row has nothing to tell yet: its first connection writes the
    /// row and refreshes. On PostgreSQL a refresh without a row must stop there, since nothing is
    /// locked: the row could appear between its read and its mark, and the mark would write the
    /// offline presence it read over the one the first refresh told.
    async fn refresh_one(&self, account: u64) -> Result<(), AppError> {
        let presence = {
            let mut tx = self.begin_write().await?;
            if self.config.database_url.is_some() && !live::lock_presence(&mut tx, account).await? {
                tx.commit().await?;
                return Ok(());
            }
            let mut presence = live::presence_of(&mut tx, account).await?;
            let Some(revision) = live::mark_sent(&mut tx, account, &presence).await? else {
                tx.commit().await?;
                return Ok(());
            };
            tx.commit().await?;
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
        let mut tx = self.begin_write().await?;
        let Some(mut session) = live::lock_session(&mut tx, id).await? else {
            return Ok(Vec::new());
        };
        if !session.is_member(account) {
            return Ok(Vec::new());
        }
        live::delete_member(&mut tx, id, account).await?;
        session.members.retain(|m| m.account_id as u64 != account);
        // A Play Together session is its host's world: it ends with the host (network ADR 0030).
        let ends_with_host =
            session.kind() == anyplay_api::SessionKind::Lan && session.leader() == account;
        let gone = if session.members.is_empty() || ends_with_host {
            let gone = live::take_of_session(&mut tx, id).await?;
            live::delete_session(&mut tx, id).await?;
            gone
        } else {
            if session.leader() == account {
                session.row.leader = session.members[0].account_id;
                live::update_session(&mut tx, &session.row).await?;
            }
            Vec::new()
        };
        tx.commit().await?;

        self.news
            .push(account, &Event::SessionGone { id: id.to_owned() });
        self.invitations_gone(&gone);
        let mut dirty = vec![account];
        if ends_with_host {
            for member in session.member_ids() {
                self.news
                    .push(member, &Event::SessionGone { id: id.to_owned() });
                dirty.push(member);
            }
        } else if !session.members.is_empty() {
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
        self.leave_party(account).await?;
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
            AuthNote::NotBefore { account, at } => {
                let entry = cache.valid_from.entry(account).or_insert(0);
                *entry = (*entry).max(at);
            }
        }
    }

    /// Signs the account out of every device and every browser session, and refuses the access
    /// tokens it has issued so far: the "not before" time is now.
    pub async fn sign_out_everywhere(&self, account: u64) -> Result<(), AppError> {
        self.refuse_tokens_until_now(account).await?;
        for device in db::devices_of(&self.db, account).await? {
            self.sign_out_device(device.id()).await?;
        }
        crate::web::store::end_sessions_of(&self.db, account).await?;
        Ok(())
    }

    /// Refuses the access tokens the account has issued so far: the "not before" time is now.
    async fn refuse_tokens_until_now(&self, account: u64) -> Result<(), AppError> {
        let at = util::now();
        crate::binding::set_tokens_valid_from(&self.db, account, at).await?;
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            let entry = cache.valid_from.entry(account).or_insert(0);
            *entry = (*entry).max(at);
        }
        self.news.tell_nodes(&AuthNote::NotBefore { account, at });
        Ok(())
    }

    /// Schedules the deletion of an account for the end of the grace period and signs it out of
    /// every device and, but for `keep` (the browser session that asked, by its hash), every
    /// browser, so that whoever holds a copy of a session or a token cannot go on with it. The
    /// player can cancel until the time. Returns that time (Unix seconds); an account that was
    /// scheduled already keeps its first time.
    pub async fn schedule_deletion(
        &self,
        account: u64,
        keep: Option<&[u8]>,
    ) -> Result<u64, AppError> {
        let at = util::now() + self.tuning.deletion_grace.as_secs();
        let new = db::schedule_deletion(&self.db, account, at).await?;
        self.sign_out_all_but(account, keep).await?;
        let at = db::deletion_of(&self.db, account).await?.unwrap_or(at);
        if new && let Some(row) = db::account(&self.db, account).await? {
            let (subject, body) = crate::mail::deletion_scheduled_message(
                &self.config.name,
                &crate::web::pages::date(at),
                &crate::mail::contact_line(&self.config),
            );
            self.send_notice(&row.email, &subject, &body).await;
        }
        Ok(at)
    }

    /// Sends a notice about an account to an address. A failure is logged and does not stop
    /// what the notice is about.
    pub async fn send_notice(&self, to: &str, subject: &str, body: &str) {
        let sent = crate::mail::within(
            Duration::from_secs(20),
            self.mailer.send_notice(to, subject, body),
        )
        .await;
        if let Err(e) = sent {
            tracing::error!(class = %e.class, status = e.status, reply = e.detail.as_deref(), "cannot send a notice");
        }
    }

    /// Signs the account out of every device, refuses the access tokens issued so far and ends
    /// every browser session but `keep` (a session's hash), the one that asked.
    pub async fn sign_out_all_but(
        &self,
        account: u64,
        keep: Option<&[u8]>,
    ) -> Result<(), AppError> {
        self.refuse_tokens_until_now(account).await?;
        for device in db::devices_of(&self.db, account).await? {
            self.sign_out_device(device.id()).await?;
        }
        for other in crate::web::store::sessions_of(&self.db, account).await? {
            if Some(other.id_hash.as_slice()) != keep {
                crate::web::store::end_session_of(&self.db, account, &other.id_hash).await?;
            }
        }
        Ok(())
    }

    /// Applies a ban to what is running: closes connections and takes the account out of its
    /// sessions. The database row is written by the caller.
    pub async fn apply_ban(
        &self,
        account: u64,
        until: Option<u64>,
        statement: Option<anyplay_api::Statement>,
    ) -> Result<(), AppError> {
        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.bans.insert(account, until);
        }
        self.news.tell_nodes(&AuthNote::Banned { account, until });
        self.news.close_account(
            account,
            &Event::Banned {
                until,
                statement: statement.map(Box::new),
            },
            close::BANNED,
            "banned",
        );
        let mut dirty = self.leave_everything(account).await?;
        live::set_offline(&mut *self.db.acquire().await?, account, util::now_ms()).await?;
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
        self.forget_conversations(account).await?;
        self.forget_media(account).await?;
        self.news.close_account(
            account,
            &Event::SignedOut,
            close::SIGNED_OUT,
            "account deleted",
        );
        db::delete_account(&self.writer, &self.db, account).await?;
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

    /// Asks every event connection of this node to close, for shutdown.
    pub fn close_all(&self) {
        self.news.close_all(1001, "going away");
    }

    /// Shows that this node is alive and carries out what is due: the connections of dead nodes,
    /// the grace periods, and invitations and challenges that ran out. Every node runs it.
    ///
    /// Every step stands alone: one that fails is logged with its name and the others still run,
    /// so a failing step cannot hold up the deletions or the retention. The sweep returns an
    /// error only when the node cannot write its heartbeat, which the metrics count as a failed
    /// sweep.
    pub async fn sweep(&self) -> Result<(), AppError> {
        self.limiter.sweep();
        step("reload bans", self.reload_bans().await);
        let now = util::now_ms();
        let live = async {
            let mut conn = self.db.acquire().await?;
            live::heartbeat(&mut conn, &self.node, now).await?;
            Ok::<_, AppError>(())
        }
        .await;
        let heartbeat_failed = live.is_err();
        step("heartbeat", live);
        let swept = async {
            let mut conn = self.db.acquire().await?;
            live::sweep_dead_nodes(&mut conn, now - ms(self.tuning.node_timeout)).await?;
            let offline =
                live::sweep_offline(&mut conn, now - ms(self.tuning.offline_grace)).await?;
            let gone = live::sweep_expired(&mut conn, now).await?;
            let leaving =
                live::due_to_leave(&mut conn, now - ms(self.tuning.session_grace)).await?;
            Ok::<_, AppError>((offline, gone, leaving))
        }
        .await;
        let (offline, gone, leaving) = step("live sessions", swept).unwrap_or_default();
        self.invitations_gone(&gone);
        step("social", self.sweep_social(now).await);
        self.sweep_deletions().await;
        self.sweep_deletion_ledger(now).await;
        let mut dirty = offline;
        for account in leaving {
            if let Some(more) = step("leave", self.leave_everything(account).await) {
                dirty.extend(more);
            }
        }
        self.refresh_presence(dirty).await;
        self.sweep_moderation().await;
        if heartbeat_failed {
            return Err(AppError::internal());
        }
        Ok(())
    }
}

/// Logs a failed sweep step by name and goes on; the result of one that worked.
fn step<T>(name: &str, result: Result<T, AppError>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(step = name, error = %error.body.message, "a sweep step failed; the others still run");
            None
        }
    }
}

impl Inner {
    /// Deletes the accounts whose grace period after a deletion request has ended. Once the
    /// time has come the player can no longer cancel ([`db::cancel_deletion`]), so a due account
    /// is deleted whatever else happens; two nodes deleting the same account is harmless. An
    /// account that cannot be deleted is tried again in ten minutes, so that a few failing rows
    /// do not hold up the others.
    async fn sweep_deletions(&self) {
        let now = util::now();
        let due = match db::deletions_due(&self.db, now, 20).await {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(%error, "cannot read the accounts due for deletion");
                return;
            }
        };
        for account in due {
            match self.delete_account(account).await {
                Ok(()) => tracing::info!("deleted an account after its grace period"),
                Err(error) => {
                    tracing::error!(
                        class = error.body.error.as_str(),
                        "cannot delete an account; trying again in ten minutes"
                    );
                    if let Err(error) = db::postpone_deletion(&self.db, account, now + 600).await {
                        tracing::error!(%error, "cannot postpone a deletion");
                    }
                }
            }
        }
    }

    /// Forgets the ledger rows of accounts deleted more than [`db::LEDGER_DAYS`] ago, the longest
    /// a backup is kept.
    async fn sweep_deletion_ledger(&self, now_ms: i64) {
        let cut = now_ms.saturating_sub(db::LEDGER_DAYS as i64 * 24 * 60 * 60 * 1000);
        let result = async {
            let mut conn = db::acquire_maintenance(&self.db).await?;
            db::sweep_deleted(&mut conn, cut).await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(step = "deletion ledger", %error, "a sweep step failed; the others still run");
        }
    }

    /// After a restore: has the accounts deleted since the backup deleted again. `extra` is the
    /// ledger of the database that was replaced (account ID and deletion time in milliseconds);
    /// it is added to the ledger of the restored one first, so that the restored database also
    /// knows these deletions for the next restore. Then every account that the ledger lists and
    /// that exists is deleted the normal way. Returns how many were deleted and how many failed.
    pub async fn replay_deletions(&self, extra: &[(u64, i64)]) -> Result<(u64, u64), AppError> {
        {
            let mut tx = self.writer.begin_maintenance(&self.db).await?;
            for (id, at) in extra {
                db::record_deleted(&mut tx, *id, *at).await?;
            }
            tx.commit().await?;
        }
        let (mut deleted, mut failed) = (0, 0);
        for account in db::deleted_but_present(&self.db).await? {
            match self.delete_account(account).await {
                Ok(()) => deleted += 1,
                Err(error) => {
                    failed += 1;
                    tracing::error!(
                        class = error.body.error.as_str(),
                        "cannot delete an account again"
                    );
                }
            }
        }
        Ok((deleted, failed))
    }

    /// Deletes moderation data past its retention, at most once an hour per node. A failure is
    /// logged and tried again at the next sweep.
    async fn sweep_moderation(&self) {
        use std::sync::atomic::Ordering;
        let now = util::now();
        let last = self.retention_at.load(Ordering::Relaxed);
        if now < last + RETENTION_EVERY_SECS
            || self
                .retention_at
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        match crate::moderation::sweep_retention(self).await {
            Ok(swept) => tracing::info!(?swept, "retention sweep"),
            Err(error) => {
                self.retention_at.store(last, Ordering::Relaxed);
                tracing::error!(error = %error.body.message, "the retention sweep failed");
            }
        }
    }

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
            let started = Instant::now();
            let result = state.sweep().await;
            state.metrics.sweep(started.elapsed(), result.is_ok());
            if let Err(error) = result {
                tracing::error!(error = %error.body.message, "the sweep failed; it runs again soon");
            }
        }
    });
}
