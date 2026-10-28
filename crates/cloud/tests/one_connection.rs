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

use common::{Reply, TestCloud, User};
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
    assert!(
        reply.status < 500,
        "{method} {path} answered {}: {}",
        reply.status,
        reply.body
    );
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
    let cy = cloud.sign_up_patiently("Cyn").await;
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
    let created = call(
        &cloud,
        Method::POST,
        "/v1/sessions",
        ann.token(),
        Some(
            json!({"title_id": "GAME00001", "title": "Test Game", "version": "01.000",
                    "build": "0.1.3", "max_members": 4, "visibility": "friends",
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

// --- the rest of the write paths -----------------------------------------------------------
//
// Each flow below reaches the `begin_write` call sites of one area. Most calls only have to be
// answered at all (not 429, not 5xx): what they answer is the business of the tests of their
// own area.

async fn post(cloud: &TestCloud, who: &User, path: &str, body: Value) -> Reply {
    call(cloud, Method::POST, path, who.token(), Some(body)).await
}

async fn post0(cloud: &TestCloud, who: &User, path: &str) -> Reply {
    call(cloud, Method::POST, path, who.token(), None).await
}

async fn get(cloud: &TestCloud, who: &User, path: &str) -> Reply {
    call(cloud, Method::GET, path, who.token(), None).await
}

async fn put(cloud: &TestCloud, who: &User, path: &str, body: Value) -> Reply {
    call(cloud, Method::PUT, path, who.token(), Some(body)).await
}

async fn patch(cloud: &TestCloud, who: &User, path: &str, body: Value) -> Reply {
    call(cloud, Method::PATCH, path, who.token(), Some(body)).await
}

async fn del(cloud: &TestCloud, who: &User, path: &str) -> Reply {
    call(cloud, Method::DELETE, path, who.token(), None).await
}

async fn befriend(cloud: &TestCloud, a: &User, b: &User) {
    post(
        cloud,
        a,
        "/v1/friends/requests",
        json!({"handle": b.handle}),
    )
    .await
    .ok(200);
    post0(cloud, b, &format!("/v1/friends/requests/{}/accept", a.id))
        .await
        .ok(204);
}

/// `lead` invites `friend`, who accepts: one party. Returns it.
async fn into_party(cloud: &TestCloud, lead: &User, friend: &User) -> Value {
    post(
        cloud,
        lead,
        "/v1/party/invitations",
        json!({"to": [friend.id]}),
    )
    .await
    .ok(200);
    let invitations = get(cloud, friend, "/v1/party/invitations").await.ok(200);
    let id = invitations.body["invitations"][0]["id"]
        .as_str()
        .expect("an invitation")
        .to_owned();
    post0(cloud, friend, &format!("/v1/party/invitations/{id}/accept"))
        .await
        .ok(200)
        .body
}

fn activity() -> Value {
    json!({"activity": {"title_id": "CUSA00001", "title": "A game", "version": "1.00"}})
}

#[tokio::test]
async fn friend_requests_are_cancelled_and_friends_removed() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    let cyn = cloud.sign_up_patiently("Cyn").await;
    post(
        &cloud,
        &ann,
        "/v1/friends/requests",
        json!({"handle": cyn.handle}),
    )
    .await
    .ok(200);
    del(&cloud, &ann, &format!("/v1/friends/requests/{}", cyn.id)).await;
    befriend(&cloud, &ann, &ben).await;
    del(&cloud, &ann, &format!("/v1/friends/{}", ben.id)).await;
    put(
        &cloud,
        &ann,
        "/v1/me/appear-offline",
        json!({"appear_offline": true}),
    )
    .await;
    put(&cloud, &ann, "/v1/presence", activity()).await;
    get(&cloud, &ann, "/v1/me/played-with").await;
}

#[tokio::test]
async fn conversations_messages_and_their_changes() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    let cyn = cloud.sign_up_patiently("Cyn").await;
    let dee = cloud.sign_up_patiently("Dee").await;
    for friend in [&ben, &cyn, &dee] {
        befriend(&cloud, &ann, friend).await;
    }
    let group = post(
        &cloud,
        &ann,
        "/v1/conversations",
        json!({"accounts": [ben.id, cyn.id], "name": "Raid night"}),
    )
    .await
    .ok(200);
    let id = group.body["id"].as_str().unwrap().to_owned();
    let sent = post(
        &cloud,
        &ann,
        &format!("/v1/conversations/{id}/messages"),
        json!({"text": "first"}),
    )
    .await
    .ok(200);
    let seq = sent.body["seq"].as_u64().expect("a number");
    let message = format!("/v1/conversations/{id}/messages/{seq}");
    patch(&cloud, &ann, &message, json!({"text": "edited"})).await;
    put(
        &cloud,
        &ben,
        &format!("{message}/reaction"),
        json!({"reaction": "gg"}),
    )
    .await;
    del(&cloud, &ben, &format!("{message}/reaction")).await;
    put(
        &cloud,
        &ben,
        &format!("/v1/conversations/{id}/typing"),
        json!({"typing": true}),
    )
    .await;
    patch(
        &cloud,
        &ben,
        &format!("/v1/conversations/{id}/settings"),
        json!({"muted": true}),
    )
    .await;
    post(
        &cloud,
        &ann,
        &format!("/v1/conversations/{id}/members"),
        json!({"accounts": [dee.id]}),
    )
    .await;
    del(&cloud, &ann, &message).await;
    del(&cloud, &cyn, &format!("/v1/conversations/{id}")).await;
    // A direct conversation with someone who is no friend is a request until it is accepted.
    let eve = cloud.sign_up_patiently("Eve").await;
    let direct = post(
        &cloud,
        &ann,
        "/v1/conversations",
        json!({"accounts": [eve.id]}),
    )
    .await
    .ok(200);
    let direct = direct.body["id"].as_str().unwrap().to_owned();
    post0(&cloud, &eve, &format!("/v1/conversations/{direct}/accept")).await;
}

#[tokio::test]
async fn a_screenshot_is_sent_as_a_media_message() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    befriend(&cloud, &ann, &ben).await;
    let chat = post(
        &cloud,
        &ann,
        "/v1/conversations",
        json!({"accounts": [ben.id]}),
    )
    .await
    .ok(200);
    let chat = chat.body["id"].as_str().unwrap().to_owned();
    let picture = image::RgbaImage::from_pixel(8, 6, image::Rgba([10, 20, 30, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let mut status = 0;
    for _ in 0..10 {
        let reply = cloud
            .http
            .post(format!(
                "http://{}/v1/conversations/{chat}/media",
                cloud.base
            ))
            .bearer_auth(ann.token())
            .header("content-type", "image/png")
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        status = reply.status().as_u16();
        // A 429 can also be the upload slots, which the tests of this process share.
        if status != 429 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(status < 500, "the upload answered {status}");
}

#[tokio::test]
async fn looking_for_group_listings_asks_and_answers() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    let cyn = cloud.sign_up_patiently("Cyn").await;
    let listing = json!({"title_id": "GAME00001", "mode": "online", "language": "en",
                         "note": "chill runs", "slots": 2});
    let made = post(&cloud, &ann, "/v1/lfg", listing).await.ok(200);
    let id = match &made.body["id"] {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    post0(&cloud, &ben, &format!("/v1/lfg/{id}/ask")).await;
    post0(&cloud, &cyn, &format!("/v1/lfg/{id}/ask")).await;
    get(&cloud, &ann, "/v1/lfg/mine").await;
    post0(
        &cloud,
        &ann,
        &format!("/v1/lfg/{id}/requests/{}/accept", ben.id),
    )
    .await;
    del(&cloud, &ann, &format!("/v1/lfg/{id}/requests/{}", cyn.id)).await;
    del(&cloud, &ben, &format!("/v1/lfg/{id}/ask")).await;
    del(&cloud, &ann, &format!("/v1/lfg/{id}")).await;
}

#[tokio::test]
async fn parties_calls_join_codes_and_sessions_in_every_shape() {
    let cloud = cloud().await;
    let ann = cloud.sign_up_patiently("Ann").await;
    let ben = cloud.sign_up_patiently("Ben").await;
    let cyn = cloud.sign_up_patiently("Cyn").await;
    befriend(&cloud, &ann, &ben).await;
    befriend(&cloud, &ann, &cyn).await;

    // A call that is declined, and the missed-call mark that is cleared.
    post(
        &cloud,
        &ann,
        "/v1/party/invitations",
        json!({"to": [cyn.id], "call": true}),
    )
    .await
    .ok(200);
    let ring = get(&cloud, &cyn, "/v1/party/invitations").await.ok(200);
    let ring = ring.body["invitations"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    del(&cloud, &cyn, &format!("/v1/party/invitations/{ring}")).await;
    del(&cloud, &cyn, &format!("/v1/calls/missed/{}", ann.id)).await;

    // The party: members, endpoint, a session it moves to, a kick, a join code.
    into_party(&cloud, &ann, &ben).await;
    put(
        &cloud,
        &ben,
        "/v1/party/endpoint",
        json!({"endpoint": endpoint(2), "relay": "https://r.example.com", "voice": true}),
    )
    .await;
    patch(&cloud, &ann, "/v1/party", json!({"leader": ben.id})).await;
    patch(&cloud, &ben, "/v1/party", json!({"leader": ann.id})).await;
    let session = post(
        &cloud,
        &ann,
        "/v1/sessions",
        json!({"title_id": "GAME00001", "title": "Test Game", "version": "01.000",
               "build": "0.1.3", "max_members": 4, "visibility": "invite",
               "properties": "", "endpoint": endpoint(1)}),
    )
    .await
    .ok(200);
    let sid = session.body["id"].as_str().unwrap().to_owned();
    post(&cloud, &ann, "/v1/party/session", json!({"session": sid})).await;
    post(
        &cloud,
        &ann,
        &format!("/v1/sessions/{sid}/invitations"),
        json!({"to": [cyn.id]}),
    )
    .await;
    patch(
        &cloud,
        &ann,
        &format!("/v1/sessions/{sid}"),
        json!({"visibility": "closed"}),
    )
    .await;
    del(
        &cloud,
        &ann,
        &format!("/v1/sessions/{sid}/members/{}", ben.id),
    )
    .await;
    let code = post(&cloud, &ann, "/v1/join-codes", json!({}))
        .await
        .ok(200);
    let code = code.body["code"].as_str().unwrap().to_owned();
    post0(&cloud, &cyn, &format!("/v1/join-codes/{code}/redeem")).await;
    del(&cloud, &ann, "/v1/join-codes/current").await;
    del(&cloud, &ann, &format!("/v1/party/members/{}", ben.id)).await;
    post0(&cloud, &ann, "/v1/party/leave").await;
}

#[tokio::test]
async fn reports_actions_and_appeals() {
    const ADMIN: &str = "an-admin-token-for-tests";
    let cloud = TestCloud::with(
        |c| {
            c.database_connections = 1;
            c.database_acquire_timeout_ms = 1500;
            c.admin_token = Some(ADMIN.into());
        },
        |_| {},
    )
    .await;
    let mona = cloud.sign_up_patiently("Mona").await;
    let kim = cloud.sign_up_patiently("Kim").await;
    let target = cloud.sign_up_patiently("Tara").await;
    call(
        &cloud,
        Method::POST,
        "/v1/moderation/moderators",
        ADMIN,
        Some(json!({"handle": mona.handle})),
    )
    .await
    .ok(200);
    post(
        &cloud,
        &kim,
        "/v1/reports",
        json!({"account": target.id, "reason": "harassment", "details": "rude in chat"}),
    )
    .await
    .ok(204);
    let queue = get(&cloud, &mona, "/v1/moderation/reports?status=open")
        .await
        .ok(200);
    let id = queue.body[0]["id"].as_u64().expect("a report");
    post(
        &cloud,
        &mona,
        &format!("/v1/moderation/reports/{id}/dismiss"),
        json!({"reason": "Nothing."}),
    )
    .await;
    let action = post(
        &cloud,
        &mona,
        "/v1/moderation/actions",
        json!({"account": target.id, "kind": "warning", "rule": "spam", "reason": "Stop."}),
    )
    .await
    .ok(200);
    if let Some(action) = action.body["id"].as_u64() {
        post(
            &cloud,
            &target,
            &format!("/v1/me/actions/{action}/appeal"),
            json!({"text": "It was a joke."}),
        )
        .await;
    }
    get(&cloud, &mona, "/v1/moderation/appeals").await;
    get(&cloud, &mona, "/v1/moderation/log?limit=20").await;
}

#[tokio::test]
async fn a_share_is_offered_accepted_changed_and_ended() {
    let cloud = TestCloud::with(
        |c| {
            c.database_connections = 1;
            c.database_acquire_timeout_ms = 1500;
            c.share_enabled = true;
        },
        |_| {},
    )
    .await;
    let host = cloud.sign_up_patiently("Host").await;
    let guest = cloud.sign_up_patiently("Guest").await;
    befriend(&cloud, &host, &guest).await;
    into_party(&cloud, &host, &guest).await;
    put(&cloud, &host, "/v1/presence", activity()).await;
    post(
        &cloud,
        &host,
        "/v1/shares",
        json!({"to": guest.id, "mode": "watch", "endpoint": endpoint(1)}),
    )
    .await;
    get(&cloud, &guest, "/v1/share/invitations").await;
    post(
        &cloud,
        &guest,
        "/v1/share/accept",
        json!({"endpoint": endpoint(2), "relay": "https://relay.example.com"}),
    )
    .await;
    patch(&cloud, &host, "/v1/share", json!({"mode": "watch"})).await;
    get(&cloud, &guest, "/v1/share").await;
    del(&cloud, &host, "/v1/share").await;
}
