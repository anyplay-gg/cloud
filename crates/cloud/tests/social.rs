mod common;

use std::time::Duration;

use anyplay_api::{AccountId, Activity, Event, FriendLists, FriendRequestAnswer, FriendState};
use common::TestCloud;
use serde_json::json;

const QUIET: Duration = Duration::from_millis(250);

fn lists(reply: common::Reply) -> FriendLists {
    reply.ok(200).json()
}

fn activity(title_id: &str) -> serde_json::Value {
    json!({"activity": {"title_id": title_id, "title": "Test Game", "version": "01.000"}})
}

#[tokio::test]
async fn a_friend_request_is_accepted_and_both_sides_are_told() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let mut eb = cloud.events(b.token()).await;

    let answer: FriendRequestAnswer = cloud
        .post("/v1/friends/requests", a.token(), json!({"handle": "sam"}))
        .await
        .ok(200)
        .json();
    assert_eq!(answer.state, FriendState::Requested);
    assert_eq!(eb.next().await, Event::Friends);
    assert_eq!(ea.next().await, Event::Friends);

    let sam = lists(cloud.get("/v1/friends", b.token()).await);
    assert_eq!(sam.incoming.len(), 1);
    assert_eq!(sam.incoming[0].account.handle, "Alec");
    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert_eq!(alec.outgoing.len(), 1);
    assert!(alec.friends.is_empty());

    // Asking twice changes nothing.
    cloud
        .post("/v1/friends/requests", a.token(), json!({"handle": "sam"}))
        .await
        .ok(200);

    cloud
        .post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
        .await
        .ok(204);
    assert_eq!(ea.next().await, Event::Friends);
    assert_eq!(eb.next().await, Event::Friends);
    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert_eq!(alec.friends.len(), 1);
    assert_eq!(alec.friends[0].account.id, AccountId(b.id));
    assert!(
        alec.friends[0].presence.online,
        "Sam holds an event connection"
    );
    assert!(alec.outgoing.is_empty());

    // Accepting again has nothing to accept.
    cloud
        .post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
        .await
        .is_error("not_found", 404);

    cloud
        .delete(&format!("/v1/friends/{}", a.id), b.token())
        .await
        .ok(204);
    assert_eq!(ea.next().await, Event::Friends);
    assert_eq!(eb.next().await, Event::Friends);
    assert!(
        lists(cloud.get("/v1/friends", a.token()).await)
            .friends
            .is_empty()
    );
    cloud
        .delete(&format!("/v1/friends/{}", a.id), b.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_request_to_someone_who_asked_us_makes_friends_at_once() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Sam").await;
    cloud
        .post("/v1/friends/requests", a.token(), json!({"handle": "Sam"}))
        .await
        .ok(200);
    let answer: FriendRequestAnswer = cloud
        .post("/v1/friends/requests", b.token(), json!({"handle": "alec"}))
        .await
        .ok(200)
        .json();
    assert_eq!(answer.state, FriendState::Friends);
    let lists = lists(cloud.get("/v1/friends", a.token()).await);
    assert_eq!(lists.friends.len(), 1);
    assert!(lists.incoming.is_empty() && lists.outgoing.is_empty());
}

#[tokio::test]
async fn declining_and_withdrawing_remove_a_request() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Sam").await;
    let c = cloud.sign_up("Kim").await;
    for target in ["Sam", "Kim"] {
        cloud
            .post("/v1/friends/requests", a.token(), json!({"handle": target}))
            .await
            .ok(200);
    }
    // Sam declines, Alec withdraws the one to Kim.
    cloud
        .delete(&format!("/v1/friends/requests/{}", a.id), b.token())
        .await
        .ok(204);
    cloud
        .delete(&format!("/v1/friends/requests/{}", c.id), a.token())
        .await
        .ok(204);
    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert!(alec.outgoing.is_empty());
    cloud
        .delete(&format!("/v1/friends/requests/{}", c.id), a.token())
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            "/v1/friends/requests",
            a.token(),
            json!({"handle": "nobody"}),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post("/v1/friends/requests", a.token(), json!({"handle": "alec"}))
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn blocks_end_friendships_hide_themselves_and_can_be_lifted() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let c = cloud.sign_up("Kim").await;
    // Kim asks Alec, and Alec asks Sam's friend... keep it to a request pair: Kim -> Alec.
    cloud
        .post("/v1/friends/requests", c.token(), json!({"handle": "Alec"}))
        .await
        .ok(200);
    let mut eb = cloud.events(b.token()).await;
    let mut ec = cloud.events(c.token()).await;

    // Alec blocks Sam (a friend) and Kim (a requester).
    cloud
        .put(&format!("/v1/blocks/{}", b.id), a.token(), None)
        .await
        .ok(204);
    cloud
        .put(&format!("/v1/blocks/{}", c.id), a.token(), None)
        .await
        .ok(204);
    cloud
        .put(&format!("/v1/blocks/{}", c.id), a.token(), None)
        .await
        .ok(204);
    assert_eq!(eb.next().await, Event::Friends, "a friendship ended");
    assert_eq!(ec.next().await, Event::Friends, "a request ended");

    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert!(alec.friends.is_empty() && alec.incoming.is_empty());
    assert_eq!(alec.blocked.len(), 2);
    assert!(
        lists(cloud.get("/v1/friends", b.token()).await)
            .friends
            .is_empty()
    );

    // Sam asks Alec: the cloud says "requested" and does nothing.
    let answer: FriendRequestAnswer = cloud
        .post("/v1/friends/requests", b.token(), json!({"handle": "Alec"}))
        .await
        .ok(200)
        .json();
    assert_eq!(answer.state, FriendState::Requested);
    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert!(alec.incoming.is_empty(), "nothing reached the blocker");
    let sam = lists(cloud.get("/v1/friends", b.token()).await);
    assert!(
        sam.outgoing.is_empty(),
        "and nothing is shown as pending either"
    );
    eb.quiet(QUIET).await;

    // Alec cannot ask someone he blocked.
    cloud
        .post("/v1/friends/requests", a.token(), json!({"handle": "Sam"}))
        .await
        .is_error("forbidden", 403);

    // Lifting the block makes asking possible again.
    cloud
        .delete(&format!("/v1/blocks/{}", b.id), a.token())
        .await
        .ok(204);
    cloud
        .delete(&format!("/v1/blocks/{}", b.id), a.token())
        .await
        .ok(204);
    assert_eq!(
        lists(cloud.get("/v1/friends", a.token()).await)
            .blocked
            .len(),
        1
    );
    cloud
        .post("/v1/friends/requests", b.token(), json!({"handle": "Alec"}))
        .await
        .ok(200);
    assert_eq!(
        lists(cloud.get("/v1/friends", a.token()).await)
            .incoming
            .len(),
        1
    );

    cloud
        .put("/v1/blocks/5", a.token(), None)
        .await
        .is_error("not_found", 404);
    cloud
        .put(&format!("/v1/blocks/{}", a.id), a.token(), None)
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn friends_see_when_a_friend_comes_online_plays_and_goes_offline() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;

    // Sam connects.
    let mut eb = cloud.events(b.token()).await;
    let Event::Presence { account, presence } = ea.next().await else {
        panic!("expected presence");
    };
    assert_eq!(account, AccountId(b.id));
    assert!(presence.online && presence.activity.is_none());

    // Sam starts a game.
    cloud
        .put("/v1/presence", b.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    let Event::Presence { presence, .. } = ea.next().await else {
        panic!("expected presence");
    };
    let playing: Activity = presence.activity.expect("an activity");
    assert_eq!(playing.title_id, "GAME00001");
    assert_eq!(playing.session, None);
    // The same activity again is not news.
    cloud
        .put("/v1/presence", b.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    ea.quiet(QUIET).await;
    // Sam does not hear about his own presence.
    eb.quiet(QUIET).await;

    // The friend list shows it too.
    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert_eq!(
        alec.friends[0].presence.activity.as_ref().unwrap().title,
        "Test Game"
    );

    // Sam stops, then drops his connection.
    cloud
        .put("/v1/presence", b.token(), Some(json!({"activity": null})))
        .await
        .ok(204);
    let Event::Presence { presence, .. } = ea.next().await else {
        panic!("expected presence");
    };
    assert!(presence.online && presence.activity.is_none());
    drop(eb);
    let Event::Presence { presence, .. } = ea.next().await else {
        panic!("expected presence");
    };
    assert!(!presence.online);
    let alec = lists(cloud.get("/v1/friends", a.token()).await);
    assert!(!alec.friends[0].presence.online);
}

#[tokio::test]
async fn a_short_reconnect_does_not_show_as_offline() {
    let cloud = TestCloud::with(|_| {}, |t| t.offline_grace = Duration::from_millis(600)).await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let eb = cloud.events(b.token()).await;
    assert!(matches!(ea.next().await, Event::Presence { .. }));
    drop(eb);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _eb = cloud.events(b.token()).await;
    ea.quiet(Duration::from_millis(900)).await;
    assert!(
        lists(cloud.get("/v1/friends", a.token()).await).friends[0]
            .presence
            .online
    );
}

#[tokio::test]
async fn only_friends_hear_about_presence() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let stranger = cloud.sign_up("Kim").await;
    let mut ek = cloud.events(stranger.token()).await;
    let _ea = cloud.events(a.token()).await;
    cloud
        .put("/v1/presence", a.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    ek.quiet(QUIET).await;
}

#[tokio::test]
async fn a_second_connection_of_a_device_replaces_the_first() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let mut first = cloud.events(a.token()).await;
    let mut second = cloud.events(a.token()).await;
    assert!(
        first.ends_within(Duration::from_secs(3)).await,
        "the old connection is closed"
    );
    second.quiet(QUIET).await;
}

#[tokio::test]
async fn virtual_addresses_come_from_the_range_and_stay_with_the_account() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Sam").await;
    let ea = cloud.events(a.token()).await;
    let eb = cloud.events(b.token()).await;
    let addr_a: std::net::Ipv4Addr = ea.address.parse().unwrap();
    let addr_b: std::net::Ipv4Addr = eb.address.parse().unwrap();
    assert!(anyplay_api::is_virtual(addr_a) && anyplay_api::is_virtual(addr_b));
    assert_ne!(addr_a, addr_b);
    for addr in [addr_a, addr_b] {
        assert!(addr.octets()[3] != 0 && addr.octets()[3] != 255);
    }
    // Reconnecting gives the same address back.
    drop(ea);
    let again = cloud.events(a.token()).await;
    assert_eq!(again.address, addr_a.to_string());
}

#[tokio::test]
async fn each_presence_change_comes_with_a_higher_revision_and_the_lists_show_the_last() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    // Sam listens first, so he hears Alec come online.
    let mut eb = cloud.events(b.token()).await;
    let _ea = cloud.events(a.token()).await;
    let revision_of = |event: Event| match event {
        Event::Presence { presence, .. } => presence.revision,
        _ => unreachable!(),
    };
    let online = revision_of(eb.until(|e| matches!(e, Event::Presence { .. })).await);
    cloud
        .put("/v1/presence", a.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    let playing = revision_of(eb.until(|e| matches!(e, Event::Presence { .. })).await);
    assert!(online > 0);
    assert!(playing > online);
    let friends = lists(cloud.get("/v1/friends", b.token()).await);
    assert_eq!(friends.friends[0].presence.revision, playing);
}
