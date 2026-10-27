//! Limits and checks around signing in: per-address limits that count an IPv6 /64 as one
//! client, a cap on new codes for the whole cloud, one live code per address, a count of wrong
//! codes per address over all its codes, proof for deleting the account, aliases of one address,
//! and a limit on refreshes per client address.

mod common;

use anyplay_proof::DeviceKey;
use common::TestCloud;
use reqwest::Method;
use serde_json::json;

/// Asks for a code as if through a reverse proxy that saw `client`.
async fn ask_from(cloud: &TestCloud, client: &str, email: &str) -> u16 {
    cloud
        .http
        .post(format!("http://{}/v1/auth/email", cloud.base))
        .header("x-forwarded-for", client)
        .json(&json!({"email": email}))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn ask(cloud: &TestCloud, email: &str) -> common::Reply {
    cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": email})),
        )
        .await
}

#[tokio::test]
async fn an_ipv6_network_counts_as_one_client() {
    let cloud = TestCloud::with(|c| c.trust_proxy = true, |t| t.rates.email_per_ip = 2).await;
    assert_eq!(
        ask_from(&cloud, "2001:db8:1:1::1", "a@example.com").await,
        200
    );
    assert_eq!(
        ask_from(&cloud, "2001:db8:1:1:ffff::2", "b@example.com").await,
        200
    );
    // A third address of the same /64 is the same client.
    assert_eq!(
        ask_from(&cloud, "2001:db8:1:1:1234:5678:9abc:def0", "c@example.com").await,
        429
    );
    // Another /64 and an IPv4 address have their own counts.
    assert_eq!(
        ask_from(&cloud, "2001:db8:1:2::1", "d@example.com").await,
        200
    );
    assert_eq!(ask_from(&cloud, "203.0.113.7", "e@example.com").await, 200);
    assert_eq!(ask_from(&cloud, "203.0.113.7", "f@example.com").await, 200);
    assert_eq!(ask_from(&cloud, "203.0.113.7", "g@example.com").await, 429);
}

#[tokio::test]
async fn new_codes_are_capped_for_the_whole_cloud() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.email_global = 3).await;
    for name in ["a", "b", "c"] {
        ask(&cloud, &format!("{name}@example.com")).await.ok(200);
    }
    ask(&cloud, "d@example.com")
        .await
        .is_error("rate_limited", 429);
}

#[tokio::test]
async fn a_new_code_replaces_the_older_ones_of_the_address() {
    let cloud = TestCloud::start().await;
    let (first, first_code) = cloud.ask_for_code("pat@example.com").await;
    let (second, second_code) = cloud.ask_for_code("pat@example.com").await;
    // The older code is no longer valid, even when typed right.
    cloud
        .enter_code(&first.challenge, &first_code, "pc")
        .await
        .is_error("code_expired", 400);
    // A resend works with the newest code.
    cloud
        .enter_code(&second.challenge, &second_code, "pc")
        .await
        .ok(200);
    // Another address is not touched.
    let (other, other_code) = cloud.ask_for_code("sam@example.com").await;
    cloud.ask_for_code("pat@example.com").await;
    cloud
        .enter_code(&other.challenge, &other_code, "pc")
        .await
        .ok(200);
}

#[tokio::test]
async fn wrong_codes_count_per_address_over_all_its_codes() {
    let cloud = TestCloud::start().await;
    let wrong = |code: &str| if code == "000000" { "111111" } else { "000000" }.to_owned();
    // Two codes spend their five tries each: ten wrong guesses.
    for _ in 0..2 {
        let (challenge, code) = cloud.ask_for_code("kim@example.com").await;
        for _ in 0..5 {
            cloud
                .enter_code(&challenge.challenge, &wrong(&code), "pc")
                .await
                .is_error("code_wrong", 400);
        }
    }
    // A new code for the address is not a new set of guesses, not even the right one works,
    // and a spelling of the same address shares the count.
    for email in ["kim@example.com", "kim+again@example.com"] {
        let (challenge, code) = cloud.ask_for_code(email).await;
        let refused = cloud.enter_code(&challenge.challenge, &code, "pc").await;
        refused.is_error("rate_limited", 429);
        // The player is told how long it lasts and that a passkey still works.
        let message = refused.body["message"].as_str().unwrap();
        assert!(message.contains("for the next 60 minutes"), "{message}");
        assert!(message.contains("passkey still works"), "{message}");
        assert!(refused.headers.contains_key("retry-after"));
    }
    // The answer to asking for a code is the same as before.
    ask(&cloud, "kim@example.com").await.ok(200);
    // Other addresses are not affected.
    let (challenge, code) = cloud.ask_for_code("lee@example.com").await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .ok(200);
}

#[tokio::test]
async fn a_few_wrong_codes_do_not_lock_the_address() {
    let cloud = TestCloud::start().await;
    let (challenge, code) = cloud.ask_for_code("kim@example.com").await;
    for _ in 0..3 {
        cloud
            .enter_code(&challenge.challenge, "12345x", "pc")
            .await
            .is_error("code_wrong", 400);
    }
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .ok(200);
}

#[tokio::test]
async fn spellings_of_one_address_share_the_limit_per_address() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.email_per_address = 2).await;
    ask(&cloud, "a.b+1@gmail.com").await.ok(200);
    ask(&cloud, "ab+2@googlemail.com").await.ok(200);
    ask(&cloud, "A.B@gmail.com")
        .await
        .is_error("rate_limited", 429);
    // Dots matter at other providers; a tag does not.
    ask(&cloud, "a.b@example.com").await.ok(200);
    ask(&cloud, "ab@example.com").await.ok(200);
    ask(&cloud, "ab+x@example.com").await.ok(200);
    ask(&cloud, "ab+y@example.com")
        .await
        .is_error("rate_limited", 429);
}

#[tokio::test]
async fn deleting_the_account_needs_the_proof_of_the_device_key() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = cloud.sign_in_keyed(&alec.email, "Keyed PC", &key).await;
    let token = &device.tokens.access;

    // A copied access token alone does nothing, and neither does another key's proof.
    cloud
        .delete("/v1/me", token)
        .await
        .is_error("unauthorized", 401);
    cloud
        .delete_me(token, &DeviceKey::generate())
        .await
        .is_error("unauthorized", 401);
    // A device without a key cannot ask either.
    cloud
        .delete("/v1/me", alec.token())
        .await
        .is_error("unauthorized", 401);
    cloud.get("/v1/me", token).await.ok(200);

    let answer = cloud.delete_me(token, &key).await.ok(200);
    assert!(answer.body["deletion"]["at"].is_u64(), "{}", answer.body);
}

#[tokio::test]
async fn refreshing_is_limited_per_client_address() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.refresh_per_ip = 2).await;
    let refresh = || {
        cloud.request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": "nope"})),
        )
    };
    for _ in 0..2 {
        refresh().await.is_error("unauthorized", 401);
    }
    refresh().await.is_error("rate_limited", 429);
}

#[tokio::test]
async fn the_sweeps_have_indexes() {
    let cloud = TestCloud::start().await;
    let names: Vec<(String,)> =
        match sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'index'")
            .fetch_all(&cloud.state.db)
            .await
        {
            Ok(rows) => rows,
            Err(_) => sqlx::query_as("SELECT indexname::text FROM pg_indexes")
                .fetch_all(&cloud.state.db)
                .await
                .unwrap(),
        };
    for wanted in [
        "challenges_expires",
        "challenges_email",
        "signups_expires",
        "invitations_deadline",
    ] {
        assert!(names.iter().any(|(n,)| n == wanted), "no index {wanted}");
    }
}
