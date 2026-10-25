//! Device binding: a refresh token only works with the key of its device, a proof works once,
//! a replaced refresh token signs the family out, "sign out everywhere" ends every device, and
//! devices from before keys existed have an explicit way over (network ADR 0016).

mod common;

use anyplay_api::{
    CodeAnswer, CodeRequest, Device, Platform, RefreshRequest, SignedIn, Tokens, path,
};
use anyplay_proof::{DeviceKey, Subject};
use common::TestCloud;
use reqwest::Method;
use serde_json::{Value, json};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn proof_for(key: &DeviceKey, refresh: &str, at: u64) -> String {
    anyplay_proof::make(
        key,
        &Subject {
            method: "POST",
            path: path::AUTH_REFRESH,
            credential: refresh,
        },
        at,
    )
}

/// A refresh request with an optional proof header and an optional key to bind.
async fn refresh(
    cloud: &TestCloud,
    refresh: &str,
    proof: Option<&str>,
    bind: Option<&DeviceKey>,
) -> common::Reply {
    let body = json!(RefreshRequest {
        refresh: refresh.into(),
        device_key: bind.map(DeviceKey::public),
    });
    let mut request = cloud
        .http
        .request(
            Method::POST,
            format!("http://{}{}", cloud.base, path::AUTH_REFRESH),
        )
        .json(&body);
    if let Some(proof) = proof {
        request = request.header(anyplay_proof::HEADER, proof);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let text = response.text().await.unwrap();
    common::Reply {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
    }
}

/// Signs `email` in on a new device that has `key` (or none, a legacy device).
async fn sign_in(cloud: &TestCloud, email: &str, name: &str, key: Option<&DeviceKey>) -> SignedIn {
    let (challenge, code) = cloud.ask_for_code(email).await;
    let reply = cloud
        .request(
            Method::POST,
            "/v1/auth/code",
            None,
            Some(json!(CodeRequest {
                challenge: challenge.challenge,
                code,
                device: Device {
                    name: name.into(),
                    platform: Platform::Linux,
                    key: key.map(DeviceKey::public),
                },
            })),
        )
        .await;
    match reply.ok(200).json() {
        CodeAnswer::SignedIn { tokens, me } => SignedIn { tokens, me },
        other => panic!("expected a sign-in, got {other:?}"),
    }
}

async fn me_status(cloud: &TestCloud, access: &str) -> u16 {
    cloud.get("/v1/me", access).await.status
}

fn tokens(reply: common::Reply) -> Tokens {
    reply.ok(200).json()
}

#[tokio::test]
async fn a_stolen_refresh_token_without_the_key_fails() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "bound pc", Some(&key)).await;
    let stolen = device.tokens.refresh.clone();

    // No proof at all.
    refresh(&cloud, &stolen, None, None)
        .await
        .is_error("unauthorized", 401);
    // A proof from another key, made for this token.
    let thief = DeviceKey::generate();
    refresh(
        &cloud,
        &stolen,
        Some(&proof_for(&thief, &stolen, now())),
        None,
    )
    .await
    .is_error("unauthorized", 401);
    // A thief who also claims a key of their own gets nowhere: the device has its key already.
    refresh(
        &cloud,
        &stolen,
        Some(&proof_for(&thief, &stolen, now())),
        Some(&thief),
    )
    .await
    .is_error("unauthorized", 401);
    // A proof the real key made for another token.
    refresh(
        &cloud,
        &stolen,
        Some(&proof_for(&key, "some-other-token", now())),
        None,
    )
    .await
    .is_error("unauthorized", 401);

    // None of that signed the device out: the owner refreshes.
    assert_eq!(me_status(&cloud, &device.tokens.access).await, 200);
    let fresh = tokens(
        refresh(
            &cloud,
            &stolen,
            Some(&proof_for(&key, &stolen, now())),
            None,
        )
        .await,
    );
    assert_ne!(fresh.refresh, stolen);
    assert_eq!(me_status(&cloud, &fresh.access).await, 200);
}

#[tokio::test]
async fn a_proof_with_a_wrong_clock_is_refused_without_signing_out() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let late = proof_for(&key, &device.tokens.refresh, now() - 300);
    let reply = refresh(&cloud, &device.tokens.refresh, Some(&late), None).await;
    reply.is_error("bad_request", 400);
    assert!(reply.body["message"].as_str().unwrap().contains("clock"));
    let ahead = proof_for(&key, &device.tokens.refresh, now() + 300);
    refresh(&cloud, &device.tokens.refresh, Some(&ahead), None)
        .await
        .is_error("bad_request", 400);
    // The token still works with a right proof.
    let ok = proof_for(&key, &device.tokens.refresh, now());
    refresh(&cloud, &device.tokens.refresh, Some(&ok), None)
        .await
        .ok(200);
}

#[tokio::test]
async fn a_replayed_proof_fails() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let access = device.tokens.access.clone();

    // The link to the account web needs a proof over the access token. The same proof, sent
    // twice, is accepted once: the access token is still good, so only the nonce stops it.
    let proof = anyplay_proof::make(
        &key,
        &Subject {
            method: "POST",
            path: path::AUTH_BROWSER_LINK,
            credential: &access,
        },
        now(),
    );
    let ask = |proof: String| {
        let request = cloud
            .http
            .post(format!("http://{}{}", cloud.base, path::AUTH_BROWSER_LINK))
            .bearer_auth(&access)
            .header(anyplay_proof::HEADER, proof);
        async move {
            let response = request.send().await.unwrap();
            response.status().as_u16()
        }
    };
    assert_eq!(ask(proof.clone()).await, 200);
    assert_eq!(ask(proof.clone()).await, 401, "the same proof twice");
    // A new proof is a new nonce.
    let again = anyplay_proof::make(
        &key,
        &Subject {
            method: "POST",
            path: path::AUTH_BROWSER_LINK,
            credential: &access,
        },
        now(),
    );
    assert_eq!(ask(again).await, 200);
    // A proof made for the refresh path does not open the link path.
    let wrong_path = proof_for(&key, &access, now());
    assert_eq!(ask(wrong_path).await, 401);
}

#[tokio::test]
async fn a_replaced_refresh_token_revokes_the_family() {
    let cloud = TestCloud::with(|c| c.refresh_replay_grace_secs = 1, |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let first = device.tokens.refresh.clone();

    let second = tokens(refresh(&cloud, &first, Some(&proof_for(&key, &first, now())), None).await);
    assert_eq!(me_status(&cloud, &second.access).await, 200);

    // The replaced token comes back with a perfectly good proof, but after the grace for a lost
    // answer: whoever sends it holds a copy, and the whole chain of the device ends.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    refresh(&cloud, &first, Some(&proof_for(&key, &first, now())), None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(
        me_status(&cloud, &second.access).await,
        401,
        "its access token"
    );
    refresh(
        &cloud,
        &second.refresh,
        Some(&proof_for(&key, &second.refresh, now())),
        None,
    )
    .await
    .is_error("unauthorized", 401);

    // The account's other devices are not touched.
    let other = sign_in(
        &cloud,
        &alec.email,
        "other pc",
        Some(&DeviceKey::generate()),
    )
    .await;
    assert_eq!(me_status(&cloud, &other.tokens.access).await, 200);
}

/// Rotates `current` once with a valid proof.
async fn step(cloud: &TestCloud, key: &DeviceKey, current: &str) -> Tokens {
    tokens(refresh(cloud, current, Some(&proof_for(key, current, now())), None).await)
}

#[tokio::test]
async fn a_replaced_refresh_token_with_a_valid_proof_is_a_lost_answer() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let r0 = device.tokens.refresh.clone();

    // The answer with R1 never arrives; a second later the device asks again with R0.
    let r1 = step(&cloud, &key, &r0).await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let r2 = step(&cloud, &key, &r0).await;
    assert_ne!(r2.refresh, r1.refresh);
    assert_eq!(me_status(&cloud, &r2.access).await, 200, "the device stays");

    // R1 was never delivered, and is dead: without the key it gets nowhere and cannot sign
    // the device out.
    refresh(&cloud, &r1.refresh, None, None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(me_status(&cloud, &r2.access).await, 200);
    // R2 works.
    let r3 = step(&cloud, &key, &r2.refresh).await;
    assert_eq!(me_status(&cloud, &r3.access).await, 200);
}

#[tokio::test]
async fn a_replaced_refresh_token_without_a_valid_proof_does_not_sign_out() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let r0 = device.tokens.refresh.clone();
    let r1 = step(&cloud, &key, &r0).await;

    refresh(&cloud, &r0, None, None)
        .await
        .is_error("unauthorized", 401);
    let thief = DeviceKey::generate();
    refresh(&cloud, &r0, Some(&proof_for(&thief, &r0, now())), None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(me_status(&cloud, &r1.access).await, 200);
    let r2 = step(&cloud, &key, &r1.refresh).await;
    assert_eq!(me_status(&cloud, &r2.access).await, 200);
}

#[tokio::test]
async fn a_grace_of_zero_keeps_the_old_contract() {
    let cloud = TestCloud::with(|c| c.refresh_replay_grace_secs = 0, |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let r0 = device.tokens.refresh.clone();
    let r1 = step(&cloud, &key, &r0).await;
    refresh(&cloud, &r0, Some(&proof_for(&key, &r0, now())), None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(me_status(&cloud, &r1.access).await, 401);
}

#[tokio::test]
async fn a_token_two_steps_back_signs_out_even_with_a_valid_proof() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let r0 = device.tokens.refresh.clone();
    let r1 = step(&cloud, &key, &r0).await;
    // Replacements in the same second cannot be told apart, so let a second pass.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let r2 = step(&cloud, &key, &r1.refresh).await;

    refresh(&cloud, &r0, Some(&proof_for(&key, &r0, now())), None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(me_status(&cloud, &r2.access).await, 401);
}

#[tokio::test]
async fn a_replaced_token_of_a_legacy_device_signs_out() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let first = alec.tokens.refresh.clone();
    let second = tokens(refresh(&cloud, &first, None, None).await);
    refresh(&cloud, &first, None, None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(me_status(&cloud, &second.access).await, 401);
}

#[tokio::test]
async fn a_proof_that_was_used_for_a_replaced_token_is_refused() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    let r0 = device.tokens.refresh.clone();
    let proof = proof_for(&key, &r0, now());
    let r1 = tokens(refresh(&cloud, &r0, Some(&proof), None).await);

    // The very same header again: only the nonce stops it, and the device stays.
    refresh(&cloud, &r0, Some(&proof), None)
        .await
        .is_error("unauthorized", 401);
    assert_eq!(me_status(&cloud, &r1.access).await, 200);
    let r2 = step(&cloud, &key, &r1.refresh).await;
    assert_eq!(me_status(&cloud, &r2.access).await, 200);
}

#[tokio::test]
async fn sign_out_everywhere_ends_every_device() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key_a = DeviceKey::generate();
    let key_b = DeviceKey::generate();
    let a = sign_in(&cloud, &alec.email, "pc a", Some(&key_a)).await;
    let b = sign_in(&cloud, &alec.email, "pc b", Some(&key_b)).await;
    let legacy = sign_in(&cloud, &alec.email, "old pc", None).await;
    let sam = cloud.sign_up("Sam").await;

    cloud.state.sign_out_everywhere(alec.id).await.unwrap();

    for (name, tokens) in [
        ("a", &a.tokens),
        ("b", &b.tokens),
        ("legacy", &legacy.tokens),
        ("first", &alec.tokens),
    ] {
        assert_eq!(
            me_status(&cloud, &tokens.access).await,
            401,
            "{name} access"
        );
        refresh(&cloud, &tokens.refresh, None, None)
            .await
            .is_error("unauthorized", 401);
    }
    assert_eq!(
        me_status(&cloud, sam.token()).await,
        200,
        "other accounts stay"
    );

    // Signing in again works.
    let again = sign_in(&cloud, &alec.email, "new pc", Some(&DeviceKey::generate())).await;
    assert_eq!(me_status(&cloud, &again.tokens.access).await, 200);
}

#[tokio::test]
async fn an_access_token_from_before_the_not_before_time_is_refused() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    assert_eq!(me_status(&cloud, alec.token()).await, 200);
    // The time moves ahead of the token while its device still exists: the account-level
    // cut-off alone refuses it.
    {
        let mut cache = cloud.state.cache.write().unwrap();
        cache.valid_from.insert(alec.id, now() + 100);
    }
    let reply = cloud.get("/v1/me", alec.token()).await;
    reply.is_error("unauthorized", 401);
    assert!(
        reply.body["message"]
            .as_str()
            .unwrap()
            .contains("signed out everywhere")
    );
}

#[tokio::test]
async fn a_legacy_device_works_until_it_is_bound_or_its_time_is_up() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    // The device the helper signed up is a legacy device: it has no key.
    let first = alec.tokens.refresh.clone();
    let second = tokens(refresh(&cloud, &first, None, None).await);
    let third = tokens(refresh(&cloud, &second.refresh, None, None).await);

    // The first refresh that brings a key and a proof made with it binds the device.
    let key = DeviceKey::generate();
    let proof = proof_for(&key, &third.refresh, now());
    let bound = tokens(refresh(&cloud, &third.refresh, Some(&proof), Some(&key)).await);
    assert_eq!(me_status(&cloud, &bound.access).await, 200);

    // From then on the proof is required, and binding again is refused.
    refresh(&cloud, &bound.refresh, None, None)
        .await
        .is_error("unauthorized", 401);
    let other = DeviceKey::generate();
    refresh(
        &cloud,
        &bound.refresh,
        Some(&proof_for(&other, &bound.refresh, now())),
        Some(&other),
    )
    .await
    .is_error("unauthorized", 401);
    let next = tokens(
        refresh(
            &cloud,
            &bound.refresh,
            Some(&proof_for(&key, &bound.refresh, now())),
            None,
        )
        .await,
    );
    assert_eq!(me_status(&cloud, &next.access).await, 200);
}

#[tokio::test]
async fn a_legacy_device_past_its_grace_period_must_sign_in_again() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    sqlx::query("UPDATE devices SET legacy_until = $1")
        .bind(now() as i64 - 1)
        .execute(&cloud.state.db)
        .await
        .unwrap();

    let reply = refresh(&cloud, &alec.tokens.refresh, None, None).await;
    reply.is_error("unauthorized", 401);
    // Not even a key and a proof save it.
    let key = DeviceKey::generate();
    let proof = proof_for(&key, &alec.tokens.refresh, now());
    refresh(&cloud, &alec.tokens.refresh, Some(&proof), Some(&key))
        .await
        .is_error("unauthorized", 401);
    // Signing in again gives a bound device.
    let fresh = sign_in(&cloud, &alec.email, "pc", Some(&key)).await;
    assert_eq!(me_status(&cloud, &fresh.tokens.access).await, 200);
}

#[tokio::test]
async fn a_cloud_can_require_keys_from_the_start() {
    let cloud = TestCloud::with(|c| c.legacy_device_days = 0, |_| {}).await;
    let (challenge, code) = cloud.ask_for_code("new@example.com").await;
    // The new address comes back as a sign-up; the device without a key is refused already
    // when it presents the code.
    let reply = cloud
        .enter_code(&challenge.challenge, &code, "old launcher")
        .await;
    reply.is_error("bad_request", 400);
    assert!(
        reply.body["message"]
            .as_str()
            .unwrap()
            .contains("device key")
    );
}

#[tokio::test]
async fn a_key_that_is_not_a_key_is_refused_at_sign_in() {
    let cloud = TestCloud::start().await;
    let (challenge, code) = cloud.ask_for_code("a@example.com").await;
    for bad in ["", "AAAA", "not base64!", &"A".repeat(60)] {
        let reply = cloud
            .request(
                Method::POST,
                "/v1/auth/code",
                None,
                Some(json!({
                    "challenge": challenge.challenge,
                    "code": code,
                    "device": {"name": "pc", "platform": "linux", "key": bad},
                })),
            )
            .await;
        reply.is_error("bad_request", 400);
    }
}
