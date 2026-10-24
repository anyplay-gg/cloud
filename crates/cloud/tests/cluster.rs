//! Several nodes of one cloud: one PostgreSQL database, one NATS server, players on different
//! nodes (`docs/ARCHITECTURE.md`, stage 2). These tests need both servers; without
//! `ANYPLAY_TEST_DATABASE_URL` and `ANYPLAY_TEST_NATS_URL` they pass without running.

mod common;

use std::sync::Arc;
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
    assert!(matches!(
        on_two.next().await,
        Event::Banned {
            until: None,
            statement: Some(_)
        }
    ));
    assert_eq!(on_two.close_code().await, Some(close::BANNED));
    let banned = wait_for(|| async { two.get("/v1/me", sam.token()).await.status == 403 }).await;
    assert!(banned, "the second node knows the ban");
    let mut again = Events::connect_raw(&two.base, sam.token()).await.unwrap();
    assert!(matches!(
        again.next_within(Duration::from_secs(2)).await,
        Some(Event::Banned {
            until: None,
            statement: Some(_)
        })
    ));
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

#[tokio::test]
async fn a_party_endpoint_announced_on_one_node_reaches_a_member_on_the_other() {
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
    two.post_empty(
        &format!("/v1/party/invitations/{}/accept", invitation.id),
        bea.token(),
    )
    .await
    .ok(200);
    bea_events
        .until(|e| matches!(e, Event::Party { party } if party.members.len() == 2))
        .await;

    // Bea announces through the second node; the leader on the first hears of it.
    let key = "ab".repeat(32);
    let answer: anyplay_api::Party = two
        .put(
            "/v1/party/endpoint",
            bea.token(),
            Some(json!({"endpoint": key, "relay": "https://r.example.com", "voice": true})),
        )
        .await
        .ok(200)
        .json();
    let Event::Party { party } = lead_events
        .until(|e| {
            matches!(e, Event::Party { party }
                if party.members.iter().any(|m| m.endpoint == "ab".repeat(32)))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(party.revision, answer.revision);
    let theirs = party
        .members
        .iter()
        .find(|m| m.account.id.0 == bea.id)
        .expect("Bea");
    assert_eq!(theirs.relay.as_deref(), Some("https://r.example.com"));
    assert!(theirs.voice);

    // The leader announces through the first node: Bea hears of it on the second, and a key
    // Bea has taken is refused whichever node asks.
    one.put(
        "/v1/party/endpoint",
        lead.token(),
        Some(json!({"endpoint": "cd".repeat(32), "voice": true})),
    )
    .await
    .ok(200);
    bea_events
        .until(|e| {
            matches!(e, Event::Party { party }
                if party.members.iter().any(|m| m.endpoint == "cd".repeat(32)))
        })
        .await;
    one.put(
        "/v1/party/endpoint",
        lead.token(),
        Some(json!({"endpoint": key, "voice": true})),
    )
    .await
    .is_error("limit", 409);

    // Leaving on the second node clears it for the leader on the first.
    two.post_empty("/v1/party/leave", bea.token()).await.ok(204);
    let Event::Party { party } = lead_events
        .until(|e| matches!(e, Event::Party { party } if party.members.len() == 1))
        .await
    else {
        unreachable!()
    };
    assert_eq!(party.members[0].endpoint, "cd".repeat(32));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn members_joining_one_session_through_both_nodes_at_once_get_different_addresses() {
    let (one, two) = cluster_or_skip!();
    let owner = one.sign_up("Alec").await;
    let session: Session = one
        .post(
            "/v1/sessions",
            owner.token(),
            json!({"title_id": "PPSA00001", "title": "Game", "version": "1.00", "build": "b1",
                   "max_members": 16, "visibility": "friends", "endpoint": "ab".repeat(32)}),
        )
        .await
        .ok(200)
        .json();
    let mut joiners = Vec::new();
    for n in 0..8 {
        let user = one.sign_up(&format!("Joiner{n}")).await;
        befriend(&one, &owner, &user).await;
        joiners.push(user);
    }
    let joins = joiners.iter().enumerate().map(|(n, user)| {
        // Every other joiner goes through the other node.
        let node = if n % 2 == 0 { &one } else { &two };
        let path = format!("/v1/sessions/{}/join", session.id);
        let body = json!({"version": "1.00", "build": "b1", "endpoint": format!("{:02x}", n + 2).repeat(32)});
        async move { node.post(&path, user.token(), body).await }
    });
    for reply in futures_util::future::join_all(joins).await {
        reply.ok(200);
    }
    let session: Session = two
        .get(&format!("/v1/sessions/{}", session.id), owner.token())
        .await
        .ok(200)
        .json();
    assert_eq!(session.members.len(), 9);
    let distinct: std::collections::HashSet<_> =
        session.members.iter().map(|m| m.address.clone()).collect();
    assert_eq!(distinct.len(), 9, "{:?}", session.members);
}

#[tokio::test]
async fn a_new_accounts_friend_requests_at_once_on_two_nodes_stay_within_its_allowance() {
    let Some(mut nodes) = TestCloud::cluster(2, |c| c.new_account_hours = 24).await else {
        eprintln!("no test database and NATS server; skipped");
        return;
    };
    let (two, one) = (
        Arc::new(nodes.pop().unwrap()),
        Arc::new(nodes.pop().unwrap()),
    );
    // A race that a lock per node lost in 2 of 5 runs: ten rounds in one run.
    for round in 0..10 {
        let nils = one.sign_up(&format!("Nils{round}")).await;
        let mut handles = Vec::new();
        for k in 0..20 {
            handles.push(one.sign_up(&format!("Pal{round}x{k}")).await.handle);
        }
        // Twenty requests at once, half through each node; a new account may send ten.
        let mut sent = Vec::new();
        for (k, handle) in handles.into_iter().enumerate() {
            let node = if k % 2 == 0 { one.clone() } else { two.clone() };
            let token = nils.token().to_owned();
            sent.push(tokio::spawn(async move {
                node.post("/v1/friends/requests", &token, json!({"handle": handle}))
                    .await
                    .status
            }));
        }
        let mut statuses = Vec::new();
        for request in sent {
            statuses.push(request.await.unwrap());
        }
        let taken = statuses.iter().filter(|s| **s == 200).count();
        let refused = statuses.iter().filter(|s| **s == 403).count();
        assert_eq!(
            (taken, refused),
            (10, 10),
            "round {round}, statuses {statuses:?}"
        );
        let outgoing = one.get("/v1/friends", nils.token()).await.ok(200).body["outgoing"]
            .as_array()
            .map(Vec::len);
        assert_eq!(outgoing, Some(10), "round {round}");
    }
}

/// Two players add each other at the same moment through different nodes: one request is stored
/// and the other turns it into a friendship, whichever comes first, and neither waits for the
/// other's lock for good.
#[tokio::test]
async fn two_players_adding_each_other_at_once_on_two_nodes_become_friends() {
    let (one, two) = cluster_or_skip!();
    let (one, two) = (Arc::new(one), Arc::new(two));
    for round in 0..10 {
        let a = one.sign_up(&format!("Ada{round}")).await;
        let b = one.sign_up(&format!("Ben{round}")).await;
        let (to_b, to_a) = tokio::join!(
            one.post(
                "/v1/friends/requests",
                a.token(),
                json!({"handle": b.handle})
            ),
            two.post(
                "/v1/friends/requests",
                b.token(),
                json!({"handle": a.handle})
            ),
        );
        let mut states = vec![
            to_b.ok(200).body["state"].as_str().map(str::to_owned),
            to_a.ok(200).body["state"].as_str().map(str::to_owned),
        ];
        states.sort();
        assert_eq!(
            states,
            [Some("friends".to_owned()), Some("requested".to_owned())],
            "round {round}"
        );
        for (node, user, other) in [(&one, &a, &b), (&two, &b, &a)] {
            let lists = node.get("/v1/friends", user.token()).await.ok(200).body;
            assert_eq!(
                lists["friends"][0]["account"]["handle"],
                json!(other.handle),
                "round {round}"
            );
            assert_eq!(lists["incoming"], json!([]), "round {round}");
            assert_eq!(lists["outgoing"], json!([]), "round {round}");
        }
    }
}
