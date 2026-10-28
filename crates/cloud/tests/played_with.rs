//! Played with recently (network `spec/cloud-api.md`, "Played with recently"): pairs are recorded
//! by the sweep, kept per account, and shown only to the account itself.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use anyplay_api::{PartyInvitations, PlayedWithList, RememberPlayedWith, Session};
use common::portal::Portal;
use common::{TestCloud, User};
use reqwest::Method;
use serde_json::json;

/// A cloud that remembers a pair at once and looks for pairs at every sweep.
async fn quick() -> TestCloud {
    TestCloud::with(
        |_| {},
        |t| {
            t.played_with_min = Duration::ZERO;
            t.played_with_every = Duration::ZERO;
        },
    )
    .await
}

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

/// Both accounts in one session of "Test Game".
async fn play_session(cloud: &TestCloud, a: &User, b: &User) -> Session {
    let made: Session = cloud
        .post(
            "/v1/sessions",
            a.token(),
            json!({
                "title_id": "GAME00001", "title": "Test Game", "version": "01.000",
                "build": "0.1.3", "max_members": 4, "visibility": "friends",
                "properties": "", "endpoint": endpoint(1), "relay": null,
            }),
        )
        .await
        .ok(200)
        .json();
    cloud
        .post(
            &format!("/v1/sessions/{}/join", made.id),
            b.token(),
            json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(2),
                   "relay": null, "invitation": null}),
        )
        .await
        .ok(200)
        .json()
}

/// Both accounts in one party.
async fn gather(cloud: &TestCloud, leader: &User, member: &User) {
    cloud.post_empty("/v1/party", leader.token()).await.ok(200);
    cloud
        .post(
            "/v1/party/invitations",
            leader.token(),
            json!({"to": [member.id]}),
        )
        .await
        .ok(200);
    let held: PartyInvitations = cloud
        .get("/v1/party/invitations", member.token())
        .await
        .ok(200)
        .json();
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", held.invitations[0].id),
            member.token(),
        )
        .await
        .ok(200);
}

async fn list(cloud: &TestCloud, user: &User) -> PlayedWithList {
    cloud
        .get("/v1/me/played-with", user.token())
        .await
        .ok(200)
        .json()
}

async fn sweep(cloud: &TestCloud) {
    cloud.state.sweep().await.unwrap();
}

async fn rows(cloud: &TestCloud) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM played_with")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    n
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn a_session_pair_is_remembered_by_both_with_the_title() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    assert!(list(&cloud, &a).await.people.is_empty());
    assert!(list(&cloud, &a).await.remember);
    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;

    let seen = list(&cloud, &a).await;
    assert_eq!(seen.people.len(), 1);
    let one = &seen.people[0];
    assert_eq!(one.account.id.0, b.id);
    assert_eq!(one.account.handle, "Ben");
    assert_eq!(one.title_id.as_deref(), Some("GAME00001"));
    assert_eq!(one.title.as_deref(), Some("Test Game"));
    assert!(one.friend);
    assert!(now().abs_diff(one.last) < 30);
    assert_eq!(list(&cloud, &b).await.people[0].account.id.0, a.id);

    // Not friends any more: the entry stays and offers "Add friend" again.
    cloud
        .delete(&format!("/v1/friends/{}", b.id), a.token())
        .await
        .ok(204);
    assert!(!list(&cloud, &a).await.people[0].friend);
}

#[tokio::test]
async fn a_pair_is_remembered_only_after_the_minimum_time() {
    // The default: 120 seconds. The members have just joined.
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    play_session(&cloud, &a, &b).await;
    cloud.state.played_with_at.store(0, Ordering::Relaxed);
    sweep(&cloud).await;
    assert!(list(&cloud, &a).await.people.is_empty());
    assert!(list(&cloud, &b).await.people.is_empty());

    // Two minutes later the next look records it.
    sqlx::query("UPDATE members SET joined = joined - 121")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    // The background sweep may hold the throttle for the same step and still be writing, so
    // the explicit sweep can return early: look until the pair shows (the write is what counts).
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while list(&cloud, &a).await.people.is_empty() {
        assert!(
            std::time::Instant::now() < until,
            "the pair was never recorded"
        );
        cloud.state.played_with_at.store(0, Ordering::Relaxed);
        sweep(&cloud).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(list(&cloud, &a).await.people.len(), 1);
}

#[tokio::test]
async fn a_party_pair_has_no_title_and_a_session_names_its_game() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    gather(&cloud, &a, &b).await;
    sweep(&cloud).await;
    let seen = list(&cloud, &a).await;
    assert_eq!(seen.people.len(), 1);
    assert!(seen.people[0].title_id.is_none() && seen.people[0].title.is_none());
    assert!(seen.people[0].friend);

    // The same two in a session: the entry names the game, whatever the party said.
    play_session(&cloud, &a, &b).await;
    sqlx::query("UPDATE played_with SET last_ms = last_ms - 600000")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    sweep(&cloud).await;
    let seen = list(&cloud, &a).await;
    assert_eq!(seen.people.len(), 1);
    assert_eq!(seen.people[0].title.as_deref(), Some("Test Game"));
}

#[tokio::test]
async fn the_switch_stops_new_entries_and_clearing_empties_only_ones_own_list() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    let set = |on: bool| {
        cloud.put(
            "/v1/me/played-with/remember",
            a.token(),
            Some(json!({"remember_played_with": on})),
        )
    };
    let off: RememberPlayedWith = set(false).await.ok(200).json();
    assert!(!off.remember_played_with);
    let read: RememberPlayedWith = cloud
        .get("/v1/me/played-with/remember", a.token())
        .await
        .ok(200)
        .json();
    assert!(!read.remember_played_with);

    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    let mine = list(&cloud, &a).await;
    assert!(!mine.remember && mine.people.is_empty());
    // Ben still remembers Ann: his switch is his own.
    assert_eq!(list(&cloud, &b).await.people.len(), 1);

    // Clearing is one's own list only.
    cloud.delete("/v1/me/played-with", b.token()).await.ok(204);
    assert!(list(&cloud, &b).await.people.is_empty());
    set(true).await.ok(200);
    sweep(&cloud).await;
    assert_eq!(list(&cloud, &a).await.people.len(), 1);
    // Ben cleared while they were together, so the look does not put Ann back.
    assert_eq!(list(&cloud, &b).await.people.len(), 0);
    // Turning it off keeps the list until the player clears it.
    set(false).await.ok(200);
    assert_eq!(list(&cloud, &a).await.people.len(), 1);
    cloud.delete("/v1/me/played-with", a.token()).await.ok(204);
    assert!(list(&cloud, &a).await.people.is_empty());
}

#[tokio::test]
async fn a_block_hides_the_entry_both_ways_and_unblocking_brings_it_back() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    cloud
        .put(&format!("/v1/blocks/{}", b.id), a.token(), None)
        .await
        .ok(204);
    assert!(list(&cloud, &a).await.people.is_empty());
    assert!(list(&cloud, &b).await.people.is_empty());
    // No new entry is written while the block holds.
    sqlx::query("DELETE FROM played_with")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    sweep(&cloud).await;
    assert_eq!(rows(&cloud).await, 0);
    cloud
        .delete(&format!("/v1/blocks/{}", b.id), a.token())
        .await
        .ok(204);
}

#[tokio::test]
async fn entries_go_with_either_account() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    assert_eq!(rows(&cloud).await, 2);
    cloud.delete_account_now(&b).await;
    assert!(list(&cloud, &a).await.people.is_empty());
    assert_eq!(rows(&cloud).await, 0);
}

#[tokio::test]
async fn an_account_keeps_the_newest_50_of_the_last_30_days() {
    let cloud = quick().await;
    let me = cloud.sign_up("Mina").await;
    let now_ms = anyplay_cloud::util::now_ms();
    let day = 24 * 60 * 60 * 1000;
    // Q00 is the oldest of 53 recent people; two more are past 30 days.
    for n in 0..55_i64 {
        let other = cloud.sign_up(&format!("Q{n:02}")).await;
        let age = if n >= 53 { 31 * day } else { (53 - n) * 1000 };
        sqlx::query("INSERT INTO played_with (account_id, other_id, last_ms) VALUES ($1, $2, $3)")
            .bind(me.id as i64)
            .bind(other.id as i64)
            .bind(now_ms - age)
            .execute(&cloud.state.db)
            .await
            .unwrap();
    }
    // Even before the sweep the answer holds the limits.
    let seen = list(&cloud, &me).await;
    assert_eq!(seen.people.len(), 50);
    assert_eq!(seen.people[0].account.handle, "Q52");
    assert!(seen.people.windows(2).all(|w| w[0].last >= w[1].last));
    sweep(&cloud).await;
    assert_eq!(rows(&cloud).await, 50);
    let after = list(&cloud, &me).await;
    assert!(!after.people.iter().any(|p| p.account.handle == "Q02"));
    assert!(!after.people.iter().any(|p| p.account.handle == "Q54"));
}

#[tokio::test]
async fn the_export_lists_the_entries_and_the_switch() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    let export = cloud.get("/v1/me/export", a.token()).await.ok(200);
    assert_eq!(export.body["remember_played_with"], true);
    assert_eq!(export.body["played_with"][0]["account"]["handle"], "Ben");
    assert_eq!(export.body["played_with"][0]["title"], "Test Game");
}

#[tokio::test]
async fn the_list_needs_a_token() {
    let cloud = quick().await;
    cloud
        .request(Method::GET, "/v1/me/played-with", None, None)
        .await
        .is_error("unauthorized", 401);
    cloud
        .request(Method::DELETE, "/v1/me/played-with", None, None)
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn the_portal_serves_the_same_list_and_switch() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    let portal = Portal::signed_in(&cloud, &a).await;
    let seen = portal.get("/me/played-with").await.ok(200);
    assert_eq!(seen["remember"], true);
    assert_eq!(seen["people"][0]["account"]["handle"], "Ben");
    let off = portal
        .send(
            Method::PUT,
            "/me/played-with/remember",
            json!({"remember_played_with": false}),
        )
        .await
        .ok(200);
    assert_eq!(off["remember_played_with"], false);
    assert_eq!(
        portal.get("/me/played-with/remember").await.ok(200)["remember_played_with"],
        false
    );
    portal.delete("/me/played-with").await.ok(200);
    assert_eq!(
        portal.get("/me/played-with").await.ok(200)["people"],
        json!([])
    );
}

#[tokio::test]
async fn a_pair_that_comes_together_after_clearing_is_remembered_again() {
    let cloud = quick().await;
    let (a, b) = cloud.friends("Ann", "Ben").await;
    let first = play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    cloud.delete("/v1/me/played-with", a.token()).await.ok(204);
    // Still together from before the clearing: not put back.
    sweep(&cloud).await;
    assert!(list(&cloud, &a).await.people.is_empty());
    // A new session later: the clearing is behind them.
    for user in [&a, &b] {
        cloud
            .post_empty(&format!("/v1/sessions/{}/leave", first.id), user.token())
            .await
            .ok(204);
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    play_session(&cloud, &a, &b).await;
    sweep(&cloud).await;
    assert_eq!(list(&cloud, &a).await.people.len(), 1);
}
