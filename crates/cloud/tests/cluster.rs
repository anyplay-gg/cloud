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
    assert_eq!(
        on_two.next().await,
        Event::Banned {
            until: None,
            statement: None,
        }
    );
    assert_eq!(on_two.close_code().await, Some(close::BANNED));
    let banned = wait_for(|| async { two.get("/v1/me", sam.token()).await.status == 403 }).await;
    assert!(banned, "the second node knows the ban");
    let mut again = Events::connect_raw(&two.base, sam.token()).await.unwrap();
    assert_eq!(
        again.next_within(Duration::from_secs(2)).await,
        Some(Event::Banned {
            until: None,
            statement: None,
        })
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

// Parties and messages across nodes

async fn befriend(from: &TestCloud, a: &common::User, b: &common::User) {
    from.post(
        "/v1/friends/requests",
        a.token(),
        json!({"handle": b.handle}),
    )
    .await
    .ok(200);
    from.post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
        .await
        .ok(204);
}

#[tokio::test]
async fn members_on_different_nodes_receive_each_others_messages() {
    let (one, two) = cluster_or_skip!();
    let (alec, bea) = one.friends("Alec", "Bea").await;
    let mut alec_events = one.events(alec.token()).await;
    let mut bea_events = two.events(bea.token()).await;

    let conversation: anyplay_api::Conversation = one
        .post(
            "/v1/conversations",
            alec.token(),
            json!({"accounts": [bea.id]}),
        )
        .await
        .ok(200)
        .json();
    // Written through one node, announced to a connection on the other, fetched from it.
    let path = format!("/v1/conversations/{}/messages", conversation.id);
    one.post(&path, alec.token(), json!({"text": "over here"}))
        .await
        .ok(200);
    let Event::Message {
        conversation: id,
        seq,
    } = bea_events
        .until(|e| matches!(e, Event::Message { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!((id.as_str(), seq), (conversation.id.as_str(), 1));
    let fetched: anyplay_api::Messages = two.get(&path, bea.token()).await.ok(200).json();
    assert_eq!(fetched.messages[0].text, "over here");

    // The answer goes the other way, with the next number.
    let reply: anyplay_api::Message = two
        .post(&path, bea.token(), json!({"text": "and over here"}))
        .await
        .ok(200)
        .json();
    assert_eq!(reply.seq, 2);
    let Event::Message { seq, .. } = alec_events
        .until(|e| matches!(e, Event::Message { seq: 2, .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(seq, 2);

    // Read marks travel too: Alec reads Bea's answer on the first node, Bea hears it on the second.
    one.put(
        &format!("/v1/conversations/{}/read", conversation.id),
        alec.token(),
        Some(json!({"seq": 2})),
    )
    .await
    .ok(204);
    let Event::Read { account, seq, .. } =
        bea_events.until(|e| matches!(e, Event::Read { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((account.0, seq), (alec.id, 2));
}

#[tokio::test]
async fn writers_on_two_nodes_at_the_same_moment_never_get_the_same_number() {
    let (one, two) = cluster_or_skip!();
    let (one, two) = (std::sync::Arc::new(one), std::sync::Arc::new(two));
    let hub = one.sign_up("Hub").await;
    let mut others = Vec::new();
    for k in 0..3 {
        let user = one.sign_up(&format!("Pal{k}")).await;
        befriend(&one, &hub, &user).await;
        others.push(user);
    }
    let group: anyplay_api::Conversation = one
        .post(
            "/v1/conversations",
            hub.token(),
            json!({"accounts": others.iter().map(|u| u.id).collect::<Vec<_>>()}),
        )
        .await
        .ok(200)
        .json();

    // Four writers, two on each node, ten messages each, all at once.
    let mut tokens = vec![hub.token().to_owned()];
    tokens.extend(others.iter().map(|u| u.token().to_owned()));
    let mut writers = Vec::new();
    for (n, token) in tokens.iter().enumerate() {
        for k in 0..10 {
            let node = if n % 2 == 0 { one.clone() } else { two.clone() };
            let token = token.clone();
            let path = format!("/v1/conversations/{}/messages", group.id);
            writers.push(tokio::spawn(async move {
                node.post(&path, &token, json!({"text": format!("w{n}-{k}")}))
                    .await
                    .ok(200)
                    .json::<anyplay_api::Message>()
                    .seq
            }));
        }
    }
    let mut numbers = Vec::new();
    for writer in writers {
        numbers.push(writer.await.unwrap());
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=40).collect::<Vec<u64>>());
    let stored: anyplay_api::Messages = two
        .get(
            &format!("/v1/conversations/{}/messages?limit=100", group.id),
            hub.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(stored.messages.len(), 40);
}

#[tokio::test]
async fn a_party_on_two_nodes_forms_chats_and_moves_into_a_session() {
    let (one, two) = cluster_or_skip!();
    let lead = one.sign_up("Lead").await;
    let bea = one.sign_up("Bea").await;
    befriend(&one, &lead, &bea).await;
    let mut lead_events = one.events(lead.token()).await;
    let mut bea_events = two.events(bea.token()).await;

    one.post(
        "/v1/party/invitations",
        lead.token(),
        json!({"to": [bea.id]}),
    )
    .await
    .ok(200);
    let Event::PartyInvitation { invitation } = bea_events
        .until(|e| matches!(e, Event::PartyInvitation { .. }))
        .await
    else {
        unreachable!()
    };
    let joined: anyplay_api::Party = two
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", invitation.id),
            bea.token(),
        )
        .await
        .ok(200)
        .json();
    // The leader on the first node hears of it, with the revision the second node wrote.
    let Event::Party { party } = lead_events
        .until(|e| matches!(e, Event::Party { party } if party.members.len() == 2))
        .await
    else {
        unreachable!()
    };
    assert_eq!(party.revision, joined.revision);

    // The party chat crosses nodes.
    one.post(
        &format!("/v1/conversations/{}/messages", party.conversation),
        lead.token(),
        json!({"text": "party up"}),
    )
    .await
    .ok(200);
    bea_events
        .until(|e| matches!(e, Event::Message { seq: 1, .. }))
        .await;

    // The leader moves the party into a session; the invitation reaches the other node.
    let session: Session = one
        .post(
            "/v1/sessions",
            lead.token(),
            json!({"title_id": "GAME00001", "title": "Game", "version": "1.00", "build": "b1",
                   "max_members": 4, "visibility": "friends", "endpoint": "ab".repeat(32)}),
        )
        .await
        .ok(200)
        .json();
    one.post(
        "/v1/party/session",
        lead.token(),
        json!({"session": session.id}),
    )
    .await
    .ok(200);
    let Event::Invitation { invitation } = bea_events
        .until(|e| matches!(e, Event::Invitation { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(invitation.session, session.id);

    // Removing a member through the second node tells the member on it.
    two.post_empty("/v1/party/leave", bea.token()).await.ok(204);
    bea_events
        .until(|e| matches!(e, Event::PartyGone { .. }))
        .await;
    let Event::Party { party } = lead_events
        .until(|e| matches!(e, Event::Party { party } if party.members.len() == 1))
        .await
    else {
        unreachable!()
    };
    assert_eq!(party.leader.0, lead.id);
}
