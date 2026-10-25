//! The JSON API of the web portal (`/web/api/v1`) and the serving of the built portal: sign in
//! and up as steps, the account, step-up, the hand-off to the app. Friends, profiles and trophies
//! are in `portal_social.rs`. Every test runs on SQLite, and on PostgreSQL when `ANYPLAY_TEST_DATABASE_URL` is set.

mod common;

use std::time::Duration;

use anyplay_api::{BrowserTokenRequest, Device, Platform, path};
use anyplay_proof::{DeviceKey, pkce};
use common::TestCloud;
use common::portal::{API, Answer, Portal};
use reqwest::Method;
use serde_json::{Value, json};
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

const ADMIN: &str = "admin-token-0123456789";
const REDIRECT: &str = "http://127.0.0.1:52000/callback";
const STATE: &str = "state-0123456789abcdef";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn origin_of(cloud: &TestCloud) -> Url {
    Url::parse(&cloud.state.web.as_ref().unwrap().origin).unwrap()
}

// The session and the flow

#[tokio::test]
async fn the_session_starts_with_a_token_and_a_cookie() {
    let cloud = TestCloud::start().await;
    let portal = Portal::new(&cloud);
    let answer = portal.get("/session").await;
    let view = answer.ok(200);
    assert_eq!(view["signed_in"], false);
    assert_eq!(view["account"], Value::Null);
    assert_eq!(view["flow"]["step"], "email");
    assert!(view["csrf"].as_str().is_some_and(|t| t.len() >= 16));
    assert_eq!(view["cloud"]["name"], "Test cloud");
    let cookie = answer.header("set-cookie").expect("a cookie");
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    // The same browser gets the same session back, without a new cookie.
    let again = portal.get("/session").await;
    assert_eq!(again.ok(200)["csrf"], view["csrf"]);
    assert!(again.header("set-cookie").is_none());
    // The answers carry the security headers of the account web.
    let csp = answer.header("content-security-policy").unwrap();
    assert!(csp.contains("script-src 'self'") && csp.contains("style-src 'self'"));
    assert_eq!(answer.header("cache-control").as_deref(), Some("no-store"));
}

#[tokio::test]
async fn an_account_signs_in_by_code_and_out_again() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::new(&cloud);
    portal.session().await;
    let first = portal.get("/session").await.json();

    portal
        .post("/sign-in/email", json!({"email": alec.email}))
        .await
        .ok(200);
    // A wrong code is explained and leaves the step where it is.
    portal
        .post("/sign-in/code", json!({"code": "000000"}))
        .await
        .error(400, "code_wrong");
    let step = portal.get("/session").await.ok(200);
    assert_eq!(step["flow"]["step"], "code");
    assert_eq!(step["flow"]["email"], alec.email);

    let code = cloud.mail.last_code(&alec.email).unwrap();
    let done = portal
        .post("/sign-in/code", json!({"code": code}))
        .await
        .ok(200);
    assert_eq!(done["flow"]["step"], "done");
    assert_eq!(done["signed_in"], true);
    assert_eq!(done["account"]["handle"], "Alec");
    assert_eq!(done["account"]["email"], alec.email);
    // Signing in made a new session: another token.
    assert_ne!(done["csrf"], first["csrf"]);
    assert!(done["fresh_until"].as_u64().unwrap() > now());

    portal.post("/sign-out", json!({})).await.ok(200);
    let after = portal.session().await;
    assert_eq!(after["signed_in"], false);
    portal.get("/account").await.error(401, "unauthorized");
}

#[tokio::test]
async fn a_code_can_be_sent_again_and_the_notice_shows_once() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::new(&cloud);
    portal.session().await;
    portal
        .post("/sign-in/email", json!({"email": alec.email}))
        .await
        .ok(200);
    let old = cloud.mail.last_code(&alec.email).unwrap();
    let resent = portal.post("/sign-in/code/resend", json!({})).await.ok(200);
    assert_eq!(resent["flow"]["step"], "code");
    assert_eq!(resent["notice"], "We sent a new code.");
    assert_eq!(portal.session().await["notice"], Value::Null);
    let new = cloud.mail.last_code(&alec.email).unwrap();
    // The old code no longer counts, the new one does.
    if old != new {
        portal
            .post("/sign-in/code", json!({"code": old}))
            .await
            .error(400, "code_wrong");
    }
    let code = cloud.mail.last_code(&alec.email).unwrap();
    let done = portal
        .post("/sign-in/code", json!({"code": code}))
        .await
        .ok(200);
    assert_eq!(done["flow"]["step"], "done");
}

#[tokio::test]
async fn a_new_address_creates_an_account_in_steps() {
    let cloud = TestCloud::with(|c| c.terms_version = Some("2026-10".into()), |_| {}).await;
    let portal = Portal::new(&cloud);
    let email = "kim@example.com";
    let view = portal.code_sign_in(email).await;
    assert_eq!(view["flow"]["step"], "details");
    assert_eq!(view["flow"]["email"], email);
    assert_eq!(view["flow"]["terms"]["version"], "2026-10");
    assert_eq!(view["signed_in"], false);

    // Each problem names the input it is about.
    let age = portal
        .post(
            "/sign-up/details",
            json!({"handle": "Kim", "age_confirmed": false, "terms_accepted": true}),
        )
        .await
        .error(400, "bad_request");
    assert_eq!(age["field"], "age_confirmed");
    let terms = portal
        .post(
            "/sign-up/details",
            json!({"handle": "Kim", "age_confirmed": true, "terms_accepted": false}),
        )
        .await
        .error(400, "bad_request");
    assert_eq!(terms["field"], "terms_accepted");
    let handle = portal
        .post(
            "/sign-up/details",
            json!({"handle": "1x", "age_confirmed": true, "terms_accepted": true}),
        )
        .await
        .error(400, "handle_invalid");
    assert_eq!(handle["field"], "handle");
    cloud.sign_up("Taken").await;
    let taken = portal
        .post(
            "/sign-up/details",
            json!({"handle": "taken", "age_confirmed": true, "terms_accepted": true}),
        )
        .await
        .error(409, "handle_taken");
    assert_eq!(taken["field"], "handle");
    // A look-alike of a taken handle is taken, and says the same.
    cloud.sign_up("Flokzy").await;
    let lookalike = portal
        .post(
            "/sign-up/details",
            json!({"handle": "FIokzy", "age_confirmed": true, "terms_accepted": true}),
        )
        .await
        .error(409, "handle_taken");
    assert_eq!(lookalike["field"], "handle");
    // Nothing of that stopped the step.
    assert_eq!(portal.session().await["flow"]["step"], "details");

    let done = portal
        .post(
            "/sign-up/details",
            json!({"handle": "Kim", "age_confirmed": true, "terms_accepted": true}),
        )
        .await
        .ok(200);
    assert_eq!(done["flow"]["step"], "done");
    assert_eq!(done["account"]["handle"], "Kim");
    // The cloud recorded the age and the terms.
    let (terms_version,): (Option<String>,) =
        sqlx::query_as("SELECT terms_version FROM accounts WHERE email = $1")
            .bind(email)
            .fetch_one(&cloud.state.db)
            .await
            .unwrap();
    assert_eq!(terms_version.as_deref(), Some("2026-10"));
}

#[tokio::test]
async fn a_closed_cloud_makes_no_new_accounts() {
    let cloud = TestCloud::with(|c| c.signup = anyplay_api::Signup::Closed, |_| {}).await;
    let portal = Portal::new(&cloud);
    portal.session().await;
    portal
        .post("/sign-in/email", json!({"email": "new@example.com"}))
        .await
        .ok(200);
    let code = cloud.mail.last_code("new@example.com").unwrap();
    portal
        .post("/sign-in/code", json!({"code": code}))
        .await
        .error(403, "forbidden");
}

#[tokio::test]
async fn a_new_version_of_the_terms_is_asked_for_at_sign_in() {
    // The account was made through the API, which shows no terms: it has accepted none.
    let cloud = TestCloud::with(|c| c.terms_version = Some("v2".into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::new(&cloud);
    let view = portal.code_sign_in(&alec.email).await;
    assert_eq!(view["flow"]["step"], "terms");
    assert_eq!(view["flow"]["terms"]["version"], "v2");
    portal
        .post("/terms", json!({"accept": false}))
        .await
        .error(400, "bad_request");
    let done = portal.post("/terms", json!({"accept": true})).await.ok(200);
    assert_eq!(done["flow"]["step"], "done");
    assert_eq!(done["signed_in"], true);
}

fn authorize_path(challenge: &str, intent: &str) -> String {
    format!(
        "/web/authorize?client=net&intent={intent}&redirect_uri={}&state={STATE}&code_challenge={challenge}&code_challenge_method=S256&device=Test%20PC",
        REDIRECT.replace(':', "%3A").replace('/', "%2F")
    )
}

fn parse_redirect(url: &str) -> (String, String) {
    assert!(url.starts_with(REDIRECT), "{url}");
    let query = url.split_once('?').unwrap().1;
    let get = |name: &str| {
        query
            .split('&')
            .find_map(|p| p.strip_prefix(&format!("{name}=")))
            .unwrap()
            .to_owned()
    };
    (get("code"), get("state"))
}

#[tokio::test]
async fn the_hand_off_to_the_app_is_a_redirect_the_page_follows() {
    let portal_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        portal_dir.path().join("index.html"),
        "<html><head></head></html>",
    )
    .unwrap();
    let dir = portal_dir.path().to_path_buf();
    let cloud = TestCloud::with(move |c| c.portal_dir = Some(dir), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let verifier = pkce::new_verifier();
    let challenge = pkce::challenge(&verifier);

    let portal = Portal::new(&cloud);
    // The app opens /web/authorize; with a portal it goes on at /sign-in.
    let start = portal
        .dispatch(
            portal
                .client
                .get(portal.url(&authorize_path(&challenge, "sign_in"))),
        )
        .await;
    assert_eq!(start.status, 303);
    assert_eq!(start.header("location").as_deref(), Some("/sign-in"));
    let session = portal.session().await;
    assert_eq!(session["authorize"]["device"], "Test PC");
    assert_eq!(session["authorize"]["create"], false);

    let done = portal.code_sign_in(&alec.email).await;
    assert_eq!(done["flow"]["step"], "handoff");
    let (code, state) = parse_redirect(done["flow"]["redirect"].as_str().unwrap());
    assert_eq!(state, STATE);
    // The browser session did its job: nothing of this sign-in stays.
    assert_eq!(done["signed_in"], false);
    assert_eq!(done["csrf"], Value::Null);
    assert_eq!(portal.session().await["flow"]["step"], "email");

    // The app trades the code for its tokens.
    let reply = cloud
        .request(
            Method::POST,
            path::AUTH_BROWSER_TOKEN,
            None,
            Some(json!(BrowserTokenRequest {
                code,
                verifier,
                device: Device {
                    name: "Test PC".into(),
                    platform: Platform::Linux,
                    key: Some(key.public()),
                },
            })),
        )
        .await
        .ok(200);
    assert_eq!(reply.body["me"]["handle"], "Alec");
}

#[tokio::test]
async fn a_hand_off_left_by_the_html_pages_shows_in_the_session() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let challenge = pkce::challenge(&pkce::new_verifier());
    let portal = Portal::new(&cloud);
    // Without a portal the app's browser goes to the HTML pages.
    let start = portal
        .dispatch(
            portal
                .client
                .get(portal.url(&authorize_path(&challenge, "create"))),
        )
        .await;
    assert_eq!(start.header("location").as_deref(), Some("/web/create"));
    let view = portal.session().await;
    assert_eq!(view["authorize"]["create"], true);
    assert_eq!(view["flow"]["create"], true);
    let done = portal.code_sign_in(&alec.email).await;
    assert_eq!(done["flow"]["step"], "handoff");
}

#[tokio::test]
async fn a_banned_player_reads_the_statement_and_appeals_through_the_session() {
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    cloud
        .request(
            Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": "Alec", "rule": "cheating", "reason": "Aimbot in the arena."})),
        )
        .await
        .ok(200);

    let portal = Portal::new(&cloud);
    let view = portal.code_sign_in(&alec.email).await;
    assert_eq!(view["flow"]["step"], "banned");
    assert_eq!(view["signed_in"], false);
    let ban = portal.get("/ban").await.ok(200);
    assert_eq!(ban["rule"], "cheating");
    assert_eq!(ban["reason"], "Aimbot in the arena.");
    assert_eq!(ban["appeal"]["state"], "open");
    assert!(!ban.to_string().contains("appeal_token"));
    portal
        .post("/ban/appeal", json!({"text": "I did not do it."}))
        .await
        .ok(200);
    assert_eq!(portal.get("/ban").await.ok(200)["appeal"]["state"], "filed");
    // The account web is closed to a banned account.
    portal.get("/account").await.error(401, "unauthorized");

    // Banned while signed in: the next request says so and the session leads to the statement.
    let bob = cloud.sign_up("Bob").await;
    let browser = Portal::signed_in(&cloud, &bob).await;
    cloud
        .request(
            Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": "Bob", "rule": "cheating", "reason": "Too good."})),
        )
        .await
        .ok(200);
    let refused = browser.get("/account").await.error(403, "banned");
    assert!(refused.get("ban").is_none(), "{refused}");
    assert_eq!(browser.session().await["flow"]["step"], "banned");
    assert_eq!(browser.get("/ban").await.ok(200)["reason"], "Too good.");
}

#[tokio::test]
async fn a_banned_address_without_an_account_gets_the_statement_in_the_error() {
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    cloud
        .request(
            Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": "Alec", "rule": "cheating", "reason": "Aimbot."})),
        )
        .await
        .ok(200);
    cloud.state.delete_account(alec.id).await.unwrap();
    let portal = Portal::new(&cloud);
    portal.session().await;
    portal
        .post("/sign-in/email", json!({"email": alec.email}))
        .await
        .ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    let refused = portal
        .post("/sign-in/code", json!({"code": code}))
        .await
        .error(403, "banned");
    assert_eq!(refused["ban"]["statement"]["rule"], "cheating");
    assert!(refused["ban"].get("appeal_token").is_none(), "{refused}");
}

// The guard

#[tokio::test]
async fn a_write_needs_the_session_and_its_csrf_token() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let url = portal.url(&format!("{API}/account/deletion"));

    // No token, a wrong token, the token of another session: all refused, nothing happens.
    for token in [
        None,
        Some("wrong".to_owned()),
        Some(Portal::new(&cloud).token()),
    ] {
        let mut request = portal.client.delete(&url).json(&json!({}));
        if let Some(token) = token {
            request = request.header("x-csrf-token", token);
        }
        portal.dispatch(request).await.error(401, "unauthorized");
    }
    // A browser without a session cannot write at all.
    let stranger = Portal::new(&cloud);
    stranger
        .dispatch(
            stranger
                .client
                .post(stranger.url(&format!("{API}/sign-out")))
                .json(&json!({})),
        )
        .await
        .error(401, "unauthorized");
    // The right token works.
    portal.delete("/account/deletion").await.ok(200);
    // A read needs no token.
    portal.get("/account").await.ok(200);
}

#[tokio::test]
async fn a_write_from_another_site_is_refused() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let url = portal.url(&format!("{API}/account/deletion"));
    let request = |headers: &[(&str, &str)]| {
        let mut request = portal
            .client
            .delete(&url)
            .header("x-csrf-token", portal.token())
            .json(&json!({}));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request
    };
    for headers in [
        vec![("origin", "https://evil.example")],
        vec![("origin", "null")],
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
    ] {
        let answer = portal.dispatch(request(&headers)).await;
        assert_eq!(answer.status, 403, "{headers:?}: {}", answer.text());
    }
    // The page's own origin and a same-origin fetch pass.
    let own = cloud.state.web.as_ref().unwrap().origin.clone();
    let answer = portal
        .dispatch(request(&[
            ("origin", own.as_str()),
            ("sec-fetch-site", "same-origin"),
        ]))
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
}

#[tokio::test]
async fn a_write_must_be_json() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let url = portal.url(&format!("{API}/account/deletion"));
    let csrf = portal.token();
    // What a form on another site could send, with every header the site could not.
    for content_type in [
        Some("application/x-www-form-urlencoded"),
        Some("multipart/form-data; boundary=x"),
        Some("text/plain"),
        None,
    ] {
        let mut request = portal
            .client
            .delete(&url)
            .header("x-csrf-token", &csrf)
            .body("{}");
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        portal
            .dispatch(request)
            .await
            .error(415, "unsupported_media_type");
    }
    // JSON with a charset is JSON.
    portal
        .dispatch(
            portal
                .client
                .delete(&url)
                .header("x-csrf-token", &csrf)
                .header("content-type", "application/json; charset=utf-8")
                .body("{}"),
        )
        .await
        .ok(200);
    // An unreadable body is a bad request, not a crash.
    portal
        .dispatch(
            portal
                .client
                .post(portal.url(&format!("{API}/sign-in/email")))
                .header("x-csrf-token", &csrf)
                .header("content-type", "application/json")
                .body("{nope"),
        )
        .await
        .error(400, "bad_request");
}

// The account

#[tokio::test]
async fn the_account_lists_passkeys_devices_and_browsers() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let second = cloud.sign_in(&alec.email, "Laptop <b>").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let other = Portal::signed_in(&cloud, &alec).await;

    let account = portal.get("/account").await.ok(200);
    assert_eq!(account["account"]["handle"], "Alec");
    assert_eq!(account["account"]["email"], alec.email);
    assert_eq!(account["passkeys"], json!([]));
    assert_eq!(account["max_passkeys"], 10);
    assert_eq!(account["deletion"], Value::Null);
    let devices = account["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().any(|d| d["name"] == "Laptop <b>"));
    assert!(devices[0]["id"].as_u64().unwrap() > 0);
    let sessions = account["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions.iter().filter(|s| s["current"] == true).count(), 1);
    assert!(sessions[0]["last_used"].as_u64().unwrap() >= sessions[0]["created"].as_u64().unwrap());
    // The ID of a session reveals nothing that could be used as the cookie.
    let cookie = portal
        .cookies
        .lock()
        .unwrap()
        .values()
        .next()
        .cloned()
        .unwrap();
    assert!(
        sessions
            .iter()
            .all(|s| !s["id"].as_str().unwrap().contains(&cookie))
    );

    // The other browser sees the same two, with itself current.
    let seen = other.get("/account").await.ok(200);
    assert_eq!(seen["sessions"].as_array().unwrap().len(), 2);
    let _ = second;
}

#[tokio::test]
async fn a_device_and_a_browser_session_are_signed_out_one_by_one() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let second = cloud.sign_in(&alec.email, "Laptop").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let other = Portal::signed_in(&cloud, &alec).await;
    let bobs = Portal::signed_in(&cloud, &bob).await;

    let account = portal.get("/account").await.ok(200);
    let laptop = account["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "Laptop")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    // Not another account's device.
    let bobs_device = bobs.get("/account").await.ok(200)["devices"][0]["id"]
        .as_u64()
        .unwrap();
    portal
        .delete(&format!("/account/devices/{bobs_device}"))
        .await
        .error(404, "not_found");
    portal
        .delete(&format!("/account/devices/{laptop}"))
        .await
        .ok(200);
    cloud
        .get("/v1/me", &second.tokens.access)
        .await
        .is_error("unauthorized", 401);
    cloud.get("/v1/me", alec.token()).await.ok(200);

    // The other browser, by its ID in the list.
    let others_id = account["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"] == false)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    bobs.delete(&format!("/account/sessions/{others_id}"))
        .await
        .error(404, "not_found");
    let ended = portal
        .delete(&format!("/account/sessions/{others_id}"))
        .await
        .ok(200);
    assert_eq!(ended["signed_out"], false);
    other.get("/account").await.error(401, "unauthorized");

    // And its own: that signs the browser out.
    let mine = portal.get("/account").await.ok(200)["sessions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ended = portal
        .delete(&format!("/account/sessions/{mine}"))
        .await
        .ok(200);
    assert_eq!(ended["signed_out"], true);
    portal.get("/account").await.error(401, "unauthorized");
}

#[tokio::test]
async fn signing_out_everywhere_needs_a_fresh_session_and_ends_everything() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let other = Portal::signed_in(&cloud, &alec).await;

    portal.age(&cloud).await;
    portal
        .post("/account/sign-out-everywhere", json!({}))
        .await
        .error(403, "step_up_required");
    cloud.get("/v1/me", alec.token()).await.ok(200);

    portal.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);
    let ended = portal
        .post("/account/sign-out-everywhere", json!({}))
        .await
        .ok(200);
    assert_eq!(ended["signed_out"], true);
    cloud
        .get("/v1/me", alec.token())
        .await
        .is_error("unauthorized", 401);
    other.get("/account").await.error(401, "unauthorized");
    portal.get("/account").await.error(401, "unauthorized");
}

#[tokio::test]
async fn step_up_by_e_mail_code() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    portal.age(&cloud).await;

    let account = portal.get("/account").await.ok(200);
    assert!(account["fresh_until"].as_u64().unwrap() < now());

    // No code was asked for.
    portal
        .post("/step-up/code", json!({"code": "123456"}))
        .await
        .error(400, "bad_request");
    let asked = portal.post("/step-up/email", json!({})).await.ok(200);
    assert_eq!(asked["step"], "code");
    let hint = asked["email_hint"].as_str().unwrap();
    assert!(hint.starts_with("a***@") && !hint.contains("alec@"));
    portal
        .post("/step-up/code", json!({"code": "000000"}))
        .await
        .error(400, "code_wrong");
    let code = cloud.mail.last_code(&alec.email).unwrap();
    let done = portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);
    assert!(done["fresh_until"].as_u64().unwrap() > now());
    assert!(
        portal.get("/account").await.ok(200)["fresh_until"]
            .as_u64()
            .unwrap()
            > now()
    );
    // The code is used up.
    portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .error(400, "bad_request");
}

/// Adds a passkey on the signed-in portal. Returns the passkey's ID and what the authenticator
/// needs to answer later.
async fn add_passkey(
    cloud: &TestCloud,
    portal: &Portal<'_>,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    name: &str,
) -> (String, String, String) {
    let started = portal
        .post("/account/passkeys/start", json!({}))
        .await
        .ok(200);
    let options = started["options"].clone();
    let user_handle = options["publicKey"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let creation: CreationChallengeResponse = serde_json::from_value(options).unwrap();
    let credential = authenticator
        .do_registration(origin_of(cloud), creation)
        .expect("the software authenticator makes a passkey");
    let raw_id = serde_json::to_value(&credential).unwrap()["rawId"]
        .as_str()
        .unwrap()
        .to_owned();
    let finished = portal
        .post(
            "/account/passkeys/finish",
            json!({"id": started["id"], "name": name, "credential": credential}),
        )
        .await
        .ok(201);
    let id = finished["passkey"]["id"].as_str().unwrap().to_owned();
    assert_eq!(finished["passkey"]["name"], name);
    (id, raw_id, user_handle)
}

fn discoverable_answer(
    cloud: &TestCloud,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    mut options: Value,
    raw_id: &str,
    user_handle: &str,
) -> Value {
    options["publicKey"]["allowCredentials"] = json!([{"type": "public-key", "id": raw_id}]);
    let request: RequestChallengeResponse = serde_json::from_value(options).unwrap();
    let assertion = authenticator
        .do_authentication(origin_of(cloud), request)
        .expect("the software authenticator signs");
    let mut assertion = serde_json::to_value(&assertion).unwrap();
    assertion["response"]["userHandle"] = json!(user_handle);
    assertion
}

#[tokio::test]
async fn a_passkey_is_added_renamed_used_for_sign_in_and_removed() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (id, raw_id, user_handle) =
        add_passkey(&cloud, &portal, &mut authenticator, "Laptop").await;

    let account = portal.get("/account").await.ok(200);
    assert_eq!(account["passkeys"][0]["id"], id);
    assert_eq!(account["passkeys"][0]["name"], "Laptop");
    assert_eq!(account["passkeys"][0]["last_used"], Value::Null);

    portal
        .send(
            Method::PATCH,
            &format!("/account/passkeys/{id}"),
            json!({"name": "Work laptop"}),
        )
        .await
        .ok(200);
    portal
        .send(
            Method::PATCH,
            &format!("/account/passkeys/{id}"),
            json!({"name": "   "}),
        )
        .await
        .error(400, "bad_request");
    portal
        .send(
            Method::PATCH,
            "/account/passkeys/bm9wZQ",
            json!({"name": "x"}),
        )
        .await
        .error(404, "not_found");
    assert_eq!(
        portal.get("/account").await.ok(200)["passkeys"][0]["name"],
        "Work laptop"
    );

    // Another browser signs in with it, no address needed.
    let fresh = Portal::new(&cloud);
    fresh.session().await;
    let started = fresh
        .post("/passkey/sign-in/start", json!({}))
        .await
        .ok(200);
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &user_handle,
    );
    let done = fresh
        .post(
            "/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer}),
        )
        .await
        .ok(200);
    assert_eq!(done["flow"]["step"], "done");
    assert_eq!(done["account"]["handle"], "Alec");
    assert!(fresh.get("/account").await.ok(200)["passkeys"][0]["last_used"].is_u64());

    // A ceremony is used once.
    let again = fresh
        .post(
            "/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer}),
        )
        .await;
    again.error(401, "unauthorized");

    // Removing it, even the last one, is allowed to a fresh session.
    portal
        .delete(&format!("/account/passkeys/{id}"))
        .await
        .ok(200);
    assert_eq!(portal.get("/account").await.ok(200)["passkeys"], json!([]));
    portal
        .delete(&format!("/account/passkeys/{id}"))
        .await
        .error(404, "not_found");
}

#[tokio::test]
async fn passkeys_need_a_fresh_session_and_a_passkey_can_be_the_step_up() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (id, raw_id, user_handle) = add_passkey(&cloud, &portal, &mut authenticator, "Key").await;

    portal.age(&cloud).await;
    portal
        .post("/account/passkeys/start", json!({}))
        .await
        .error(403, "step_up_required");
    portal
        .delete(&format!("/account/passkeys/{id}"))
        .await
        .error(403, "step_up_required");
    // Renaming is not sensitive.
    portal
        .send(
            Method::PATCH,
            &format!("/account/passkeys/{id}"),
            json!({"name": "Key 2"}),
        )
        .await
        .ok(200);

    // The passkey of the account proves it again.
    let started = portal
        .post("/passkey/sign-in/start", json!({}))
        .await
        .ok(200);
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &user_handle,
    );
    let done = portal
        .post(
            "/step-up/passkey/finish",
            json!({"id": started["id"], "credential": answer}),
        )
        .await
        .ok(200);
    assert!(done["fresh_until"].as_u64().unwrap() > now());
    portal
        .delete(&format!("/account/passkeys/{id}"))
        .await
        .ok(200);

    // A passkey of another account does not.
    let bobs = Portal::signed_in(&cloud, &bob).await;
    let mut other = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (_, other_raw, other_handle) = add_passkey(&cloud, &bobs, &mut other, "Bob's").await;
    portal.age(&cloud).await;
    let started = portal
        .post("/passkey/sign-in/start", json!({}))
        .await
        .ok(200);
    let answer = discoverable_answer(
        &cloud,
        &mut other,
        started["options"].clone(),
        &other_raw,
        &other_handle,
    );
    portal
        .post(
            "/step-up/passkey/finish",
            json!({"id": started["id"], "credential": answer}),
        )
        .await
        .error(401, "unauthorized");
    portal
        .post("/account/sign-out-everywhere", json!({}))
        .await
        .error(403, "step_up_required");
}

#[tokio::test]
async fn the_export_is_a_json_download() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let answer = portal.get("/account/export").await;
    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.header("content-type").as_deref(),
        Some("application/json")
    );
    assert!(
        answer
            .header("content-disposition")
            .unwrap()
            .starts_with("attachment")
    );
    let body = answer.json();
    assert_eq!(body["account"]["handle"], "Alec");
    assert!(body["passkeys"].is_array());
    // Signed out, it is not there.
    Portal::new(&cloud)
        .get("/account/export")
        .await
        .error(401, "unauthorized");
}

#[tokio::test]
async fn an_e_mail_change_needs_a_fresh_session_and_a_code_from_the_new_address() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let other = Portal::signed_in(&cloud, &alec).await;

    portal.age(&cloud).await;
    portal
        .post("/account/email", json!({"email": "new@example.com"}))
        .await
        .error(403, "step_up_required");
    portal.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);

    // Not an address, not the same, not one that is taken: nothing is sent to learn which.
    portal
        .post("/account/email", json!({"email": "nope"}))
        .await
        .error(400, "bad_request");
    portal
        .post("/account/email", json!({"email": alec.email}))
        .await
        .error(400, "bad_request");
    portal
        .post("/account/email/confirm", json!({"code": "123456"}))
        .await
        .error(400, "bad_request");

    let asked = portal
        .post("/account/email", json!({"email": " New@Example.com "}))
        .await
        .ok(200);
    assert_eq!(asked["email"], "new@example.com");
    assert_eq!(
        portal.get("/account").await.ok(200)["pending_email"],
        "new@example.com"
    );
    portal
        .post("/account/email/confirm", json!({"code": "000000"}))
        .await
        .error(400, "code_wrong");
    let code = cloud.mail.last_code("new@example.com").unwrap();
    let done = portal
        .post("/account/email/confirm", json!({"code": code}))
        .await
        .ok(200);
    assert_eq!(done["email"], "new@example.com");
    let account = portal.get("/account").await.ok(200);
    assert_eq!(account["account"]["email"], "new@example.com");
    assert_eq!(account["pending_email"], Value::Null);
    // The old address is told, in plain words; the new one is named.
    let (subject, text) = cloud.mail.last_notice(&alec.email).expect("a notice");
    assert!(subject.contains("changed") && !subject.contains("new@example.com"));
    // The new address is masked, and the advice is to ask the operator, not to sign in with
    // an address that no longer opens the account.
    assert!(text.contains("n***@example.com") && !text.contains("new@example.com"));
    assert!(
        text.contains("If it was not you")
            && text.contains("contact the operator of Test cloud (http://localhost:")
    );
    assert!(!text.contains("sign in"));
    assert!(cloud.mail.last_notice("new@example.com").is_none());
    // Whatever else was signed in is not any more: the other browser and the device. The browser
    // that confirmed stays.
    other.get("/account").await.error(401, "unauthorized");
    cloud
        .get("/v1/me", alec.token())
        .await
        .is_error("unauthorized", 401);
    let again = cloud.sign_in("new@example.com", "pc").await;
    assert_eq!(again.me.email, "new@example.com");

    // An address another account has: the code goes to the owner, and the change is refused
    // with the same words as any other address that cannot be used.
    portal
        .post("/account/email", json!({"email": bob.email}))
        .await
        .ok(200);
    let code = cloud.mail.last_code(&bob.email).unwrap();
    portal
        .post("/account/email/confirm", json!({"code": code}))
        .await
        .error(400, "bad_request");
    assert_eq!(
        portal.get("/account").await.ok(200)["account"]["email"],
        "new@example.com"
    );
}

#[tokio::test]
async fn deleting_the_account_waits_for_the_grace_period_and_can_be_cancelled() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let other = Portal::signed_in(&cloud, &alec).await;

    portal.age(&cloud).await;
    portal
        .post("/account/deletion", json!({"handle": "Alec"}))
        .await
        .error(403, "step_up_required");
    portal.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);

    let wrong = portal
        .post("/account/deletion", json!({"handle": "Bob"}))
        .await
        .error(400, "bad_request");
    assert_eq!(wrong["field"], "handle");

    let before = now();
    let started = portal
        .post("/account/deletion", json!({"handle": "alec"}))
        .await
        .ok(200);
    let at = started["deletion"]["at"].as_u64().unwrap();
    assert!(at >= before + 30 * 24 * 3600 && at <= now() + 30 * 24 * 3600);
    // Asking again keeps the first time.
    let again = portal
        .post("/account/deletion", json!({"handle": "alec"}))
        .await
        .ok(200);
    assert_eq!(again["deletion"]["at"], at);

    // The account is dormant: devices and other browsers are signed out; this browser stays to
    // cancel.
    cloud
        .get("/v1/me", alec.token())
        .await
        .is_error("unauthorized", 401);
    other.get("/account").await.error(401, "unauthorized");
    assert_eq!(portal.get("/account").await.ok(200)["deletion"]["at"], at);
    assert!(cloud.state.db.acquire().await.is_ok());
    assert!(
        anyplay_cloud::db::account(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_some()
    );

    let cancelled = portal.delete("/account/deletion").await.ok(200);
    assert_eq!(cancelled["deletion"], Value::Null);
    assert_eq!(
        portal.get("/account").await.ok(200)["deletion"],
        Value::Null
    );
    // Cancelling twice is harmless.
    portal.delete("/account/deletion").await.ok(200);
}

#[tokio::test]
async fn an_account_is_deleted_when_the_grace_period_ends() {
    let cloud = TestCloud::with(|_| {}, |t| t.deletion_grace = Duration::from_millis(300)).await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let (a, b) = cloud.friends("Carol", "Dave").await;
    let _ = (a, b);
    let portal = Portal::signed_in(&cloud, &alec).await;
    portal
        .post("/account/deletion", json!({"handle": "Alec"}))
        .await
        .ok(200);
    let mut gone = false;
    for _ in 0..100 {
        if anyplay_cloud::db::account(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_none()
        {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the sweep deletes the account after the grace period");
    portal.get("/account").await.error(401, "unauthorized");
    // Others are untouched.
    assert!(
        anyplay_cloud::db::account(&cloud.state.db, bob.id)
            .await
            .unwrap()
            .is_some()
    );
}

// Serving the portal

const INDEX: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>AnyPlay</title></head><body><div id=\"root\"></div><script type=\"module\" src=\"/assets/app-1a2b3c.js\"></script></body></html>";

fn built_portal() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), INDEX).unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(
        dir.path().join("assets/app-1a2b3c.js"),
        "console.log('app')",
    )
    .unwrap();
    std::fs::write(dir.path().join("assets/app-1a2b3c.css"), "body{}").unwrap();
    std::fs::write(
        dir.path().join("favicon.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    std::fs::write(dir.path().join("secret.txt"), "not in the portal").unwrap();
    dir
}

async fn cloud_with_portal(dir: &tempfile::TempDir) -> TestCloud {
    let path = dir.path().to_path_buf();
    TestCloud::with(move |c| c.portal_dir = Some(path), |_| {}).await
}

async fn fetch(cloud: &TestCloud, path: &str) -> Answer {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}{path}", cloud.base))
        .send()
        .await
        .unwrap();
    Answer {
        status: response.status().as_u16(),
        headers: response.headers().clone(),
        bytes: response.bytes().await.unwrap().to_vec(),
    }
}

#[tokio::test]
async fn the_shell_answers_every_route_of_the_app() {
    let dir = built_portal();
    let cloud = cloud_with_portal(&dir).await;
    for route in [
        "/",
        "/sign-in",
        "/create",
        "/account",
        "/account/security",
        "/account/devices/more/deep",
        "/friends",
        "/friends/requests",
        "/trophies",
        "/trophies/PPSA01234",
        "/index.html",
    ] {
        let answer = fetch(&cloud, route).await;
        assert_eq!(answer.status, 200, "{route}");
        assert_eq!(answer.text(), INDEX, "{route}");
        assert_eq!(
            answer.header("cache-control").as_deref(),
            Some("no-cache"),
            "{route}"
        );
        assert_eq!(
            answer.header("content-type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        // The same security headers as the account web: nothing inline, nothing from elsewhere.
        let csp = answer.header("content-security-policy").unwrap();
        assert!(csp.contains("script-src 'self'") && csp.contains("style-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(
            answer.header("x-content-type-options").as_deref(),
            Some("nosniff")
        );
        assert_eq!(answer.header("x-frame-options").as_deref(), Some("DENY"));
    }
}

#[tokio::test]
async fn files_of_the_build_are_served_with_their_cache_rules() {
    let dir = built_portal();
    let cloud = cloud_with_portal(&dir).await;
    let js = fetch(&cloud, "/assets/app-1a2b3c.js").await;
    assert_eq!(js.status, 200);
    assert_eq!(js.text(), "console.log('app')");
    assert_eq!(
        js.header("content-type").as_deref(),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        js.header("cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );
    assert!(js.header("content-security-policy").is_some());
    let css = fetch(&cloud, "/assets/app-1a2b3c.css").await;
    assert_eq!(
        css.header("content-type").as_deref(),
        Some("text/css; charset=utf-8")
    );
    let icon = fetch(&cloud, "/favicon.svg").await;
    assert_eq!(icon.status, 200);
    assert_eq!(
        icon.header("content-type").as_deref(),
        Some("image/svg+xml")
    );
    assert_eq!(
        icon.header("cache-control").as_deref(),
        Some("public, max-age=3600")
    );
}

#[tokio::test]
async fn the_portal_shadows_nothing_and_leaks_nothing() {
    let dir = built_portal();
    let cloud = cloud_with_portal(&dir).await;
    // The cloud's own routes are untouched.
    assert_eq!(fetch(&cloud, "/healthz").await.text(), "ok");
    assert_eq!(fetch(&cloud, "/readyz").await.status, 200);
    let info = fetch(&cloud, "/v1/info").await;
    assert_eq!(info.status, 200);
    assert!(info.json()["web"].is_string());
    let old = fetch(&cloud, "/web/sign-in").await;
    assert_eq!(old.status, 200);
    assert!(old.text().contains("<form"), "the HTML pages still work");
    assert_eq!(fetch(&cloud, "/web/api/v1/session").await.status, 200);
    fetch(&cloud, "/v1/me").await.error(401, "unauthorized");

    // What is neither a route of the app nor a file is the cloud's not_found, as JSON.
    for missing in [
        "/nope",
        "/assets/missing.js",
        "/assets/",
        "/account.html",
        "/web/nope",
        "/v1/nope",
    ] {
        let answer = fetch(&cloud, missing).await;
        assert_eq!(answer.status, 404, "{missing}");
        assert_eq!(answer.json()["error"], "not_found", "{missing}");
    }
    // Nothing outside the build is reachable, and the shell is not a catch-all.
    for escape in [
        "/assets/..%2fsecret.txt",
        "/assets/%2e%2e/secret.txt",
        "/assets/..%5csecret.txt",
        "/..%2fsecret.txt",
        "/%2e%2e%2fsecret.txt",
    ] {
        // A client library would normalise the dots away; send the bytes as they are.
        let request = format!(
            "GET {escape} HTTP/1.1
Host: x
Connection: close

"
        );
        let answer = cloud.raw(request.as_bytes()).await;
        assert!(
            answer.status == 404 || answer.status == 400,
            "{escape}: {}",
            answer.status
        );
        assert!(
            !answer.body.to_string().contains("not in the portal"),
            "{escape}"
        );
    }
    // A file in the folder that is not under assets/ is served by name, like the icon; the
    // build folder is public by design.
    assert_eq!(fetch(&cloud, "/secret.txt").await.status, 200);
}

#[tokio::test]
async fn without_a_portal_the_routes_are_not_there() {
    let cloud = TestCloud::start().await;
    for route in [
        "/",
        "/sign-in",
        "/account",
        "/assets/app.js",
        "/favicon.svg",
        "/u/alec",
    ] {
        let answer = fetch(&cloud, route).await;
        assert_eq!(answer.status, 404, "{route}");
        assert_eq!(answer.json()["error"], "not_found");
    }
    // The API is there.
    assert_eq!(fetch(&cloud, "/web/api/v1/session").await.status, 200);
}

#[tokio::test]
async fn a_profile_page_carries_open_graph_tags_of_what_a_visitor_may_see() {
    let dir = built_portal();
    let cloud = cloud_with_portal(&dir).await;
    let alec = cloud.sign_up("Alec").await;
    cloud.sign_up("Bob").await;

    // No text yet: a plain description.
    let page = fetch(&cloud, "/u/alec").await;
    assert_eq!(page.status, 200);
    let text = page.text();
    assert!(text.contains("<title>Alec on Test cloud</title>"), "{text}");
    assert!(text.contains("<meta property=\"og:title\" content=\"Alec on Test cloud\">"));
    assert!(
        text.contains("<meta property=\"og:description\" content=\"Alec plays on Test cloud.\">")
    );
    assert!(text.contains("og:url\" content=\"http://localhost:"));
    assert!(text.contains("/u/Alec\">"));
    assert!(text.contains("<div id=\"root\">") && text.contains("/assets/app-1a2b3c.js"));
    assert_eq!(page.header("cache-control").as_deref(), Some("no-cache"));
    assert!(page.header("content-security-policy").is_some());

    // The text is in the tags only when everyone may read it, and it is escaped.
    cloud
        .patch(
            "/v1/me/profile",
            alec.token(),
            json!({"about": "I <b>play</b> \"hard\" & 'fast'"}),
        )
        .await
        .ok(200);
    let hidden = fetch(&cloud, "/u/alec").await.text();
    assert!(
        hidden.contains("Alec plays on Test cloud."),
        "friends-only text stays out"
    );
    assert!(!hidden.contains("play</b>"));
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"about": "everyone"})),
        )
        .await
        .ok(200);
    let shown = fetch(&cloud, "/u/ALEC").await.text();
    assert!(
        shown.contains(
            "content=\"I &lt;b&gt;play&lt;/b&gt; &quot;hard&quot; &amp; &#39;fast&#39;\""
        ),
        "{shown}"
    );
    assert!(!shown.contains("<b>play"));
    assert!(!shown.contains("content=\"I <b>"));

    // A player who does not exist: 404, with the plain shell and no tags.
    for name in ["nobody", "x%22%3E%3Cscript%3E", "way-too-long-for-a-handle"] {
        let page = fetch(&cloud, &format!("/u/{name}")).await;
        assert_eq!(page.status, 404, "{name}");
        assert_eq!(page.text(), INDEX, "{name}");
    }
}

#[tokio::test]
async fn the_app_opened_by_the_launcher_goes_on_in_the_portal_only_when_there_is_one() {
    let challenge = pkce::challenge(&pkce::new_verifier());
    let dir = built_portal();
    let with = cloud_with_portal(&dir).await;
    let without = TestCloud::start().await;
    // Another service serves the account web: the same landing, and the cloud serves no portal.
    let external = TestCloud::with(|c| c.portal_external = true, |_| {}).await;
    for (cloud, sign_in, create) in [
        (&with, "/sign-in", "/create"),
        (&external, "/sign-in", "/create"),
        (&without, "/web/sign-in", "/web/create"),
    ] {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for (intent, expected) in [("sign_in", sign_in), ("create", create)] {
            let response = client
                .get(format!(
                    "http://{}{}",
                    cloud.base,
                    authorize_path(&challenge, intent)
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 303);
            assert_eq!(response.headers()["location"], expected);
        }
        // An invalid request is still refused before anything shows.
        let refused = client
            .get(format!("http://{}/web/authorize?client=net", cloud.base))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status().as_u16(), 400);
    }
}

// Review fixes

#[tokio::test]
async fn every_way_to_delete_the_account_waits_for_the_grace_period() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let other_device = cloud.sign_in(&alec.email, "Laptop").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let elsewhere = Portal::signed_in(&cloud, &alec).await;

    // The HTML delete form, with the token of the portal session (the same cookie).
    let form = portal
        .dispatch(
            portal
                .client
                .post(portal.url("/web/account/delete"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(format!("csrf={}&handle=Alec", portal.token())),
        )
        .await;
    assert_eq!(form.status, 200, "{}", form.text());
    assert!(form.text().contains("scheduled"), "{}", form.text());
    assert!(
        anyplay_cloud::db::account(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_some(),
        "nothing is deleted at once"
    );
    let at = anyplay_cloud::db::deletion_of(&cloud.state.db, alec.id)
        .await
        .unwrap()
        .expect("scheduled");
    assert!(at > now() + 29 * 24 * 3600);
    // Devices and other browsers are out; the browser that asked can cancel.
    cloud
        .get("/v1/me", alec.token())
        .await
        .is_error("unauthorized", 401);
    cloud
        .get("/v1/me", &other_device.tokens.access)
        .await
        .is_error("unauthorized", 401);
    elsewhere.get("/account").await.error(401, "unauthorized");
    assert_eq!(portal.get("/account").await.ok(200)["deletion"]["at"], at);

    // The API of the launcher answers with the scheduled time, and keeps the first one.
    let renewed = cloud.sign_in(&alec.email, "Desk").await;
    let answer = cloud.delete("/v1/me", &renewed.tokens.access).await.ok(200);
    assert_eq!(answer.body["deletion"]["at"], at);
    assert!(
        anyplay_cloud::db::account(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_some()
    );
    cloud
        .get("/v1/me", &renewed.tokens.access)
        .await
        .is_error("unauthorized", 401);

    // The tokens are refused from now on: the "not before" time moved.
    let valid = anyplay_cloud::binding::all_tokens_valid_from(&cloud.state.db)
        .await
        .unwrap();
    assert!(valid.iter().any(|(id, from)| *id == alec.id && *from > 0));

    // That signed every browser out too. The player signs in again and cancels in the portal.
    portal.get("/account").await.error(401, "unauthorized");
    let back = Portal::signed_in(&cloud, &alec).await;
    assert_eq!(back.get("/account").await.ok(200)["deletion"]["at"], at);
    back.delete("/account/deletion").await.ok(200);
    assert_eq!(
        anyplay_cloud::db::deletion_of(&cloud.state.db, alec.id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_cancel_wins_before_the_time_and_the_sweep_wins_after() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let a = Portal::signed_in(&cloud, &alec).await;
    let b = Portal::signed_in(&cloud, &bob).await;
    a.post("/account/deletion", json!({"handle": "Alec"}))
        .await
        .ok(200);
    b.post("/account/deletion", json!({"handle": "Bob"}))
        .await
        .ok(200);
    // Alec cancels in time and the sweep leaves the account alone.
    a.delete("/account/deletion").await.ok(200);
    // Bob's time has come: the sweep is certain, a cancel is too late.
    sqlx::query("UPDATE accounts SET deletion_at = 1 WHERE id = $1")
        .bind(bob.id as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    b.delete("/account/deletion").await.error(403, "forbidden");
    cloud.state.sweep().await.unwrap();
    let db = &cloud.state.db;
    assert!(
        anyplay_cloud::db::account(db, alec.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        anyplay_cloud::db::account(db, bob.id)
            .await
            .unwrap()
            .is_none()
    );

    // An account the sweep cannot delete is moved back, so it does not hold the others up.
    let cleo = cloud.sign_up("Cleo").await;
    sqlx::query("UPDATE accounts SET deletion_at = 1 WHERE id = $1")
        .bind(cleo.id as i64)
        .execute(db)
        .await
        .unwrap();
    anyplay_cloud::db::postpone_deletion(db, cleo.id, now() + 600)
        .await
        .unwrap();
    assert!(
        anyplay_cloud::db::deletions_due(db, now(), 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_stale_session_cannot_export_or_end_anything_but_itself() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let device = cloud.sign_in(&alec.email, "Laptop").await;
    let victim = Portal::signed_in(&cloud, &alec).await;
    let thief = Portal::signed_in(&cloud, &alec).await;
    let account = thief.get("/account").await.ok(200);
    let victims = |account: &Value, current: bool| {
        account["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["current"] == current)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let id_of_victim = victims(&account, false);
    let own = victims(&account, true);
    let device_id = account["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "Laptop")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    thief.age(&cloud).await;

    thief
        .get("/account/export")
        .await
        .error(403, "step_up_required");
    thief
        .delete(&format!("/account/sessions/{id_of_victim}"))
        .await
        .error(403, "step_up_required");
    thief
        .delete(&format!("/account/devices/{device_id}"))
        .await
        .error(403, "step_up_required");
    // Nothing happened to the owner's sessions and devices.
    victim.get("/account").await.ok(200);
    cloud.get("/v1/me", &device.tokens.access).await.ok(200);

    // After a step-up the export and the sign-outs work.
    thief.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    thief
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);
    assert_eq!(thief.get("/account/export").await.status, 200);
    thief
        .delete(&format!("/account/sessions/{id_of_victim}"))
        .await
        .ok(200);
    victim.get("/account").await.error(401, "unauthorized");

    // A browser may always sign itself out, fresh or not.
    thief.age(&cloud).await;
    thief
        .delete(&format!("/account/sessions/{own}"))
        .await
        .ok(200);
}

#[tokio::test]
async fn cookieless_page_loads_have_a_bucket_of_their_own() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.portal_sessions_per_ip = 3).await;
    let alec = cloud.sign_up("Alec").await;
    // The sign-in code limits are not the bucket of page loads: ten loads do not use them up.
    for _ in 0..3 {
        Portal::new(&cloud).get("/session").await.ok(200);
    }
    Portal::new(&cloud)
        .get("/session")
        .await
        .error(429, "rate_limited");
    // A browser that has its session is not counted, and asking for a code still works.
    let (token, _) = anyplay_cloud::web::store::create_session(
        &cloud.state.db,
        None,
        &anyplay_cloud::web::store::SessionData::default(),
        Duration::from_secs(600),
    )
    .await
    .unwrap();
    let portal = Portal::new(&cloud);
    portal
        .cookies
        .lock()
        .unwrap()
        .insert("anyplay_web".into(), token);
    portal.session().await;
    portal
        .post("/sign-in/email", json!({"email": alec.email}))
        .await
        .ok(200);
}

#[tokio::test]
async fn a_read_of_the_session_does_not_undo_what_another_request_wrote() {
    use anyplay_cloud::web::store::{
        EmailChange, SessionData, create_session, load_session, save_data,
    };
    let cloud = TestCloud::start().await;
    let pool = &cloud.state.db;
    let alec = cloud.sign_up("Alec").await;
    let idle = Duration::from_secs(600);
    let max = Duration::from_secs(3600);
    let (token, _) = create_session(
        pool,
        Some(alec.id),
        &SessionData {
            notice: Some("hello".into()),
            ..SessionData::default()
        },
        idle,
    )
    .await
    .unwrap();
    // Two requests load the same session.
    let mut reader = load_session(pool, &token, idle, max)
        .await
        .unwrap()
        .unwrap();
    let mut writer = load_session(pool, &token, idle, max)
        .await
        .unwrap()
        .unwrap();
    // The writer proves who the player is again and starts an e-mail change.
    writer.data.verified_ms = Some(12_345);
    writer.data.email_change = Some(EmailChange {
        challenge: "c".into(),
        email: "new@example.com".into(),
    });
    save_data(pool, &writer).await.unwrap();
    // The reader, which had loaded before, takes the notice and saves.
    reader.data.notice = None;
    save_data(pool, &reader).await.unwrap();

    let stored = load_session(pool, &token, idle, max)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.data.notice, None, "the reader's change is kept");
    assert_eq!(
        stored.data.verified_ms,
        Some(12_345),
        "the writer's is not undone"
    );
    assert_eq!(
        stored.data.email_change.map(|c| c.email).as_deref(),
        Some("new@example.com")
    );
}

#[tokio::test]
async fn a_scheduled_deletion_is_mailed_whichever_way_it_was_asked_for() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let a = Portal::signed_in(&cloud, &alec).await;
    assert_eq!(cloud.mail.notice_count(&alec.email), 0);

    // The portal.
    a.post("/account/deletion", json!({"handle": "Alec"}))
        .await
        .ok(200);
    let at = anyplay_cloud::db::deletion_of(&cloud.state.db, alec.id)
        .await
        .unwrap()
        .unwrap();
    let (subject, text) = cloud.mail.last_notice(&alec.email).expect("a notice");
    assert!(subject.contains("scheduled for deletion"));
    assert!(text.contains(&anyplay_cloud::web::pages::date(at)));
    assert!(text.contains("cancel the deletion"));
    assert!(text.contains("contact the operator of Test cloud (http://localhost:"));
    // Asking again does not mail again.
    a.post("/account/deletion", json!({"handle": "Alec"}))
        .await
        .ok(200);
    assert_eq!(cloud.mail.notice_count(&alec.email), 1);

    // The API of the launcher.
    cloud.delete("/v1/me", bob.token()).await.ok(200);
    assert_eq!(cloud.mail.notice_count(&bob.email), 1);
    // Nobody else was mailed.
    assert_eq!(cloud.mail.notice_count(&alec.email), 1);
}

#[tokio::test]
async fn an_account_with_a_passkey_changes_its_address_only_after_a_passkey() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let plain = cloud.sign_up("Plain").await;
    let portal = Portal::signed_in(&cloud, &alec).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (_, raw_id, user_handle) = add_passkey(&cloud, &portal, &mut authenticator, "Key").await;

    // Fresh from a code is not enough: the change is about the address a code goes to.
    let refused = portal
        .post("/account/email", json!({"email": "new@example.com"}))
        .await
        .error(403, "step_up_required");
    assert_eq!(refused["methods"], json!(["passkey"]));
    portal.age(&cloud).await;
    portal.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&alec.email).unwrap();
    portal
        .post("/step-up/code", json!({"code": code}))
        .await
        .ok(200);
    let still = portal
        .post("/account/email", json!({"email": "new@example.com"}))
        .await
        .error(403, "step_up_required");
    assert_eq!(still["methods"], json!(["passkey"]));
    // Other sensitive actions take the code, and say both ways.
    portal.age(&cloud).await;
    let other = portal
        .post("/account/sign-out-everywhere", json!({}))
        .await
        .error(403, "step_up_required");
    assert_eq!(other["methods"], json!(["passkey", "email_code"]));

    // A passkey does.
    let started = portal
        .post("/passkey/sign-in/start", json!({}))
        .await
        .ok(200);
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &user_handle,
    );
    portal
        .post(
            "/step-up/passkey/finish",
            json!({"id": started["id"], "credential": answer}),
        )
        .await
        .ok(200);
    portal
        .post("/account/email", json!({"email": "new@example.com"}))
        .await
        .ok(200);
    let code = cloud.mail.last_code("new@example.com").unwrap();
    // The confirmation asks for it too; the passkey is still fresh.
    portal
        .post("/account/email/confirm", json!({"code": code}))
        .await
        .ok(200);

    // An account without a passkey needs no passkey: the code is all it has.
    let p = Portal::signed_in(&cloud, &plain).await;
    p.age(&cloud).await;
    p.post("/step-up/email", json!({})).await.ok(200);
    let code = cloud.mail.last_code(&plain.email).unwrap();
    p.post("/step-up/code", json!({"code": code})).await.ok(200);
    p.post("/account/email", json!({"email": "other@example.com"}))
        .await
        .ok(200);

    // A sign-in with the passkey counts as one.
    let again = Portal::new(&cloud);
    again.session().await;
    let started = again
        .post("/passkey/sign-in/start", json!({}))
        .await
        .ok(200);
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &user_handle,
    );
    again
        .post(
            "/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer}),
        )
        .await
        .ok(200);
    again
        .post("/account/email", json!({"email": "third@example.com"}))
        .await
        .ok(200);
}

// The link of the app's account button

/// A device with a key, signed in by API, and a link from the cloud the way the app asks for it.
async fn account_link(cloud: &TestCloud, email: &str) -> (anyplay_api::SignedIn, String) {
    let key = DeviceKey::generate();
    let (challenge, code) = cloud.ask_for_code(email).await;
    let reply = cloud
        .request(
            Method::POST,
            "/v1/auth/code",
            None,
            Some(json!(anyplay_api::CodeRequest {
                challenge: challenge.challenge,
                code,
                device: Device {
                    name: "Keyed PC".into(),
                    platform: Platform::Linux,
                    key: Some(key.public()),
                },
            })),
        )
        .await
        .ok(200);
    let device = match reply.json() {
        anyplay_api::CodeAnswer::SignedIn { tokens, me } => anyplay_api::SignedIn { tokens, me },
        other => panic!("expected a sign-in, got {other:?}"),
    };
    let access = device.tokens.access.clone();
    let proof = anyplay_proof::make(
        &key,
        &anyplay_proof::Subject {
            method: "POST",
            path: path::AUTH_BROWSER_LINK,
            credential: &access,
        },
        now(),
    );
    let response = cloud
        .http
        .post(format!("http://{}{}", cloud.base, path::AUTH_BROWSER_LINK))
        .bearer_auth(&access)
        .header(anyplay_proof::HEADER, proof)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let link: anyplay_api::AccountLink = response.json().await.unwrap();
    (device, link.url)
}

fn code_in(url: &str) -> String {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "c")
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
async fn the_account_link_points_at_the_website_when_another_service_serves_it() {
    let own = TestCloud::start().await;
    let external = TestCloud::with(|c| c.portal_external = true, |_| {}).await;
    for (cloud, expected) in [(&own, "/web/link"), (&external, "/link")] {
        let alec = cloud.sign_up("Alec").await;
        let (_, url) = account_link(cloud, &alec.email).await;
        assert_eq!(Url::parse(&url).unwrap().path(), expected);
        assert!(!code_in(&url).is_empty());
    }

    // The old address of a launcher from before the move goes on to the website. A cloud with
    // its own account web keeps the page.
    let alec = external.sign_up("Beth").await;
    let (_, url) = account_link(&external, &alec.email).await;
    let code = code_in(&url);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let old = client
        .get(format!("http://{}/web/link?c={code}", external.base))
        .send()
        .await
        .unwrap();
    assert_eq!(old.status().as_u16(), 303);
    assert_eq!(old.headers()["location"], format!("/link?c={code}"));
    // The redirect used nothing up.
    let portal = Portal::new(&external);
    portal.session().await;
    portal.get(&format!("/link/{code}")).await.ok(200);

    let own_alec = own.sign_up("Cleo").await;
    let (_, url) = account_link(&own, &own_alec.email).await;
    let page = client
        .get(format!("http://{}/web/link?c={}", own.base, code_in(&url)))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status().as_u16(), 200);
}

#[tokio::test]
async fn the_link_page_asks_and_only_the_press_signs_the_browser_in() {
    let cloud = TestCloud::with(|c| c.portal_external = true, |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    let (_, url) = account_link(&cloud, &alec.email).await;
    let code = code_in(&url);

    let portal = Portal::new(&cloud);
    let first = portal.session().await;
    // Asking names the account and does not use the code up, however often a program asks.
    for _ in 0..3 {
        let peek = portal.get(&format!("/link/{code}")).await.ok(200);
        assert_eq!(peek["handle"], "Alec");
    }
    portal.get("/account").await.error(401, "unauthorized");

    // A press without the token of the session signs nobody in and keeps the code.
    portal
        .dispatch(
            portal
                .client
                .post(portal.url(&format!("{API}/link/{code}")))
                .json(&json!({})),
        )
        .await
        .error(401, "unauthorized");
    // Neither does a press that is not JSON.
    portal
        .dispatch(
            portal
                .client
                .post(portal.url(&format!("{API}/link/{code}")))
                .header("x-csrf-token", portal.token())
                .header("content-type", "text/plain")
                .body("{}"),
        )
        .await
        .error(415, "unsupported_media_type");
    portal.get(&format!("/link/{code}")).await.ok(200);

    let view = portal
        .post(&format!("/link/{code}"), json!({}))
        .await
        .ok(200);
    assert_eq!(view["signed_in"], true);
    assert_eq!(view["account"]["handle"], "Alec");
    assert_eq!(view["flow"]["step"], "done");
    assert_ne!(view["csrf"], first["csrf"], "a new session");
    portal.get("/account").await.ok(200);

    // Used once, for a look as well as for a press.
    portal
        .get(&format!("/link/{code}"))
        .await
        .error(404, "not_found");
    let again = Portal::new(&cloud);
    again.session().await;
    again
        .post(&format!("/link/{code}"), json!({}))
        .await
        .error(404, "not_found");
    again.get("/account").await.error(401, "unauthorized");

    // A code nobody made, and one that is far too long.
    portal.get("/link/nothing").await.error(404, "not_found");
    portal
        .get(&format!("/link/{}", "x".repeat(300)))
        .await
        .error(404, "not_found");

    // The device that asked signing out kills a link it made.
    let (device, url) = account_link(&cloud, &alec.email).await;
    let code = code_in(&url);
    cloud
        .state
        .sign_out_device(device.tokens.device)
        .await
        .unwrap();
    let late = Portal::new(&cloud);
    late.session().await;
    late.post(&format!("/link/{code}"), json!({}))
        .await
        .error(404, "not_found");
    late.get("/account").await.error(401, "unauthorized");
}

#[tokio::test]
async fn a_banned_account_gets_no_session_from_a_link_but_the_statement() {
    let cloud = TestCloud::with(
        |c| {
            c.portal_external = true;
            c.admin_token = Some(ADMIN.into());
        },
        |_| {},
    )
    .await;
    let alec = cloud.sign_up("Alec").await;
    let (_, url) = account_link(&cloud, &alec.email).await;
    let code = code_in(&url);
    cloud
        .request(
            Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": "Alec", "rule": "cheating", "reason": "Test ban"})),
        )
        .await
        .ok(200);

    let portal = Portal::new(&cloud);
    portal.session().await;
    let refused = portal
        .post(&format!("/link/{code}"), json!({}))
        .await
        .error(403, "banned");
    assert!(refused["ban"].get("appeal_token").is_none(), "{refused}");
    portal.get("/account").await.error(401, "unauthorized");
    // The browser learns of the ban and shows the statement, where it can appeal.
    assert_eq!(portal.session().await["flow"]["step"], "banned");
    let shown = portal.get("/ban").await.ok(200);
    assert_eq!(shown["reason"], "Test ban");
    assert_eq!(shown["appeal"]["state"], "open");
    // The code is used up.
    portal
        .get(&format!("/link/{code}"))
        .await
        .error(404, "not_found");
}
