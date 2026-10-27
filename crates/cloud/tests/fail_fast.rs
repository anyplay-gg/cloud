//! A saturated store is answered fast and retryable instead of hanging for sqlx's 30 s wait.

mod common;

use std::time::{Duration, Instant};

use anyplay_api::{CodeAnswer, SignedIn};
use common::{Reply, TestCloud, User};
use reqwest::Method;
use serde_json::{Value, json};

/// A request that is asked again while the answer is 429. A pool of two connections with a
/// short acquire timeout is starved by the cloud's own background work now and then (the sweep,
/// the heartbeat), most often on a slow runner, so the setup of these tests must not depend on
/// the pool being free at that moment. What the tests assert is about the state they make
/// themselves: [`hold`] takes the whole pool, and only then is anything measured.
async fn until_served(
    cloud: &TestCloud,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let reply = cloud
            .request(method.clone(), path, token, body.clone())
            .await;
        if reply.status != 429 || Instant::now() > until {
            return reply;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `TestCloud::sign_up`, patient with a starved pool.
async fn sign_up(cloud: &TestCloud, handle: &str) -> User {
    let email = format!("{}@example.com", handle.to_lowercase());
    let asked = until_served(
        cloud,
        Method::POST,
        "/v1/auth/email",
        None,
        Some(json!({ "email": email })),
    )
    .await
    .ok(200);
    let challenge = asked.body["challenge"].as_str().unwrap().to_owned();
    let code = cloud.mail.last_code(&email).expect("a mail was sent");
    let answer: CodeAnswer = until_served(
        cloud,
        Method::POST,
        "/v1/auth/code",
        None,
        Some(json!({
            "challenge": challenge,
            "code": code,
            "device": TestCloud::device("first pc"),
        })),
    )
    .await
    .ok(200)
    .json();
    let CodeAnswer::NewAccount { signup, .. } = answer else {
        panic!("expected a new account for {email}");
    };
    let signed_in: SignedIn = until_served(
        cloud,
        Method::POST,
        "/v1/auth/signup",
        None,
        Some(json!({
            "signup": signup,
            "handle": handle,
            "age_confirmed": true,
            "device": TestCloud::device("test pc"),
        })),
    )
    .await
    .ok(200)
    .json();
    User {
        email,
        id: signed_in.me.id.0,
        handle: signed_in.me.handle,
        tokens: signed_in.tokens,
    }
}

/// Takes both connections of the two-connection pool, waiting for the cloud's own background
/// work to give them back first. From here on the pool is empty by the test's own doing, not by
/// luck of timing.
async fn hold(cloud: &TestCloud) -> Vec<sqlx::pool::PoolConnection<sqlx::Any>> {
    let mut held = Vec::new();
    let until = Instant::now() + Duration::from_secs(30);
    while held.len() < 2 {
        match cloud.state.db.acquire().await {
            Ok(connection) => held.push(connection),
            Err(error) => {
                assert!(Instant::now() < until, "cannot take the pool: {error}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    held
}

/// A cloud with a two-connection pool whose connections the test can hold. Runs on PostgreSQL
/// when `ANYPLAY_TEST_DATABASE_URL` is set, else on SQLite.
async fn starved(acquire_ms: u64, request_secs: u64) -> TestCloud {
    TestCloud::with(
        |c| {
            c.database_connections = 2;
            c.database_acquire_timeout_ms = acquire_ms;
            c.request_timeout_secs = request_secs;
        },
        |_| {},
    )
    .await
}

#[tokio::test]
async fn a_starved_pool_answers_429_with_retry_after_within_the_acquire_timeout() {
    let cloud = starved(500, 30).await;
    let a = sign_up(&cloud, "Alec").await;
    let held = hold(&cloud).await;
    let started = Instant::now();
    let reply = cloud.get("/v1/friends", a.token()).await;
    let took = started.elapsed();
    assert_eq!(reply.status, 429, "{:?}", reply.body);
    assert_eq!(reply.body["error"], "rate_limited");
    let retry: u64 = reply.headers["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=2).contains(&retry));
    assert!(
        took < Duration::from_secs(3),
        "answered after {took:?}, not within the acquire timeout"
    );
    assert!(took >= Duration::from_millis(400), "{took:?}");

    // Once a connection is free again the same call works.
    // A dropped connection goes back to the pool on a task of its own, so the very next call
    // can still find the pool empty (seen on the slower Windows runner): ask until it works.
    drop(held);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let reply = cloud.get("/v1/friends", a.token()).await;
        if reply.status == 200 {
            break;
        }
        assert!(
            Instant::now() < until,
            "still {}: {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn the_request_timeout_answers_503_with_retry_after() {
    // The pool would wait 10 s; the request gives up after 1.
    let cloud = starved(10_000, 1).await;
    let a = sign_up(&cloud, "Alec").await;
    let _held = hold(&cloud).await;
    let started = Instant::now();
    let reply = cloud.get("/v1/friends", a.token()).await;
    let took = started.elapsed();
    assert_eq!(reply.status, 503, "{:?}", reply.body);
    assert!(reply.headers.contains_key("retry-after"));
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[tokio::test]
async fn the_event_connection_is_not_cut_by_the_request_timeout() {
    let cloud = starved(5000, 1).await;
    let a = sign_up(&cloud, "Alec").await;
    let mut events = cloud.events(a.token()).await;
    // Past the request timeout the socket is still open: it only ends by a close frame.
    let ended = events.next_within(Duration::from_millis(2500)).await;
    assert!(ended.is_none(), "unexpected {ended:?}");
    cloud.get("/v1/friends", a.token()).await.ok(200);
}

#[tokio::test]
async fn requests_over_the_cap_are_shed_with_503() {
    let cloud = TestCloud::with(
        |c| {
            c.max_in_flight_requests = 1;
            c.database_connections = 2;
            c.database_acquire_timeout_ms = 2000;
        },
        |_| {},
    )
    .await;
    let a = sign_up(&cloud, "Alec").await;
    let _held = hold(&cloud).await;
    // The first request takes the one place and waits for the pool; the second is turned away.
    let slow = tokio::spawn({
        let (base, token) = (cloud.base.clone(), a.token().to_owned());
        async move {
            reqwest::Client::new()
                .get(format!("http://{base}/v1/friends"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let shed = cloud.get("/v1/friends", a.token()).await;
    assert_eq!(shed.status, 503);
    assert!(shed.headers.contains_key("retry-after"));
    assert_eq!(slow.await.unwrap(), 429);
}

#[tokio::test]
async fn the_health_checks_answer_when_the_cap_is_full() {
    let cloud = TestCloud::with(
        |c| {
            c.max_in_flight_requests = 1;
            c.database_connections = 2;
            c.database_acquire_timeout_ms = 2000;
        },
        |_| {},
    )
    .await;
    let a = sign_up(&cloud, "Alec").await;
    let _held = hold(&cloud).await;
    let slow = tokio::spawn({
        let (base, token) = (cloud.base.clone(), a.token().to_owned());
        async move {
            reqwest::Client::new()
                .get(format!("http://{base}/v1/friends"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The one place is taken: an ordinary request is shed, the probes are not.
    assert_eq!(cloud.get("/v1/friends", a.token()).await.status, 503);
    let client = reqwest::Client::new();
    let get = |path: &'static str| {
        let client = client.clone();
        let url = format!("http://{}{path}", cloud.base);
        async move {
            let reply = client.get(url).send().await.unwrap();
            (reply.status().as_u16(), reply.text().await.unwrap())
        }
    };
    assert_eq!(get("/healthz").await, (200, "ok".to_owned()));
    // The store is starved here, so readiness says so itself; the cap does not answer for it.
    let (status, body) = get("/readyz").await;
    assert_eq!(
        (status, body.as_str()),
        (503, "the store does not answer"),
        "readiness was shed"
    );
    slow.await.unwrap();
}

#[tokio::test]
async fn a_write_that_ran_into_the_request_timeout_still_finishes() {
    // The pool would wait 10 s for the write; the request gives up after 1 s, the write does not.
    let cloud = starved(10_000, 1).await;
    let a = sign_up(&cloud, "Alec").await;
    let held = hold(&cloud).await;
    let started = Instant::now();
    let reply = cloud
        .put(
            "/v1/me/appear-offline",
            a.token(),
            Some(serde_json::json!({"appear_offline": true})),
        )
        .await;
    assert_eq!(reply.status, 503, "{:?}", reply.body);
    assert!(reply.headers.contains_key("retry-after"));
    assert!(started.elapsed() < Duration::from_secs(3));
    let message = reply.body["message"].as_str().unwrap_or_default();
    assert!(message.contains("may still go through"), "{message}");

    // Once the store is free again the write it was working on lands, without a second call.
    drop(held);
    let mut set = false;
    for _ in 0..40 {
        let now = cloud.get("/v1/me/appear-offline", a.token()).await;
        if now.status == 200 && now.body["appear_offline"] == true {
            set = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(set, "the write was dropped with the request");
}
