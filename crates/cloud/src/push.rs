//! Web Push: notifications for friend requests, party invitations and new messages, sent to the
//! browsers that players subscribed in the portal (network `spec/portal-api.md`, "Notifications").
//!
//! A subscription is the address of a push service (the browser maker's), plus the keys that
//! encrypt what we send to it. The cloud signs each request with its own VAPID key (RFC 8292) and
//! encrypts the payload for the browser (RFC 8291, `aes128gcm`). The crypto is OpenSSL, which the
//! cloud links for passkeys anyway, so Web Push adds no dependency.
//!
//! * [`Vapid`]: the cloud's key and the `Authorization` header it makes.
//! * [`encrypt`] and [`decrypt`]: the message encoding. `decrypt` is the browser's half, for tests.
//! * The table `push_subscriptions` and its queries.
//! * [`notify`]: sends one notification to every subscription of an account that wants the kind,
//!   in a background task, and removes subscriptions that are gone.
//!
//! A payload never holds the text of a message: only who wrote, so the page or the launcher shows
//! the rest after the player signs in.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openssl::bn::{BigNum, BigNumContext};
use openssl::derive::Deriver;
use openssl::ec::{EcGroup, EcKey, EcPoint, PointConversionForm};
use openssl::ecdsa::EcdsaSig;
use openssl::hash::{MessageDigest, hash};
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::rand::rand_bytes;
use openssl::sign::Signer;
use openssl::symm::{Cipher, decrypt_aead, encrypt_aead};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Semaphore;

use crate::db::Pool;
use crate::error::AppError;
use crate::state::AppState;
use crate::util::now;

/// Subscriptions one account may hold: a browser or two on each of a few machines.
pub const MAX_SUBSCRIPTIONS: i64 = 10;
/// Failures in a row (other than "gone") after which a subscription is removed.
pub const MAX_FAILURES: i64 = 5;
const MAX_ENDPOINT: usize = 1024;
const MAX_LABEL: usize = 60;
/// Requests to push services at the same time, on one node.
const CONCURRENCY: usize = 16;
/// Notifications waiting for a free slot, on one node. More are dropped: a push is a courtesy.
const MAX_PENDING: usize = 2000;
/// The header, the padding byte and the tag around the plaintext, in a message of one record.
const MESSAGE_LIMIT: usize = 4096;
const RECORD_OVERHEAD: usize = 86 + 1 + 16;
/// How long a signed VAPID token lives (RFC 8292 allows up to 24 hours).
const VAPID_LIFETIME: u64 = 12 * 60 * 60;

/// The push services we send to. A subscription's address is chosen by the browser, but a player
/// can send us any text, and the cloud would post to it: only the services of the browsers are
/// allowed, so a subscription cannot point the cloud at an internal address.
const SERVICES: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "push.services.mozilla.com",
    "push.apple.com",
    "notify.windows.com",
];

/// What happens that a player may want to hear about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    FriendRequest,
    PartyInvite,
    Message,
}

pub const KINDS: [Kind; 3] = [Kind::FriendRequest, Kind::PartyInvite, Kind::Message];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::FriendRequest => "friend_request",
            Kind::PartyInvite => "party_invite",
            Kind::Message => "message",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        KINDS.into_iter().find(|k| k.name() == name)
    }

    /// How long a push service keeps the notification for a browser that is off.
    fn ttl(self) -> u64 {
        match self {
            Kind::FriendRequest => 3 * 24 * 60 * 60,
            Kind::PartyInvite => 15 * 60,
            Kind::Message => 24 * 60 * 60,
        }
    }

    fn urgency(self) -> &'static str {
        match self {
            Kind::FriendRequest => "normal",
            Kind::PartyInvite | Kind::Message => "high",
        }
    }
}

// Keys

fn group() -> Result<EcGroup> {
    Ok(EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?)
}

/// The cloud's VAPID key: an ECDSA P-256 key pair and the contact for the push services.
pub struct Vapid {
    key: EcKey<Private>,
    public: Vec<u8>,
    subject: String,
}

impl Vapid {
    /// `private` is the 32-byte scalar in unpadded base64url (see [`generate_key`]). `subject` is
    /// a `mailto:` or `https:` contact.
    pub fn new(private: &str, subject: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(private.trim().trim_end_matches('='))
            .context("the VAPID private key is not base64url")?;
        if bytes.len() != 32 {
            bail!("the VAPID private key must be 32 bytes");
        }
        let group = group()?;
        let mut ctx = BigNumContext::new()?;
        let scalar = BigNum::from_slice(&bytes)?;
        let mut point = EcPoint::new(&group)?;
        point
            .mul_generator2(&group, &scalar, &mut ctx)
            .context("the VAPID private key is not a point on P-256")?;
        let key = EcKey::from_private_components(&group, &scalar, &point)
            .context("the VAPID private key is not valid")?;
        key.check_key()
            .context("the VAPID private key is not valid")?;
        let public = point.to_bytes(&group, PointConversionForm::UNCOMPRESSED, &mut ctx)?;
        Ok(Self {
            key,
            public,
            subject: subject.trim().to_owned(),
        })
    }

    /// The public key as the browser wants it for `applicationServerKey`: the uncompressed point
    /// in unpadded base64url.
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.public)
    }

    /// The `Authorization` header for a push service at `origin` (`https://host`).
    pub fn authorization(&self, origin: &str, at: u64) -> Result<String> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(
            json!({"aud": origin, "exp": at + VAPID_LIFETIME, "sub": self.subject}).to_string(),
        );
        let signing_input = format!("{header}.{claims}");
        let digest = hash(MessageDigest::sha256(), signing_input.as_bytes())?;
        let signature = EcdsaSig::sign(&digest, &self.key)?;
        let mut raw = signature.r().to_vec_padded(32)?;
        raw.extend(signature.s().to_vec_padded(32)?);
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            URL_SAFE_NO_PAD.encode(raw),
            self.public_key()
        ))
    }
}

/// A new VAPID key pair: the private scalar and the public point, both unpadded base64url. The
/// operator puts the private one into `ANYPLAY_CLOUD_VAPID_PRIVATE_KEY`.
pub fn generate_key() -> Result<(String, String)> {
    let group = group()?;
    let key = EcKey::generate(&group)?;
    let private = key.private_key().to_vec_padded(32)?;
    let mut ctx = BigNumContext::new()?;
    let public = key
        .public_key()
        .to_bytes(&group, PointConversionForm::UNCOMPRESSED, &mut ctx)?;
    Ok((
        URL_SAFE_NO_PAD.encode(private),
        URL_SAFE_NO_PAD.encode(public),
    ))
}

// Encoding (RFC 8291)

fn hmac(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let key = PKey::hmac(key)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
    signer.update(data)?;
    Ok(signer.sign_to_vec()?)
}

/// HKDF with SHA-256 for outputs of up to 32 bytes (RFC 5869: one block).
fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>> {
    let prk = hmac(salt, ikm)?;
    let mut input = info.to_vec();
    input.push(1);
    let mut okm = hmac(&prk, &input)?;
    okm.truncate(len);
    Ok(okm)
}

fn point_key(group: &EcGroup, bytes: &[u8]) -> Result<EcKey<openssl::pkey::Public>> {
    if bytes.len() != 65 || bytes[0] != 4 {
        bail!("a public key is an uncompressed P-256 point of 65 bytes");
    }
    let mut ctx = BigNumContext::new()?;
    let point = EcPoint::from_bytes(group, bytes, &mut ctx)?;
    let key = EcKey::from_public_key(group, &point)?;
    key.check_key().context("the public key is not on P-256")?;
    Ok(key)
}

fn ecdh(private: &EcKey<Private>, peer: &EcKey<openssl::pkey::Public>) -> Result<Vec<u8>> {
    let private = PKey::from_ec_key(private.clone())?;
    let peer = PKey::from_ec_key(peer.clone())?;
    let mut deriver = Deriver::new(&private)?;
    deriver.set_peer(&peer)?;
    Ok(deriver.derive_to_vec()?)
}

/// The content-encryption key and nonce for a message.
fn content_keys(
    secret: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let ikm = hkdf(auth, secret, &key_info, 32)?;
    let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16)?;
    let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12)?;
    Ok((cek, nonce))
}

/// Encrypts `plaintext` for the browser with the public key `ua_public` and the secret `auth`
/// of its subscription: the whole body of the request, header and one record. `as_key` is a key
/// made for this message alone and `salt` is random.
pub fn encrypt(
    plaintext: &[u8],
    ua_public: &[u8],
    auth: &[u8],
    as_key: &EcKey<Private>,
    salt: &[u8; 16],
) -> Result<Vec<u8>> {
    if auth.len() != 16 {
        bail!("the authentication secret is 16 bytes");
    }
    if plaintext.len() + RECORD_OVERHEAD > MESSAGE_LIMIT {
        bail!("the message is too long");
    }
    let group = group()?;
    let ua = point_key(&group, ua_public)?;
    let mut ctx = BigNumContext::new()?;
    let as_public =
        as_key
            .public_key()
            .to_bytes(&group, PointConversionForm::UNCOMPRESSED, &mut ctx)?;
    let secret = ecdh(as_key, &ua)?;
    let (cek, nonce) = content_keys(&secret, auth, ua_public, &as_public, salt)?;
    let mut record = plaintext.to_vec();
    record.push(2); // the last record
    let mut tag = [0u8; 16];
    let sealed = encrypt_aead(
        Cipher::aes_128_gcm(),
        &cek,
        Some(&nonce),
        &[],
        &record,
        &mut tag,
    )?;
    let mut message = Vec::with_capacity(86 + sealed.len() + 16);
    message.extend_from_slice(salt);
    message.extend_from_slice(&(MESSAGE_LIMIT as u32).to_be_bytes());
    message.push(as_public.len() as u8);
    message.extend_from_slice(&as_public);
    message.extend_from_slice(&sealed);
    message.extend_from_slice(&tag);
    Ok(message)
}

/// The browser's half of [`encrypt`]: opens a message with the subscription's private key.
/// The cloud never needs it; tests use it to see what a browser would show.
pub fn decrypt(message: &[u8], ua_private: &EcKey<Private>, auth: &[u8]) -> Result<Vec<u8>> {
    if message.len() < 86 + 16 + 1 || message[20] != 65 {
        bail!("not a message of one record");
    }
    let salt = &message[..16];
    let as_public = &message[21..86];
    let group = group()?;
    let as_key = point_key(&group, as_public)?;
    let mut ctx = BigNumContext::new()?;
    let ua_public =
        ua_private
            .public_key()
            .to_bytes(&group, PointConversionForm::UNCOMPRESSED, &mut ctx)?;
    let secret = ecdh(ua_private, &as_key)?;
    let (cek, nonce) = content_keys(&secret, auth, &ua_public, as_public, salt)?;
    let (sealed, tag) = message[86..].split_at(message.len() - 86 - 16);
    let mut record = decrypt_aead(Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], sealed, tag)?;
    match record.pop() {
        Some(2) => Ok(record),
        _ => bail!("the record has no end marker"),
    }
}

// Subscriptions

/// The keys of a subscription as the browser gives them, in base64url.
#[derive(Debug, Deserialize)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

/// What `PUT /push/subscription` takes.
#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub endpoint: String,
    pub keys: Keys,
    pub label: Option<String>,
    pub kinds: Option<Vec<String>>,
}

/// A subscription as the portal sees it. The keys and the address stay with the cloud.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SubscriptionView {
    pub id: u64,
    pub label: String,
    pub kinds: Vec<&'static str>,
    pub created_at: u64,
}

type Raw = (i64, String, String, String, String, String, i64);

fn view_of(raw: &Raw) -> SubscriptionView {
    SubscriptionView {
        id: raw.0.max(0) as u64,
        label: raw.4.clone(),
        kinds: kinds_of(&raw.5),
        created_at: raw.6.max(0) as u64,
    }
}

fn kinds_of(text: &str) -> Vec<&'static str> {
    KINDS
        .into_iter()
        .filter(|k| text.split(',').any(|n| n == k.name()))
        .map(Kind::name)
        .collect()
}

pub async fn subscriptions_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<SubscriptionView>> {
    let rows: Vec<Raw> = sqlx::query_as(
        "SELECT id, endpoint, p256dh, auth, label, kinds, created_at FROM push_subscriptions WHERE account_id = $1 ORDER BY id",
    )
    .bind(account as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(view_of).collect())
}

/// Whether the address is one we may post to: HTTPS on the default port, on a host of a push
/// service. With `any_host` (tests) any address is.
pub fn endpoint_allowed(endpoint: &str, any_host: bool) -> bool {
    if endpoint.len() > MAX_ENDPOINT {
        return false;
    }
    let Ok(url) = Url::parse(endpoint) else {
        return false;
    };
    if any_host {
        return matches!(url.scheme(), "http" | "https") && url.host_str().is_some();
    }
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.domain() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    SERVICES
        .iter()
        .any(|service| host == *service || host.ends_with(&format!(".{service}")))
}

fn clean_label(label: Option<String>) -> Result<String, AppError> {
    let label = label.unwrap_or_default();
    let label = label.trim();
    if label.chars().count() > MAX_LABEL || label.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "A device name has at most 60 printable characters.",
        ));
    }
    Ok(label.to_owned())
}

/// A subscription as it is stored: the keys checked, the kinds as text.
struct Checked {
    endpoint: String,
    p256dh: String,
    auth: String,
    label: String,
    kinds: String,
}

/// Checks the body of a subscription and returns what to store.
fn checked(body: SubscribeBody, any_host: bool) -> Result<Checked, AppError> {
    if !endpoint_allowed(&body.endpoint, any_host) {
        return Err(AppError::bad_request(
            "This is not the address of a push service.",
        ));
    }
    let p256dh = URL_SAFE_NO_PAD
        .decode(body.keys.p256dh.trim_end_matches('='))
        .ok()
        .filter(|k| point_key(&group().expect("P-256 exists"), k).is_ok());
    let auth = URL_SAFE_NO_PAD
        .decode(body.keys.auth.trim_end_matches('='))
        .ok()
        .filter(|a| a.len() == 16);
    let (Some(_), Some(_)) = (p256dh, auth) else {
        return Err(AppError::bad_request(
            "The subscription keys are not valid.",
        ));
    };
    let kinds = match body.kinds {
        None => KINDS.to_vec(),
        Some(names) => {
            let mut kinds = Vec::new();
            for name in &names {
                let kind = Kind::parse(name)
                    .ok_or_else(|| AppError::bad_request(format!("{name:?} is not a kind.")))?;
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            kinds
        }
    };
    let kinds = kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(",");
    Ok(Checked {
        endpoint: body.endpoint,
        p256dh: body.keys.p256dh.trim_end_matches('=').to_owned(),
        auth: body.keys.auth.trim_end_matches('=').to_owned(),
        label: clean_label(body.label)?,
        kinds,
    })
}

/// Stores a subscription for the account. The same address under another account moves to this
/// one: the browser is the one that signed in last.
pub async fn subscribe(
    pool: &Pool,
    account: u64,
    body: SubscribeBody,
    any_host: bool,
) -> Result<SubscriptionView, AppError> {
    let Checked {
        endpoint,
        p256dh,
        auth,
        label,
        kinds,
    } = checked(body, any_host)?;
    let at = now() as i64;
    let update = "UPDATE push_subscriptions SET account_id = $2, p256dh = $3, auth = $4, \
                  label = $5, kinds = $6, failures = 0 WHERE endpoint = $1";
    for _ in 0..2 {
        let changed = sqlx::query(update)
            .bind(&endpoint)
            .bind(account as i64)
            .bind(&p256dh)
            .bind(&auth)
            .bind(&label)
            .bind(&kinds)
            .execute(pool)
            .await?
            .rows_affected();
        if changed == 0 {
            let (held,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM push_subscriptions WHERE account_id = $1")
                    .bind(account as i64)
                    .fetch_one(pool)
                    .await?;
            if held >= MAX_SUBSCRIPTIONS {
                return Err(AppError::limit(
                    "This account has the most notification devices allowed. Remove one first.",
                ));
            }
            let inserted = sqlx::query(
                "INSERT INTO push_subscriptions \
                 (account_id, endpoint, p256dh, auth, label, kinds, failures, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, 0, $7)",
            )
            .bind(account as i64)
            .bind(&endpoint)
            .bind(&p256dh)
            .bind(&auth)
            .bind(&label)
            .bind(&kinds)
            .bind(at)
            .execute(pool)
            .await;
            match inserted {
                Ok(_) => {}
                // Another request stored the same address first: update it on the next round.
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let row: Raw = sqlx::query_as(
            "SELECT id, endpoint, p256dh, auth, label, kinds, created_at FROM push_subscriptions WHERE endpoint = $1",
        )
        .bind(&endpoint)
        .fetch_one(pool)
        .await?;
        return Ok(view_of(&row));
    }
    Err(AppError::internal())
}

/// Removes the account's subscription for this address; true when there was one.
pub async fn unsubscribe(pool: &Pool, account: u64, endpoint: &str) -> sqlx::Result<bool> {
    let result =
        sqlx::query("DELETE FROM push_subscriptions WHERE account_id = $1 AND endpoint = $2")
            .bind(account as i64)
            .bind(endpoint)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes a subscription by its number; true when the account had it.
pub async fn unsubscribe_id(pool: &Pool, account: u64, id: u64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM push_subscriptions WHERE account_id = $1 AND id = $2")
        .bind(account as i64)
        .bind(id as i64)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

// Sending

/// The cloud's side of Web Push, present when the VAPID key is set.
pub struct Pusher {
    vapid: Vapid,
    client: reqwest::Client,
    slots: Semaphore,
    pending: AtomicUsize,
    pub sent: AtomicU64,
    pub failed: AtomicU64,
    pub pruned: AtomicU64,
}

impl Pusher {
    pub fn new(vapid: Vapid) -> Self {
        Self {
            vapid,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("anyplay-cloud/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("the HTTP client builds"),
            slots: Semaphore::new(CONCURRENCY),
            pending: AtomicUsize::new(0),
            sent: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            pruned: AtomicU64::new(0),
        }
    }

    pub fn public_key(&self) -> String {
        self.vapid.public_key()
    }
}

/// What a notification says. `title` and `body` are shown by the service worker; `url` is the
/// page it opens (a path on the portal); `tag` lets a newer notification replace an older one.
#[derive(Debug, Serialize)]
pub struct Note {
    pub kind: &'static str,
    pub title: String,
    pub body: String,
    pub url: String,
    pub tag: String,
}

impl Note {
    pub fn friend_request(from: &str) -> Self {
        Self {
            kind: Kind::FriendRequest.name(),
            title: "Friend request".into(),
            body: format!("{from} sent you a friend request"),
            url: "/account/friends".into(),
            tag: format!("friend_request:{}", from.to_ascii_lowercase()),
        }
    }

    pub fn party_invite(from: &str) -> Self {
        Self {
            kind: Kind::PartyInvite.name(),
            title: "Party invitation".into(),
            body: format!("{from} invited you to a party"),
            url: "/account".into(),
            tag: format!("party_invite:{}", from.to_ascii_lowercase()),
        }
    }

    pub fn game_invite(from: &str) -> Self {
        Self {
            kind: Kind::PartyInvite.name(),
            title: "Game invitation".into(),
            body: format!("{from} invited you to play"),
            url: "/account".into(),
            tag: format!("party_invite:{}", from.to_ascii_lowercase()),
        }
    }

    pub fn message(from: &str, conversation: &str) -> Self {
        Self {
            kind: Kind::Message.name(),
            title: "New message".into(),
            body: format!("{from} sent you a message"),
            url: "/account/friends".into(),
            tag: format!("message:{conversation}"),
        }
    }
}

/// Tells every browser of `account` that wants this kind. It returns at once; the sending runs in
/// a task of its own. Nothing happens when the cloud has no VAPID key.
pub fn notify(state: &AppState, account: u64, note: Note) {
    spawn(state, move |state| async move {
        deliver(&state, account, &note).await
    });
}

/// Like [`notify`], for a sender known by account number: the handle is looked up in the task.
pub fn notify_from(state: &AppState, to: u64, from: u64, make: fn(&str) -> Note) {
    spawn(state, move |state| async move {
        let Some(sender) = crate::db::account(&state.db, from).await? else {
            return Ok(());
        };
        deliver(&state, to, &make(&sender.handle)).await
    });
}

/// Runs the sending in a task of its own, unless the cloud sends no notifications or too many
/// wait already.
fn spawn<F>(state: &AppState, work: impl FnOnce(AppState) -> F + Send + 'static)
where
    F: std::future::Future<Output = sqlx::Result<()>> + Send + 'static,
{
    let Some(pusher) = &state.push else { return };
    if pusher.pending.fetch_add(1, Ordering::Relaxed) >= MAX_PENDING {
        pusher.pending.fetch_sub(1, Ordering::Relaxed);
        tracing::warn!("too many notifications waiting; one is dropped");
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = work(state.clone()).await {
            tracing::warn!(%error, "cannot send notifications");
        }
        if let Some(pusher) = &state.push {
            pusher.pending.fetch_sub(1, Ordering::Relaxed);
        }
    });
}

async fn deliver(state: &AppState, account: u64, note: &Note) -> sqlx::Result<()> {
    if state.push.is_none() {
        return Ok(());
    }
    let Some(kind) = Kind::parse(note.kind) else {
        return Ok(());
    };
    let rows: Vec<Raw> = sqlx::query_as(
        "SELECT id, endpoint, p256dh, auth, label, kinds, created_at FROM push_subscriptions WHERE account_id = $1 ORDER BY id",
    )
    .bind(account as i64)
    .fetch_all(&state.db)
    .await?;
    let payload = serde_json::to_vec(note).unwrap_or_default();
    let any_host = state.tuning.push_any_host;
    let mut sends = Vec::new();
    for row in rows {
        if !kinds_of(&row.5).contains(&kind.name()) {
            continue;
        }
        let state = state.clone();
        let payload = payload.clone();
        sends.push(tokio::spawn(async move {
            send_one(&state, &row, kind, &payload, any_host).await;
        }));
    }
    for send in sends {
        let _ = send.await;
    }
    Ok(())
}

/// What came of one request.
enum Outcome {
    Delivered,
    /// The push service says the subscription is gone.
    Gone,
    Failed,
}

async fn send_one(state: &AppState, row: &Raw, kind: Kind, payload: &[u8], any_host: bool) {
    let Some(pusher) = &state.push else { return };
    let Ok(_slot) = pusher.slots.acquire().await else {
        return;
    };
    let outcome = request(pusher, row, kind, payload, any_host).await;
    let id = row.0;
    match outcome {
        Outcome::Delivered => {
            pusher.sent.fetch_add(1, Ordering::Relaxed);
            let _ = sqlx::query(
                "UPDATE push_subscriptions SET failures = 0, last_ok_at = $2 WHERE id = $1",
            )
            .bind(id)
            .bind(now() as i64)
            .execute(&state.db)
            .await;
        }
        Outcome::Gone => {
            pusher.pruned.fetch_add(1, Ordering::Relaxed);
            let _ = sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
                .bind(id)
                .execute(&state.db)
                .await;
        }
        Outcome::Failed => {
            pusher.failed.fetch_add(1, Ordering::Relaxed);
            let failures: Option<(i64,)> = sqlx::query_as(
                "UPDATE push_subscriptions SET failures = failures + 1 WHERE id = $1 RETURNING failures",
            )
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            if failures.is_some_and(|(n,)| n >= MAX_FAILURES) {
                pusher.pruned.fetch_add(1, Ordering::Relaxed);
                let _ = sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
                    .bind(id)
                    .execute(&state.db)
                    .await;
            }
        }
    }
}

/// Encrypts with a new key and salt.
fn seal(payload: &[u8], ua_public: &[u8], auth: &[u8]) -> Result<Vec<u8>> {
    let group = group()?;
    let as_key = EcKey::generate(&group)?;
    let mut salt = [0u8; 16];
    rand_bytes(&mut salt)?;
    encrypt(payload, ua_public, auth, &as_key, &salt)
}

async fn request(
    pusher: &Pusher,
    row: &Raw,
    kind: Kind,
    payload: &[u8],
    any_host: bool,
) -> Outcome {
    // The log never holds an address or a key: only the host of the service and what came of it.
    let Ok(url) = Url::parse(&row.1) else {
        return Outcome::Gone;
    };
    if !endpoint_allowed(&row.1, any_host) {
        return Outcome::Gone;
    }
    let host = url.host_str().unwrap_or_default().to_owned();
    let (Ok(ua_public), Ok(auth)) = (
        URL_SAFE_NO_PAD.decode(&row.2),
        URL_SAFE_NO_PAD.decode(&row.3),
    ) else {
        return Outcome::Gone;
    };
    let Ok(body) = seal(payload, &ua_public, &auth) else {
        return Outcome::Gone;
    };
    let origin = url.origin().ascii_serialization();
    let Ok(authorization) = pusher.vapid.authorization(&origin, now()) else {
        return Outcome::Failed;
    };
    let result = pusher
        .client
        .post(url)
        .header("Authorization", authorization)
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("TTL", kind.ttl().to_string())
        .header("Urgency", kind.urgency())
        .body(body)
        .send()
        .await;
    match result {
        Ok(response) if response.status().is_success() => Outcome::Delivered,
        Ok(response) if matches!(response.status().as_u16(), 404 | 410) => Outcome::Gone,
        Ok(response) => {
            tracing::debug!(service = %host, status = response.status().as_u16(), "a push service refused");
            Outcome::Failed
        }
        Err(error) => {
            tracing::debug!(service = %host, timeout = error.is_timeout(), "cannot reach a push service");
            Outcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(private: &str) -> EcKey<Private> {
        let group = group().unwrap();
        let scalar = BigNum::from_slice(&URL_SAFE_NO_PAD.decode(private).unwrap()).unwrap();
        let mut ctx = BigNumContext::new().unwrap();
        let mut point = EcPoint::new(&group).unwrap();
        point.mul_generator2(&group, &scalar, &mut ctx).unwrap();
        EcKey::from_private_components(&group, &scalar, &point).unwrap()
    }

    fn b64(text: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(text).unwrap()
    }

    // RFC 8291, appendix A.
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const PLAINTEXT: &str = "When I grow up, I want to be a watermelon";
    const HEADER: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const SEALED: &str =
        "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ";

    fn published() -> Vec<u8> {
        [b64(HEADER), b64(SEALED)].concat()
    }

    #[test]
    fn the_rfc_8291_example_encrypts_to_the_published_message() {
        let salt: [u8; 16] = b64(SALT).try_into().unwrap();
        let message = encrypt(
            PLAINTEXT.as_bytes(),
            &b64(UA_PUBLIC),
            &b64(AUTH),
            &key(AS_PRIVATE),
            &salt,
        )
        .unwrap();
        assert_eq!(message, published());
    }

    #[test]
    fn the_rfc_8291_example_decrypts_with_the_browsers_key() {
        let plain = decrypt(&published(), &key(UA_PRIVATE), &b64(AUTH)).unwrap();
        assert_eq!(plain, PLAINTEXT.as_bytes());
        // A wrong secret opens nothing.
        assert!(decrypt(&published(), &key(UA_PRIVATE), &[0; 16]).is_err());
    }

    #[test]
    fn a_message_that_does_not_fit_one_record_is_refused() {
        let as_key = EcKey::generate(&group().unwrap()).unwrap();
        let big = vec![b'x'; 4000];
        assert!(encrypt(&big, &b64(UA_PUBLIC), &b64(AUTH), &as_key, &[0; 16]).is_err());
        assert!(encrypt(b"x", &b64(UA_PUBLIC), &[0; 8], &as_key, &[0; 16]).is_err());
        assert!(encrypt(b"x", &[4; 65], &b64(AUTH), &as_key, &[0; 16]).is_err());
    }

    #[test]
    fn a_generated_key_makes_a_vapid_header_that_verifies() {
        let (private, public) = generate_key().unwrap();
        assert_eq!(b64(&public).len(), 65);
        let vapid = Vapid::new(&private, "mailto:ops@example.com").unwrap();
        assert_eq!(vapid.public_key(), public);
        let header = vapid.authorization("https://push.example", 1_000).unwrap();
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (token, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, public);
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).unwrap();
        assert_eq!(claims["aud"], "https://push.example");
        assert_eq!(claims["sub"], "mailto:ops@example.com");
        assert_eq!(claims["exp"], 1_000 + VAPID_LIFETIME);
        let header: serde_json::Value = serde_json::from_slice(&b64(parts[0])).unwrap();
        assert_eq!(header["alg"], "ES256");
        // The signature is r || s over SHA-256 of "header.claims".
        let raw = b64(parts[2]);
        assert_eq!(raw.len(), 64);
        let signature = EcdsaSig::from_private_components(
            BigNum::from_slice(&raw[..32]).unwrap(),
            BigNum::from_slice(&raw[32..]).unwrap(),
        )
        .unwrap();
        let digest = hash(
            MessageDigest::sha256(),
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
        )
        .unwrap();
        let group = group().unwrap();
        let public_key = point_key(&group, &b64(&public)).unwrap();
        assert!(signature.verify(&digest, &public_key).unwrap());
    }

    #[test]
    fn a_bad_vapid_key_is_refused() {
        assert!(Vapid::new("not base64 !", "mailto:a@b.c").is_err());
        assert!(Vapid::new("AAAA", "mailto:a@b.c").is_err());
        assert!(Vapid::new(&URL_SAFE_NO_PAD.encode([0u8; 32]), "mailto:a@b.c").is_err());
    }

    #[test]
    fn only_push_services_are_allowed_as_endpoints() {
        let ok = |url: &str| endpoint_allowed(url, false);
        assert!(ok("https://fcm.googleapis.com/fcm/send/abc"));
        assert!(ok("https://updates.push.services.mozilla.com/wpush/v2/abc"));
        assert!(ok("https://web.push.apple.com/abc"));
        assert!(ok("https://db5p.notify.windows.com/?token=abc"));
        assert!(!ok("http://fcm.googleapis.com/fcm/send/abc"));
        assert!(!ok("https://fcm.googleapis.com:8443/x"));
        assert!(!ok("https://evilfcm.googleapis.com.example.com/x"));
        assert!(!ok("https://notfcm.googleapis.com.evil.net/x"));
        assert!(!ok("https://user@fcm.googleapis.com/x"));
        assert!(!ok("https://127.0.0.1/x"));
        assert!(!ok("https://localhost/x"));
        assert!(!ok("https://[::1]/x"));
        assert!(!ok("ftp://fcm.googleapis.com/x"));
        assert!(!ok(&format!(
            "https://fcm.googleapis.com/{}",
            "a".repeat(1100)
        )));
        assert!(endpoint_allowed("http://127.0.0.1:9/x", true));
    }

    #[test]
    fn kinds_round_trip_through_the_stored_text() {
        assert_eq!(
            kinds_of("friend_request,message"),
            ["friend_request", "message"]
        );
        assert_eq!(kinds_of(""), Vec::<&str>::new());
        assert_eq!(kinds_of("message,nonsense"), ["message"]);
        for kind in KINDS {
            assert_eq!(Kind::parse(kind.name()), Some(kind));
        }
    }

    #[test]
    fn a_payload_names_the_sender_and_never_a_text() {
        let note = Note::message("alec", "c1");
        let json = serde_json::to_value(&note).unwrap();
        assert_eq!(json["kind"], "message");
        assert_eq!(json["body"], "alec sent you a message");
        assert_eq!(json["url"], "/account/friends");
        assert!(serde_json::to_vec(&note).unwrap().len() < 400);
    }
}
