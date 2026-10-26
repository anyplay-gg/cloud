//! Appearing offline (network ADR 0033): friends see an offline account while it stays signed in,
//! plays, joins and is invited.

mod common;

use std::time::Duration;

use anyplay_api::{AppearOffline, Event, FriendLists, Me, Presence, Session};
use common::portal::Portal;
use common::{Events, TestCloud};
use reqwest::Method;
use serde_json::{Value, json};

const QUIET: Duration = Duration::from_millis(250);

fn activity(title_id: &str) -> Value {
    json!({"activity": {"title_id": title_id, "title": "Test Game", "version": "01.000"}})
}

fn session_body() -> Value {
    json!({
        "title_id": "GAME00001", "title": "Test Game", "version": "01.000", "build": "0.1.3",
        "max_members": 4, "visibility": "friends", "properties": "",
        "endpoint": "01".repeat(32), "relay": null,
    })
}

async fn presence_of_friend(cloud: &TestCloud, token: &str) -> Presence {
    let lists: FriendLists = cloud.get("/v1/friends", token).await.ok(200).json();
    lists.friends[0].presence.clone()
}

async fn next_presence(events: &mut Events) -> Presence {
    match events.until(|e| matches!(e, Event::Presence { .. })).await {
        Event::Presence { presence, .. } => presence,
        _ => unreachable!(),
    }
}

async fn set(cloud: &TestCloud, token: &str, on: bool) -> AppearOffline {
    cloud
        .put(
            "/v1/me/appear-offline",
            token,
            Some(json!({"appear_offline": on})),
        )
        .await
        .ok(200)
        .json()
}

async fn setting(cloud: &TestCloud, token: &str) -> bool {
    cloud
        .get("/v1/me/appear-offline", token)
        .await
        .ok(200)
        .json::<AppearOffline>()
        .appear_offline
}

#[tokio::test]
async fn friends_see_an_account_that_appears_offline_as_offline() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let mut eb = cloud.events(b.token()).await;
    assert!(next_presence(&mut ea).await.online);
    cloud
        .put("/v1/presence", b.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    let playing = next_presence(&mut ea).await;
    assert!(playing.online && playing.activity.is_some());

    // Sam hides. Alec hears of an offline Sam with a newer revision, and so does the list.
    assert!(set(&cloud, b.token(), true).await.appear_offline);
    let hidden = next_presence(&mut ea).await;
    assert!(!hidden.online && hidden.activity.is_none());
    assert!(hidden.revision > playing.revision);
    assert_eq!(
        presence_of_friend(&cloud, a.token()).await,
        Presence {
            online: false,
            activity: None,
            revision: hidden.revision,
        }
    );

    // Sam hears of it on his own connection, and his own account says so.
    assert_eq!(
        eb.until(|e| matches!(e, Event::AppearOffline { .. })).await,
        Event::AppearOffline {
            appear_offline: true
        }
    );
    let me: Me = cloud.get("/v1/me", b.token()).await.ok(200).json();
    assert!(me.appear_offline);
    assert!(setting(&cloud, b.token()).await);
    let alec: Me = cloud.get("/v1/me", a.token()).await.ok(200).json();
    assert!(!alec.appear_offline, "the setting is Sam's alone");

    // Nothing Sam does leaks: another game, a session friends could join, the same setting again.
    cloud
        .put("/v1/presence", b.token(), Some(activity("GAME00002")))
        .await
        .ok(204);
    let session: Session = cloud
        .post("/v1/sessions", b.token(), session_body())
        .await
        .ok(200)
        .json();
    set(&cloud, b.token(), true).await;
    ea.quiet(QUIET).await;
    assert!(!presence_of_friend(&cloud, a.token()).await.online);

    // He can still invite, and Alec gets it.
    let invited = cloud
        .post(
            &format!("/v1/sessions/{}/invitations", session.id),
            b.token(),
            json!({"to": [a.id]}),
        )
        .await
        .ok(200);
    assert_eq!(invited.json::<Value>()["sent"], json!([a.id]));
    ea.until(|e| matches!(e, Event::Invitation { .. })).await;

    // Showing again tells Alec what Sam plays now, with the session he can join.
    assert!(!set(&cloud, b.token(), false).await.appear_offline);
    let shown = next_presence(&mut ea).await;
    assert!(shown.online);
    let now = shown.activity.expect("the game Sam plays");
    assert_eq!(now.title_id, "GAME00002");
    assert!(shown.revision > hidden.revision);
    assert!(!setting(&cloud, b.token()).await);
}

#[tokio::test]
async fn a_hidden_account_going_offline_is_no_news_and_the_setting_stays() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let eb = cloud.events(b.token()).await;
    assert!(next_presence(&mut ea).await.online);
    set(&cloud, b.token(), true).await;
    assert!(!next_presence(&mut ea).await.online);

    // The connection drops and comes back: Alec hears nothing, the list stays offline.
    drop(eb);
    ea.quiet(Duration::from_millis(700)).await;
    let _eb = cloud.events(b.token()).await;
    ea.quiet(QUIET).await;
    assert!(!presence_of_friend(&cloud, a.token()).await.online);

    // Another device signs in to the same account and finds the setting.
    let signed_in = cloud.sign_in(&b.email, "second pc").await;
    assert!(signed_in.me.appear_offline);
}

#[tokio::test]
async fn a_new_account_appears_online_and_a_game_played_while_hidden_is_news_later() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    assert!(!setting(&cloud, b.token()).await);
    let mut ea = cloud.events(a.token()).await;
    let _eb = cloud.events(b.token()).await;
    assert!(next_presence(&mut ea).await.online);

    set(&cloud, b.token(), true).await;
    assert!(!next_presence(&mut ea).await.online);
    cloud
        .put("/v1/presence", b.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    ea.quiet(QUIET).await;

    // Visible again, the same game is a first play: the feed item is made now.
    set(&cloud, b.token(), false).await;
    assert!(next_presence(&mut ea).await.online);
    cloud
        .put("/v1/presence", b.token(), Some(activity("GAME00001")))
        .await
        .ok(204);
    ea.until(|e| matches!(e, Event::Feed)).await;
}

#[tokio::test]
async fn the_setting_needs_a_boolean_and_a_signed_in_account() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    cloud
        .put("/v1/me/appear-offline", a.token(), Some(json!({})))
        .await
        .is_error("bad_request", 400);
    cloud
        .put(
            "/v1/me/appear-offline",
            a.token(),
            Some(json!({"appear_offline": "yes"})),
        )
        .await
        .is_error("bad_request", 400);
    assert_eq!(
        cloud
            .put(
                "/v1/me/appear-offline",
                "not a token",
                Some(json!({"appear_offline": true})),
            )
            .await
            .status,
        401
    );
    assert!(!setting(&cloud, a.token()).await);
}

#[tokio::test]
async fn the_portal_reads_and_sets_it_and_friends_there_see_the_result() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let _eb = cloud.events(b.token()).await;
    let alec = Portal::signed_in(&cloud, &a).await;
    let sam = Portal::signed_in(&cloud, &b).await;
    assert_eq!(
        alec.get("/friends").await.ok(200)["friends"][0]["presence"]["online"],
        true
    );

    assert_eq!(
        sam.get("/me/appear-offline").await.ok(200),
        json!({"appear_offline": false})
    );
    assert_eq!(
        sam.send(
            Method::PUT,
            "/me/appear-offline",
            json!({"appear_offline": true})
        )
        .await
        .ok(200),
        json!({"appear_offline": true})
    );
    assert_eq!(
        sam.get("/account").await.ok(200)["account"]["appear_offline"],
        true
    );
    assert_eq!(
        alec.get("/friends").await.ok(200)["friends"][0]["presence"]["online"],
        false
    );
    sam.send(
        Method::PUT,
        "/me/appear-offline",
        json!({"appear_offline": "yes"}),
    )
    .await
    .error(400, "bad_request");

    // The launcher's call and the portal's are one setting.
    set(&cloud, b.token(), false).await;
    assert_eq!(
        sam.get("/me/appear-offline").await.ok(200),
        json!({"appear_offline": false})
    );
}
