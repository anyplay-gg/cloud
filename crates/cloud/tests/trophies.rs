mod common;

use anyplay_api::{
    AccountTrophies, Audience, AvatarSet, Cards, Count, Grade, MAX_CARDS, MAX_TROPHY_NUMBER,
    MAX_TROPHY_UPLOAD, Privacy, RARITY_MIN_PLAYERS, Rarity, TrophiesAdded, TrophyRarity,
};
use common::{Reply, TestCloud, User};
use image::{ImageFormat, Rgba, RgbaImage};
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

async fn upload(cloud: &TestCloud, user: &User, title: &str, trophies: Vec<Value>) -> Reply {
    cloud
        .put(
            &format!("/v1/me/trophies/{title}"),
            user.token(),
            Some(json!({"trophies": trophies})),
        )
        .await
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

/// The status of `viewer` asking for the trophies of `owner`.
async fn status(cloud: &TestCloud, viewer: &User, owner: &User) -> u16 {
    cloud
        .get(
            &format!("/v1/accounts/{}/trophies", owner.id),
            viewer.token(),
        )
        .await
        .status
}

/// The counts on the gamer card of `owner` as `viewer` gets it.
async fn card_of(cloud: &TestCloud, viewer: &User, owner: &User) -> Option<Count> {
    let cards: Cards = cloud
        .get(&format!("/v1/cards?accounts={}", owner.id), viewer.token())
        .await
        .ok(200)
        .json();
    assert_eq!(cards.cards.len(), 1);
    assert_eq!(cards.cards[0].account.id.0, owner.id);
    cards.cards[0].trophies
}

#[tokio::test]
async fn trophies_are_recorded_and_keep_their_first_time_and_grade() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let t = now() - 1000;

    let added: TrophiesAdded = upload(
        &cloud,
        &alec,
        "PPSA01234",
        vec![trophy(0, "bronze", t), trophy(1, "gold", t + 5)],
    )
    .await
    .ok(200)
    .json();
    assert_eq!(added.added, 2);

    // The same trophy again, with another time and grade, changes nothing; a new one counts once
    // even when it comes twice.
    let added: TrophiesAdded = upload(
        &cloud,
        &alec,
        "PPSA01234",
        vec![
            trophy(0, "platinum", t + 99),
            trophy(2, "silver", t + 10),
            trophy(2, "silver", t + 10),
        ],
    )
    .await
    .ok(200)
    .json();
    assert_eq!(added.added, 1);
    upload(&cloud, &alec, "PPSA99999", vec![trophy(7, "platinum", t)])
        .await
        .ok(200);

    let mine: AccountTrophies = cloud
        .get(&format!("/v1/accounts/{}/trophies", alec.id), alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(mine.titles.len(), 2);
    assert_eq!(mine.titles[0].title_id, "PPSA01234");
    let kept: Vec<_> = mine.titles[0]
        .trophies
        .iter()
        .map(|x| (x.number, x.grade, x.earned))
        .collect();
    assert_eq!(
        kept,
        vec![
            (0, Grade::Bronze, t),
            (1, Grade::Gold, t + 5),
            (2, Grade::Silver, t + 10)
        ]
    );
    assert_eq!(mine.titles[1].title_id, "PPSA99999");

    // An empty upload is fine.
    let none: TrophiesAdded = upload(&cloud, &alec, "PPSA01234", vec![])
        .await
        .ok(200)
        .json();
    assert_eq!(none.added, 0);
}

#[tokio::test]
async fn bad_uploads_are_refused_and_record_nothing() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let t = now();
    let one = || vec![trophy(0, "bronze", t)];

    for title in ["a%20b", "a%3Fb", &"x".repeat(65)] {
        upload(&cloud, &alec, title, one())
            .await
            .is_error("bad_request", 400);
    }
    // One bad trophy refuses the whole upload.
    upload(
        &cloud,
        &alec,
        "PPSA01234",
        vec![
            trophy(0, "bronze", t),
            trophy(MAX_TROPHY_NUMBER, "bronze", t),
        ],
    )
    .await
    .is_error("bad_request", 400);
    upload(
        &cloud,
        &alec,
        "PPSA01234",
        vec![json!({"number": 70000, "grade": "gold", "earned": t})],
    )
    .await
    .is_error("bad_request", 400);
    upload(&cloud, &alec, "PPSA01234", vec![trophy(0, "diamond", t)])
        .await
        .is_error("bad_request", 400);
    // A day of slack for clocks, no more.
    upload(
        &cloud,
        &alec,
        "PPSA01234",
        vec![trophy(0, "bronze", t + 2 * 86_400)],
    )
    .await
    .is_error("bad_request", 400);
    let many: Vec<_> = (0..=MAX_TROPHY_UPLOAD as u16)
        .map(|n| trophy(n, "bronze", t))
        .collect();
    upload(&cloud, &alec, "PPSA01234", many)
        .await
        .is_error("too_large", 413);

    let mine: AccountTrophies = cloud
        .get(&format!("/v1/accounts/{}/trophies", alec.id), alec.token())
        .await
        .ok(200)
        .json();
    assert!(mine.titles.is_empty());

    // The most an upload may carry, and a time a few hours ahead, pass.
    let most: Vec<_> = (0..MAX_TROPHY_UPLOAD as u16)
        .map(|n| trophy(n, "bronze", t + 3600))
        .collect();
    let added: TrophiesAdded = upload(&cloud, &alec, "PPSA01234", most)
        .await
        .ok(200)
        .json();
    assert_eq!(added.added as usize, MAX_TROPHY_UPLOAD);

    // Without a token there is nothing to upload to.
    cloud
        .request(
            reqwest::Method::PUT,
            "/v1/me/trophies/PPSA01234",
            None,
            Some(json!({"trophies": []})),
        )
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn privacy_defaults_to_friends_and_is_kept() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let p: Privacy = cloud
        .get("/v1/me/privacy", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(p.trophies, Audience::Friends);
    set_audience(&cloud, &alec, "nobody").await;
    set_audience(&cloud, &alec, "everyone").await;
    let p: Privacy = cloud
        .get("/v1/me/privacy", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(p.trophies, Audience::Everyone);
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "public"})),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn who_sees_trophies_follows_the_audience_and_blocks() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let stranger = cloud.sign_up("Stranger").await;
    let t = now();
    upload(
        &cloud,
        &alec,
        "PPSA01234",
        vec![
            trophy(0, "bronze", t),
            trophy(1, "bronze", t),
            trophy(2, "platinum", t),
        ],
    )
    .await
    .ok(200);
    let counts = Count {
        bronze: 2,
        platinum: 1,
        ..Count::default()
    };

    // The default: the account itself and its friends.
    assert_eq!(status(&cloud, &alec, &alec).await, 200);
    assert_eq!(status(&cloud, &sam, &alec).await, 200);
    assert_eq!(status(&cloud, &stranger, &alec).await, 404);
    assert_eq!(card_of(&cloud, &sam, &alec).await, Some(counts));
    assert_eq!(card_of(&cloud, &stranger, &alec).await, None);

    set_audience(&cloud, &alec, "everyone").await;
    assert_eq!(status(&cloud, &stranger, &alec).await, 200);
    assert_eq!(card_of(&cloud, &stranger, &alec).await, Some(counts));

    set_audience(&cloud, &alec, "nobody").await;
    assert_eq!(
        status(&cloud, &alec, &alec).await,
        200,
        "the account always"
    );
    assert_eq!(card_of(&cloud, &alec, &alec).await, Some(counts));
    assert_eq!(status(&cloud, &sam, &alec).await, 404);
    assert_eq!(status(&cloud, &stranger, &alec).await, 404);
    assert_eq!(card_of(&cloud, &sam, &alec).await, None);

    // A block hides them in either direction, even for everyone.
    set_audience(&cloud, &alec, "everyone").await;
    cloud
        .put(&format!("/v1/blocks/{}", stranger.id), alec.token(), None)
        .await
        .ok(204);
    assert_eq!(status(&cloud, &stranger, &alec).await, 404);
    assert_eq!(card_of(&cloud, &stranger, &alec).await, None);
    set_audience(&cloud, &sam, "everyone").await;
    cloud
        .put(&format!("/v1/blocks/{}", alec.id), sam.token(), None)
        .await
        .ok(204);
    assert_eq!(status(&cloud, &alec, &sam).await, 404);
    assert_eq!(status(&cloud, &sam, &alec).await, 404);
    assert_eq!(status(&cloud, &alec, &alec).await, 200);

    // An account that does not exist looks like one that hides.
    cloud
        .get("/v1/accounts/999999/trophies", alec.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn rarity_is_given_from_ten_players() {
    let cloud = TestCloud::start().await;
    let t = now();
    let mut players = Vec::new();
    for n in 0..RARITY_MIN_PLAYERS {
        players.push(cloud.sign_up(&format!("Player{n}")).await);
    }
    let asker = players[0].token().to_owned();
    let rarity = async |title: &str| -> Rarity {
        cloud
            .get(&format!("/v1/titles/{title}/rarity"), &asker)
            .await
            .ok(200)
            .json()
    };

    // Nine players: everyone has trophy 0, the first three also trophy 1. Nothing is told, not
    // even how many players there are.
    for (n, player) in players.iter().take(9).enumerate() {
        let mut list = vec![trophy(0, "bronze", t)];
        if n < 3 {
            list.push(trophy(1, "gold", t));
        }
        upload(&cloud, player, "PPSA01234", list).await.ok(200);
    }
    let r = rarity("PPSA01234").await;
    assert_eq!(r.title_id, "PPSA01234");
    assert_eq!((r.players, r.trophies.len()), (0, 0));

    upload(
        &cloud,
        &players[9],
        "PPSA01234",
        vec![trophy(0, "bronze", t)],
    )
    .await
    .ok(200);
    let r = rarity("PPSA01234").await;
    assert_eq!(r.players, 10);
    assert_eq!(
        r.trophies,
        vec![
            TrophyRarity {
                number: 0,
                earned_by: 10
            },
            TrophyRarity {
                number: 1,
                earned_by: 3
            }
        ]
    );

    // Other titles count on their own, and a bad title ID is refused.
    let r = rarity("PPSA00000").await;
    assert_eq!((r.players, r.trophies.len()), (0, 0));
    cloud
        .get("/v1/titles/a%20b/rarity", &asker)
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn cards_are_limited_and_leave_out_unknown_accounts() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let stranger = cloud.sign_up("Stranger").await;
    let t = now();
    upload(&cloud, &sam, "PPSA01234", vec![trophy(0, "gold", t)])
        .await
        .ok(200);

    let ids = format!("{},{},987654,{},{}", sam.id, stranger.id, sam.id, alec.id);
    let cards: Cards = cloud
        .get(&format!("/v1/cards?accounts={ids}"), alec.token())
        .await
        .ok(200)
        .json();
    let seen: Vec<_> = cards
        .cards
        .iter()
        .map(|c| (c.account.handle.as_str(), c.trophies.map(|c| c.gold)))
        .collect();
    assert_eq!(
        seen.len(),
        3,
        "each account once, the unknown one left out: {seen:?}"
    );
    for (handle, gold) in [("Alec", Some(0)), ("Sam", Some(1)), ("Stranger", None)] {
        assert!(seen.contains(&(handle, gold)), "{handle}: {seen:?}");
    }

    let none: Cards = cloud
        .get("/v1/cards?accounts=", alec.token())
        .await
        .ok(200)
        .json();
    assert!(none.cards.is_empty());
    cloud
        .get("/v1/cards", alec.token())
        .await
        .is_error("bad_request", 400);
    cloud
        .get("/v1/cards?accounts=1,x", alec.token())
        .await
        .is_error("bad_request", 400);
    let list = |n: usize| {
        (1..=n as u64)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    cloud
        .get(
            &format!("/v1/cards?accounts={}", list(MAX_CARDS + 1)),
            alec.token(),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .get(
            &format!("/v1/cards?accounts={}", list(MAX_CARDS)),
            alec.token(),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn export_holds_the_records_and_deletion_removes_them() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let t = now() - 10;
    upload(&cloud, &alec, "PPSA01234", vec![trophy(3, "silver", t)])
        .await
        .ok(200);
    set_audience(&cloud, &alec, "nobody").await;

    let export = cloud.get("/v1/me/export", alec.token()).await.ok(200).body;
    assert_eq!(
        export["trophies"],
        json!([{"title_id": "PPSA01234", "number": 3, "grade": "silver", "earned": t}])
    );
    assert_eq!(
        export["privacy"],
        json!({"trophies": "nobody", "about": "friends", "favourites": "friends",
               "avatar": "friends", "playtime": "friends", "feed": "friends"})
    );

    cloud.delete_account_now(&alec).await;
    let db = &cloud.state.db;
    assert_eq!(
        anyplay_cloud::db::title_players(db, "PPSA01234")
            .await
            .unwrap(),
        0
    );
    let (settings,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM privacy")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(settings, 0);
}

/// Sets who sees the trophies and the avatar of `user`.
async fn set_parts(cloud: &TestCloud, user: &User, trophies: &str, avatar: &str) {
    cloud
        .put(
            "/v1/me/privacy",
            user.token(),
            Some(json!({"trophies": trophies, "avatar": avatar})),
        )
        .await
        .ok(200);
}

/// Gives `user` a picture of its own colour and returns its hash.
async fn give_avatar(cloud: &TestCloud, user: &User, shade: u8) -> String {
    let picture = RgbaImage::from_pixel(32, 32, Rgba([shade, 40, 200, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    let set: AvatarSet = cloud
        .http
        .put(format!("http://{}/v1/me/avatar", cloud.base))
        .bearer_auth(user.token())
        .header("content-type", "image/png")
        .body(bytes)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    set.avatar
}

#[tokio::test]
async fn one_call_for_a_mix_of_accounts_gives_each_its_own_card() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let friend_hidden = cloud.sign_up("Hidden").await;
    let stranger = cloud.sign_up("Stranger").await;
    let shy = cloud.sign_up("Shy").await;
    let blocker = cloud.sign_up("Blocker").await;
    let blocked = cloud.sign_up("Blocked").await;
    let bare = cloud.sign_up("Bare").await;
    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": friend_hidden.handle}),
        )
        .await
        .ok(200);
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", alec.id),
            friend_hidden.token(),
        )
        .await
        .ok(204);
    let t = now();
    let grades = |n: &[(&str, u16)]| -> Vec<Value> {
        let mut number = 0;
        let mut out = Vec::new();
        for (grade, count) in n {
            for _ in 0..*count {
                out.push(trophy(number, grade, t));
                number += 1;
            }
        }
        out
    };
    // Different counts for everyone, so a mix-up between accounts shows.
    for (user, list) in [
        (&alec, grades(&[("platinum", 1)])),
        (&sam, grades(&[("bronze", 3), ("gold", 1)])),
        (&friend_hidden, grades(&[("silver", 4)])),
        (
            &stranger,
            grades(&[("silver", 2), ("gold", 2), ("bronze", 1)]),
        ),
        (&shy, grades(&[("bronze", 5)])),
        (&blocker, grades(&[("gold", 6)])),
        (&blocked, grades(&[("gold", 7)])),
    ] {
        upload(&cloud, user, "PPSA01234", list).await.ok(200);
    }
    set_parts(&cloud, &alec, "nobody", "nobody").await;
    // Sam keeps the default: friends.
    set_parts(&cloud, &friend_hidden, "nobody", "friends").await;
    set_parts(&cloud, &stranger, "everyone", "everyone").await;
    // Shy keeps the default too, and Bare has no trophies and no avatar but shows everything.
    set_parts(&cloud, &blocker, "everyone", "everyone").await;
    set_parts(&cloud, &blocked, "everyone", "everyone").await;
    set_parts(&cloud, &bare, "everyone", "everyone").await;
    let mut hashes = std::collections::HashMap::new();
    for (shade, user) in [
        &alec,
        &sam,
        &friend_hidden,
        &stranger,
        &shy,
        &blocker,
        &blocked,
    ]
    .into_iter()
    .enumerate()
    {
        hashes.insert(user.id, give_avatar(&cloud, user, shade as u8 * 30).await);
    }
    assert_eq!(
        hashes
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        hashes.len(),
        "the pictures differ"
    );
    cloud
        .put(&format!("/v1/blocks/{}", blocked.id), alec.token(), None)
        .await
        .ok(204);
    cloud
        .put(&format!("/v1/blocks/{}", alec.id), blocker.token(), None)
        .await
        .ok(204);

    let missing = 987_654;
    let ids = [
        alec.id,
        sam.id,
        friend_hidden.id,
        stranger.id,
        shy.id,
        blocker.id,
        blocked.id,
        bare.id,
        missing,
    ];
    let list: Vec<String> = ids.iter().rev().map(u64::to_string).collect();
    let cards: Cards = cloud
        .get(
            &format!("/v1/cards?accounts={}", list.join(",")),
            alec.token(),
        )
        .await
        .ok(200)
        .json();

    let count = |bronze, silver, gold, platinum| {
        Some(Count {
            bronze,
            silver,
            gold,
            platinum,
        })
    };
    let hash = |user: &User| Some(hashes[&user.id].clone());
    let expected = [
        // Own beats the privacy and the default.
        (&alec, count(0, 0, 0, 1), hash(&alec)),
        // A friend shows what is not for nobody.
        (&sam, count(3, 0, 1, 0), hash(&sam)),
        (&friend_hidden, None, hash(&friend_hidden)),
        (&stranger, count(1, 2, 2, 0), hash(&stranger)),
        // A stranger sees nothing of the default.
        (&shy, None, None),
        // Blocked in either direction: nothing.
        (&blocker, None, None),
        (&blocked, None, None),
        // Nothing to show is still an answer, not a missing part.
        (&bare, count(0, 0, 0, 0), None),
    ];
    let mut sorted: Vec<_> = expected.iter().collect();
    sorted.sort_by_key(|(user, _, _)| user.id);
    let got: Vec<_> = cards
        .cards
        .iter()
        .map(|c| {
            (
                c.account.id.0,
                c.account.handle.clone(),
                c.trophies,
                c.avatar.clone(),
            )
        })
        .collect();
    let want: Vec<_> = sorted
        .iter()
        .map(|(user, trophies, avatar)| (user.id, user.handle.clone(), *trophies, avatar.clone()))
        .collect();
    assert_eq!(got, want);
}
