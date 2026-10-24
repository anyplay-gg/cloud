//! Several nodes of one cloud: one PostgreSQL database, one NATS server, players on different
//! nodes (`docs/ARCHITECTURE.md`, stage 2). These tests need both servers; without
//! `ANYPLAY_TEST_DATABASE_URL` and `ANYPLAY_TEST_NATS_URL` they pass without running.

mod common;

use std::time::Duration;

use anyplay_api::{Event, Session, close};
use common::{Events, TestCloud};
use reqwest::Method;
use serde_json::json;

const ADMIN_TOKEN: &str = "an-admin-token-for-tests";

async fn two_nodes() -> Option<(TestCloud, TestCloud)> {
    let mut nodes = TestCloud::cluster(2, |c| c.admin_token = Some(ADMIN_TOKEN.into())).await?;
    let b = nodes.pop()?;
    let a = nodes.pop()?;
    Some((a, b))
}

macro_rules! cluster_or_skip {
    () => {
        match two_nodes().await {
            Some(nodes) => nodes,
            None => {
                eprintln!("no test database and NATS server; skipped");
                return;
            }
        }
    };
}

#[tokio::test]
async fn a_device_signed_in_through_one_node_is_known_to_the_other() {
    let (one, two) = cluster_or_skip!();
    let alec = one.sign_up("Alec").await;
    two.get("/v1/me", alec.token()).await.ok(200);
}

#[tokio::test]
async fn friends_on_different_nodes_see_each_other_come_online_and_play() {
    let (one, two) = cluster_or_skip!();
    let (alec, bea) = one.friends("Alec", "Bea").await;
    // Bea listens through the second node.
    let mut bea_events = two.events(bea.token()).await;
    // Alec comes online through the first.
    let _alec_events = one.events(alec.token()).await;
    let Event::Presence { account, presence } = bea_events
        .until(|e| matches!(e, Event::Presence { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(account.0, alec.id);
    assert!(presence.online);

    one.put(
        "/v1/presence",
        alec.token(),
        Some(json!({"activity": {"title_id": "PPSA00001", "title": "Game", "version": "1.00"}})),
    )
    .await
    .ok(204);
    let Event::Presence { presence, .. } = bea_events
        .until(|e| matches!(e, Event::Presence { presence, .. } if presence.activity.is_some()))
        .await
    else {
        unreachable!()
    };
    assert_eq!(presence.activity.unwrap().title_id, "PPSA00001");
}

#[tokio::test]
async fn a_session_changed_on_one_node_reaches_members_on_the_other() {
    let (one, two) = cluster_or_skip!();
    let (alec, bea) = one.friends("Alec", "Bea").await;
    let mut alec_events = one.events(alec.token()).await;
    let _bea_events = two.events(bea.token()).await;
    let endpoint = "ab".repeat(32);
    let session: Session = one
        .post(
            "/v1/sessions",
            alec.token(),
            json!({"title_id": "PPSA00001", "title": "Game", "version": "1.00", "build": "b1",
                   "max_members": 4, "visibility": "friends", "endpoint": endpoint}),
        )
        .await
        .ok(200)
        .json();
    // Bea joins through the other node; Alec hears of it.
    two.post(
        &format!("/v1/sessions/{}/join", session.id),
        bea.token(),
        json!({"version": "1.00", "build": "b1", "endpoint": "cd".repeat(32)}),
    )
    .await
    .ok(200);
    let Event::Session { session } = alec_events
        .until(|e| matches!(e, Event::Session { session } if session.members.len() == 2))
        .await
    else {
        unreachable!()
    };
    assert_eq!(session.members[1].account.id.0, bea.id);
}

#[tokio::test]
async fn signing_out_on_one_node_closes_the_connection_on_the_other() {
    let (one, two) = cluster_or_skip!();
    let alec = one.sign_up("Alec").await;
    let mut on_two = two.events(alec.token()).await;
    one.post_empty("/v1/auth/signout", alec.token())
        .await
        .ok(204);
    assert_eq!(on_two.next().await, Event::SignedOut);
    assert_eq!(on_two.close_code().await, Some(close::SIGNED_OUT));
    // The second node no longer takes the token either.
    let refused = wait_for(|| async { two.get("/v1/me", alec.token()).await.status == 401 }).await;
    assert!(refused, "the second node forgot the device");
}

#[tokio::test]
async fn a_ban_on_one_node_holds_on_the_other() {
    let (one, two) = cluster_or_skip!();
    let sam = one.sign_up("Sam").await;
    let mut on_two = two.events(sam.token()).await;
    one.request(
        Method::POST,
        "/admin/v1/bans",
        Some(ADMIN_TOKEN),
        Some(json!({"handle": "sam", "reason": "cheating"})),
    )
    .await
    .ok(200);
    assert_eq!(on_two.next().await, Event::Banned { until: None });
    assert_eq!(on_two.close_code().await, Some(close::BANNED));
    let banned = wait_for(|| async { two.get("/v1/me", sam.token()).await.status == 403 }).await;
    assert!(banned, "the second node knows the ban");
    let mut again = Events::connect_raw(&two.base, sam.token()).await.unwrap();
    assert_eq!(
        again.next_within(Duration::from_secs(2)).await,
        Some(Event::Banned { until: None })
    );
}

/// True as soon as `check` holds, within a few seconds: news between nodes takes a moment.
async fn wait_for<F, Fut>(check: F) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..50 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}
