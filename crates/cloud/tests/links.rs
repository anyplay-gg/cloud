//! Connections: linking, unlinking and showing Discord and Steam accounts through the portal API,
//! against a fake Discord and a fake Steam that run in this process.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyplay_cloud::Tuning;
use anyplay_cloud::config::LinkEndpoints;
use anyplay_cloud::limits::Rates;
use anyplay_cloud::links;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::portal::{Answer, Portal};
use common::{TestCloud, User};
use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const CLIENT_ID: &str = "1234567890";
const CLIENT_SECRET: &str = "s3cret-of-the-test-app";
const STEAM_KEY: &str = "TESTKEY0123456789";
const OPENID_NS: &str = "http://specs.openid.net/auth/2.0";
const ALEC_STEAM: &str = "76561198000000001";
const BOB_STEAM: &str = "76561198000000002";

// The fake providers

#[derive(Default)]
struct Fake {
    /// Codes Discord has issued and nobody has used: code to (PKCE challenge, user).
    codes: HashMap<String, (String, Value)>,
    used_codes: HashSet<String>,
    tokens: HashMap<String, Value>,
    revoked: Vec<String>,
    /// What the token endpoint received, as forms.
    token_requests: Vec<HashMap<String, String>>,
    /// Steam answers not yet checked: nonce to the fields that were signed.
    assertions: HashMap<String, HashMap<String, String>>,
    players: HashMap<String, Value>,
    down: bool,
    counter: u32,
}

type Shared = Arc<Mutex<Fake>>;

fn form(body: &[u8]) -> HashMap<String, String> {
    serde_urlencoded::from_bytes(body).unwrap_or_default()
}

async fn discord_token(State(fake): State<Shared>, body: Bytes) -> axum::response::Response {
    let mut fake = fake.lock().unwrap();
    if fake.down {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let fields = form(&body);
    fake.token_requests.push(fields.clone());
    let bad =
        |why: &str| (StatusCode::BAD_REQUEST, axum::Json(json!({"error": why}))).into_response();
    if fields.get("client_id").map(String::as_str) != Some(CLIENT_ID)
        || fields.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET)
        || fields.get("grant_type").map(String::as_str) != Some("authorization_code")
    {
        return bad("invalid_client");
    }
    let code = fields.get("code").cloned().unwrap_or_default();
    if fake.used_codes.contains(&code) {
        return bad("invalid_grant");
    }
    let Some((challenge, user)) = fake.codes.remove(&code) else {
        return bad("invalid_grant");
    };
    fake.used_codes.insert(code);
    let verifier = fields.get("code_verifier").cloned().unwrap_or_default();
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != challenge {
        return bad("invalid_grant");
    }
    fake.counter += 1;
    let token = format!("token-{}", fake.counter);
    fake.tokens.insert(token.clone(), user);
    axum::Json(json!({"access_token": token, "token_type": "Bearer", "scope": "identify"}))
        .into_response()
}

async fn discord_me(State(fake): State<Shared>, headers: HeaderMap) -> axum::response::Response {
    let fake = fake.lock().unwrap();
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    match fake.tokens.get(token) {
        Some(user) if !fake.revoked.contains(&token.to_owned()) => {
            axum::Json(user.clone()).into_response()
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn discord_revoke(State(fake): State<Shared>, body: Bytes) -> axum::response::Response {
    let fields = form(&body);
    if let Some(token) = fields.get("token") {
        fake.lock().unwrap().revoked.push(token.clone());
    }
    axum::Json(json!({})).into_response()
}

async fn steam_check(State(fake): State<Shared>, body: Bytes) -> axum::response::Response {
    let mut fake = fake.lock().unwrap();
    let fields = form(&body);
    let valid = fields.get("openid.mode").map(String::as_str) == Some("check_authentication")
        && fields
            .get("openid.response_nonce")
            .and_then(|nonce| fake.assertions.remove(nonce))
            .is_some_and(|signed| {
                // The mode changes to check_authentication; every other field must be as signed.
                signed
                    .iter()
                    .filter(|(name, _)| *name != "openid.mode")
                    .all(|(name, value)| fields.get(name) == Some(value))
            });
    format!("ns:{OPENID_NS}\nis_valid:{valid}\n").into_response()
}

async fn steam_players(
    State(fake): State<Shared>,
    Query(query): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let fake = fake.lock().unwrap();
    if fake.down || query.get("key").map(String::as_str) != Some(STEAM_KEY) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let players: Vec<Value> = query
        .get("steamids")
        .and_then(|id| fake.players.get(id))
        .cloned()
        .into_iter()
        .collect();
    axum::Json(json!({"response": {"players": players}})).into_response()
}

struct Providers {
    fake: Shared,
    port: u16,
}

impl Providers {
    async fn start() -> Self {
        let fake: Shared = Arc::default();
        let app = Router::new()
            .route("/api/oauth2/token", post(discord_token))
            .route("/api/oauth2/token/revoke", post(discord_revoke))
            .route("/api/users/@me", get(discord_me))
            .route("/openid/login", post(steam_check))
            .route("/ISteamUser/GetPlayerSummaries/v0002/", get(steam_players))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        {
            let mut fake = fake.lock().unwrap();
            fake.players.insert(
                ALEC_STEAM.into(),
                json!({
                    "steamid": ALEC_STEAM,
                    "personaname": "alec_on_steam",
                    "avatarfull": "https://avatars.steamstatic.com/abc_full.jpg",
                }),
            );
            fake.players.insert(
                BOB_STEAM.into(),
                json!({
                    "steamid": BOB_STEAM,
                    "personaname": "bob",
                    // Not a picture of Steam's: the cloud drops it.
                    "avatarfull": "https://evil.example/x.jpg",
                }),
            );
        }
        Self { fake, port }
    }

    fn endpoints(&self) -> LinkEndpoints {
        let base = format!("http://127.0.0.1:{}", self.port);
        LinkEndpoints {
            discord_web: "https://discord.test".into(),
            discord_api: format!("{base}/api"),
            discord_cdn: "https://cdn.discord.test".into(),
            steam_openid: format!("{base}/openid"),
            steam_api: base,
        }
    }

    fn openid(&self) -> String {
        format!("http://127.0.0.1:{}/openid", self.port)
    }

    /// A cloud with both providers on and pointed at the fakes.
    async fn cloud(&self) -> TestCloud {
        self.cloud_with(|_| {}, |_| {}).await
    }

    async fn cloud_with(
        &self,
        config: impl FnOnce(&mut anyplay_cloud::Config),
        tuning: impl FnOnce(&mut Tuning),
    ) -> TestCloud {
        let endpoints = self.endpoints();
        TestCloud::with(
            |c| {
                c.discord_client_id = Some(CLIENT_ID.into());
                c.discord_client_secret = Some(CLIENT_SECRET.into());
                c.steam_api_key = Some(STEAM_KEY.into());
                config(c);
            },
            |t| {
                t.links = endpoints;
                tuning(t);
            },
        )
        .await
    }

    /// What Discord does when the player agrees: a code for this challenge and this user.
    fn discord_code(&self, challenge: &str, id: &str, global_name: Option<&str>) -> String {
        let mut fake = self.fake.lock().unwrap();
        fake.counter += 1;
        let code = format!("code-{}", fake.counter);
        let user = json!({
            "id": id,
            "username": format!("user{id}"),
            "global_name": global_name,
            "avatar": "a1b2c3",
        });
        fake.codes
            .insert(code.clone(), (challenge.to_owned(), user));
        code
    }

    /// What Steam does when the player agrees: the fields it signs. Steam remembers the answer
    /// until it is checked once.
    fn steam_assertion(&self, steam_id: &str, return_to: &str) -> Vec<(String, String)> {
        let openid = self.openid();
        let claimed = format!("{openid}/id/{steam_id}");
        let mut fake = self.fake.lock().unwrap();
        fake.counter += 1;
        let nonce = format!("2026-10-24T12:00:00Znonce{}", fake.counter);
        let fields: Vec<(String, String)> = [
            ("openid.ns", OPENID_NS.to_owned()),
            ("openid.mode", "id_res".to_owned()),
            ("openid.op_endpoint", format!("{openid}/login")),
            ("openid.claimed_id", claimed.clone()),
            ("openid.identity", claimed),
            ("openid.return_to", return_to.to_owned()),
            ("openid.response_nonce", nonce.clone()),
            ("openid.assoc_handle", "1234567890".to_owned()),
            (
                "openid.signed",
                "signed,op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle"
                    .to_owned(),
            ),
            ("openid.sig", "c2ln".to_owned()),
        ]
        .into_iter()
        .map(|(n, v)| (n.to_owned(), v))
        .collect();
        fake.assertions
            .insert(nonce, fields.iter().cloned().collect());
        fields
    }

    fn revoked(&self) -> Vec<String> {
        self.fake.lock().unwrap().revoked.clone()
    }

    fn set_down(&self, down: bool) {
        self.fake.lock().unwrap().down = down;
    }
}

// The browser

fn params(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn origin(cloud: &TestCloud) -> String {
    cloud.state.web.as_ref().unwrap().origin.clone()
}

/// Starts a link and returns the address the page would send the browser to.
async fn start(portal: &Portal<'_>, provider: &str) -> String {
    let answer = portal
        .post(&format!("/me/links/{provider}/start"), json!({}))
        .await;
    answer.ok(200)["url"].as_str().unwrap().to_owned()
}

/// The provider sends the browser back. Returns the answer; its `Location` is the portal page.
async fn back(portal: &Portal<'_>, provider: &str, query: &str) -> Answer {
    portal
        .dispatch(
            portal
                .client
                .get(portal.url(&format!("/web/links/{provider}/callback?{query}"))),
        )
        .await
}

fn result_of(answer: &Answer, provider: &str) -> String {
    assert_eq!(answer.status, 303, "body: {}", answer.text());
    let location = answer.header("location").unwrap();
    let prefix = format!("/account/connections?link={provider}&result=");
    location
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("unexpected location {location}"))
        .to_owned()
}

fn query(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).unwrap()
}

/// The whole of a Discord link, as the browser walks it. Returns the flag.
async fn link_discord(
    providers: &Providers,
    portal: &Portal<'_>,
    discord_id: &str,
    name: Option<&str>,
) -> String {
    let url = start(portal, "discord").await;
    let sent = params(&url);
    let code = providers.discord_code(&sent["code_challenge"], discord_id, name);
    let answer = back(
        portal,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    result_of(&answer, "discord")
}

/// The same for Steam.
async fn link_steam(providers: &Providers, portal: &Portal<'_>, steam_id: &str) -> String {
    let url = start(portal, "steam").await;
    let sent = params(&url);
    let fields = providers.steam_assertion(steam_id, &sent["openid.return_to"]);
    let mut pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    pairs.push(("state", state_of_return_to(&sent["openid.return_to"])));
    let answer = back(portal, "steam", &query(&pairs)).await;
    result_of(&answer, "steam")
}

fn state_of_return_to(return_to: &str) -> &str {
    return_to.split_once("?state=").unwrap().1
}

async fn links_of(portal: &Portal<'_>) -> Value {
    portal.get("/me/links").await.ok(200)["links"].clone()
}

fn entry<'a>(links: &'a Value, provider: &str) -> &'a Value {
    links
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["provider"] == provider)
        .unwrap()
}

async fn user_with_portal<'a>(cloud: &'a TestCloud, name: &str) -> (User, Portal<'a>) {
    let user = cloud.sign_up(name).await;
    let portal = Portal::signed_in(cloud, &user).await;
    (user, portal)
}

// Discord

#[tokio::test]
async fn a_discord_account_is_linked_listed_and_unlinked() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (alec, portal) = user_with_portal(&cloud, "Alec").await;

    // Both providers are listed, linked or not.
    let links = links_of(&portal).await;
    assert_eq!(links.as_array().unwrap().len(), 2);
    for (provider, available) in [("discord", true), ("steam", true)] {
        let one = entry(&links, provider);
        assert_eq!(one["available"], available);
        assert_eq!(one["linked"], false);
        assert_eq!(one["external_id"], Value::Null);
        assert_eq!(one["visibility"], Value::Null);
    }
    assert_eq!(links[0]["provider"], "discord");

    // The address on Discord: code flow with PKCE, the scope identify, our callback.
    let url = start(&portal, "discord").await;
    assert!(
        url.starts_with("https://discord.test/oauth2/authorize?"),
        "{url}"
    );
    let sent = params(&url);
    assert_eq!(sent["response_type"], "code");
    assert_eq!(sent["client_id"], CLIENT_ID);
    assert_eq!(sent["scope"], "identify");
    assert_eq!(
        sent["redirect_uri"],
        format!("{}/web/links/discord/callback", origin(&cloud))
    );
    assert_eq!(sent["code_challenge_method"], "S256");
    assert!(sent["state"].len() >= 24 && sent["code_challenge"].len() == 43);
    assert!(
        !url.contains(CLIENT_SECRET),
        "the secret never leaves the cloud"
    );

    // The player agrees; Discord sends the browser back.
    let code = providers.discord_code(&sent["code_challenge"], "80351110224678912", Some("Alec D"));
    let answer = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&answer, "discord"), "linked");
    assert_eq!(answer.header("cache-control").as_deref(), Some("no-store"));

    // The token request carried the verifier and the redirect; the token was given back.
    {
        let fake = providers.fake.lock().unwrap();
        let request = fake.token_requests.last().unwrap();
        assert_eq!(request["redirect_uri"], sent["redirect_uri"]);
        assert_eq!(request["code_verifier"].len(), 43);
    }
    let revoked = providers.revoked();
    assert_eq!(revoked.len(), 1, "the token is given back: {revoked:?}");
    assert!(revoked[0].starts_with("token-"));

    let links = links_of(&portal).await;
    let discord = entry(&links, "discord");
    assert_eq!(discord["linked"], true);
    assert_eq!(discord["external_id"], "80351110224678912");
    assert_eq!(discord["display_name"], "Alec D");
    assert_eq!(
        discord["avatar_url"],
        "https://cdn.discord.test/avatars/80351110224678912/a1b2c3.png"
    );
    assert_eq!(
        discord["profile_url"],
        "https://discord.com/users/80351110224678912"
    );
    assert_eq!(discord["visibility"], "friends");
    assert!(discord["linked_at"].as_u64().unwrap() > 1_700_000_000);
    assert_eq!(entry(&links, "steam")["linked"], false);

    // A second start is refused while the account is linked.
    portal
        .post("/me/links/discord/start", json!({}))
        .await
        .error(409, "already_linked");

    // The data export lists the connection, without any token.
    let export = portal.get("/account/export").await.ok(200);
    assert_eq!(export["linked_accounts"][0]["provider"], "discord");
    assert_eq!(
        export["linked_accounts"][0]["external_id"],
        "80351110224678912"
    );
    assert!(!export.to_string().contains("token-"));

    // Unlinking, twice: the second finds nothing and is harmless.
    portal.delete("/me/links/discord").await.ok(200);
    portal.delete("/me/links/discord").await.ok(200);
    assert_eq!(entry(&links_of(&portal).await, "discord")["linked"], false);
    let _ = alec;
}

#[tokio::test]
async fn a_discord_name_falls_back_to_the_username() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    assert_eq!(
        link_discord(&providers, &portal, "42", None).await,
        "linked"
    );
    assert_eq!(
        entry(&links_of(&portal).await, "discord")["display_name"],
        "user42"
    );
}

#[tokio::test]
async fn a_discord_return_that_does_not_belong_to_the_start_links_nothing() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;

    // No start at all.
    let answer = back(&portal, "discord", "code=x&state=y").await;
    assert_eq!(result_of(&answer, "discord"), "expired");

    // A state that does not fit. The start is used up by it: the right state finds nothing.
    let sent = params(&start(&portal, "discord").await);
    let code = providers.discord_code(&sent["code_challenge"], "1", None);
    let wrong = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", "nope")]),
    )
    .await;
    assert_eq!(result_of(&wrong, "discord"), "expired");
    let right = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&right, "discord"), "expired");
    // Discord was not even asked.
    assert!(providers.fake.lock().unwrap().token_requests.is_empty());

    // No state at all.
    let sent = params(&start(&portal, "discord").await);
    let code = providers.discord_code(&sent["code_challenge"], "1", None);
    let none = back(&portal, "discord", &query(&[("code", &code)])).await;
    assert_eq!(result_of(&none, "discord"), "expired");

    // The state of a start of another browser session is no use here.
    let other = Portal::signed_in(&cloud, &cloud.sign_up("Bob").await).await;
    let theirs = params(&start(&other, "discord").await);
    let sent = params(&start(&portal, "discord").await);
    let code = providers.discord_code(&sent["code_challenge"], "1", None);
    let swapped = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", &theirs["state"])]),
    )
    .await;
    assert_eq!(result_of(&swapped, "discord"), "expired");

    // A start of Discord does not finish at the Steam route.
    let sent = params(&start(&portal, "discord").await);
    let steam = back(&portal, "steam", &query(&[("state", &sent["state"])])).await;
    assert_eq!(result_of(&steam, "steam"), "expired");

    // Not a provider: a plain 404, not a redirect.
    assert_eq!(back(&portal, "xbox", "state=a").await.status, 404);

    assert_eq!(entry(&links_of(&portal).await, "discord")["linked"], false);
}

#[tokio::test]
async fn discord_must_confirm_the_pkce_the_code_and_be_up() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;

    // A code that Discord bound to another challenge: the verifier does not fit.
    let sent = params(&start(&portal, "discord").await);
    let other_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"another verifier"));
    let code = providers.discord_code(&other_challenge, "1", None);
    let answer = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&answer, "discord"), "failed");

    // A code that was used before.
    assert_eq!(link_discord(&providers, &portal, "7", None).await, "linked");
    portal.delete("/me/links/discord").await.ok(200);
    let used = providers
        .fake
        .lock()
        .unwrap()
        .used_codes
        .iter()
        .next()
        .cloned()
        .unwrap();
    let sent = params(&start(&portal, "discord").await);
    let answer = back(
        &portal,
        "discord",
        &query(&[("code", &used), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&answer, "discord"), "failed");

    // The player said no on Discord's page.
    let sent = params(&start(&portal, "discord").await);
    let answer = back(
        &portal,
        "discord",
        &query(&[("error", "access_denied"), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&answer, "discord"), "denied");

    // Discord is down.
    providers.set_down(true);
    assert_eq!(link_discord(&providers, &portal, "8", None).await, "failed");
    providers.set_down(false);

    // Nothing was linked by any of this, and a start works again.
    assert_eq!(entry(&links_of(&portal).await, "discord")["linked"], false);
    assert_eq!(link_discord(&providers, &portal, "9", None).await, "linked");
}

#[tokio::test]
async fn a_start_that_waits_too_long_is_expired() {
    let providers = Providers::start().await;
    let cloud = providers
        .cloud_with(|_| {}, |t| t.link_pending_ttl = Duration::from_millis(100))
        .await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    let sent = params(&start(&portal, "discord").await);
    let code = providers.discord_code(&sent["code_challenge"], "1", None);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let answer = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&answer, "discord"), "expired");
}

// One identity, one account

#[tokio::test]
async fn an_identity_belongs_to_one_account_and_an_account_has_one_link_per_provider() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (alec_user, alec) = user_with_portal(&cloud, "Alec").await;
    let (_, bob) = user_with_portal(&cloud, "Bob").await;

    assert_eq!(
        link_discord(&providers, &alec, "555", Some("Shared")).await,
        "linked"
    );
    assert_eq!(link_steam(&providers, &alec, ALEC_STEAM).await, "linked");

    // Bob tries the same identities: refused, and nothing says who has them.
    assert_eq!(link_discord(&providers, &bob, "555", None).await, "taken");
    assert_eq!(link_steam(&providers, &bob, ALEC_STEAM).await, "taken");
    for provider in ["discord", "steam"] {
        assert_eq!(entry(&links_of(&bob).await, provider)["linked"], false);
    }

    // Another identity is fine, and a different provider too.
    assert_eq!(link_discord(&providers, &bob, "556", None).await, "linked");

    // Alec lets go of Discord: the identity is free for Bob after Bob lets go of his.
    alec.delete("/me/links/discord").await.ok(200);
    bob.delete("/me/links/discord").await.ok(200);
    assert_eq!(link_discord(&providers, &bob, "555", None).await, "linked");

    // A second browser of the same account finishes a link while the first still waits.
    let alec_two = Portal::signed_in(&cloud, &alec_user).await;
    let sent = params(&start(&alec, "discord").await);
    assert_eq!(
        link_discord(&providers, &alec_two, "600", None).await,
        "linked"
    );
    let code = providers.discord_code(&sent["code_challenge"], "601", None);
    let late = back(
        &alec,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&late, "discord"), "exists");
    assert_eq!(
        entry(&links_of(&alec).await, "discord")["external_id"],
        "600"
    );
}

#[tokio::test]
async fn deleting_an_account_deletes_its_connections() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (alec, portal) = user_with_portal(&cloud, "Alec").await;
    assert_eq!(
        link_discord(&providers, &portal, "555", None).await,
        "linked"
    );
    anyplay_cloud::db::delete_account(&cloud.state.writer, &cloud.state.db, alec.id)
        .await
        .unwrap();
    assert!(
        links::rows_of(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_empty()
    );
}

// Step-up

#[tokio::test]
async fn linking_needs_a_fresh_session_and_unlinking_does_not() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (alec, portal) = user_with_portal(&cloud, "Alec").await;
    assert_eq!(
        link_discord(&providers, &portal, "555", None).await,
        "linked"
    );

    // An old session cannot start a link ...
    portal.age(&cloud).await;
    portal.delete("/me/links/discord").await.ok(200);
    let body = portal
        .post("/me/links/discord/start", json!({}))
        .await
        .error(403, "step_up_required");
    assert_eq!(body["methods"], json!(["passkey", "email_code"]));
    // ... but it can read, and change who sees one.
    assert_eq!(entry(&links_of(&portal).await, "discord")["linked"], false);

    // After a step-up it can.
    portal.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);
    assert_eq!(
        link_discord(&providers, &portal, "555", None).await,
        "linked"
    );
}

#[tokio::test]
async fn a_session_that_went_stale_while_the_player_agreed_does_not_link() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    let sent = params(&start(&portal, "discord").await);
    let code = providers.discord_code(&sent["code_challenge"], "1", None);
    // The session grows old between the start and the return.
    sqlx::query(
        "UPDATE web_sessions SET created_ms = created_ms - 3600000 WHERE account_id IS NOT NULL",
    )
    .execute(&cloud.state.db)
    .await
    .unwrap();
    let answer = back(
        &portal,
        "discord",
        &query(&[("code", &code), ("state", &sent["state"])]),
    )
    .await;
    assert_eq!(result_of(&answer, "discord"), "fresh");
    assert!(providers.fake.lock().unwrap().token_requests.is_empty());
    assert_eq!(entry(&links_of(&portal).await, "discord")["linked"], false);
}

#[tokio::test]
async fn the_return_of_a_browser_without_a_session_goes_to_sign_in() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let stranger = Portal::new(&cloud);
    let answer = back(&stranger, "discord", "code=x&state=y").await;
    assert_eq!(answer.status, 303);
    assert_eq!(answer.header("location").as_deref(), Some("/sign-in"));
}

// The hop from the provider's site

#[tokio::test]
async fn a_cross_site_return_is_forwarded_so_that_the_strict_cookie_comes_along() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    let sent = params(&start(&portal, "discord").await);
    let code = providers.discord_code(&sent["code_challenge"], "5", None);
    let q = query(&[("code", &code), ("state", &sent["state"])]);

    // The first request comes from Discord's page. Nothing is done with it.
    let hop = portal
        .dispatch(
            portal
                .client
                .get(portal.url(&format!("/web/links/discord/callback?{q}")))
                .header("sec-fetch-site", "cross-site"),
        )
        .await;
    assert_eq!(hop.status, 200);
    assert!(hop.header("content-type").unwrap().starts_with("text/html"));
    let page = hop.text();
    assert!(
        page.contains(&format!(
            "http-equiv=\"refresh\" content=\"0;url=/web/links/discord/callback?{}\"",
            q.replace('&', "&amp;")
        )),
        "{page}"
    );
    assert!(providers.fake.lock().unwrap().token_requests.is_empty());

    // The page forwards: the same address, now from our own page.
    let done = portal
        .dispatch(
            portal
                .client
                .get(portal.url(&format!("/web/links/discord/callback?{q}")))
                .header("sec-fetch-site", "same-origin"),
        )
        .await;
    assert_eq!(result_of(&done, "discord"), "linked");

    // What comes in the query is escaped where it is put into the page.
    let hostile =
        portal
            .dispatch(
                portal
                    .client
                    .get(portal.url(
                        "/web/links/discord/callback?a=%22%3E%3Cscript%3Ealert(1)%3C/script%3E",
                    ))
                    .header("sec-fetch-site", "cross-site"),
            )
            .await;
    assert_eq!(hostile.status, 200);
    assert!(!hostile.text().contains("<script>"), "{}", hostile.text());
    assert!(hostile.text().contains("%22%3E%3Cscript"));
}

// Steam

#[tokio::test]
async fn a_steam_account_is_linked_with_its_persona() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;

    let url = start(&portal, "steam").await;
    assert!(
        url.starts_with(&format!("{}/login?", providers.openid())),
        "{url}"
    );
    let sent = params(&url);
    assert_eq!(sent["openid.ns"], OPENID_NS);
    assert_eq!(sent["openid.mode"], "checkid_setup");
    let select = "http://specs.openid.net/auth/2.0/identifier_select";
    assert_eq!(sent["openid.identity"], select);
    assert_eq!(sent["openid.claimed_id"], select);
    assert_eq!(sent["openid.realm"], origin(&cloud));
    let return_to = &sent["openid.return_to"];
    assert!(
        return_to.starts_with(&format!(
            "{}/web/links/steam/callback?state=",
            origin(&cloud)
        )),
        "{return_to}"
    );
    assert!(!url.contains(STEAM_KEY));

    let fields = providers.steam_assertion(ALEC_STEAM, return_to);
    let mut pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    pairs.push(("state", state_of_return_to(return_to)));
    let answer = back(&portal, "steam", &query(&pairs)).await;
    assert_eq!(result_of(&answer, "steam"), "linked");

    let links = links_of(&portal).await;
    let steam = entry(&links, "steam");
    assert_eq!(steam["external_id"], ALEC_STEAM);
    assert_eq!(steam["display_name"], "alec_on_steam");
    assert_eq!(
        steam["avatar_url"],
        "https://avatars.steamstatic.com/abc_full.jpg"
    );
    assert_eq!(
        steam["profile_url"],
        format!("https://steamcommunity.com/profiles/{ALEC_STEAM}")
    );
    assert_eq!(steam["visibility"], "friends");

    // The assertion cannot be used again.
    let again = back(&portal, "steam", &query(&pairs)).await;
    assert_eq!(result_of(&again, "steam"), "expired");
}

#[tokio::test]
async fn a_picture_that_is_not_steams_is_dropped_and_a_missing_key_leaves_the_id_alone() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Bob").await;
    assert_eq!(link_steam(&providers, &portal, BOB_STEAM).await, "linked");
    let steam = entry(&links_of(&portal).await, "steam").clone();
    assert_eq!(steam["display_name"], "bob");
    assert_eq!(steam["avatar_url"], Value::Null);

    // Without a Web API key: the ID only.
    let keyless = providers
        .cloud_with(|c| c.steam_api_key = None, |_| {})
        .await;
    let (_, portal) = user_with_portal(&keyless, "Alec").await;
    assert_eq!(link_steam(&providers, &portal, ALEC_STEAM).await, "linked");
    let steam = entry(&links_of(&portal).await, "steam").clone();
    assert_eq!(steam["external_id"], ALEC_STEAM);
    assert_eq!(steam["display_name"], Value::Null);
    assert_eq!(steam["avatar_url"], Value::Null);

    // Steam's Web API failing does not fail the link either.
    let (_, portal) = user_with_portal(&cloud, "Cleo").await;
    providers.set_down(true);
    assert_eq!(link_steam(&providers, &portal, ALEC_STEAM).await, "linked");
    providers.set_down(false);
    assert_eq!(
        entry(&links_of(&portal).await, "steam")["display_name"],
        Value::Null
    );
}

/// A Steam return built by hand: starts a link, lets `tamper` change the signed fields (Steam
/// signed the original), and returns the flag.
async fn steam_return(
    providers: &Providers,
    portal: &Portal<'_>,
    steam_id: &str,
    tamper: impl FnOnce(&mut Vec<(String, String)>, &str),
) -> String {
    let sent = params(&start(portal, "steam").await);
    let return_to = sent["openid.return_to"].clone();
    let mut fields = providers.steam_assertion(steam_id, &return_to);
    tamper(&mut fields, &return_to);
    let mut pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let state = state_of_return_to(&return_to).to_owned();
    if !pairs.iter().any(|(n, _)| *n == "state") {
        pairs.push(("state", &state));
    }
    result_of(&back(portal, "steam", &query(&pairs)).await, "steam")
}

fn set(fields: &mut [(String, String)], name: &str, value: &str) {
    for field in fields.iter_mut().filter(|(n, _)| n == name) {
        field.1 = value.to_owned();
    }
}

#[tokio::test]
async fn a_forged_steam_return_links_nothing() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    let openid = providers.openid();

    // The claimed ID changed after Steam signed: Steam says no.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        let forged = format!("{openid}/id/{BOB_STEAM}");
        set(f, "openid.claimed_id", &forged);
        set(f, "openid.identity", &forged);
    })
    .await;
    assert_eq!(flag, "failed");

    // An answer nobody signed: Steam has never seen the nonce.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        set(f, "openid.response_nonce", "made-up-nonce");
    })
    .await;
    assert_eq!(flag, "failed");

    // Signed by Steam, but for another return address: another site's, or another state.
    for other in [
        "https://evil.example/web/links/steam/callback?state=x".to_owned(),
        format!("{}/web/links/steam/callback?state=another", origin(&cloud)),
        format!("{}/web/links/steam/callback", origin(&cloud)),
    ] {
        let sent = params(&start(&portal, "steam").await);
        let state = state_of_return_to(&sent["openid.return_to"]).to_owned();
        // Steam signed whatever return_to the attacker's own start asked for.
        let fields = providers.steam_assertion(ALEC_STEAM, &other);
        let mut pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        pairs.push(("state", &state));
        let flag = result_of(&back(&portal, "steam", &query(&pairs)).await, "steam");
        assert_eq!(flag, "failed", "{other}");
    }

    // A claimed ID that is not Steam's, signed by "Steam".
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        // The fake signed the right values; the attacker's copy of the assertion differs and the
        // check at Steam fails too. Either way the cloud's own rule must refuse first.
        set(
            f,
            "openid.claimed_id",
            "https://evil.example/openid/id/76561198000000001",
        );
        set(
            f,
            "openid.identity",
            "https://evil.example/openid/id/76561198000000001",
        );
    })
    .await;
    assert_eq!(flag, "failed");

    // The identity differs from the claimed ID.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        let other = format!("{openid}/id/{BOB_STEAM}");
        set(f, "openid.identity", &other);
    })
    .await;
    assert_eq!(flag, "failed");

    // Not an answer: another mode, another version, an endpoint that is not Steam's.
    for (name, value) in [
        ("openid.mode", "setup_needed"),
        ("openid.ns", "http://specs.openid.net/auth/1.1"),
        ("openid.op_endpoint", "https://evil.example/openid/login"),
    ] {
        let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| set(f, name, value)).await;
        assert_eq!(flag, "failed", "{name}");
    }

    // The fields that matter must be signed.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        set(
            f,
            "openid.signed",
            "signed,op_endpoint,claimed_id,identity,response_nonce,assoc_handle",
        );
    })
    .await;
    assert_eq!(flag, "failed");

    // A field that comes twice is a trick to make the check and the use read different things.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        f.push((
            "openid.claimed_id".into(),
            format!("{openid}/id/{BOB_STEAM}"),
        ));
    })
    .await;
    assert_eq!(flag, "failed");

    // A SteamID64 that cannot be an individual's.
    let flag = steam_return(&providers, &portal, "76561197960265728", |_, _| {}).await;
    assert_eq!(flag, "failed");

    // The state in the query does not belong to the start.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        f.push(("state".into(), "not-the-state".into()));
    })
    .await;
    assert_eq!(flag, "expired");

    // The player cancelled on Steam's page.
    let flag = steam_return(&providers, &portal, ALEC_STEAM, |f, _| {
        set(f, "openid.mode", "cancel");
    })
    .await;
    assert_eq!(flag, "denied");

    assert_eq!(entry(&links_of(&portal).await, "steam")["linked"], false);
    // The honest one still works afterwards.
    assert_eq!(link_steam(&providers, &portal, ALEC_STEAM).await, "linked");
}

// Visibility

#[tokio::test]
async fn the_visibility_of_a_connection_is_set_per_provider() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    assert_eq!(
        link_discord(&providers, &portal, "555", Some("Alec D")).await,
        "linked"
    );

    for audience in ["everyone", "nobody", "friends"] {
        let view = portal
            .send(
                Method::PATCH,
                "/me/links/discord",
                json!({"visibility": audience}),
            )
            .await
            .ok(200);
        assert_eq!(view["provider"], "discord");
        assert_eq!(view["visibility"], audience);
        assert_eq!(
            entry(&links_of(&portal).await, "discord")["visibility"],
            audience
        );
    }
    portal
        .send(
            Method::PATCH,
            "/me/links/discord",
            json!({"visibility": "world"}),
        )
        .await
        .error(400, "bad_request");
    portal
        .send(Method::PATCH, "/me/links/discord", json!({}))
        .await
        .error(400, "bad_request");
    // Nothing to change when there is no connection, or no such provider.
    portal
        .send(
            Method::PATCH,
            "/me/links/steam",
            json!({"visibility": "everyone"}),
        )
        .await
        .error(404, "not_found");
    portal
        .send(
            Method::PATCH,
            "/me/links/xbox",
            json!({"visibility": "everyone"}),
        )
        .await
        .error(404, "not_found");
    portal
        .delete("/me/links/xbox")
        .await
        .error(404, "not_found");
    portal
        .post("/me/links/xbox/start", json!({}))
        .await
        .error(404, "not_found");

    // A new connection starts with friends, whatever the last one had.
    portal
        .send(
            Method::PATCH,
            "/me/links/discord",
            json!({"visibility": "everyone"}),
        )
        .await
        .ok(200);
    portal.delete("/me/links/discord").await.ok(200);
    assert_eq!(
        link_discord(&providers, &portal, "555", None).await,
        "linked"
    );
    assert_eq!(
        entry(&links_of(&portal).await, "discord")["visibility"],
        "friends"
    );
}

fn shown(profile: &Value) -> Vec<String> {
    profile["links"]
        .as_array()
        .unwrap_or_else(|| panic!("links is a list: {profile}"))
        .iter()
        .map(|l| l["provider"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_profile_shows_the_connections_that_the_viewer_may_see() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (alec, a) = user_with_portal(&cloud, "Alec").await;
    let (bob, b) = user_with_portal(&cloud, "Bob").await;
    let (_, c) = user_with_portal(&cloud, "Cleo").await;
    let (dave, d) = user_with_portal(&cloud, "Dave").await;
    let (erin, e) = user_with_portal(&cloud, "Erin").await;
    let signed_out = Portal::new(&cloud);
    signed_out.session().await;

    // Bob is a friend. Dave is blocked by Alec; Erin blocked Alec.
    b.post("/friends/requests", json!({"handle": "alec"}))
        .await
        .ok(200);
    a.post(&format!("/friends/requests/{}/accept", bob.id), json!({}))
        .await
        .ok(200);
    a.send(Method::PUT, &format!("/blocks/{}", dave.id), json!({}))
        .await
        .ok(200);
    e.send(Method::PUT, &format!("/blocks/{}", alec.id), json!({}))
        .await
        .ok(200);
    let _ = (dave, erin);

    assert_eq!(
        link_discord(&providers, &a, "555", Some("Alec D")).await,
        "linked"
    );
    assert_eq!(link_steam(&providers, &a, ALEC_STEAM).await, "linked");
    let viewers = [
        ("out", &signed_out),
        ("friend", &b),
        ("stranger", &c),
        ("blocked", &d),
        ("blocker", &e),
        ("self", &a),
    ];
    let see = |viewer: &str, discord: &str, steam: &str| -> Vec<&'static str> {
        let friend = matches!(viewer, "friend" | "self");
        let all = |audience: &str| match audience {
            "everyone" => true,
            "friends" => friend,
            _ => viewer == "self",
        };
        let mut out = Vec::new();
        if all(discord) {
            out.push("discord");
        }
        if all(steam) {
            out.push("steam");
        }
        out
    };

    // The default is friends: strangers, visitors and blocked viewers see none.
    let cases = [
        ("friends", "friends"),
        ("everyone", "friends"),
        ("friends", "nobody"),
        ("nobody", "everyone"),
        ("everyone", "everyone"),
        ("nobody", "nobody"),
    ];
    for (discord, steam) in cases {
        for (provider, audience) in [("discord", discord), ("steam", steam)] {
            a.send(
                Method::PATCH,
                &format!("/me/links/{provider}"),
                json!({"visibility": audience}),
            )
            .await
            .ok(200);
        }
        for (name, viewer) in viewers {
            let profile = viewer.get("/users/alec").await.ok(200);
            assert_eq!(
                shown(&profile),
                see(name, discord, steam),
                "{name} sees discord={discord} steam={steam}: {profile}"
            );
        }
    }

    // A blocked viewer sees exactly what a visitor who is not signed in sees.
    a.send(
        Method::PATCH,
        "/me/links/discord",
        json!({"visibility": "everyone"}),
    )
    .await
    .ok(200);
    a.send(
        Method::PATCH,
        "/me/links/steam",
        json!({"visibility": "friends"}),
    )
    .await
    .ok(200);
    for viewer in [&d, &e] {
        assert_eq!(shown(&viewer.get("/users/alec").await.ok(200)), ["discord"]);
    }

    // What a public link holds: enough to show and to follow, nothing about the setting.
    let profile = c.get("/users/alec").await.ok(200);
    assert_eq!(
        profile["links"][0],
        json!({
            "provider": "discord",
            "external_id": "555",
            "display_name": "Alec D",
            "avatar_url": "https://cdn.discord.test/avatars/555/a1b2c3.png",
            "profile_url": "https://discord.com/users/555",
        })
    );
    // The player's own page shows the profile with all of it.
    assert_eq!(
        shown(&a.get("/users/alec").await.ok(200)),
        ["discord", "steam"]
    );
}

// Off, rate limits and the page's policy

#[tokio::test]
async fn a_provider_without_settings_is_off() {
    let cloud = TestCloud::start().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    let links = links_of(&portal).await;
    assert_eq!(entry(&links, "discord")["available"], false);
    assert_eq!(entry(&links, "steam")["available"], true);
    portal
        .post("/me/links/discord/start", json!({}))
        .await
        .error(409, "provider_unavailable");
    // A return for it finds no start.
    let answer = back(&portal, "discord", "code=x&state=y").await;
    assert_eq!(result_of(&answer, "discord"), "expired");
}

#[tokio::test]
async fn starting_a_link_is_limited_per_account() {
    let providers = Providers::start().await;
    let cloud = providers
        .cloud_with(
            |_| {},
            |t| {
                t.rates = Rates {
                    link_starts: 2,
                    ..t.rates.clone()
                }
            },
        )
        .await;
    let (_, a) = user_with_portal(&cloud, "Alec").await;
    let (_, b) = user_with_portal(&cloud, "Bob").await;
    start(&a, "discord").await;
    start(&a, "steam").await;
    let limited = a.post("/me/links/discord/start", json!({})).await;
    limited.error(429, "rate_limited");
    assert!(limited.header("retry-after").is_some());
    // The limit hangs on the account.
    start(&b, "discord").await;
}

#[tokio::test]
async fn the_writes_need_the_csrf_token_and_a_session() {
    let providers = Providers::start().await;
    let cloud = providers.cloud().await;
    let (_, portal) = user_with_portal(&cloud, "Alec").await;
    let no_token = portal
        .dispatch(
            portal
                .client
                .post(portal.url("/web/api/v1/me/links/discord/start"))
                .json(&json!({})),
        )
        .await;
    no_token.error(401, "unauthorized");
    let signed_out = Portal::new(&cloud);
    signed_out.session().await;
    signed_out.get("/me/links").await.error(401, "unauthorized");
    signed_out
        .post("/me/links/discord/start", json!({}))
        .await
        .error(401, "unauthorized");
}
