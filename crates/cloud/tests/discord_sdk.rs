//! The token routes for Discord's Social SDK, against a fake Discord token endpoint that runs in
//! this process: the code and PKCE exchange, refresh, the redirect allow-list, the identity check
//! against a linked account, rate limits, and that the cloud keeps nothing.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyplay_api::{DiscordTokens, path};
use anyplay_cloud::Tuning;
use anyplay_cloud::config::LinkEndpoints;
use anyplay_cloud::limits::Rates;
use anyplay_cloud::links::{self, Identity, Provider};
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{TestCloud, User};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const CLIENT_ID: &str = "1563714917998133288";
const CLIENT_SECRET: &str = "s3cret-of-the-test-app";
const REDIRECT: &str = "http://127.0.0.1/callback";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

#[derive(Default)]
struct Fake {
    /// Codes not yet used: code to (PKCE challenge, redirect, Discord user ID).
    codes: HashMap<String, (String, String, String)>,
    /// Access tokens to user IDs; refresh tokens to user IDs.
    access: HashMap<String, String>,
    refresh: HashMap<String, String>,
    revoked: Vec<String>,
    /// Every form the token endpoint received.
    requests: Vec<HashMap<String, String>>,
    /// Answer the token endpoint with this status instead, when set.
    token_status: Option<u16>,
    counter: u32,
}

type Shared = Arc<Mutex<Fake>>;

fn form(body: &[u8]) -> HashMap<String, String> {
    serde_urlencoded::from_bytes(body).unwrap_or_default()
}

fn error(status: StatusCode, why: &str) -> axum::response::Response {
    (status, axum::Json(json!({"error": why}))).into_response()
}

async fn token(State(fake): State<Shared>, body: Bytes) -> axum::response::Response {
    let mut fake = fake.lock().unwrap();
    let fields = form(&body);
    fake.requests.push(fields.clone());
    if let Some(status) = fake.token_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    if fields.get("client_id").map(String::as_str) != Some(CLIENT_ID)
        || fields.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET)
    {
        return error(StatusCode::UNAUTHORIZED, "invalid_client");
    }
    let user = match fields.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let code = fields.get("code").cloned().unwrap_or_default();
            let Some((challenge, redirect, user)) = fake.codes.remove(&code) else {
                return error(StatusCode::BAD_REQUEST, "invalid_grant");
            };
            let verifier = fields.get("code_verifier").cloned().unwrap_or_default();
            if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != challenge
                || fields.get("redirect_uri") != Some(&redirect)
            {
                return error(StatusCode::BAD_REQUEST, "invalid_grant");
            }
            user
        }
        Some("refresh_token") => {
            let old = fields.get("refresh_token").cloned().unwrap_or_default();
            // A refresh token is replaced at each use.
            let Some(user) = fake.refresh.remove(&old) else {
                return error(StatusCode::BAD_REQUEST, "invalid_grant");
            };
            user
        }
        _ => return error(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    };
    fake.counter += 1;
    let (access, refresh) = (
        format!("access-{}", fake.counter),
        format!("refresh-{}", fake.counter),
    );
    fake.access.insert(access.clone(), user.clone());
    fake.refresh.insert(refresh.clone(), user);
    axum::Json(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": 604_800,
        "refresh_token": refresh,
        "scope": "identify openid sdk.social_layer_presence",
    }))
    .into_response()
}

async fn me(State(fake): State<Shared>, headers: HeaderMap) -> axum::response::Response {
    let fake = fake.lock().unwrap();
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    match fake.access.get(token) {
        Some(id) => axum::Json(json!({"id": id, "username": "player"})).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn revoke(State(fake): State<Shared>, body: Bytes) -> axum::response::Response {
    if let Some(token) = form(&body).get("token") {
        fake.lock().unwrap().revoked.push(token.clone());
    }
    axum::Json(json!({})).into_response()
}

struct Discord {
    fake: Shared,
    port: u16,
}

impl Discord {
    async fn start() -> Self {
        let fake: Shared = Arc::default();
        let app = Router::new()
            .route("/api/oauth2/token", post(token))
            .route("/api/oauth2/token/revoke", post(revoke))
            .route("/api/users/@me", get(me))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { fake, port }
    }

    async fn cloud(&self, tuning: impl FnOnce(&mut Tuning)) -> TestCloud {
        let base = format!("http://127.0.0.1:{}", self.port);
        TestCloud::with(
            |c| {
                c.discord_client_id = Some(CLIENT_ID.into());
                c.discord_client_secret = Some(CLIENT_SECRET.into());
            },
            |t| {
                t.links = LinkEndpoints {
                    discord_api: format!("{base}/api"),
                    ..LinkEndpoints::default()
                };
                tuning(t);
            },
        )
        .await
    }

    /// What Discord does when the player agrees: a code for the verifier's challenge.
    fn code_for(&self, user_id: &str) -> String {
        let mut fake = self.fake.lock().unwrap();
        fake.counter += 1;
        let code = format!("code-{}", fake.counter);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
        fake.codes
            .insert(code.clone(), (challenge, REDIRECT.into(), user_id.into()));
        code
    }

    fn revoked(&self) -> Vec<String> {
        self.fake.lock().unwrap().revoked.clone()
    }

    fn requests(&self) -> Vec<HashMap<String, String>> {
        self.fake.lock().unwrap().requests.clone()
    }
}

/// Every value of every table, as text, for either database.
async fn dump(pool: &anyplay_cloud::db::Pool) -> String {
    use sqlx::Row as _;
    let mut out = String::new();
    let sqlite = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(pool)
        .await;
    if let Ok(tables) = sqlite {
        for table in tables {
            let table: String = table.get(0);
            let columns = sqlx::query(sqlx::AssertSqlSafe(format!(
                "PRAGMA table_info(\"{table}\")"
            )))
            .fetch_all(pool)
            .await
            .unwrap();
            let names: Vec<String> = columns.iter().map(|c| c.get::<String, _>(1)).collect();
            if names.is_empty() {
                continue;
            }
            let joined = names
                .iter()
                .map(|n| format!("COALESCE(CAST(\"{n}\" AS TEXT), '')"))
                .collect::<Vec<_>>()
                .join(" || '|' || ");
            for row in sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {joined} FROM \"{table}\""
            )))
            .fetch_all(pool)
            .await
            .unwrap()
            {
                out.push_str(&row.get::<String, _>(0));
                out.push('\n');
            }
        }
    } else {
        let tables = sqlx::query("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")
            .fetch_all(pool)
            .await
            .unwrap();
        for table in tables {
            let table: String = table.get(0);
            for row in sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT t::text FROM \"{table}\" t"
            )))
            .fetch_all(pool)
            .await
            .unwrap()
            {
                out.push_str(&row.get::<String, _>(0));
                out.push('\n');
            }
        }
    }
    assert!(!out.is_empty(), "the dump read nothing");
    out
}

fn exchange(code: &str) -> Value {
    json!({"code": code, "code_verifier": VERIFIER, "redirect_uri": REDIRECT})
}

async fn alec(cloud: &TestCloud) -> User {
    cloud.sign_up("Alec").await
}

#[tokio::test]
async fn a_code_is_traded_for_tokens_with_the_secret_and_the_verifier() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    let code = discord.code_for("4242");
    let reply = cloud
        .post(path::ME_DISCORD_TOKEN, user.token(), exchange(&code))
        .await
        .ok(200);
    assert_eq!(reply.headers.get("cache-control").unwrap(), "no-store");
    let tokens: DiscordTokens = reply.json();
    assert_eq!(tokens.access, "access-2");
    assert_eq!(tokens.refresh, "refresh-2");
    assert_eq!(tokens.expires_in, 604_800);
    let sent = &discord.requests()[0];
    assert_eq!(sent["grant_type"], "authorization_code");
    assert_eq!(sent["client_secret"], CLIENT_SECRET);
    assert_eq!(sent["code_verifier"], VERIFIER);
    assert_eq!(sent["redirect_uri"], REDIRECT);
    // A used code is spent.
    cloud
        .post(path::ME_DISCORD_TOKEN, user.token(), exchange(&code))
        .await
        .is_error("grant_refused", 400);
}

#[tokio::test]
async fn a_refresh_token_is_traded_and_replaced_at_each_use() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    let first: DiscordTokens = cloud
        .post(
            path::ME_DISCORD_TOKEN,
            user.token(),
            exchange(&discord.code_for("4242")),
        )
        .await
        .ok(200)
        .json();
    let second: DiscordTokens = cloud
        .post(
            path::ME_DISCORD_REFRESH,
            user.token(),
            json!({"refresh": first.refresh}),
        )
        .await
        .ok(200)
        .json();
    assert_ne!(second.refresh, first.refresh);
    assert_eq!(discord.requests()[1]["grant_type"], "refresh_token");
    // The old one no longer works: Discord refuses and the cloud says so.
    cloud
        .post(
            path::ME_DISCORD_REFRESH,
            user.token(),
            json!({"refresh": first.refresh}),
        )
        .await
        .is_error("grant_refused", 400);
}

#[tokio::test]
async fn the_routes_need_a_signed_in_device() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    cloud
        .post(path::ME_DISCORD_TOKEN, "nope", exchange("c"))
        .await
        .is_error("unauthorized", 401);
    cloud
        .post(path::ME_DISCORD_REFRESH, "nope", json!({"refresh": "r"}))
        .await
        .is_error("unauthorized", 401);
    assert!(discord.requests().is_empty(), "Discord was not asked");
}

#[tokio::test]
async fn a_cloud_without_a_discord_application_has_no_routes_to_offer() {
    let cloud = TestCloud::start().await;
    let user = alec(&cloud).await;
    cloud
        .post(path::ME_DISCORD_TOKEN, user.token(), exchange("c"))
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            path::ME_DISCORD_REFRESH,
            user.token(),
            json!({"refresh": "r"}),
        )
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn bad_input_never_reaches_discord() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    let bad = [
        json!({"code": "c", "code_verifier": VERIFIER, "redirect_uri": "https://evil.example/cb"}),
        json!({"code": "c", "code_verifier": "short", "redirect_uri": REDIRECT}),
        json!({"code": "", "code_verifier": VERIFIER, "redirect_uri": REDIRECT}),
        json!({"code": "has space", "code_verifier": VERIFIER, "redirect_uri": REDIRECT}),
        json!({"code": "c"}),
    ];
    for body in bad {
        cloud
            .post(path::ME_DISCORD_TOKEN, user.token(), body)
            .await
            .is_error("bad_request", 400);
    }
    cloud
        .post(
            path::ME_DISCORD_REFRESH,
            user.token(),
            json!({"refresh": ""}),
        )
        .await
        .is_error("bad_request", 400);
    assert!(discord.requests().is_empty());
}

#[tokio::test]
async fn the_portals_callback_is_a_registered_redirect_too() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    let origin = cloud.state.web.as_ref().unwrap().origin.clone();
    let portal = format!("{origin}/web/links/discord/callback");
    let code = discord.code_for("4242");
    discord.fake.lock().unwrap().codes.get_mut(&code).unwrap().1 = portal.clone();
    cloud
        .post(
            path::ME_DISCORD_TOKEN,
            user.token(),
            json!({"code": code, "code_verifier": VERIFIER, "redirect_uri": portal}),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn a_linked_account_may_only_connect_its_own_discord_identity() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    links::insert(
        &cloud.state.db,
        user.id,
        Provider::Discord,
        &Identity {
            external_id: "4242".into(),
            display_name: Some("Alec".into()),
            avatar: None,
        },
        1,
    )
    .await
    .unwrap();
    // Someone else's Discord account: refused, and the tokens are revoked at Discord.
    let other = discord.code_for("999");
    cloud
        .post(path::ME_DISCORD_TOKEN, user.token(), exchange(&other))
        .await
        .is_error("forbidden", 403);
    assert_eq!(discord.revoked(), ["access-2", "refresh-2"]);
    // The linked one works.
    cloud
        .post(
            path::ME_DISCORD_TOKEN,
            user.token(),
            exchange(&discord.code_for("4242")),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn discord_failures_are_told_apart() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    // Discord is down: the player may try again later.
    discord.fake.lock().unwrap().token_status = Some(503);
    cloud
        .post(
            path::ME_DISCORD_TOKEN,
            user.token(),
            exchange(&discord.code_for("1")),
        )
        .await
        .is_error("internal", 500);
    // Discord throttles the cloud.
    discord.fake.lock().unwrap().token_status = Some(429);
    cloud
        .post(
            path::ME_DISCORD_REFRESH,
            user.token(),
            json!({"refresh": "r"}),
        )
        .await
        .is_error("rate_limited", 429);
    // The cloud's own secret is wrong: not the player's fault.
    discord.fake.lock().unwrap().token_status = Some(401);
    cloud
        .post(
            path::ME_DISCORD_REFRESH,
            user.token(),
            json!({"refresh": "r"}),
        )
        .await
        .is_error("internal", 500);
}

#[tokio::test]
async fn trades_are_limited_per_account() {
    let discord = Discord::start().await;
    let cloud = discord
        .cloud(|t| {
            t.rates = Rates {
                discord_grants_per_account: 2,
                ..t.rates.clone()
            }
        })
        .await;
    let a = alec(&cloud).await;
    let b = cloud.sign_up("Bob").await;
    for _ in 0..2 {
        cloud
            .post(path::ME_DISCORD_REFRESH, a.token(), json!({"refresh": "r"}))
            .await
            .is_error("grant_refused", 400);
    }
    let limited = cloud
        .post(path::ME_DISCORD_REFRESH, a.token(), json!({"refresh": "r"}))
        .await;
    limited.is_error("rate_limited", 429);
    assert!(limited.headers.get("retry-after").is_some());
    // Another account has its own allowance.
    cloud
        .post(path::ME_DISCORD_REFRESH, b.token(), json!({"refresh": "r"}))
        .await
        .is_error("grant_refused", 400);
}

#[tokio::test]
async fn nothing_of_discord_is_kept_in_the_database() {
    let discord = Discord::start().await;
    let cloud = discord.cloud(|_| {}).await;
    let user = alec(&cloud).await;
    let code = discord.code_for("987654321987654321");
    let tokens: DiscordTokens = cloud
        .post(path::ME_DISCORD_TOKEN, user.token(), exchange(&code))
        .await
        .ok(200)
        .json();
    // Every table, every text column: neither a token nor the Discord ID appears anywhere.
    let dump = dump(&cloud.state.db).await;
    for secret in [
        tokens.access.as_str(),
        tokens.refresh.as_str(),
        "987654321987654321",
        code.as_str(),
    ] {
        assert!(!dump.contains(secret), "{secret} was stored");
    }
    let rows = links::rows_of(&cloud.state.db, user.id).await.unwrap();
    assert!(rows.is_empty(), "connecting creates no connection");
}
