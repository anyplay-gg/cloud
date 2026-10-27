mod common;

use std::time::Duration;

use anyplay_api::{
    AccountId, Event, Info, Share, ShareGoneReason, ShareInvitations, ShareMode, SharePath,
    ShareState,
};
use common::{Events, Reply, TestCloud, User};
use serde_json::{Value, json};

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
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

/// `lead` invites `friend`, who accepts, so that both are in one party. Returns the friend's
/// event connection, welcome read.
async fn into_party(cloud: &TestCloud, lead: &User, friend: &User) -> Events {
    befriend(cloud, lead, friend).await;
    let mut events = cloud.events(friend.token()).await;
    cloud
        .post(
            "/v1/party/invitations",
            lead.token(),
            json!({"to": [friend.id]}),
        )
        .await
        .ok(200);
    let invitation = match events
        .until(|e| matches!(e, Event::PartyInvitation { .. }))
        .await
    {
        Event::PartyInvitation { invitation } => invitation,
        _ => unreachable!(),
    };
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", invitation.id),
            friend.token(),
        )
        .await
        .ok(200);
    events
}

async fn play(cloud: &TestCloud, user: &User) {
    cloud
        .put(
            "/v1/presence",
            user.token(),
            Some(json!({"activity": {"title_id": "CUSA00001", "title": "A game", "version": "1.00"}})),
        )
        .await
        .ok(204);
}

async fn stop_playing(cloud: &TestCloud, user: &User) {
    cloud
        .put("/v1/presence", user.token(), Some(json!({"activity": null})))
        .await
        .ok(204);
}

async fn invite(cloud: &TestCloud, host: &User, guest: &User) -> Reply {
    cloud
        .post(
            "/v1/shares",
            host.token(),
            json!({"to": guest.id, "mode": "watch", "endpoint": endpoint(1)}),
        )
        .await
}

async fn accept(cloud: &TestCloud, guest: &User) -> Reply {
    cloud
        .post(
            "/v1/share/accept",
            guest.token(),
            json!({"endpoint": endpoint(2), "relay": "https://relay.example.com"}),
        )
        .await
}

/// A host who plays and a guest in the same party, both with an event connection, on a cloud
/// with share play on.
struct Pair {
    cloud: TestCloud,
    host: User,
    guest: User,
    eh: Events,
    eg: Events,
}

async fn pair_with(
    config: impl FnOnce(&mut anyplay_cloud::Config),
    tuning: impl FnOnce(&mut anyplay_cloud::Tuning),
) -> Pair {
    let cloud = TestCloud::with(
        |c| {
            c.share_enabled = true;
            c.admin_token = Some("an-admin-token-for-tests".into());
            config(c);
        },
        tuning,
    )
    .await;
    let host = cloud.sign_up("Host").await;
    let guest = cloud.sign_up("Guest").await;
    let eh = cloud.events(host.token()).await;
    let eg = into_party(&cloud, &host, &guest).await;
    play(&cloud, &host).await;
    Pair {
        cloud,
        host,
        guest,
        eh,
        eg,
    }
}

async fn pair() -> Pair {
    pair_with(|_| {}, |_| {}).await
}

async fn active(p: &Pair) -> Share {
    invite(&p.cloud, &p.host, &p.guest).await.ok(200);
    accept(&p.cloud, &p.guest).await.ok(200).json()
}

async fn share_event_with(events: &mut Events, revision: u64) -> Share {
    loop {
        match events.until(|e| matches!(e, Event::Share { .. })).await {
            Event::Share { share } if share.revision >= revision => return *share,
            _ => {}
        }
    }
}

async fn gone(events: &mut Events) -> (String, ShareGoneReason) {
    match events
        .until(|e| matches!(e, Event::ShareGone { .. }))
        .await
    {
        Event::ShareGone { id, reason } => (id, reason),
        _ => unreachable!(),
    }
}

async fn no_share(cloud: &TestCloud, user: &User) {
    cloud
        .get("/v1/share", user.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn share_play_is_off_by_default_and_info_says_so() {
    let cloud = TestCloud::start().await;
    let info: Info = cloud.request(reqwest::Method::GET, "/v1/info", None, None).await.ok(200).json();
    let share = info.share.expect("info has share");
    assert!(!share.enabled);
    assert_eq!(share.relay_kbps, 6000);

    let user = cloud.sign_up("Ann").await;
    let other = cloud.sign_up("Ben").await;
    invite(&cloud, &user, &other).await.is_error("forbidden", 403);
    cloud
        .get("/v1/share", user.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .get("/v1/share/invitations", user.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .post(
            "/v1/share/accept",
            user.token(),
            json!({"endpoint": endpoint(2)}),
        )
        .await
        .is_error("forbidden", 403);
    cloud
        .delete("/v1/share", user.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .patch("/v1/share", user.token(), json!({"mode": "watch"}))
        .await
        .is_error("forbidden", 403);

    let on = TestCloud::with(
        |c| {
            c.share_enabled = true;
            c.share_relay_kbps = 4500;
        },
        |_| {},
    )
    .await;
    let info: Info = on.request(reqwest::Method::GET, "/v1/info", None, None).await.ok(200).json();
    let share = info.share.unwrap();
    assert!(share.enabled);
    assert_eq!(share.relay_kbps, 4500);
}

#[tokio::test]
async fn an_invitation_then_accepting_makes_an_active_share() {
    let mut p = pair().await;
    let made: Share = invite(&p.cloud, &p.host, &p.guest).await.ok(200).json();
    assert_eq!(made.state, ShareState::Invited);
    assert_eq!(made.mode, ShareMode::Watch);
    assert_eq!(made.host.id, AccountId(p.host.id));
    assert_eq!(made.guest.id, AccountId(p.guest.id));
    assert_eq!(
        (made.title_id.as_str(), made.title.as_str()),
        ("CUSA00001", "A game")
    );
    assert_eq!(made.host_endpoint, endpoint(1));
    assert_eq!(made.guest_endpoint, "");
    assert_eq!(made.path, None);

    // The guest hears of the invitation, which names the title and the mode, and may read it.
    let invitation = match p
        .eg
        .until(|e| matches!(e, Event::ShareInvitation { .. }))
        .await
    {
        Event::ShareInvitation { invitation } => invitation,
        _ => unreachable!(),
    };
    assert_eq!(invitation.share, made.id);
    assert_ne!(invitation.id, made.id);
    assert_eq!(invitation.from.id, AccountId(p.host.id));
    assert_eq!(invitation.title, "A game");
    assert_eq!(invitation.mode, ShareMode::Watch);
    let left = invitation.expires.saturating_sub(anyplay_cloud::util::now());
    assert!((55..=61).contains(&left), "expires in {left} s");
    let held: ShareInvitations = p
        .cloud
        .get("/v1/share/invitations", p.guest.token())
        .await
        .ok(200)
        .json();
    assert_eq!(held.invitations, std::slice::from_ref(&invitation));
    let none: ShareInvitations = p
        .cloud
        .get("/v1/share/invitations", p.host.token())
        .await
        .ok(200)
        .json();
    assert!(none.invitations.is_empty());

    // Before accepting, the guest does not see where the host is. The host sees its own.
    let seen: Share = p.cloud.get("/v1/share", p.guest.token()).await.ok(200).json();
    assert_eq!(seen.id, made.id);
    assert_eq!(seen.host_endpoint, "");
    assert_eq!(seen.host_relay, None);
    let host_view: Share = p.cloud.get("/v1/share", p.host.token()).await.ok(200).json();
    assert_eq!(host_view.host_endpoint, endpoint(1));

    let accepted: Share = accept(&p.cloud, &p.guest).await.ok(200).json();
    assert_eq!(accepted.state, ShareState::Active);
    assert_eq!(accepted.host_endpoint, endpoint(1));
    assert_eq!(accepted.guest_endpoint, endpoint(2));
    assert_eq!(
        accepted.guest_relay.as_deref(),
        Some("https://relay.example.com")
    );
    assert!(accepted.revision > made.revision);

    // Both hear of it, and the invitation is gone for the guest.
    assert!(matches!(
        p.eg.until(|e| matches!(e, Event::ShareInvitationGone { .. })).await,
        Event::ShareInvitationGone { id } if id == invitation.id
    ));
    let for_guest = share_event_with(&mut p.eg, accepted.revision).await;
    assert_eq!(for_guest, accepted);
    let for_host = share_event_with(&mut p.eh, accepted.revision).await;
    assert_eq!(for_host.state, ShareState::Active);
    assert_eq!(for_host.guest_endpoint, endpoint(2));
    let held: ShareInvitations = p
        .cloud
        .get("/v1/share/invitations", p.guest.token())
        .await
        .ok(200)
        .json();
    assert!(held.invitations.is_empty());
    // Accepting twice is no second share.
    accept(&p.cloud, &p.guest).await.is_error("bad_request", 400);
    // The host cannot accept its own invitation.
    accept(&p.cloud, &p.host).await.is_error("forbidden", 403);
}

#[tokio::test]
async fn only_the_host_changes_mode_and_path_and_each_change_is_announced() {
    let mut p = pair().await;
    let first = active(&p).await;

    p.cloud
        .patch("/v1/share", p.guest.token(), json!({"mode": "together"}))
        .await
        .is_error("forbidden", 403);
    p.cloud
        .patch("/v1/share", p.host.token(), json!({}))
        .await
        .is_error("bad_request", 400);
    p.cloud
        .patch("/v1/share", p.host.token(), json!({"mode": "spectate"}))
        .await
        .is_error("bad_request", 400);
    p.cloud
        .patch("/v1/share", p.host.token(), json!({"path": "carrier_pigeon"}))
        .await
        .is_error("bad_request", 400);

    let second: Share = p
        .cloud
        .patch(
            "/v1/share",
            p.host.token(),
            json!({"mode": "hand_over", "path": "direct"}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(second.mode, ShareMode::HandOver);
    assert_eq!(second.path, Some(SharePath::Direct));
    assert!(second.revision > first.revision);
    let for_guest = share_event_with(&mut p.eg, second.revision).await;
    assert_eq!(for_guest.mode, ShareMode::HandOver);
    assert_eq!(for_guest.host_endpoint, endpoint(1));
    let third: Share = p
        .cloud
        .patch("/v1/share", p.host.token(), json!({"path": "relay"}))
        .await
        .ok(200)
        .json();
    assert_eq!(third.path, Some(SharePath::Relay));
    assert_eq!(third.mode, ShareMode::HandOver);
    assert!(third.revision > second.revision);
    let for_host = share_event_with(&mut p.eh, third.revision).await;
    assert_eq!(for_host, third);
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let p = pair().await;
    for body in [
        json!({"to": p.guest.id, "mode": "watch", "endpoint": "abc"}),
        json!({"to": p.guest.id, "mode": "watch", "endpoint": "A".repeat(64)}),
        json!({"to": p.guest.id, "mode": "watch", "endpoint": endpoint(1), "relay": "http://relay.example.com"}),
        json!({"to": p.guest.id, "mode": "watch", "endpoint": endpoint(1), "relay": "relay.example.com"}),
        json!({"to": p.guest.id, "mode": "teleport", "endpoint": endpoint(1)}),
        json!({"to": p.host.id, "mode": "watch", "endpoint": endpoint(1)}),
    ] {
        p.cloud
            .post("/v1/shares", p.host.token(), body.clone())
            .await
            .is_error("bad_request", 400);
    }
    no_share(&p.cloud, &p.host).await;
    // The same checks guard the guest's endpoint.
    invite(&p.cloud, &p.host, &p.guest).await.ok(200);
    for body in [
        json!({"endpoint": "nope"}),
        json!({"endpoint": endpoint(2), "relay": "http://relay.example.com"}),
        json!({"endpoint": endpoint(1)}),
    ] {
        p.cloud
            .post("/v1/share/accept", p.guest.token(), body)
            .await
            .is_error("bad_request", 400);
    }
}

#[tokio::test]
async fn the_rules_for_starting_a_share() {
    let p = pair().await;

    // Not in one party: forbidden. A friend who is not in the party, a stranger.
    let cy = p.cloud.sign_up("Cyra").await;
    invite(&p.cloud, &p.host, &cy).await.is_error("forbidden", 403);
    befriend(&p.cloud, &p.host, &cy).await;
    invite(&p.cloud, &p.host, &cy).await.is_error("forbidden", 403);
    // Nobody to invite: no such account.
    p.cloud
        .post(
            "/v1/shares",
            p.host.token(),
            json!({"to": 999_999_999u64, "mode": "watch", "endpoint": endpoint(1)}),
        )
        .await
        .is_error("forbidden", 403);

    // No game running: not_found.
    stop_playing(&p.cloud, &p.host).await;
    invite(&p.cloud, &p.host, &p.guest).await.is_error("not_found", 404);
    // The guest cannot share a game it does not run, either.
    invite(&p.cloud, &p.guest, &p.host).await.is_error("not_found", 404);
    play(&p.cloud, &p.host).await;

    // A block between them: forbidden, whoever blocked.
    p.cloud
        .put(&format!("/v1/blocks/{}", p.host.id), p.guest.token(), None)
        .await
        .ok(204);
    invite(&p.cloud, &p.host, &p.guest).await.is_error("forbidden", 403);
    no_share(&p.cloud, &p.host).await;
}

#[tokio::test]
async fn an_account_is_in_one_share_at_a_time() {
    let p = pair().await;
    // A third member of the party, who plays too.
    let cy = p.cloud.sign_up("Cyra").await;
    befriend(&p.cloud, &p.host, &cy).await;
    p.cloud
        .post(
            "/v1/party/invitations",
            p.host.token(),
            json!({"to": [cy.id]}),
        )
        .await
        .ok(200);
    let held: anyplay_api::PartyInvitations = p
        .cloud
        .get("/v1/party/invitations", cy.token())
        .await
        .ok(200)
        .json();
    p.cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", held.invitations[0].id),
            cy.token(),
        )
        .await
        .ok(200);
    play(&p.cloud, &cy).await;

    invite(&p.cloud, &p.host, &p.guest).await.ok(200);
    // The host is in a share, so is the guest, even before it accepts.
    invite(&p.cloud, &p.host, &cy).await.is_error("limit", 409);
    invite(&p.cloud, &cy, &p.guest).await.is_error("limit", 409);
    invite(&p.cloud, &cy, &p.host).await.is_error("limit", 409);
    invite(&p.cloud, &p.guest, &cy).await.is_error("not_found", 404);
    accept(&p.cloud, &p.guest).await.ok(200);
    invite(&p.cloud, &p.host, &cy).await.is_error("limit", 409);
}

#[tokio::test]
async fn outsiders_get_nothing_and_endpoints_are_not_leaked() {
    let mut p = pair().await;
    let cy = p.cloud.sign_up("Cyra").await;
    let mut ec = p.cloud.events(cy.token()).await;
    befriend(&p.cloud, &p.host, &cy).await;

    let made: Share = invite(&p.cloud, &p.host, &p.guest).await.ok(200).json();
    accept(&p.cloud, &p.guest).await.ok(200);
    // An outsider reads, ends, changes and accepts nothing.
    no_share(&p.cloud, &cy).await;
    p.cloud
        .delete("/v1/share", cy.token())
        .await
        .is_error("not_found", 404);
    p.cloud
        .patch("/v1/share", cy.token(), json!({"mode": "together"}))
        .await
        .is_error("not_found", 404);
    accept(&p.cloud, &cy).await.is_error("not_found", 404);
    let held: ShareInvitations = p
        .cloud
        .get("/v1/share/invitations", cy.token())
        .await
        .ok(200)
        .json();
    assert!(held.invitations.is_empty());
    // The share is untouched and the outsider heard nothing: no invitation, no share, no
    // endpoint.
    let still: Share = p.cloud.get("/v1/share", p.host.token()).await.ok(200).json();
    assert_eq!(still.id, made.id);
    assert_eq!(still.mode, ShareMode::Watch);
    p.cloud
        .patch("/v1/share", p.host.token(), json!({"mode": "together"}))
        .await
        .ok(200);
    p.cloud.delete("/v1/share", p.host.token()).await.ok(204);
    gone(&mut p.eg).await;
    while let Some(event) = ec.next_within(Duration::from_millis(300)).await {
        let text = serde_json::to_string(&event).unwrap();
        assert!(
            !text.contains("share") && !text.contains(&endpoint(1)) && !text.contains(&endpoint(2)),
            "an outsider heard {text}"
        );
    }
    // Neither does a friend of the host who is not in the party get the invitation of another.
    let _ = &mut p.eh;
}

#[tokio::test]
async fn starting_shares_is_limited_per_account_and_hour() {
    let p = pair().await;
    for n in 0..10 {
        // Each request counts, whatever becomes of it.
        p.cloud
            .post(
                "/v1/shares",
                p.host.token(),
                json!({"to": p.host.id, "mode": "watch", "endpoint": endpoint(1)}),
            )
            .await
            .is_error("bad_request", 400);
        let _ = n;
    }
    p.cloud
        .post(
            "/v1/shares",
            p.host.token(),
            json!({"to": p.guest.id, "mode": "watch", "endpoint": endpoint(1)}),
        )
        .await
        .is_error("rate_limited", 429);
    // The limit is the host's, not the guest's.
    no_share(&p.cloud, &p.guest).await;
}

#[tokio::test]
async fn the_host_ends_and_the_guest_declines_or_leaves() {
    let mut p = pair().await;

    // The guest declines an invitation.
    let made: Share = invite(&p.cloud, &p.host, &p.guest).await.ok(200).json();
    let invitation = match p
        .eg
        .until(|e| matches!(e, Event::ShareInvitation { .. }))
        .await
    {
        Event::ShareInvitation { invitation } => invitation,
        _ => unreachable!(),
    };
    p.cloud.delete("/v1/share", p.guest.token()).await.ok(204);
    assert_eq!(
        gone(&mut p.eh).await,
        (made.id.clone(), ShareGoneReason::Declined)
    );
    assert!(matches!(
        p.eg.until(|e| matches!(e, Event::ShareInvitationGone { .. })).await,
        Event::ShareInvitationGone { id } if id == invitation.id
    ));
    no_share(&p.cloud, &p.host).await;
    no_share(&p.cloud, &p.guest).await;
    p.cloud
        .delete("/v1/share", p.guest.token())
        .await
        .is_error("not_found", 404);

    // The guest leaves an active share.
    let made = active(&p).await;
    p.cloud.delete("/v1/share", p.guest.token()).await.ok(204);
    assert_eq!(gone(&mut p.eh).await, (made.id.clone(), ShareGoneReason::Left));
    assert_eq!(gone(&mut p.eg).await, (made.id, ShareGoneReason::Left));

    // The host ends one, an invited one as well.
    let made = active(&p).await;
    p.cloud.delete("/v1/share", p.host.token()).await.ok(204);
    assert_eq!(gone(&mut p.eg).await, (made.id.clone(), ShareGoneReason::Ended));
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::Ended));
    let made: Share = invite(&p.cloud, &p.host, &p.guest).await.ok(200).json();
    p.cloud.delete("/v1/share", p.host.token()).await.ok(204);
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::Ended));
    assert!(matches!(
        p.eg.until(|e| matches!(e, Event::ShareInvitationGone { .. })).await,
        Event::ShareInvitationGone { .. }
    ));
    // Accepting what is gone fails, and a new share can start at once.
    accept(&p.cloud, &p.guest).await.is_error("not_found", 404);
    invite(&p.cloud, &p.host, &p.guest).await.ok(200);
}

#[tokio::test]
async fn the_share_ends_when_the_hosts_game_ends() {
    let mut p = pair().await;
    let made = active(&p).await;
    stop_playing(&p.cloud, &p.host).await;
    assert_eq!(
        gone(&mut p.eg).await,
        (made.id.clone(), ShareGoneReason::GameEnded)
    );
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::GameEnded));
    no_share(&p.cloud, &p.guest).await;

    // An invitation that is not accepted yet ends with the game as well.
    play(&p.cloud, &p.host).await;
    invite(&p.cloud, &p.host, &p.guest).await.ok(200);
    stop_playing(&p.cloud, &p.host).await;
    p.eg
        .until(|e| matches!(e, Event::ShareInvitationGone { .. }))
        .await;
    no_share(&p.cloud, &p.guest).await;
}

#[tokio::test]
async fn the_share_ends_when_either_leaves_the_party() {
    let mut p = pair().await;
    let made = active(&p).await;
    p.cloud
        .post_empty("/v1/party/leave", p.guest.token())
        .await
        .ok(204);
    assert_eq!(gone(&mut p.eh).await, (made.id.clone(), ShareGoneReason::Left));
    assert_eq!(gone(&mut p.eg).await, (made.id, ShareGoneReason::Left));

}

#[tokio::test]
async fn the_share_ends_when_the_host_leaves_the_party() {
    let mut p = pair().await;
    let made = active(&p).await;
    p.cloud
        .post_empty("/v1/party/leave", p.host.token())
        .await
        .ok(204);
    assert_eq!(gone(&mut p.eg).await, (made.id, ShareGoneReason::Left));
}

#[tokio::test]
async fn the_share_ends_when_a_block_is_created() {
    let mut p = pair().await;
    let made = active(&p).await;
    p.cloud
        .put(&format!("/v1/blocks/{}", p.host.id), p.guest.token(), None)
        .await
        .ok(204);
    assert_eq!(
        gone(&mut p.eh).await,
        (made.id.clone(), ShareGoneReason::Blocked)
    );
    assert_eq!(gone(&mut p.eg).await, (made.id, ShareGoneReason::Blocked));
}

#[tokio::test]
async fn the_share_ends_when_an_account_is_banned() {
    let mut p = pair().await;
    let made = active(&p).await;
    p.cloud
        .request(
            reqwest::Method::POST,
            "/admin/v1/bans",
            Some("an-admin-token-for-tests"),
            Some(json!({"handle": p.guest.handle, "reason": "cheating"})),
        )
        .await
        .ok(200);
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::Banned));
}

#[tokio::test]
async fn the_share_ends_when_an_account_is_deleted() {
    let mut p = pair().await;
    let made = active(&p).await;
    p.cloud.delete_account_now(&p.guest).await;
    assert_eq!(gone(&mut p.eh).await.0, made.id);
    no_share(&p.cloud, &p.host).await;
}

#[tokio::test]
async fn an_invitation_that_is_not_accepted_in_time_ends_the_share() {
    let mut p = pair_with(|_| {}, |t| {
        t.share_invitation_ttl = Duration::from_millis(600);
    })
    .await;
    let made: Share = invite(&p.cloud, &p.host, &p.guest).await.ok(200).json();
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::Timeout));
    p.eg
        .until(|e| matches!(e, Event::ShareInvitationGone { .. }))
        .await;
    no_share(&p.cloud, &p.host).await;
    accept(&p.cloud, &p.guest).await.is_error("not_found", 404);
}

#[tokio::test]
async fn an_account_without_an_event_connection_for_too_long_ends_the_share() {
    let mut p = pair_with(|_| {}, |t| {
        t.share_absent = Duration::from_millis(900);
    })
    .await;
    let made = active(&p).await;
    // The guest's connection goes; the host stays connected.
    drop(p.eg);
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::Timeout));
}

#[tokio::test]
async fn relayed_time_beyond_the_daily_allowance_ends_the_share() {
    let mut p = pair_with(
        |c| c.share_relay_minutes_per_day = 1,
        |t| t.share_minute = Duration::from_millis(600),
    )
    .await;
    let made = active(&p).await;
    // Direct time costs nothing.
    p.cloud
        .patch("/v1/share", p.host.token(), json!({"path": "direct"}))
        .await
        .ok(200);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    no_share_gone_yet(&p).await;
    p.cloud
        .patch("/v1/share", p.host.token(), json!({"path": "relay"}))
        .await
        .ok(200);
    assert_eq!(
        gone(&mut p.eg).await,
        (made.id.clone(), ShareGoneReason::Quota)
    );
    assert_eq!(gone(&mut p.eh).await, (made.id, ShareGoneReason::Quota));

    // The time is the host's for the day: the next relayed share has little left.
    invite(&p.cloud, &p.host, &p.guest).await.ok(200);
    accept(&p.cloud, &p.guest).await.ok(200);
    p.cloud
        .patch("/v1/share", p.host.token(), json!({"path": "relay"}))
        .await
        .ok(200);
    gone(&mut p.eh).await;
}

async fn no_share_gone_yet(p: &Pair) {
    let share: Value = p.cloud.get("/v1/share", p.host.token()).await.ok(200).json();
    assert_eq!(share["state"], "active");
}
