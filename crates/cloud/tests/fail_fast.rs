//! A saturated store is answered fast and retryable instead of hanging for sqlx's 30 s wait.

mod common;

use std::time::{Duration, Instant};

use common::TestCloud;

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
    let a = cloud.sign_up("Alec").await;
    let held = (
        cloud.state.db.acquire().await.unwrap(),
        cloud.state.db.acquire().await.unwrap(),
    );
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
    drop(held);
    cloud.get("/v1/friends", a.token()).await.ok(200);
}

#[tokio::test]
async fn the_request_timeout_answers_503_with_retry_after() {
    // The pool would wait 10 s; the request gives up after 1.
    let cloud = starved(10_000, 1).await;
    let a = cloud.sign_up("Alec").await;
    let _held = (
        cloud.state.db.acquire().await.unwrap(),
        cloud.state.db.acquire().await.unwrap(),
    );
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
    let a = cloud.sign_up("Alec").await;
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
    let a = cloud.sign_up("Alec").await;
    let _held = (
        cloud.state.db.acquire().await.unwrap(),
        cloud.state.db.acquire().await.unwrap(),
    );
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
