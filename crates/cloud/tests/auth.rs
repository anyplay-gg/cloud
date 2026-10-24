mod common;

use std::time::Duration;

use anyplay_api::{Account, ApiError, CodeAnswer, Event, Info, Me, Signup, Tokens, close};
use common::{Events, TestCloud};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn info_needs_no_account() {
    let cloud = TestCloud::with(
        |c| {
            c.privacy_url = Some("https://example.com/privacy".into());
            c.signup = Signup::Closed;
        },
        |_| {},
    )
    .await;
    let reply = cloud
        .request(Method::GET, "/v1/info", None, None)
        .await
        .ok(200);
    let info: Info = reply.json();
    assert_eq!(info.name, "Test cloud");
    assert_eq!(info.signup, Signup::Closed);
    assert_eq!(info.privacy.as_deref(), Some("https://example.com/privacy"));
    assert_eq!(info.terms, None);
}

#[tokio::test]
async fn two_accounts_sign_up_by_code_and_look_each_other_up() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("sam_2").await;
    assert_ne!(alec.id, sam.id);

    let me: Me = cloud.get("/v1/me", alec.token()).await.ok(200).json();
    assert_eq!(me.handle, "Alec");
    assert_eq!(me.email, "alec@example.com");
    assert!(me.id.0 >= 1 && me.id.0 < (1 << 53));

    let found: Account = cloud
        .get("/v1/accounts?handle=SAM_2", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(found.id.0, sam.id);
    assert_eq!(found.handle, "sam_2", "the owner's spelling is kept");
    let by_id: Account = cloud
        .get(&format!("/v1/accounts/{}", alec.id), sam.token())
        .await
        .ok(200)
        .json();
    assert_eq!(by_id.handle, "Alec");
    cloud
        .get("/v1/accounts/987654321", sam.token())
        .await
        .is_error("not_found", 404);
    cloud
        .get("/v1/accounts/not-a-number", sam.token())
        .await
        .is_error("not_found", 404);
    cloud
        .get("/v1/accounts?handle=nobody", sam.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn the_same_address_signs_in_again_on_a_second_device() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let second = cloud.sign_in(&alec.email, "laptop").await;
    assert_eq!(second.me.id.0, alec.id);
    assert_ne!(second.tokens.device, alec.tokens.device);
    // Both devices work.
    cloud.get("/v1/me", alec.token()).await.ok(200);
    cloud.get("/v1/me", &second.tokens.access).await.ok(200);
    // Signing one out leaves the other.
    cloud
        .post_empty("/v1/auth/signout", &second.tokens.access)
        .await
        .ok(204);
    cloud
        .get("/v1/me", &second.tokens.access)
        .await
        .is_error("unauthorized", 401);
    cloud.get("/v1/me", alec.token()).await.ok(200);
}

#[tokio::test]
async fn asking_for_a_code_does_not_reveal_whether_an_account_exists() {
    let cloud = TestCloud::start().await;
    cloud.sign_up("Alec").await;
    let known = cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": "alec@example.com"})),
        )
        .await
        .ok(200);
    let unknown = cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": "nobody@example.com"})),
        )
        .await
        .ok(200);
    let keys = |r: &common::Reply| {
        let mut keys: Vec<String> = r.body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(&known), keys(&unknown));
    assert_eq!(known.body["code_length"], unknown.body["code_length"]);
    assert_eq!(
        cloud.mail.count("nobody@example.com"),
        1,
        "a mail goes out either way"
    );

    cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": "not an address"})),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_wrong_code_is_refused_and_five_tries_spend_the_challenge() {
    let cloud = TestCloud::start().await;
    let (challenge, code) = cloud.ask_for_code("Try@Example.com").await;
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(challenge.code_length, 6);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..5 {
        cloud
            .enter_code(&challenge.challenge, wrong, "pc")
            .await
            .is_error("code_wrong", 400);
    }
    // Even the right code is useless now.
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("code_expired", 400);
    cloud
        .enter_code("no-such-challenge", &code, "pc")
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn a_code_expires() {
    let cloud = TestCloud::with(|_| {}, |t| t.code_ttl = Duration::from_millis(100)).await;
    let (challenge, code) = cloud.ask_for_code("slow@example.com").await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn a_code_works_once() {
    let cloud = TestCloud::start().await;
    let (challenge, code) = cloud.ask_for_code("once@example.com").await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .ok(200);
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn handles_follow_the_rules_and_are_unique_without_regard_to_case() {
    let cloud = TestCloud::start().await;
    cloud.sign_up("Alec").await;

    let signup = |email: &str| {
        let cloud = &cloud;
        let email = email.to_owned();
        async move {
            let (challenge, code) = cloud.ask_for_code(&email).await;
            let answer: CodeAnswer = cloud
                .enter_code(&challenge.challenge, &code, "pc")
                .await
                .ok(200)
                .json();
            match answer {
                CodeAnswer::NewAccount { signup, .. } => signup,
                other => panic!("{other:?}"),
            }
        }
    };
    let token = signup("new@example.com").await;

    cloud
        .try_sign_up(&token, "ALEC", true)
        .await
        .is_error("handle_taken", 409);
    cloud
        .try_sign_up(&token, "ab", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "9lives", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "dot.ted", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "AnyPlay", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "ADMIN", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "playstation_fan", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "XboxKid", true)
        .await
        .is_error("handle_invalid", 400);
    cloud
        .try_sign_up(&token, "fine_name", false)
        .await
        .is_error("bad_request", 400);
    // The token survives these mistakes.
    cloud.try_sign_up(&token, "fine_name", true).await.ok(200);
    // And is spent by the success.
    cloud
        .try_sign_up(&token, "another", true)
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn closed_sign_ups_still_let_existing_accounts_in() {
    let cloud = TestCloud::with(|c| c.signup = Signup::Closed, |_| {}).await;
    let (challenge, code) = cloud.ask_for_code("new@example.com").await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("forbidden", 403);
}

#[tokio::test]
async fn refresh_replaces_the_token_and_a_replaced_one_signs_the_device_out() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let mut events = cloud.events(alec.token()).await;

    let first = alec.tokens.refresh.clone();
    let second: Tokens = cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": first})),
        )
        .await
        .ok(200)
        .json();
    assert_ne!(second.refresh, first);
    assert_eq!(second.device, alec.tokens.device);
    cloud.get("/v1/me", &second.access).await.ok(200);

    // The new token works and is replaced in turn.
    let third: Tokens = cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": second.refresh})),
        )
        .await
        .ok(200)
        .json();

    // The first token comes back: only a copy could do that.
    cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": first})),
        )
        .await
        .is_error("unauthorized", 401);

    // The device is out: its event connection is told and closed, and nothing works any more.
    assert_eq!(events.next().await, Event::SignedOut);
    assert_eq!(events.close_code().await, Some(close::SIGNED_OUT));
    cloud
        .get("/v1/me", &third.access)
        .await
        .is_error("unauthorized", 401);
    cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": third.refresh})),
        )
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn garbage_tokens_and_bodies_get_error_bodies() {
    let cloud = TestCloud::start().await;
    cloud
        .request(Method::GET, "/v1/me", None, None)
        .await
        .is_error("unauthorized", 401);
    cloud
        .get("/v1/me", "v1.not.a-token")
        .await
        .is_error("unauthorized", 401);
    cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"wrong": 1})),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .request(Method::GET, "/v1/nothing", None, None)
        .await
        .is_error("not_found", 404);
    let reply = cloud.request(Method::DELETE, "/v1/info", None, None).await;
    assert_eq!(reply.status, 405);
    let error: ApiError = reply.error();
    assert!(!error.message.is_empty());
}

#[tokio::test]
async fn bodies_over_64_kib_are_refused() {
    let cloud = TestCloud::start().await;
    let big = "a".repeat(70 * 1024);
    cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": big})),
        )
        .await
        .is_error("too_large", 413);
}

#[tokio::test]
async fn an_expired_access_token_is_told_to_refresh() {
    let cloud = TestCloud::with(|_| {}, |t| t.access_ttl_secs = 0).await;
    let alec = cloud.sign_up("Alec").await;
    cloud
        .get("/v1/me", alec.token())
        .await
        .is_error("unauthorized", 401);
    // The event connection opens and closes at once with the code for an expired token.
    let mut events = Events::connect_raw(&cloud.base, alec.token())
        .await
        .unwrap();
    assert_eq!(events.close_code().await, Some(close::TOKEN_EXPIRED));
    // A refresh still works, because it does not need the access token.
    cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": alec.tokens.refresh})),
        )
        .await
        .ok(200);
}
