mod common;

use std::time::Duration;

use anyplay_api::{
    AccountId, CallEnd, Event, Invited, MissedCall, MissedCalls, Party, PartyInvitation,
};
use common::{Events, Reply, TestCloud, User};
use serde_json::json;

async fn call(cloud: &TestCloud, from: &User, to: &[&User]) -> Reply {
    cloud
        .post(
            "/v1/party/invitations",
            from.token(),
            json!({"to": to.iter().map(|u| u.id).collect::<Vec<_>>(), "call": true}),
        )
        .await
}

async fn ring(events: &mut Events) -> PartyInvitation {
    match events
        .until(|e| matches!(e, Event::PartyInvitation { .. }))
        .await
    {
        Event::PartyInvitation { invitation } => invitation,
        _ => unreachable!(),
    }
}

async fn missed_event(events: &mut Events) -> MissedCall {
    match events
        .until(|e| matches!(e, Event::MissedCall { .. }))
        .await
    {
        Event::MissedCall { call } => call,
        _ => unreachable!(),
    }
}

/// Reads the ring and the missed call of a call nobody answers. A short ring time can run out
/// before the ring itself is read off the connection, so the two may arrive in either order.
async fn ring_and_miss(events: &mut Events) -> (PartyInvitation, MissedCall) {
    let (mut ringing, mut missed) = (None, None);
    while ringing.is_none() || missed.is_none() {
        match events
            .until(|e| matches!(e, Event::PartyInvitation { .. } | Event::MissedCall { .. }))
            .await
        {
            Event::PartyInvitation { invitation } => ringing = Some(invitation),
            Event::MissedCall { call } => missed = Some(call),
            _ => unreachable!(),
        }
    }
    (ringing.unwrap(), missed.unwrap())
}

async fn ended(events: &mut Events) -> (AccountId, CallEnd) {
    match events.until(|e| matches!(e, Event::CallEnded { .. })).await {
        Event::CallEnded { account, reason } => (account, reason),
        _ => unreachable!(),
    }
}

async fn missed(cloud: &TestCloud, user: &User) -> Vec<MissedCall> {
    cloud
        .get("/v1/calls/missed", user.token())
        .await
        .ok(200)
        .json::<MissedCalls>()
        .missed
}

async fn accept(cloud: &TestCloud, user: &User, invitation: &PartyInvitation) -> Reply {
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", invitation.id),
            user.token(),
        )
        .await
}

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

#[tokio::test]
async fn a_call_rings_for_a_friend_and_accepting_joins_the_party() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut eb = cloud.events(ben.token()).await;

    let sent: Invited = call(&cloud, &ann, &[&ben]).await.ok(200).json();
    assert_eq!(sent.sent, [AccountId(ben.id)]);
    let ringing = ring(&mut eb).await;
    assert!(ringing.call);
    assert_eq!(ringing.from.id, AccountId(ann.id));
    assert_eq!(ringing.members, 1);
    // A call rings 30 seconds, not 15 minutes.
    let left = ringing.expires.saturating_sub(anyplay_cloud::util::now());
    assert!((25..=31).contains(&left), "rings {left} s");

    let party: Party = accept(&cloud, &ben, &ringing).await.ok(200).json();
    assert_eq!(party.members.len(), 2);
    assert_eq!(party.leader, AccountId(ann.id));
    assert!(missed(&cloud, &ben).await.is_empty());
}

#[tokio::test]
async fn an_ordinary_invitation_is_no_call() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut eb = cloud.events(ben.token()).await;
    cloud
        .post(
            "/v1/party/invitations",
            ann.token(),
            json!({"to": [ben.id]}),
        )
        .await
        .ok(200);
    assert!(!ring(&mut eb).await.call);
}

#[tokio::test]
async fn a_call_that_rings_out_is_missed_and_the_caller_hears_so() {
    let cloud = TestCloud::with(|_| {}, |t| t.call_ttl = Duration::from_millis(600)).await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut ea = cloud.events(ann.token()).await;
    let mut eb = cloud.events(ben.token()).await;

    call(&cloud, &ann, &[&ben]).await.ok(200);
    let (ringing, miss) = ring_and_miss(&mut eb).await;
    assert_eq!(miss.from.id, AccountId(ann.id));
    assert_eq!(miss.from.handle, ann.handle);
    assert_eq!(ended(&mut ea).await, (AccountId(ben.id), CallEnd::NoAnswer));
    // The invitation is gone, and answering late finds nothing.
    accept(&cloud, &ben, &ringing)
        .await
        .is_error("not_found", 404);

    let list = missed(&cloud, &ben).await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].from.id, AccountId(ann.id));
    // A second call from the same friend replaces the first.
    call(&cloud, &ann, &[&ben]).await.ok(200);
    missed_event(&mut eb).await;
    assert_eq!(missed(&cloud, &ben).await.len(), 1);
    // Seeing it clears it, and the account's devices are told.
    let mut eb2 = cloud.events(ben.token()).await;
    cloud
        .delete(&format!("/v1/calls/missed/{}", ann.id), ben.token())
        .await
        .ok(204);
    assert!(matches!(
        eb2.until(|e| matches!(e, Event::MissedCallGone { .. })).await,
        Event::MissedCallGone { account } if account == AccountId(ann.id)
    ));
    assert!(missed(&cloud, &ben).await.is_empty());
}

#[tokio::test]
async fn a_declined_call_is_not_missed() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut ea = cloud.events(ann.token()).await;
    let mut eb = cloud.events(ben.token()).await;
    call(&cloud, &ann, &[&ben]).await.ok(200);
    let ringing = ring(&mut eb).await;
    cloud
        .delete(
            &format!("/v1/party/invitations/{}", ringing.id),
            ben.token(),
        )
        .await
        .ok(204);
    assert_eq!(ended(&mut ea).await, (AccountId(ben.id), CallEnd::Declined));
    assert!(missed(&cloud, &ben).await.is_empty());
}

#[tokio::test]
async fn hanging_up_before_the_answer_is_a_missed_call() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut eb = cloud.events(ben.token()).await;
    call(&cloud, &ann, &[&ben]).await.ok(200);
    ring(&mut eb).await;
    cloud
        .post_empty("/v1/party/leave", ann.token())
        .await
        .ok(204);
    missed_event(&mut eb).await;
    assert_eq!(missed(&cloud, &ben).await.len(), 1);
}

#[tokio::test]
async fn only_friends_who_are_free_and_not_blocked_are_rung() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let stranger = cloud.sign_up("Stranger").await;
    let busy = cloud.sign_up("Busy").await;
    befriend(&cloud, &ann, &busy).await;
    // Busy is in a party of its own already.
    cloud.post_empty("/v1/party", busy.token()).await.ok(200);

    let sent: Invited = call(&cloud, &ann, &[&stranger, &busy]).await.ok(200).json();
    assert!(sent.sent.is_empty(), "{:?}", sent.sent);

    // A block in either direction keeps the call out.
    cloud
        .put(&format!("/v1/blocks/{}", ann.id), ben.token(), None)
        .await
        .ok(204);
    let sent: Invited = call(&cloud, &ann, &[&ben]).await.ok(200).json();
    assert!(sent.sent.is_empty());
}

#[tokio::test]
async fn adding_to_a_call_rings_another_friend_and_only_the_leader_rings() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let cy = cloud.sign_up("Cyd").await;
    befriend(&cloud, &ann, &cy).await;
    befriend(&cloud, &ben, &cy).await;
    let mut eb = cloud.events(ben.token()).await;
    let mut ec = cloud.events(cy.token()).await;
    call(&cloud, &ann, &[&ben]).await.ok(200);
    let ringing = ring(&mut eb).await;
    accept(&cloud, &ben, &ringing).await.ok(200);
    call(&cloud, &ben, &[&cy]).await.is_error("forbidden", 403);
    call(&cloud, &ann, &[&cy]).await.ok(200);
    let ringing = ring(&mut ec).await;
    assert!(ringing.call);
    assert_eq!(ringing.members, 2);
}

#[tokio::test]
async fn calls_are_limited_per_minute() {
    let cloud = TestCloud::with(
        |_| {},
        |t| {
            t.rates.calls = 2;
        },
    )
    .await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    call(&cloud, &ann, &[&ben]).await.ok(200);
    call(&cloud, &ann, &[&ben]).await.ok(200);
    call(&cloud, &ann, &[&ben])
        .await
        .is_error("rate_limited", 429);
}

#[tokio::test]
async fn ending_the_friendship_clears_missed_calls() {
    let cloud = TestCloud::with(|_| {}, |t| t.call_ttl = Duration::from_millis(400)).await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut eb = cloud.events(ben.token()).await;
    call(&cloud, &ann, &[&ben]).await.ok(200);
    missed_event(&mut eb).await;
    assert_eq!(missed(&cloud, &ben).await.len(), 1);
    cloud
        .delete(&format!("/v1/friends/{}", ann.id), ben.token())
        .await
        .ok(204);
    assert!(missed(&cloud, &ben).await.is_empty());
}

#[tokio::test]
async fn the_leader_sets_a_discord_link_for_the_party() {
    let cloud = TestCloud::start().await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let mut eb = cloud.events(ben.token()).await;
    call(&cloud, &ann, &[&ben]).await.ok(200);
    let ringing = ring(&mut eb).await;
    accept(&cloud, &ben, &ringing).await.ok(200);

    let link = "https://discord.com/channels/123/456";
    let party: Party = cloud
        .patch("/v1/party", ann.token(), json!({"discord": link}))
        .await
        .ok(200)
        .json();
    assert_eq!(party.discord.as_deref(), Some(link));
    // Every member sees it, also after a read.
    let theirs: Party = cloud.get("/v1/party", ben.token()).await.ok(200).json();
    assert_eq!(theirs.discord.as_deref(), Some(link));

    // Only the leader sets it, and only a Discord address.
    cloud
        .patch("/v1/party", ben.token(), json!({"discord": link}))
        .await
        .is_error("forbidden", 403);
    for bad in [
        "http://discord.gg/abc",
        "https://discord.com.evil.example/channels/1/2",
        "https://example.org/https://discord.gg/abc",
        "https://discord.gg/a b",
        "javascript:alert(1)",
    ] {
        cloud
            .patch("/v1/party", ann.token(), json!({"discord": bad}))
            .await
            .is_error("bad_request", 400);
    }
    let party: Party = cloud.get("/v1/party", ann.token()).await.ok(200).json();
    assert_eq!(
        party.discord.as_deref(),
        Some(link),
        "refusals changed nothing"
    );

    // An invite link works, and the empty string removes the link.
    let party: Party = cloud
        .patch(
            "/v1/party",
            ann.token(),
            json!({"discord": "https://discord.gg/9zYqf8fEsh"}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(
        party.discord.as_deref(),
        Some("https://discord.gg/9zYqf8fEsh")
    );
    let party: Party = cloud
        .patch("/v1/party", ann.token(), json!({"discord": ""}))
        .await
        .ok(200)
        .json();
    assert_eq!(party.discord, None);
}
