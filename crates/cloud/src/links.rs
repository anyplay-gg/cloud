//! Connections: the accounts of other services (Discord, Steam) that a player linked to their
//! AnyPlay account (network `spec/portal-api.md`, "Connections").
//!
//! A connection is an identity and nothing else: the provider's ID of the account, a display
//! name and a picture. The cloud never keeps a token. Discord is OAuth 2.0 with PKCE and the scope
//! `identify`; Steam is OpenID 2.0, checked with Steam. [`begin`] makes the address to send the
//! browser to and the state to keep in the browser session; [`discord_identity`] and
//! [`steam_identity`] turn the browser's return into the identity, or refuse it. The routes are in
//! [`crate::web::api::links`].
//!
//! Like [`crate::db`], every query is written once with `$1` parameters for SQLite and PostgreSQL.

use std::sync::OnceLock;
use std::time::Duration;

use anyplay_api::Audience;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::config::LinkEndpoints;
use crate::db::{self, Pool};
use crate::state::AppState;
use crate::util::{random_token, sha256};
use crate::web::store::PendingLink;

/// A service whose accounts can be linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Discord,
    Steam,
}

impl Provider {
    /// Every provider, in the order the portal lists them.
    pub const ALL: [Provider; 2] = [Provider::Discord, Provider::Steam];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Discord => "discord",
            Provider::Steam => "steam",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == text)
    }
}

/// True when this cloud can link accounts of `provider`: it has the account web, and for Discord
/// a Discord application.
pub fn available(state: &AppState, provider: Provider) -> bool {
    state.web.is_some()
        && match provider {
            Provider::Discord => state.config.discord().is_some(),
            Provider::Steam => true,
        }
}

/// Where the provider sends the browser back to.
pub fn callback_url(origin: &str, provider: Provider) -> String {
    format!("{origin}/web/links/{}/callback", provider.name())
}

/// Who the account is on the provider, as the provider confirmed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub external_id: String,
    pub display_name: Option<String>,
    /// A Discord avatar hash, or the address of a Steam picture.
    pub avatar: Option<String>,
}

/// The provider did not confirm an identity. The reason is in the log, never in the answer.
#[derive(Debug)]
pub struct Unconfirmed;

// Starting

/// The address to send the browser to, and what to keep in the session until it comes back.
/// `None` when the provider is off.
pub fn begin(state: &AppState, provider: Provider) -> Option<(String, PendingLink)> {
    if !available(state, provider) {
        return None;
    }
    let origin = &state.web.as_ref()?.origin;
    let endpoints = &state.tuning.links;
    let callback = callback_url(origin, provider);
    let token = random_token(24);
    let mut pending = PendingLink {
        provider: provider.name().to_owned(),
        state: token.clone(),
        verifier: None,
        started_ms: crate::util::now_ms(),
    };
    let url = match provider {
        Provider::Discord => {
            let (client_id, _) = state.config.discord()?;
            let verifier = random_token(32);
            let challenge = URL_SAFE_NO_PAD.encode(sha256(verifier.as_bytes()));
            pending.verifier = Some(verifier);
            let mut url =
                Url::parse(&format!("{}/oauth2/authorize", endpoints.discord_web)).ok()?;
            url.query_pairs_mut()
                .append_pair("response_type", "code")
                .append_pair("client_id", client_id)
                .append_pair("scope", "identify")
                .append_pair("redirect_uri", &callback)
                .append_pair("state", &token)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("prompt", "consent");
            url
        }
        Provider::Steam => {
            let mut url = Url::parse(&format!("{}/login", endpoints.steam_openid)).ok()?;
            let identifier_select = "http://specs.openid.net/auth/2.0/identifier_select";
            url.query_pairs_mut()
                .append_pair("openid.ns", OPENID_NS)
                .append_pair("openid.mode", "checkid_setup")
                .append_pair("openid.identity", identifier_select)
                .append_pair("openid.claimed_id", identifier_select)
                .append_pair("openid.realm", origin)
                .append_pair("openid.return_to", &steam_return_to(&callback, &token));
            url
        }
    };
    Some((url.into(), pending))
}

const OPENID_NS: &str = "http://specs.openid.net/auth/2.0";

/// The address Steam sends the browser back to: ours, with the state of the session in it. Steam
/// signs it, so the state cannot be swapped on the way.
pub fn steam_return_to(callback: &str, state: &str) -> String {
    format!("{callback}?state={state}")
}

// Talking to the providers

const MAX_ANSWER: usize = 64 * 1024;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("anyplay-cloud/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("the HTTP client builds")
    })
}

/// The body of an answer, if it fits. A provider that sends more is not one to trust.
async fn read_body(mut response: reqwest::Response) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_ANSWER as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_ANSWER {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

/// Sends the request and reads a successful answer. Logs a failure by what went wrong, never by
/// address (a Steam address holds the key) or body.
async fn send(
    what: &'static str,
    request: reqwest::RequestBuilder,
) -> Result<Vec<u8>, Unconfirmed> {
    let response = request.send().await.map_err(|error| {
        tracing::warn!(error = %error.without_url(), "{what}: the request failed");
        Unconfirmed
    })?;
    let status = response.status();
    if !status.is_success() {
        tracing::warn!(%status, "{what}: refused");
        return Err(Unconfirmed);
    }
    read_body(response).await.ok_or_else(|| {
        tracing::warn!("{what}: the answer is too large or broke off");
        Unconfirmed
    })
}

fn form(pairs: &[(&str, &str)]) -> Result<String, Unconfirmed> {
    serde_urlencoded::to_string(pairs).map_err(|_| Unconfirmed)
}

fn post_form(url: String, body: String) -> reqwest::RequestBuilder {
    client()
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
}

fn parse<T: for<'de> Deserialize<'de>>(what: &'static str, body: &[u8]) -> Result<T, Unconfirmed> {
    serde_json::from_slice(body).map_err(|_| {
        tracing::warn!("{what}: the answer is not what was expected");
        Unconfirmed
    })
}

/// A name from a provider: no control or direction-changing characters, at most 64 characters.
fn clean_name(text: &str) -> Option<String> {
    let name: String = text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
                )
        })
        .take(64)
        .collect();
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

fn is_digits(text: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

// Discord

#[derive(Deserialize)]
struct DiscordToken {
    access_token: String,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
    username: String,
    #[serde(default)]
    global_name: Option<String>,
    #[serde(default)]
    avatar: Option<String>,
}

/// Trades the code for a token, reads who the player is, and lets go of the token.
pub async fn discord_identity(
    state: &AppState,
    code: &str,
    verifier: &str,
) -> Result<Identity, Unconfirmed> {
    const WHAT: &str = "Discord connection";
    let (client_id, secret) = state.config.discord().ok_or(Unconfirmed)?;
    let origin = &state.web.as_ref().ok_or(Unconfirmed)?.origin;
    if code.is_empty() || code.len() > 512 {
        return Err(Unconfirmed);
    }
    let api = &state.tuning.links.discord_api;
    let redirect = callback_url(origin, Provider::Discord);
    let body = form(&[
        ("client_id", client_id),
        ("client_secret", secret),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", verifier),
    ])?;
    let token: DiscordToken = parse(
        WHAT,
        &send(WHAT, post_form(format!("{api}/oauth2/token"), body)).await?,
    )?;
    let user = send(
        WHAT,
        client()
            .get(format!("{api}/users/@me"))
            .bearer_auth(&token.access_token),
    )
    .await;
    // The identity is read; the token has no further use. Telling Discord ends it there too. A
    // failure changes nothing here: the token is dropped either way.
    if let Ok(body) = form(&[
        ("token", token.access_token.as_str()),
        ("client_id", client_id),
        ("client_secret", secret),
    ]) {
        let _ = post_form(format!("{api}/oauth2/token/revoke"), body)
            .send()
            .await;
    }
    let user: DiscordUser = parse(WHAT, &user?)?;
    if !is_digits(&user.id, 1..=20) {
        tracing::warn!("{WHAT}: the user ID is not a snowflake");
        return Err(Unconfirmed);
    }
    let avatar = user.avatar.filter(|hash| {
        (1..=64).contains(&hash.len())
            && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    });
    let display_name = user
        .global_name
        .as_deref()
        .and_then(clean_name)
        .or_else(|| clean_name(&user.username));
    Ok(Identity {
        external_id: user.id,
        display_name,
        avatar,
    })
}

// Steam

/// What Steam must have signed, or the answer says nothing about the player.
const STEAM_SIGNED: [&str; 6] = [
    "op_endpoint",
    "claimed_id",
    "identity",
    "return_to",
    "response_nonce",
    "assoc_handle",
];

/// The SteamID64 of the first individual account.
const STEAM_BASE: u64 = 76_561_197_960_265_728;

/// The SteamID64 in a claimed ID of the form `{openid}/id/{17 digits}`.
fn steam_id_of(claimed: &str, openid: &str) -> Option<String> {
    let id = claimed.strip_prefix(&format!("{openid}/id/"))?;
    if !is_digits(id, 17..=17) {
        return None;
    }
    let number: u64 = id.parse().ok()?;
    let account = number.checked_sub(STEAM_BASE)?;
    ((1..=u64::from(u32::MAX)).contains(&account)).then(|| id.to_owned())
}

/// Checks the answer Steam sent the browser with and asks Steam whether it made it. `query` is
/// the query of the callback, as the browser sent it; `return_to` is the address this start
/// asked Steam to return to.
pub async fn steam_identity(
    state: &AppState,
    query: &[(String, String)],
    return_to: &str,
) -> Result<Identity, Unconfirmed> {
    const WHAT: &str = "Steam connection";
    let endpoints = &state.tuning.links;
    let mut fields: Vec<(&str, &str)> = Vec::new();
    for (name, value) in query {
        if name.starts_with("openid.") {
            // A field twice is a way to make the check and the use read different things.
            if fields.iter().any(|(seen, _)| *seen == name.as_str()) {
                tracing::warn!("{WHAT}: a field came twice");
                return Err(Unconfirmed);
            }
            fields.push((name.as_str(), value.as_str()));
        }
    }
    let field = |name: &str| {
        fields
            .iter()
            .find(|(seen, _)| *seen == name)
            .map(|(_, value)| *value)
    };
    let refuse = |why: &'static str| {
        tracing::warn!("{WHAT}: {why}");
        Err(Unconfirmed)
    };
    if field("openid.ns") != Some(OPENID_NS) || field("openid.mode") != Some("id_res") {
        return refuse("not an OpenID 2.0 assertion");
    }
    if field("openid.return_to") != Some(return_to) {
        return refuse("the return address is not the one this start asked for");
    }
    if field("openid.op_endpoint") != Some(format!("{}/login", endpoints.steam_openid).as_str()) {
        return refuse("the answer is not from Steam's endpoint");
    }
    let signed: Vec<&str> = field("openid.signed")
        .unwrap_or_default()
        .split(',')
        .collect();
    if !STEAM_SIGNED.iter().all(|name| signed.contains(name)) {
        return refuse("a field that matters is not signed");
    }
    let claimed = field("openid.claimed_id").unwrap_or_default();
    if field("openid.identity") != Some(claimed) {
        return refuse("the identity is not the claimed ID");
    }
    let Some(steam_id) = steam_id_of(claimed, &endpoints.steam_openid) else {
        return refuse("the claimed ID is not a SteamID64 of Steam");
    };
    // Steam says whether it made the answer, and accepts it once.
    let mut check: Vec<(&str, &str)> = fields
        .iter()
        .map(|(name, value)| {
            if *name == "openid.mode" {
                (*name, "check_authentication")
            } else {
                (*name, *value)
            }
        })
        .collect();
    check.sort_by_key(|(name, _)| *name != "openid.mode");
    let answer = send(
        WHAT,
        post_form(format!("{}/login", endpoints.steam_openid), form(&check)?),
    )
    .await?;
    let confirmed = String::from_utf8_lossy(&answer)
        .lines()
        .any(|line| line.trim() == "is_valid:true");
    if !confirmed {
        return refuse("Steam does not confirm the answer");
    }
    let (display_name, avatar) = steam_persona(state, &steam_id).await.unwrap_or_default();
    Ok(Identity {
        external_id: steam_id,
        display_name,
        avatar,
    })
}

#[derive(Deserialize)]
struct SteamSummaries {
    response: SteamPlayers,
}

#[derive(Deserialize)]
struct SteamPlayers {
    #[serde(default)]
    players: Vec<SteamPlayer>,
}

#[derive(Deserialize)]
struct SteamPlayer {
    steamid: String,
    #[serde(default)]
    personaname: Option<String>,
    #[serde(default)]
    avatarfull: Option<String>,
}

/// True for a picture on Steam's own servers.
fn steam_picture(text: &str) -> bool {
    let Ok(url) = Url::parse(text) else {
        return false;
    };
    text.len() <= 300
        && url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|h| h == "steamstatic.com" || h.ends_with(".steamstatic.com"))
}

/// The persona name and picture, if the cloud has a key and Steam answers. Without either the
/// connection holds the ID only.
async fn steam_persona(
    state: &AppState,
    steam_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    let key = state.config.steam_key()?;
    let mut url = Url::parse(&format!(
        "{}/ISteamUser/GetPlayerSummaries/v0002/",
        state.tuning.links.steam_api
    ))
    .ok()?;
    url.query_pairs_mut()
        .append_pair("key", key)
        .append_pair("steamids", steam_id);
    let body = send("Steam persona", client().get(url)).await.ok()?;
    let summaries: SteamSummaries = parse("Steam persona", &body).ok()?;
    let player = summaries
        .response
        .players
        .into_iter()
        .find(|p| p.steamid == steam_id)?;
    Some((
        player.personaname.as_deref().and_then(clean_name),
        player.avatarfull.filter(|url| steam_picture(url)),
    ))
}

// The store

/// A connection as the store has it.
#[derive(Clone, Debug)]
pub struct Row {
    pub provider: String,
    pub external_id: String,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
    pub audience: Audience,
    pub linked_at: u64,
}

type RawRow = (String, String, Option<String>, Option<String>, String, i64);

fn row_of(raw: RawRow) -> Row {
    Row {
        provider: raw.0,
        external_id: raw.1,
        display_name: raw.2,
        avatar: raw.3,
        audience: db::audience_of(&raw.4),
        linked_at: raw.5.max(0) as u64,
    }
}

/// The connections of an account, by provider.
pub async fn rows_of(pool: &Pool, account: u64) -> sqlx::Result<Vec<Row>> {
    let rows: Vec<RawRow> = sqlx::query_as(
        "SELECT provider, external_id, display_name, avatar, audience, linked_at          FROM linked_accounts WHERE account_id = $1 ORDER BY provider",
    )
    .bind(account as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_of).collect())
}

pub async fn row(pool: &Pool, account: u64, provider: Provider) -> sqlx::Result<Option<Row>> {
    let raw: Option<RawRow> = sqlx::query_as(
        "SELECT provider, external_id, display_name, avatar, audience, linked_at          FROM linked_accounts WHERE account_id = $1 AND provider = $2",
    )
    .bind(account as i64)
    .bind(provider.name())
    .fetch_optional(pool)
    .await?;
    Ok(raw.map(row_of))
}

/// Why a link was not made.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The account has a connection for the provider.
    Exists,
    /// Another account has this identity.
    Taken,
}

/// Links the identity to the account, visible to friends.
pub async fn insert(
    pool: &Pool,
    account: u64,
    provider: Provider,
    identity: &Identity,
    at: u64,
) -> Result<(), LinkError> {
    let result = sqlx::query(
        "INSERT INTO linked_accounts \
         (account_id, provider, external_id, display_name, avatar, audience, linked_at) \
         VALUES ($1, $2, $3, $4, $5, 'friends', $6)",
    )
    .bind(account as i64)
    .bind(provider.name())
    .bind(&identity.external_id)
    .bind(identity.display_name.as_deref())
    .bind(identity.avatar.as_deref())
    .bind(at as i64)
    .execute(pool)
    .await;
    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            // Two keys can break: the account's own row for the provider, or the identity.
            match row(pool, account, provider).await {
                Ok(Some(_)) => Err(LinkError::Exists),
                _ => Err(LinkError::Taken),
            }
        }
        Err(error) => {
            tracing::error!(%error, "cannot insert a connection");
            // The caller shows `failed`; nothing was linked.
            Err(LinkError::Taken)
        }
    }
}

/// Sets who sees the connection; false when there is none.
pub async fn set_audience(
    pool: &Pool,
    account: u64,
    provider: Provider,
    audience: Audience,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE linked_accounts SET audience = $3 WHERE account_id = $1 AND provider = $2",
    )
    .bind(account as i64)
    .bind(provider.name())
    .bind(db::audience_name(audience))
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes the connection; false when there was none.
pub async fn delete(pool: &Pool, account: u64, provider: Provider) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM linked_accounts WHERE account_id = $1 AND provider = $2")
        .bind(account as i64)
        .bind(provider.name())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

// Views

/// A provider in `GET /me/links`, linked or not.
#[derive(Debug, Serialize)]
pub struct LinkView {
    pub provider: &'static str,
    pub available: bool,
    pub linked: bool,
    pub external_id: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub profile_url: Option<String>,
    pub linked_at: Option<u64>,
    pub visibility: Option<Audience>,
}

/// A connection on a public profile.
#[derive(Debug, Serialize)]
pub struct PublicLink {
    pub provider: String,
    pub external_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub profile_url: String,
}

fn avatar_url(endpoints: &LinkEndpoints, row: &Row) -> Option<String> {
    let avatar = row.avatar.as_deref()?;
    match row.provider.as_str() {
        "discord" => Some(format!(
            "{}/avatars/{}/{avatar}.png",
            endpoints.discord_cdn, row.external_id
        )),
        _ => Some(avatar.to_owned()),
    }
}

fn profile_url(row: &Row) -> String {
    match row.provider.as_str() {
        "discord" => format!("https://discord.com/users/{}", row.external_id),
        _ => format!("https://steamcommunity.com/profiles/{}", row.external_id),
    }
}

pub fn view(state: &AppState, provider: Provider, row: Option<&Row>) -> LinkView {
    let endpoints = &state.tuning.links;
    LinkView {
        provider: provider.name(),
        available: available(state, provider),
        linked: row.is_some(),
        external_id: row.map(|r| r.external_id.clone()),
        display_name: row.and_then(|r| r.display_name.clone()),
        avatar_url: row.and_then(|r| avatar_url(endpoints, r)),
        profile_url: row.map(profile_url),
        linked_at: row.map(|r| r.linked_at),
        visibility: row.map(|r| r.audience),
    }
}

/// Every provider with the account's connection for it, if any.
pub async fn views(state: &AppState, account: u64) -> sqlx::Result<Vec<LinkView>> {
    let rows = rows_of(&state.db, account).await?;
    Ok(Provider::ALL
        .into_iter()
        .map(|p| view(state, p, rows.iter().find(|r| r.provider == p.name())))
        .collect())
}

/// The connections of `owner` that a viewer may see.
pub async fn public(
    state: &AppState,
    owner: u64,
    seen: &crate::visibility::Viewing,
) -> sqlx::Result<Vec<PublicLink>> {
    let endpoints = &state.tuning.links;
    Ok(rows_of(&state.db, owner)
        .await?
        .into_iter()
        .filter(|row| seen.can_see(row.audience))
        .map(|row| PublicLink {
            avatar_url: avatar_url(endpoints, &row),
            profile_url: profile_url(&row),
            provider: row.provider,
            external_id: row.external_id,
            display_name: row.display_name,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_id_is_a_steam_id_of_steam() {
        let openid = "https://steamcommunity.com/openid";
        let ok = "https://steamcommunity.com/openid/id/76561198000000000";
        assert_eq!(
            steam_id_of(ok, openid).as_deref(),
            Some("76561198000000000")
        );
        for bad in [
            "https://steamcommunity.com.evil.example/openid/id/76561198000000000",
            "http://steamcommunity.com/openid/id/76561198000000000",
            "https://steamcommunity.com/openid/id/7656119800000000",
            "https://steamcommunity.com/openid/id/765611980000000000",
            "https://steamcommunity.com/openid/id/76561198000000000/x",
            "https://steamcommunity.com/openid/id/7656119800000000a",
            "https://steamcommunity.com/openid/id/00000000000000001",
            "https://steamcommunity.com/openid/id/76561197960265728",
            "https://steamcommunity.com/openid/id/76561206555000000",
            "https://steamcommunity.com/openid/identifier_select",
        ] {
            assert_eq!(steam_id_of(bad, openid), None, "{bad}");
        }
    }

    #[test]
    fn names_from_a_provider_are_cleaned() {
        assert_eq!(clean_name("  Alec\u{202E}x\n").as_deref(), Some("Alecx"));
        assert_eq!(clean_name("\u{200B}\n"), None);
        assert_eq!(clean_name(&"a".repeat(100)).unwrap().chars().count(), 64);
    }

    #[test]
    fn a_picture_must_be_on_steams_own_host() {
        assert!(steam_picture("https://avatars.steamstatic.com/x_full.jpg"));
        assert!(steam_picture("https://steamstatic.com/x.jpg"));
        assert!(!steam_picture("http://avatars.steamstatic.com/x.jpg"));
        assert!(!steam_picture("https://evilsteamstatic.com/x.jpg"));
        assert!(!steam_picture("https://steamstatic.com.evil.example/x.jpg"));
        assert!(!steam_picture("javascript:alert(1)"));
    }

    #[test]
    fn providers_are_found_by_name() {
        assert_eq!(Provider::parse("discord"), Some(Provider::Discord));
        assert_eq!(Provider::parse("steam"), Some(Provider::Steam));
        assert_eq!(Provider::parse("Steam"), None);
        assert_eq!(Provider::parse(""), None);
    }
}
