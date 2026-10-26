//! Comparing trophies, trophy points and levels, and secret trophies (network ADR 0029).

mod common;

use anyplay_api::{Count, Grade, ProfilePage, TrophyCompare, TrophyLevel};
use common::portal::Portal;
use common::{Reply, TestCloud, User};
use serde_json::{Value, json};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn trophy(number: u16, grade: &str, earned: u64) -> Value {
    json!({"number": number, "grade": grade, "earned": earned})
}

async fn upload(cloud: &TestCloud, user: &User, title: &str, trophies: Vec<Value>) {
    cloud
        .put(
            &format!("/v1/me/trophies/{title}"),
            user.token(),
            Some(json!({"trophies": trophies})),
        )
        .await
        .ok(200);
}

async fn secret(cloud: &TestCloud, user: &User, what: &str, title: &str, numbers: &[u16]) -> Value {
    cloud
        .put(
            &format!("/v1/me/trophies/{title}/{what}"),
            user.token(),
            Some(json!({"numbers": numbers})),
        )
        .await
        .ok(200)
        .body
}

async fn compare_reply(cloud: &TestCloud, viewer: &User, other: &User, title: &str) -> Reply {
    cloud
        .get(
            &format!("/v1/accounts/{}/compare/{title}", other.id),
            viewer.token(),
        )
        .await
}

async fn compare(cloud: &TestCloud, viewer: &User, other: &User, title: &str) -> TrophyCompare {
    compare_reply(cloud, viewer, other, title)
        .await
        .ok(200)
        .json()
}

fn numbers(side: &anyplay_api::CompareSide) -> Vec<u16> {
    side.trophies.iter().map(|t| t.number).collect()
}

async fn profile(cloud: &TestCloud, viewer: &User, owner: &User) -> ProfilePage {
    cloud
        .get(
            &format!("/v1/accounts/{}/profile", owner.id),
            viewer.token(),
        )
        .await
        .ok(200)
        .json()
}

async fn set_audience(cloud: &TestCloud, user: &User, audience: &str) {
    cloud
        .put(
            "/v1/me/privacy",
            user.token(),
            Some(json!({"trophies": audience})),
        )
        .await
        .ok(200);
}

/// Alec has trophies in two games, Sam in one of them.
async fn two_players(cloud: &TestCloud) -> (User, User, u64) {
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let t = now() - 5000;
    upload(
        cloud,
        &alec,
        "GAME00001",
        vec![
            trophy(0, "bronze", t),
            trophy(1, "gold", t + 10),
            trophy(2, "platinum", t + 20),
        ],
    )
    .await;
    upload(
        cloud,
        &alec,
        "GAME00002",
        vec![trophy(4, "silver", t + 30), trophy(5, "silver", t + 31)],
    )
    .await;
    upload(
        cloud,
        &sam,
        "GAME00001",
        vec![trophy(0, "bronze", t + 100), trophy(3, "silver", t + 110)],
    )
    .await;
    (alec, sam, t)
}

#[tokio::test]
async fn two_players_are_compared_in_one_game_with_totals_and_levels() {
    let cloud = TestCloud::start().await;
    let (alec, sam, t) = two_players(&cloud).await;

    let seen = compare(&cloud, &alec, &sam, "GAME00001").await;
    assert_eq!(seen.title_id, "GAME00001");
    assert_eq!(seen.you.account, alec.id);
    assert_eq!(seen.them.account, sam.id);
    assert_eq!(numbers(&seen.you), vec![0, 1, 2]);
    assert_eq!(numbers(&seen.them), vec![0, 3]);
    assert_eq!(seen.you.trophies[1].grade, Grade::Gold);
    assert_eq!(seen.you.trophies[1].earned, t + 10);
    assert_eq!(seen.them.trophies[0].earned, t + 100);
    // Alec: bronze 10 + gold 60 + platinum 150 in the game, plus two silver (50) in all.
    assert_eq!(
        seen.you.title.counts,
        Count {
            bronze: 1,
            silver: 0,
            gold: 1,
            platinum: 1
        }
    );
    assert_eq!(seen.you.title.points, 220);
    assert_eq!(seen.you.total.points, 270);
    assert_eq!(seen.you.total.counts.silver, 2);
    assert_eq!(seen.you.level, TrophyLevel::of(270));
    assert_eq!((seen.you.level.level, seen.you.level.to), (4, Some(360)));
    // Sam: bronze 10 + silver 25.
    assert_eq!(seen.them.title.points, 35);
    assert_eq!(seen.them.total.points, 35);
    assert_eq!(seen.them.level.level, 1);
    assert_eq!((seen.them.level.from, seen.them.level.to), (0, Some(72)));

    // The other way round, "you" is the caller.
    let back = compare(&cloud, &sam, &alec, "GAME00001").await;
    assert_eq!(back.you, seen.them);
    assert_eq!(back.them, seen.you);

    // A game neither has: empty lists and zero counts, not an error.
    let none = compare(&cloud, &alec, &sam, "GAME00009").await;
    assert!(none.you.trophies.is_empty() && none.them.trophies.is_empty());
    assert_eq!(none.you.title.points, 0);
    assert_eq!(none.you.total.points, 270);
    // A game only Alec has.
    let only = compare(&cloud, &alec, &sam, "GAME00002").await;
    assert_eq!(numbers(&only.you), vec![4, 5]);
    assert!(only.them.trophies.is_empty());
}

#[tokio::test]
async fn comparing_follows_the_trophy_privacy_and_blocks() {
    let cloud = TestCloud::start().await;
    let (alec, sam, _) = two_players(&cloud).await;
    let stranger = cloud.sign_up("Stranger").await;

    // Friends only by default.
    compare_reply(&cloud, &stranger, &sam, "GAME00001")
        .await
        .is_error("not_found", 404);
    compare_reply(&cloud, &alec, &sam, "GAME00001")
        .await
        .ok(200);

    set_audience(&cloud, &sam, "nobody").await;
    compare_reply(&cloud, &alec, &sam, "GAME00001")
        .await
        .is_error("not_found", 404);
    // The hidden side does not stop the owner from comparing in the other direction only if the
    // other may be seen: Alec still shows his trophies to friends.
    compare_reply(&cloud, &sam, &alec, "GAME00001")
        .await
        .ok(200);

    set_audience(&cloud, &sam, "everyone").await;
    compare_reply(&cloud, &stranger, &sam, "GAME00001")
        .await
        .ok(200);

    // A block in either direction hides them.
    cloud
        .put(&format!("/v1/blocks/{}", stranger.id), sam.token(), None)
        .await
        .ok(204);
    compare_reply(&cloud, &stranger, &sam, "GAME00001")
        .await
        .is_error("not_found", 404);

    // Not with oneself, not with an account that is not there, not with a bad title.
    compare_reply(&cloud, &alec, &alec, "GAME00001")
        .await
        .is_error("bad_request", 400);
    cloud
        .get("/v1/accounts/999999/compare/GAME00001", alec.token())
        .await
        .is_error("not_found", 404);
    cloud
        .get(
            &format!("/v1/accounts/{}/compare/a%20b", sam.id),
            alec.token(),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .get(&format!("/v1/accounts/{}/compare/GAME00001", sam.id), "")
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn the_profile_shows_points_and_level_when_the_trophies_are_shown() {
    let cloud = TestCloud::start().await;
    let (alec, sam, _) = two_players(&cloud).await;

    let page = profile(&cloud, &sam, &alec).await;
    assert_eq!(page.trophies.unwrap().platinum, 1);
    let level = page.level.unwrap();
    assert_eq!((level.points, level.level, level.from), (270, 4, 252));
    assert_eq!(level.to, Some(360));
    // Your own page shows it too.
    assert_eq!(profile(&cloud, &alec, &alec).await.level, Some(level));

    // Hidden trophies, hidden points: the page says nothing about either.
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "nobody"})),
        )
        .await
        .ok(200);
    let page = profile(&cloud, &sam, &alec).await;
    assert!(page.trophies.is_none() && page.level.is_none());
    let raw = cloud
        .get(&format!("/v1/accounts/{}/profile", alec.id), sam.token())
        .await
        .ok(200)
        .body;
    assert!(raw.get("level").is_none(), "{raw}");
}

#[tokio::test]
async fn a_secret_trophy_stays_hidden_from_others_until_it_is_revealed() {
    let cloud = TestCloud::start().await;
    let (alec, sam, t) = two_players(&cloud).await;

    // Sam's number 3 is secret.
    assert_eq!(
        secret(&cloud, &sam, "secret", "GAME00001", &[3, 3]).await["changed"],
        1
    );
    assert_eq!(
        secret(&cloud, &sam, "secret", "GAME00001", &[3]).await["changed"],
        0
    );

    let seen = compare(&cloud, &alec, &sam, "GAME00001").await;
    assert_eq!(numbers(&seen.them), vec![0]);
    assert_eq!(seen.them.title.points, 10);
    assert_eq!(seen.them.total.points, 10);
    assert_eq!(seen.them.total.counts.silver, 0);
    // Sam sees all of his own.
    let own = compare(&cloud, &sam, &alec, "GAME00001").await;
    assert_eq!(numbers(&own.you), vec![0, 3]);
    assert_eq!(own.you.total.points, 35);

    // The same on the profile, the gamer card, the list and the account's own export.
    let page = profile(&cloud, &alec, &sam).await;
    assert_eq!(page.trophies.unwrap().silver, 0);
    assert_eq!(page.level.unwrap().points, 10);
    assert_eq!(profile(&cloud, &sam, &sam).await.level.unwrap().points, 35);
    let cards = cloud
        .get(&format!("/v1/cards?accounts={}", sam.id), alec.token())
        .await
        .ok(200)
        .body;
    assert_eq!(cards["cards"][0]["trophies"]["silver"], 0);
    let list = cloud
        .get(&format!("/v1/accounts/{}/trophies", sam.id), alec.token())
        .await
        .ok(200)
        .body;
    assert_eq!(list["titles"][0]["trophies"].as_array().unwrap().len(), 1);
    let export = cloud.get("/v1/me/export", sam.token()).await.ok(200).body;
    assert_eq!(export["secret_trophies"][0]["number"], 3);
    assert_eq!(export["secret_trophies"][0]["revealed"], false);

    // Revealing is the owner's choice, and shows the record as it was.
    assert_eq!(
        secret(&cloud, &sam, "reveal", "GAME00001", &[3, 7]).await["changed"],
        1
    );
    let seen = compare(&cloud, &alec, &sam, "GAME00001").await;
    assert_eq!(numbers(&seen.them), vec![0, 3]);
    assert_eq!(seen.them.trophies[1].earned, t + 110);
    assert_eq!(seen.them.total.points, 35);
    // Marking it again does not hide it again.
    assert_eq!(
        secret(&cloud, &sam, "secret", "GAME00001", &[3]).await["changed"],
        0
    );
    assert_eq!(
        numbers(&compare(&cloud, &alec, &sam, "GAME00001").await.them),
        vec![0, 3]
    );
}

#[tokio::test]
async fn a_secret_marked_before_the_upload_hides_the_record_and_makes_no_feed_item() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let t = now();
    secret(&cloud, &sam, "secret", "GAME00003", &[1]).await;
    upload(
        &cloud,
        &sam,
        "GAME00003",
        vec![trophy(1, "gold", t), trophy(2, "bronze", t)],
    )
    .await;

    let seen = compare(&cloud, &alec, &sam, "GAME00003").await;
    assert_eq!(numbers(&seen.them), vec![2]);
    let feed = cloud.get("/v1/feed", alec.token()).await.ok(200).body;
    let items: Vec<(u64, u64)> = feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "trophy")
        .map(|item| {
            (
                item["account"]["id"].as_u64().unwrap(),
                item["number"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(items, vec![(sam.id, 2)], "{feed}");
}

#[tokio::test]
async fn marking_and_revealing_check_their_numbers() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    for what in ["secret", "reveal"] {
        cloud
            .put(
                &format!("/v1/me/trophies/GAME00001/{what}"),
                alec.token(),
                Some(json!({"numbers": [2048]})),
            )
            .await
            .is_error("bad_request", 400);
        cloud
            .put(
                &format!("/v1/me/trophies/GAME00001/{what}"),
                alec.token(),
                Some(json!({"numbers": vec![1; 513]})),
            )
            .await
            .is_error("too_large", 413);
        cloud
            .put(
                &format!("/v1/me/trophies/a%20b/{what}"),
                alec.token(),
                Some(json!({"numbers": [1]})),
            )
            .await
            .is_error("bad_request", 400);
    }
}

#[tokio::test]
async fn the_portal_compares_and_shows_points_and_levels() {
    let cloud = TestCloud::start().await;
    let (alec, sam, _) = two_players(&cloud).await;
    let a = Portal::signed_in(&cloud, &alec).await;
    let out = Portal::new(&cloud);

    let answer = a.get("/users/sam/compare/GAME00001").await.ok(200);
    assert_eq!(answer["title_id"], "GAME00001");
    assert_eq!(answer["you"]["account"], alec.id);
    assert_eq!(answer["them"]["trophies"].as_array().unwrap().len(), 2);
    assert_eq!(answer["you"]["total"]["points"], 270);
    assert_eq!(answer["you"]["level"]["level"], 4);
    assert_eq!(answer["them"]["level"]["to"], 72);
    // Names, texts and art are not there: numbers, grades and times only.
    let first = answer["you"]["trophies"][0].as_object().unwrap();
    let mut keys: Vec<_> = first.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["earned", "grade", "number"]);

    a.get("/users/alec/compare/GAME00001")
        .await
        .error(400, "bad_request");
    a.get("/users/nobody/compare/GAME00001")
        .await
        .error(404, "not_found");
    out.get("/users/sam/compare/GAME00001")
        .await
        .error(401, "unauthorized");

    let mine = a.get("/me/trophies").await.ok(200);
    assert_eq!(mine["points"], 270);
    assert_eq!(mine["level"]["level"], 4);
    assert_eq!(mine["level"]["from"], 252);
    let user = a.get("/users/sam").await.ok(200);
    assert_eq!(user["level"]["points"], 35);

    // The portal hides a secret trophy like the API does.
    secret(&cloud, &sam, "secret", "GAME00001", &[3]).await;
    let user = a.get("/users/sam").await.ok(200);
    assert_eq!(user["level"]["points"], 10);
    assert_eq!(user["trophies"]["silver"], 0);
    let theirs = a.get("/users/sam/trophies").await.ok(200);
    assert_eq!(theirs["points"], 10);
    // A visitor who is not signed in gets no page while Sam keeps it off, and once it is on
    // the trophies Sam shows to friends show on it.
    out.get("/users/sam").await.error(404, "not_found");
    cloud.show_page(&sam).await;
    let page = out.get("/users/sam").await.ok(200);
    assert_eq!(page["level"]["points"], 10);
}
