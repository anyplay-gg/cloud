//! Every path that writes works with a pool of ONE connection.
//!
//! A handler that holds a connection (a store transaction) and, still inside it, asks the pool
//! for another one needs two connections to finish. With as many such calls at once as the pool
//! has connections, every one waits for another until the acquire timeout, and the node stops
//! answering store requests (docs/LOAD.md, "A performance bug: joins deadlock the connection
//! pool"). A pool of one connection finds this for a single call, with no concurrency and no
//! timing luck: the call can only succeed if it never needs a second connection at the same
//! time. Each call is asked again a few times, because the cloud's own background work (the
//! sweep) holds the one connection now and then; a nested ask is refused every time.

mod common;

use std::time::Duration;

use common::{Reply, TestCloud};
use reqwest::Method;
use serde_json::{Value, json};

async fn cloud() -> TestCloud {
    TestCloud::with(
        |c| {
            c.database_connections = 1;
            c.database_acquire_timeout_ms = 1500;
        },
        |_| {},
    )
    .await
}

/// Asks, and asks again while the answer is 429 (the pool did not hand out its connection in
/// time), at most four times.
async fn call(
    cloud: &TestCloud,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> Reply {
    let mut reply = cloud
        .request(method.clone(), path, Some(token), body.clone())
        .await;
    for _ in 0..3 {
        if reply.status != 429 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        reply = cloud
            .request(method.clone(), path, Some(token), body.clone())
            .await;
    }
    assert_ne!(
        reply.status, 429,
        "{method} {path} needs a second connection while it holds the first: {}",
        reply.body
    );
    reply
}

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

#[tokio::test]
async fn friends_blocks_and_the_friend_list() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    let cy = cloud.sign_up_patiently("Cy").await;
    call(&cloud, Method::GET, "/v1/me", ann.token(), None)
        .await
        .ok(200);
    call(
        &cloud,
        Method::POST,
        "/v1/friends/requests",
        ann.token(),
        Some(json!({"handle": ben.handle})),
    )
    .await
    .ok(200);
    call(
        &cloud,
        Method::POST,
        &format!("/v1/friends/requests/{}/accept", ann.id),
        ben.token(),
        None,
    )
    .await
    .ok(204);
    call(&cloud, Method::GET, "/v1/friends", ann.token(), None)
        .await
        .ok(200);
    call(
        &cloud,
        Method::PUT,
        &format!("/v1/blocks/{}", cy.id),
        ann.token(),
        None,
    )
    .await;
    call(
        &cloud,
        Method::DELETE,
        &format!("/v1/blocks/{}", cy.id),
        ann.token(),
        None,
    )
    .await;
}

#[tokio::test]
async fn a_party_and_its_chat() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    call(
        &cloud,
        Method::POST,
        "/v1/friends/requests",
        ann.token(),
        Some(json!({"handle": ben.handle})),
    )
    .await
    .ok(200);
    call(
        &cloud,
        Method::POST,
        &format!("/v1/friends/requests/{}/accept", ann.id),
        ben.token(),
        None,
    )
    .await
    .ok(204);
    call(
        &cloud,
        Method::POST,
        "/v1/party/invitations",
        ann.token(),
        Some(json!({"to": [ben.id]})),
    )
    .await
    .ok(200);
    let invitations = call(
        &cloud,
        Method::GET,
        "/v1/party/invitations",
        ben.token(),
        None,
    )
    .await
    .ok(200);
    let invitation = invitations.body["invitations"][0]["id"]
        .as_str()
        .expect("an invitation")
        .to_owned();
    let party = call(
        &cloud,
        Method::POST,
        &format!("/v1/party/invitations/{invitation}/accept"),
        ben.token(),
        None,
    )
    .await
    .ok(200);
    let conversation = party.body["conversation"].as_str().unwrap().to_owned();
    call(
        &cloud,
        Method::POST,
        &format!("/v1/conversations/{conversation}/messages"),
        ann.token(),
        Some(json!({"text": "hello"})),
    )
    .await
    .ok(200);
    call(
        &cloud,
        Method::GET,
        &format!("/v1/conversations/{conversation}/messages"),
        ben.token(),
        None,
    )
    .await
    .ok(200);
    call(&cloud, Method::GET, "/v1/party", ben.token(), None)
        .await
        .ok(200);
    call(&cloud, Method::POST, "/v1/party/leave", ben.token(), None).await;
}

#[tokio::test]
async fn a_session_is_created_joined_and_left() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    let created = call(
        &cloud,
        Method::POST,
        "/v1/sessions",
        ann.token(),
        Some(
            json!({"title_id": "GAME00001", "title": "Test Game", "version": "01.000",
                    "build": "0.1.3", "max_members": 4, "visibility": "public",
                    "properties": "", "endpoint": endpoint(1)}),
        ),
    )
    .await
    .ok(200);
    let id = created.body["id"].as_str().unwrap().to_owned();
    call(
        &cloud,
        Method::POST,
        &format!("/v1/sessions/{id}/join"),
        ben.token(),
        Some(
            json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(2),
                    "relay": null, "invitation": null}),
        ),
    )
    .await
    .ok(200);
    call(
        &cloud,
        Method::GET,
        &format!("/v1/sessions/{id}"),
        ben.token(),
        None,
    )
    .await
    .ok(200);
    call(
        &cloud,
        Method::POST,
        &format!("/v1/sessions/{id}/leave"),
        ben.token(),
        None,
    )
    .await;
    call(
        &cloud,
        Method::POST,
        &format!("/v1/sessions/{id}/leave"),
        ann.token(),
        None,
    )
    .await;
}
