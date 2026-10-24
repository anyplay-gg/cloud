mod common;

use std::time::Duration;

use anyplay_api::{AccountId, Event, Invitation, Invited, Session, Visibility};
use base64::Engine as _;
use common::{Events, TestCloud, User};
use serde_json::{Value, json};

const QUIET: Duration = Duration::from_millis(250);

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn create_body(owner_endpoint: u8, max: u16, visibility: &str) -> Value {
    json!({
        "title_id": "GAME00001", "title": "Test Game", "version": "01.000", "build": "0.1.3",
        "max_members": max, "visibility": visibility, "properties": "",
        "endpoint": endpoint(owner_endpoint), "relay": "https://relay-a.example",
    })
}

fn join_body(n: u8) -> Value {
    json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(n), "relay": null, "invitation": null})
}

async fn create(cloud: &TestCloud, user: &User, n: u8, max: u16, visibility: &str) -> Session {
    cloud
        .post(
            "/v1/sessions",
            user.token(),
            create_body(n, max, visibility),
        )
        .await
        .ok(200)
        .json()
}

async fn join(cloud: &TestCloud, user: &User, id: &str, n: u8) -> Session {
    cloud
        .post(
            &format!("/v1/sessions/{id}/join"),
            user.token(),
            join_body(n),
        )
        .await
        .ok(200)
        .json()
}

fn is_session(event: &Event) -> bool {
    matches!(event, Event::Session { .. })
}

async fn next_session(events: &mut Events) -> Session {
    match events.until(is_session).await {
        Event::Session { session } => session,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_session_is_created_joined_and_left_with_events_for_every_member() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let mut eb = cloud.events(b.token()).await;
    // Sam connected after Alec, so Alec has one presence event in his queue.
    let _ = ea.until(|e| matches!(e, Event::Presence { .. })).await;

    let created = create(&cloud, &a, 1, 4, "friends").await;
    assert_eq!(created.leader, AccountId(a.id));
    assert_eq!(created.members.len(), 1);
    assert_eq!(created.members[0].endpoint, endpoint(1));
    assert_eq!(
        created.members[0].relay.as_deref(),
        Some("https://relay-a.example")
    );
    assert_eq!(created.members[0].account.handle, "Alec");
    let id = created.id.clone();
    let parts: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(parts, [8, 4, 4, 4, 12]);

    // Sam joins as a friend.
    let joined = join(&cloud, &b, &id, 2).await;
    assert_eq!(joined.members.len(), 2);
    let ha: std::net::Ipv4Addr = joined.members[0].address.parse().unwrap();
    let hb: std::net::Ipv4Addr = joined.members[1].address.parse().unwrap();
    assert!(anyplay_api::is_virtual(ha) && anyplay_api::is_virtual(hb) && ha != hb);
    assert_eq!(next_session(&mut ea).await.members.len(), 2);
    assert_eq!(next_session(&mut eb).await.members.len(), 2);

    // Both see the same session, with the same addresses.
    let seen_by_a: Session = cloud
        .get(&format!("/v1/sessions/{id}"), a.token())
        .await
        .ok(200)
        .json();
    assert_eq!(seen_by_a, joined);

    // The leader changes the properties.
    let props = base64::engine::general_purpose::STANDARD.encode(b"hello game");
    let updated: Session = cloud
        .patch(
            &format!("/v1/sessions/{id}"),
            a.token(),
            json!({"properties": props}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(updated.properties, props);
    assert_eq!(next_session(&mut eb).await.properties, props);
    assert_eq!(next_session(&mut ea).await.properties, props);
    cloud
        .patch(
            &format!("/v1/sessions/{id}"),
            b.token(),
            json!({"properties": ""}),
        )
        .await
        .is_error("forbidden", 403);

    // Joining again only moves the endpoint and relay.
    let again: Session = cloud
        .post(
            &format!("/v1/sessions/{id}/join"),
            b.token(),
            json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(9), "relay": "https://relay-b.example"}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(again.members.len(), 2);
    let sam = again
        .members
        .iter()
        .find(|m| m.account.id.0 == b.id)
        .unwrap();
    assert_eq!(sam.endpoint, endpoint(9));
    assert_eq!(sam.relay.as_deref(), Some("https://relay-b.example"));
    assert_eq!(sam.joined, joined.members[1].joined);
    assert_eq!(next_session(&mut ea).await.members.len(), 2);
    // ...and a wrong version guard still applies to it.
    cloud
        .post(
            &format!("/v1/sessions/{id}/join"),
            b.token(),
            json!({"version": "02.000", "build": "0.1.3", "endpoint": endpoint(9)}),
        )
        .await
        .is_error("version_mismatch", 409);

    // Sam leaves; Alec is alone again.
    cloud
        .post_empty(&format!("/v1/sessions/{id}/leave"), b.token())
        .await
        .ok(204);
    assert_eq!(
        eb.until(|e| matches!(e, Event::SessionGone { .. })).await,
        Event::SessionGone { id: id.clone() }
    );
    assert_eq!(next_session(&mut ea).await.members.len(), 1);
    cloud
        .post_empty(&format!("/v1/sessions/{id}/leave"), b.token())
        .await
        .is_error("not_found", 404);

    // The last member closes it.
    cloud
        .post_empty(&format!("/v1/sessions/{id}/leave"), a.token())
        .await
        .ok(204);
    cloud
        .get(&format!("/v1/sessions/{id}"), a.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn the_longest_standing_member_leads_after_the_leader_leaves() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Sam").await;
    let c = cloud.sign_up("Kim").await;
    for (x, y) in [(&a, &b), (&a, &c)] {
        cloud
            .post(
                "/v1/friends/requests",
                x.token(),
                json!({"handle": y.handle}),
            )
            .await
            .ok(200);
        cloud
            .post_empty(&format!("/v1/friends/requests/{}/accept", x.id), y.token())
            .await
            .ok(204);
    }
    let _ea = cloud.events(a.token()).await;
    let _ec = cloud.events(c.token()).await;
    let session = create(&cloud, &a, 1, 4, "friends").await;
    join(&cloud, &c, &session.id, 3).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    join(&cloud, &b, &session.id, 2).await;
    let mut eb = cloud.events(b.token()).await;

    cloud
        .post_empty(&format!("/v1/sessions/{}/leave", session.id), a.token())
        .await
        .ok(204);
    let now = next_session(&mut eb).await;
    assert_eq!(now.leader, AccountId(c.id), "Kim joined before Sam");
    assert_eq!(now.members.len(), 2);

    // The new leader hands over by name, and the old one cannot.
    let handed: Session = cloud
        .patch(
            &format!("/v1/sessions/{}", session.id),
            c.token(),
            json!({"leader": b.id}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(handed.leader, AccountId(b.id));
    cloud
        .patch(
            &format!("/v1/sessions/{}", session.id),
            c.token(),
            json!({"visibility": "closed"}),
        )
        .await
        .is_error("forbidden", 403);
    cloud
        .patch(
            &format!("/v1/sessions/{}", session.id),
            b.token(),
            json!({"leader": a.id}),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn the_leader_can_remove_a_member() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let session = create(&cloud, &a, 1, 4, "friends").await;
    join(&cloud, &b, &session.id, 2).await;
    let mut eb = cloud.events(b.token()).await;

    cloud
        .delete(
            &format!("/v1/sessions/{}/members/{}", session.id, a.id),
            a.token(),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .delete(
            &format!("/v1/sessions/{}/members/{}", session.id, a.id),
            b.token(),
        )
        .await
        .is_error("forbidden", 403);
    cloud
        .delete(
            &format!("/v1/sessions/{}/members/{}", session.id, b.id),
            a.token(),
        )
        .await
        .ok(204);
    assert_eq!(
        eb.until(|e| matches!(e, Event::SessionGone { .. })).await,
        Event::SessionGone {
            id: session.id.clone()
        }
    );
    cloud
        .delete(
            &format!("/v1/sessions/{}/members/{}", session.id, b.id),
            a.token(),
        )
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn joining_checks_blocks_then_visibility_then_version_then_capacity() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let friend = cloud.sign_up("Fran").await;
    let stranger = cloud.sign_up("Sue").await;
    let late = cloud.sign_up("Late").await;
    for other in [&friend, &late] {
        cloud
            .post(
                "/v1/friends/requests",
                a.token(),
                json!({"handle": other.handle}),
            )
            .await
            .ok(200);
        cloud
            .post_empty(
                &format!("/v1/friends/requests/{}/accept", a.id),
                other.token(),
            )
            .await
            .ok(204);
    }
    let session = create(&cloud, &a, 1, 2, "friends").await;
    let path = format!("/v1/sessions/{}/join", session.id);

    // A stranger cannot see a friends-only session.
    cloud
        .post(&path, stranger.token(), join_body(5))
        .await
        .is_error("not_found", 404);
    cloud
        .get(&format!("/v1/sessions/{}", session.id), stranger.token())
        .await
        .is_error("not_found", 404);
    // A friend with the wrong version learns what is expected.
    let reply = cloud
        .post(
            &path,
            friend.token(),
            json!({"version": "02.000", "build": "0.1.3", "endpoint": endpoint(2)}),
        )
        .await;
    reply.is_error("version_mismatch", 409);
    assert_eq!(
        reply.body["expected"],
        json!({"version": "01.000", "build": "0.1.3"})
    );
    let reply = cloud
        .post(
            &path,
            friend.token(),
            json!({"version": "01.000", "build": "9.9.9", "endpoint": endpoint(2)}),
        )
        .await;
    reply.is_error("version_mismatch", 409);
    // Bad input.
    cloud
        .post(
            &path,
            friend.token(),
            json!({"version": "01.000", "build": "0.1.3", "endpoint": "ABC"}),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .post(&path, friend.token(), json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(2), "relay": "ftp://x"}))
        .await
        .is_error("bad_request", 400);
    // The right version fills the session...
    join(&cloud, &friend, &session.id, 2).await;
    // ...and the next friend finds it full.
    cloud
        .post(&path, late.token(), join_body(3))
        .await
        .is_error("full", 409);

    // A block between the joiner and any member beats everything else, even a full session.
    cloud
        .put(&format!("/v1/blocks/{}", late.id), friend.token(), None)
        .await
        .ok(204);
    cloud
        .post(&path, late.token(), join_body(3))
        .await
        .is_error("forbidden", 403);
    // It also works the other way round.
    cloud
        .delete(&format!("/v1/blocks/{}", late.id), friend.token())
        .await
        .ok(204);
    cloud
        .put(&format!("/v1/blocks/{}", friend.id), late.token(), None)
        .await
        .ok(204);
    cloud
        .post(&path, late.token(), join_body(3))
        .await
        .is_error("forbidden", 403);
}

#[tokio::test]
async fn invite_only_and_closed_sessions() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let invite_only = create(&cloud, &a, 1, 4, "invite").await;
    let path = format!("/v1/sessions/{}/join", invite_only.id);
    // Friendship is not enough.
    cloud
        .post(&path, b.token(), join_body(2))
        .await
        .is_error("not_found", 404);

    let mut eb = cloud.events(b.token()).await;
    let invited: Invited = cloud
        .post(
            &format!("/v1/sessions/{}/invitations", invite_only.id),
            a.token(),
            json!({"to": [b.id, 424242]}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(invited.sent, vec![AccountId(b.id)], "only friends receive");
    let Event::Invitation { invitation } =
        eb.until(|e| matches!(e, Event::Invitation { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(invitation.from.handle, "Alec");
    assert_eq!(invitation.session, invite_only.id);
    assert_eq!(invitation.title, "Test Game");
    // Asking again does not send a second one.
    cloud
        .post(
            &format!("/v1/sessions/{}/invitations", invite_only.id),
            a.token(),
            json!({"to": [b.id]}),
        )
        .await
        .ok(200);
    eb.quiet(QUIET).await;

    // The invitation lets Sam see and join the session, and is used up.
    cloud
        .get(&format!("/v1/sessions/{}", invite_only.id), b.token())
        .await
        .ok(200);
    let joined: Session = cloud
        .post(
            &path,
            b.token(),
            json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(2), "invitation": invitation.id}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(joined.members.len(), 2);
    assert_eq!(
        eb.until(|e| matches!(e, Event::InvitationGone { .. }))
            .await,
        Event::InvitationGone {
            id: invitation.id.clone()
        }
    );

    // Closed sessions take nobody, and send no invitations.
    let closed = create(&cloud, &a, 3, 4, "closed").await;
    cloud
        .post(
            &format!("/v1/sessions/{}/join", closed.id),
            b.token(),
            join_body(2),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            &format!("/v1/sessions/{}/invitations", closed.id),
            a.token(),
            json!({"to": [b.id]}),
        )
        .await
        .is_error("forbidden", 403);
    // Only members invite.
    cloud
        .post(
            &format!("/v1/sessions/{}/invitations", closed.id),
            b.token(),
            json!({"to": [a.id]}),
        )
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn invitations_can_be_declined_expire_and_end_with_the_session_or_a_block() {
    let cloud = TestCloud::with(|_| {}, |t| t.invitation_ttl = Duration::from_millis(700)).await;
    let a = cloud.sign_up("Alec").await;
    let others = [
        cloud.sign_up("Sam").await,
        cloud.sign_up("Kim").await,
        cloud.sign_up("Lou").await,
        cloud.sign_up("Max").await,
    ];
    for other in &others {
        cloud
            .post(
                "/v1/friends/requests",
                a.token(),
                json!({"handle": other.handle}),
            )
            .await
            .ok(200);
        cloud
            .post_empty(
                &format!("/v1/friends/requests/{}/accept", a.id),
                other.token(),
            )
            .await
            .ok(204);
    }
    let [sam, kim, lou, max] = &others;
    let mut es = cloud.events(sam.token()).await;
    let mut ek = cloud.events(kim.token()).await;
    let mut el = cloud.events(lou.token()).await;
    let mut em = cloud.events(max.token()).await;

    let session = create(&cloud, &a, 1, 8, "friends").await;
    let invite = |ids: Vec<u64>| {
        let cloud = &cloud;
        let a = &a;
        let path = format!("/v1/sessions/{}/invitations", session.id);
        async move {
            cloud
                .post(&path, a.token(), json!({"to": ids}))
                .await
                .ok(200)
        }
    };
    invite(vec![sam.id, kim.id, lou.id, max.id]).await;
    let mut ids = Vec::new();
    for events in [&mut es, &mut ek, &mut el, &mut em] {
        let Event::Invitation { invitation } = events
            .until(|e| matches!(e, Event::Invitation { .. }))
            .await
        else {
            unreachable!()
        };
        ids.push(invitation);
    }
    let [for_sam, for_kim, for_lou, for_max]: [Invitation; 4] = ids.try_into().unwrap();

    // Sam declines; nobody else may.
    cloud
        .delete(&format!("/v1/invitations/{}", for_sam.id), kim.token())
        .await
        .is_error("not_found", 404);
    cloud
        .delete(&format!("/v1/invitations/{}", for_sam.id), sam.token())
        .await
        .ok(204);
    assert_eq!(
        es.until(|e| matches!(e, Event::InvitationGone { .. }))
            .await,
        Event::InvitationGone {
            id: for_sam.id.clone()
        }
    );

    // Alec blocks Kim: her invitation goes.
    cloud
        .put(&format!("/v1/blocks/{}", kim.id), a.token(), None)
        .await
        .ok(204);
    assert_eq!(
        ek.until(|e| matches!(e, Event::InvitationGone { .. }))
            .await,
        Event::InvitationGone {
            id: for_kim.id.clone()
        }
    );

    // Lou's runs out of time.
    assert_eq!(
        el.until(|e| matches!(e, Event::InvitationGone { .. }))
            .await,
        Event::InvitationGone {
            id: for_lou.id.clone()
        }
    );

    // Max's would have, but the session closes first... use a new session for that.
    let other = create(&cloud, &a, 2, 4, "friends").await;
    let fresh = cloud
        .post(
            &format!("/v1/sessions/{}/invitations", other.id),
            a.token(),
            json!({"to": [max.id]}),
        )
        .await
        .ok(200);
    let _: Invited = fresh.json();
    let Event::Invitation { invitation: fresh } =
        em.until(|e| matches!(e, Event::Invitation { .. })).await
    else {
        unreachable!()
    };
    cloud
        .post_empty(&format!("/v1/sessions/{}/leave", other.id), a.token())
        .await
        .ok(204);
    assert_eq!(
        em.until(|e| matches!(e, Event::InvitationGone { id } if *id == fresh.id))
            .await,
        Event::InvitationGone { id: fresh.id }
    );
    let _ = for_max;
}

#[tokio::test]
async fn limits_on_sessions_and_input_are_enforced() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    for n in 0..4 {
        create(&cloud, &a, n + 1, 2, "friends").await;
    }
    cloud
        .post("/v1/sessions", a.token(), create_body(9, 2, "friends"))
        .await
        .is_error("limit", 409);

    let b = cloud.sign_up("Sam").await;
    for (field, value) in [
        ("max_members", json!(1)),
        ("max_members", json!(65)),
        ("endpoint", json!("nope")),
        ("title_id", json!("")),
    ] {
        let mut body = create_body(1, 2, "friends");
        body[field] = value;
        cloud
            .post("/v1/sessions", b.token(), body)
            .await
            .is_error("bad_request", 400);
    }
    let mut body = create_body(1, 2, "friends");
    body["relay"] = json!("not a url");
    cloud
        .post("/v1/sessions", b.token(), body)
        .await
        .is_error("bad_request", 400);

    // Properties: up to 8 KiB decoded.
    let engine = base64::engine::general_purpose::STANDARD;
    let mut body = create_body(1, 2, "friends");
    body["properties"] = json!(engine.encode(vec![7u8; 8192]));
    cloud.post("/v1/sessions", b.token(), body).await.ok(200);
    let mut body = create_body(1, 2, "friends");
    body["properties"] = json!(engine.encode(vec![7u8; 8193]));
    cloud
        .post("/v1/sessions", b.token(), body)
        .await
        .is_error("too_large", 413);
    let mut body = create_body(1, 2, "friends");
    body["properties"] = json!("not base64!");
    cloud
        .post("/v1/sessions", b.token(), body)
        .await
        .is_error("bad_request", 400);
    let mut body = create_body(1, 2, "friends");
    body["properties"] = json!("A".repeat(20_000));
    cloud
        .post("/v1/sessions", b.token(), body)
        .await
        .is_error("too_large", 413);
}

#[tokio::test]
async fn a_member_that_stays_disconnected_leaves_its_sessions() {
    let cloud = TestCloud::with(|_| {}, |t| t.session_grace = Duration::from_millis(900)).await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let session = create(&cloud, &a, 1, 4, "friends").await;
    let mut ea = cloud.events(a.token()).await;
    let eb = cloud.events(b.token()).await;
    join(&cloud, &b, &session.id, 2).await;
    assert_eq!(next_session(&mut ea).await.members.len(), 2);

    // Sam drops and comes back inside the grace period: still a member.
    drop(eb);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let eb = cloud.events(b.token()).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let seen: Session = cloud
        .get(&format!("/v1/sessions/{}", session.id), a.token())
        .await
        .ok(200)
        .json();
    assert_eq!(seen.members.len(), 2);

    // Dropping for good removes him after the grace period (900 ms in tests).
    drop(eb);
    let after = next_session(&mut ea).await;
    assert_eq!(after.members.len(), 1);
    assert_eq!(after.members[0].account.id.0, a.id);
}

#[tokio::test]
async fn an_account_that_never_connects_does_not_stay_in_a_session() {
    let cloud = TestCloud::with(|_| {}, |t| t.session_grace = Duration::from_millis(900)).await;
    let a = cloud.sign_up("Alec").await;
    let session = create(&cloud, &a, 1, 4, "friends").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    cloud
        .get(&format!("/v1/sessions/{}", session.id), a.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn friends_see_a_joinable_session_in_presence() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut eb = cloud.events(b.token()).await;
    let _ea = cloud.events(a.token()).await;
    let _ = eb
        .until(|e| matches!(e, Event::Presence { presence, .. } if presence.online))
        .await;

    cloud
        .put("/v1/presence", a.token(), Some(json!({"activity": {"title_id": "GAME00001", "title": "Test Game", "version": "01.000"}})))
        .await
        .ok(204);
    let Event::Presence { presence, .. } = eb.until(|e| matches!(e, Event::Presence { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(presence.activity.unwrap().session, None);

    let session = create(&cloud, &a, 1, 2, "friends").await;
    let Event::Presence { presence, .. } = eb.until(|e| matches!(e, Event::Presence { .. })).await
    else {
        unreachable!()
    };
    let reference = presence
        .activity
        .unwrap()
        .session
        .expect("a session reference");
    assert_eq!(reference.id, session.id);
    assert!(reference.joinable);
    assert_eq!((reference.members, reference.max_members), (1, 2));

    // Filling it makes it not joinable; closing it to friends removes it.
    join(&cloud, &b, &session.id, 2).await;
    let Event::Presence { presence, .. } = eb.until(|e| matches!(e, Event::Presence { .. })).await
    else {
        unreachable!()
    };
    assert!(!presence.activity.unwrap().session.unwrap().joinable);
    cloud
        .patch(
            &format!("/v1/sessions/{}", session.id),
            a.token(),
            json!({"visibility": "invite"}),
        )
        .await
        .ok(200);
    let Event::Presence { presence, .. } = eb.until(|e| matches!(e, Event::Presence { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(presence.activity.unwrap().session, None);
    let _ = Visibility::Invite;
}

/// A game sets its activity and opens its session at nearly the same time, as a launcher and a
/// game do. Whatever the order, the last presence a friend is told is the one the cloud holds:
/// with the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_opened_while_the_activity_changes_reaches_friends() {
    const ROUNDS: usize = 20;
    let cloud = TestCloud::start().await;
    let mut wrong = Vec::new();
    for round in 0..ROUNDS {
        let (a, b) = cloud
            .friends(&format!("Alec{round}"), &format!("Sam{round}"))
            .await;
        let mut eb = cloud.events(b.token()).await;
        let _ea = cloud.events(a.token()).await;
        let _ = eb
            .until(|e| matches!(e, Event::Presence { presence, .. } if presence.online))
            .await;
        let activity = json!({"activity": {"title_id": "GAME00001", "title": "Test Game", "version": "01.000"}});
        let (presence, session) = tokio::join!(
            cloud.put("/v1/presence", a.token(), Some(activity)),
            cloud.post("/v1/sessions", a.token(), create_body(1, 4, "friends")),
        );
        presence.ok(204);
        let session: Session = session.ok(200).json();
        // Both requests have told the friends by the time they answer.
        let mut last = None;
        while let Some(event) = eb.next_within(Duration::from_millis(200)).await {
            if let Event::Presence { presence, .. } = event {
                if last
                    .as_ref()
                    .is_none_or(|l: &anyplay_api::Presence| presence.revision > l.revision)
                {
                    last = Some(presence);
                }
            }
        }
        let told = last
            .and_then(|p| p.activity)
            .and_then(|a| a.session)
            .map(|s| s.id);
        if told.as_deref() != Some(session.id.as_str()) {
            wrong.push((round, told));
        }
    }
    assert!(
        wrong.is_empty(),
        "rounds where the friend was told a stale presence: {wrong:?}"
    );
}

/// A player sets an activity while their first connection opens. Whatever the order, the last
/// presence a friend is told is online: never the offline one read before the connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_activity_set_while_the_first_connection_opens_reaches_friends_online() {
    const ROUNDS: usize = 20;
    let cloud = TestCloud::start().await;
    let mut wrong = Vec::new();
    for round in 0..ROUNDS {
        let (a, b) = cloud
            .friends(&format!("Ada{round}"), &format!("Ben{round}"))
            .await;
        let mut eb = cloud.events(b.token()).await;
        let activity = json!({"activity": {"title_id": "GAME00001", "title": "Test Game", "version": "01.000"}});
        let (presence, _ea) = tokio::join!(
            cloud.put("/v1/presence", a.token(), Some(activity)),
            cloud.events(a.token()),
        );
        presence.ok(204);
        let mut last = None;
        while let Some(event) = eb.next_within(Duration::from_millis(200)).await {
            if let Event::Presence { account, presence } = event {
                if account.0 == a.id
                    && last
                        .as_ref()
                        .is_none_or(|l: &anyplay_api::Presence| presence.revision > l.revision)
                {
                    last = Some(presence);
                }
            }
        }
        if !last.as_ref().is_some_and(|p| p.online) {
            wrong.push((round, last.map(|p| (p.online, p.revision))));
        }
    }
    assert!(
        wrong.is_empty(),
        "rounds where the friend was last told offline: {wrong:?}"
    );
}

#[tokio::test]
async fn every_change_to_a_session_raises_its_revision() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let created = create(&cloud, &a, 1, 4, "friends").await;
    assert_eq!(created.revision, 0);
    let joined = join(&cloud, &b, &created.id, 2).await;
    assert!(joined.revision > created.revision);
    let changed: Session = cloud
        .patch(
            &format!("/v1/sessions/{}", created.id),
            a.token(),
            json!({"properties": "AQID"}),
        )
        .await
        .ok(200)
        .json();
    assert!(changed.revision > joined.revision);
    let read: Session = cloud
        .get(&format!("/v1/sessions/{}", created.id), a.token())
        .await
        .ok(200)
        .json();
    assert_eq!(read.revision, changed.revision, "reading changes nothing");
}

/// A join checks blocks on its own transaction. Asking the pool for a second connection while it
/// held one stalled every join until the pool's timeout once as many joins ran at once as the pool
/// had connections (the load harness found it), so here more joins than connections run together.
#[tokio::test]
async fn joins_at_once_never_wait_on_each_other_for_a_connection() {
    let cloud = TestCloud::with(|c| c.database_connections = 2, |_| {}).await;
    let host = cloud.sign_up("Host").await;
    let mut joiners = Vec::new();
    for n in 0..8 {
        let joiner = cloud.sign_up(&format!("Joiner{n}")).await;
        cloud
            .post(
                "/v1/friends/requests",
                host.token(),
                json!({"handle": joiner.handle}),
            )
            .await
            .ok(200);
        cloud
            .post_empty(
                &format!("/v1/friends/requests/{}/accept", host.id),
                joiner.token(),
            )
            .await
            .ok(204);
        joiners.push(joiner);
    }
    let session = create(&cloud, &host, 1, 16, "friends").await;
    let joins = joiners
        .iter()
        .zip(2u8..)
        .map(|(joiner, n)| join(&cloud, joiner, &session.id, n));
    let joined = tokio::time::timeout(
        Duration::from_secs(10),
        futures_util::future::join_all(joins),
    )
    .await
    .expect("the joins finish without waiting for the pool");
    let last = joined.iter().max_by_key(|s| s.members.len()).unwrap();
    assert_eq!(last.members.len(), 9);
}

// Virtual addresses belong to the members of a session (network ADR 0020).

async fn befriend(cloud: &TestCloud, a: &User, b: &User) {
    cloud
        .post(
            "/v1/friends/requests",
            a.token(),
            json!({"handle": b.handle}),
        )
        .await
        .ok(200);
    cloud
        .post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
        .await
        .ok(204);
}

fn address_of(session: &Session, account: u64) -> std::net::Ipv4Addr {
    session
        .members
        .iter()
        .find(|m| m.account.id.0 == account)
        .expect("a member")
        .address
        .parse()
        .unwrap()
}

#[tokio::test]
async fn members_get_their_preferred_address_and_no_two_share_one() {
    let cloud = TestCloud::start().await;
    let owner = cloud.sign_up("Alec").await;
    let mut session = create(&cloud, &owner, 1, 16, "friends").await;
    let mut friends = Vec::new();
    for n in 0..8u8 {
        let user = cloud.sign_up(&format!("Friend{n}")).await;
        friends.push(user);
    }
    for user in &friends {
        befriend(&cloud, &owner, user).await;
        session = join(&cloud, user, &session.id, 2).await;
    }
    assert_eq!(session.members.len(), 9);
    let mut seen = std::collections::HashSet::new();
    for member in &session.members {
        let address: std::net::Ipv4Addr = member.address.parse().unwrap();
        assert!(anyplay_api::is_assignable(address), "{address}");
        assert!(seen.insert(address), "{address} twice");
        // No one here collides with another's preferred address, so each gets its own.
        assert_eq!(
            address,
            anyplay_cloud::addresses::preferred(member.account.id.0)
        );
    }
}

#[tokio::test]
async fn an_address_held_by_a_member_moves_a_newcomer_to_the_next_one() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let session = create(&cloud, &a, 1, 4, "friends").await;
    // Another node, from before session addresses, gave Alec the address Sam would get.
    let wanted = anyplay_cloud::addresses::preferred(b.id);
    sqlx::query("UPDATE members SET address = $1 WHERE session_id = $2")
        .bind(wanted.to_string())
        .bind(&session.id)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let joined = join(&cloud, &b, &session.id, 2).await;
    assert_eq!(address_of(&joined, a.id), wanted);
    let moved = address_of(&joined, b.id);
    assert_ne!(moved, wanted);
    assert!(anyplay_api::is_assignable(moved));
    // The address stays when Sam joins again, and with it when Sam only moves his endpoint.
    let again = join(&cloud, &b, &session.id, 3).await;
    assert_eq!(address_of(&again, b.id), moved);
}

#[tokio::test]
async fn one_player_has_the_same_address_in_two_sessions_and_a_session_never_needs_a_lease() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let first = create(&cloud, &a, 1, 4, "friends").await;
    let second = create(&cloud, &b, 2, 4, "friends").await;
    let first = join(&cloud, &b, &first.id, 3).await;
    let second = join(&cloud, &a, &second.id, 4).await;
    assert_eq!(address_of(&first, a.id), address_of(&second, a.id));
    assert_eq!(address_of(&first, b.id), address_of(&second, b.id));
    // Nothing is leased any more: being online or in a session writes no address row.
    let _events = cloud.events(a.token()).await;
    let (leases,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM leases")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(leases, 0);
}

#[tokio::test]
async fn a_member_who_leaves_and_joins_again_gets_its_address_back_while_nobody_took_it() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let session = create(&cloud, &a, 1, 4, "friends").await;
    let with_sam = join(&cloud, &b, &session.id, 2).await;
    let sam = address_of(&with_sam, b.id);
    cloud
        .post_empty(&format!("/v1/sessions/{}/leave", session.id), b.token())
        .await
        .ok(204);
    let back = join(&cloud, &b, &session.id, 2).await;
    assert_eq!(address_of(&back, b.id), sam);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn members_joining_at_the_same_moment_all_get_different_addresses() {
    let cloud = TestCloud::start().await;
    let owner = cloud.sign_up("Alec").await;
    let session = create(&cloud, &owner, 1, 16, "friends").await;
    let mut users = Vec::new();
    for n in 0..10u8 {
        let user = cloud.sign_up(&format!("Joiner{n}")).await;
        befriend(&cloud, &owner, &user).await;
        users.push(user);
    }
    let joins = users.iter().enumerate().map(|(n, user)| {
        let cloud = &cloud;
        let id = session.id.clone();
        async move {
            cloud
                .post(
                    &format!("/v1/sessions/{id}/join"),
                    user.token(),
                    join_body(10 + n as u8),
                )
                .await
        }
    });
    for reply in futures_util::future::join_all(joins).await {
        reply.ok(200);
    }
    let session: Session = cloud
        .get(&format!("/v1/sessions/{}", session.id), owner.token())
        .await
        .ok(200)
        .json();
    assert_eq!(session.members.len(), 11);
    let distinct: std::collections::HashSet<_> =
        session.members.iter().map(|m| m.address.clone()).collect();
    assert_eq!(distinct.len(), 11);
}
