mod common;

use std::time::Duration;

use anyplay_api::{
    Event, Feed, FeedEvent, FeedItem, Grade, Privacy, ProfilePage, Reaction, ReactionCounts,
    Reactions,
};
use anyplay_cloud::db::feed::{NewItem, record};
use common::{Reply, TestCloud, User};
use serde_json::{Value, json};
use sqlx::{Connection as _, Executor as _};

const GAME: &str = "GAME00001";
const OTHER: &str = "GAME00002";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
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

async fn feed_at(cloud: &TestCloud, user: &User, query: &str) -> Feed {
    cloud
        .get(&format!("/v1/feed{query}"), user.token())
        .await
        .ok(200)
        .json()
}

async fn feed(cloud: &TestCloud, user: &User) -> Feed {
    feed_at(cloud, user, "").await
}

/// The titles of the `first_played` items of a feed, newest first.
fn played(feed: &Feed) -> Vec<String> {
    feed.items
        .iter()
        .filter_map(|item| match &item.event {
            FeedEvent::FirstPlayed { title_id } => Some(title_id.clone()),
            _ => None,
        })
        .collect()
}

fn of_kind<'a>(feed: &'a Feed, kind: &str) -> Vec<&'a FeedItem> {
    let json = serde_json::to_value(feed).unwrap();
    feed.items
        .iter()
        .zip(json["items"].as_array().unwrap())
        .filter(|(_, json)| json["kind"] == kind)
        .map(|(item, _)| item)
        .collect()
}

/// Writes an item for `owner` without going through the hooks.
async fn put_item(cloud: &TestCloud, owner: &User, item: NewItem) {
    let recorded = record(
        &cloud.state.writer,
        &cloud.state.db,
        now(),
        10_000,
        &[(owner.id, item)],
    )
    .await
    .unwrap();
    assert_eq!(recorded, 1);
}

async fn played_item(cloud: &TestCloud, owner: &User, title: &str) -> u64 {
    put_item(
        cloud,
        owner,
        NewItem::FirstPlayed {
            title_id: title.into(),
        },
    )
    .await;
    let feed = feed(cloud, owner).await;
    feed.items
        .iter()
        .find(
            |item| matches!(&item.event, FeedEvent::FirstPlayed { title_id } if title_id == title),
        )
        .expect("the item is in its owner's feed")
        .id
}

async fn set_feed(cloud: &TestCloud, user: &User, audience: &str) {
    cloud
        .put(
            "/v1/me/privacy",
            user.token(),
            Some(json!({"feed": audience})),
        )
        .await
        .ok(200);
}

async fn react(cloud: &TestCloud, user: &User, item: u64, reaction: &str) -> Reply {
    cloud
        .put(
            &format!("/v1/feed/{item}/reaction"),
            user.token(),
            Some(json!({"reaction": reaction})),
        )
        .await
}

async fn unreact(cloud: &TestCloud, user: &User, item: u64) -> Reply {
    cloud
        .delete(&format!("/v1/feed/{item}/reaction"), user.token())
        .await
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

fn activity(title: &str) -> Value {
    json!({"activity": {"title_id": title, "title": "A game", "version": "1.0"}})
}

async fn play(cloud: &TestCloud, user: &User, title: &str) {
    cloud
        .put("/v1/presence", user.token(), Some(activity(title)))
        .await
        .ok(204);
}

async fn count(cloud: &TestCloud, sql: &str, id: u64) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id as i64)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    n
}

// Privacy and the migration

#[tokio::test]
async fn the_feed_visibility_starts_at_friends_and_is_kept() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let privacy: Privacy = cloud
        .get("/v1/me/privacy", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(privacy.feed, anyplay_api::Audience::Friends);
    // A row written without the part keeps the default.
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "nobody"})),
        )
        .await
        .ok(200);
    let privacy: Value = cloud.get("/v1/me/privacy", alec.token()).await.ok(200).body;
    assert_eq!(privacy["feed"], "friends");
    assert_eq!(privacy["trophies"], "nobody");
    set_feed(&cloud, &alec, "nobody").await;
    let privacy: Privacy = cloud
        .get("/v1/me/privacy", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(privacy.feed, anyplay_api::Audience::Nobody);
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"feed": "public"})),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn the_migration_gives_existing_rows_the_default_and_remembers_played_titles() {
    sqlx::any::install_default_drivers();
    let dir = tempfile::tempdir().unwrap();
    let (url, folder) = match common::postgres_database().await {
        Some(url) => (url, "postgres"),
        None => {
            let file = dir.path().join("old.db");
            let path = file.to_string_lossy().replace('\\', "/");
            (format!("sqlite:{path}?mode=rwc"), "sqlite")
        }
    };
    let mut files: Vec<_> = std::fs::read_dir(format!(
        "{}/migrations/{folder}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .collect();
    files.sort();
    let mut conn = sqlx::AnyConnection::connect(&url).await.unwrap();
    let (before, from) = files.split_at(
        files
            .iter()
            .position(|f| f.file_name().unwrap().to_string_lossy().starts_with("0017"))
            .expect("the feed migration exists"),
    );
    for file in before {
        let sql = std::fs::read_to_string(file).unwrap();
        conn.execute(sqlx::AssertSqlSafe(sql)).await.unwrap();
    }
    for sql in [
        "INSERT INTO accounts (id, handle, handle_key, email, created) \
         VALUES (1, 'Alec', 'alec', 'alec@example.com', 1)",
        "INSERT INTO privacy (account_id, trophies, about, favourites, avatar, playtime) \
         VALUES (1, 'everyone', 'nobody', 'friends', 'friends', 'friends')",
        "INSERT INTO trophies (account_id, title_id, number, grade, earned) \
         VALUES (1, 'GAME00001', 1, 'gold', 5)",
        "INSERT INTO playtime (account_id, title_id, day, minutes) \
         VALUES (1, 'GAME00001', '2026-01-01', 5)",
        "INSERT INTO playtime (account_id, title_id, day, minutes) \
         VALUES (1, 'GAME00002', '2026-01-01', 5)",
    ] {
        conn.execute(sql).await.unwrap();
    }
    for file in from {
        let sql = std::fs::read_to_string(file).unwrap();
        conn.execute(sqlx::AssertSqlSafe(sql)).await.unwrap();
    }
    let row: (String, String, String) =
        sqlx::query_as("SELECT trophies, about, feed FROM privacy WHERE account_id = 1")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(row, ("everyone".into(), "nobody".into(), "friends".into()));
    let titles: Vec<(String,)> =
        sqlx::query_as("SELECT title_id FROM feed_firsts WHERE account_id = 1 ORDER BY title_id")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert_eq!(
        titles,
        [("GAME00001".to_owned(),), ("GAME00002".to_owned(),)]
    );
}

// Who sees what

#[tokio::test]
async fn a_feed_holds_own_and_friends_items_and_nobody_elses() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let quiet = cloud.sign_up("Quiet").await;
    let stranger = cloud.sign_up("Stranger").await;
    befriend(&cloud, &alec, &sam).await;
    befriend(&cloud, &alec, &quiet).await;
    played_item(&cloud, &alec, "GAME00010").await;
    played_item(&cloud, &sam, "GAME00011").await;
    played_item(&cloud, &quiet, "GAME00012").await;
    played_item(&cloud, &stranger, "GAME00013").await;

    let seen = played(&feed(&cloud, &alec).await);
    assert_eq!(
        seen,
        ["GAME00012", "GAME00011", "GAME00010"],
        "newest first"
    );
    // Sam's friends are Alec only.
    assert_eq!(
        played(&feed(&cloud, &sam).await),
        ["GAME00011", "GAME00010"]
    );
    assert_eq!(played(&feed(&cloud, &stranger).await), ["GAME00013"]);

    // `nobody` hides every item of the account from others, the old ones too, and the owner
    // still sees them (and the friends' items as before).
    set_feed(&cloud, &quiet, "nobody").await;
    assert_eq!(
        played(&feed(&cloud, &alec).await),
        ["GAME00011", "GAME00010"]
    );
    assert_eq!(
        played(&feed(&cloud, &quiet).await),
        ["GAME00012", "GAME00010"]
    );
    // `everyone` shows what `friends` does: a stranger sees no feed.
    set_feed(&cloud, &sam, "everyone").await;
    assert_eq!(played(&feed(&cloud, &stranger).await), ["GAME00013"]);
    assert_eq!(
        played(&feed(&cloud, &alec).await),
        ["GAME00011", "GAME00010"]
    );
}

#[tokio::test]
async fn a_block_hides_items_in_both_directions() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let alec_item = played_item(&cloud, &alec, GAME).await;
    let sam_item = played_item(&cloud, &sam, OTHER).await;
    assert_eq!(played(&feed(&cloud, &alec).await), [OTHER, GAME]);

    cloud
        .put(&format!("/v1/blocks/{}", sam.id), alec.token(), None)
        .await
        .ok(204);
    // Blocking ends the friendship: each keeps only their own items.
    assert_eq!(played(&feed(&cloud, &alec).await), [GAME]);
    assert_eq!(played(&feed(&cloud, &sam).await), [OTHER]);
    react(&cloud, &alec, sam_item, "gg")
        .await
        .is_error("not_found", 404);
    react(&cloud, &sam, alec_item, "gg")
        .await
        .is_error("not_found", 404);
    // Even with the friendship back (written directly), the block holds.
    sqlx::query("INSERT INTO friends (low, high, since) VALUES ($1, $2, 1)")
        .bind(alec.id.min(sam.id) as i64)
        .bind(alec.id.max(sam.id) as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(played(&feed(&cloud, &alec).await), [GAME]);
    assert_eq!(played(&feed(&cloud, &sam).await), [OTHER]);
    react(&cloud, &sam, alec_item, "gg")
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_party_item_belongs_to_its_members() {
    let cloud = TestCloud::start().await;
    let lead = cloud.sign_up("Lead").await;
    let mate = cloud.sign_up("Mate").await;
    let bystander = cloud.sign_up("Bystander").await;
    befriend(&cloud, &lead, &mate).await;
    befriend(&cloud, &lead, &bystander).await;
    // The leader shows its feed to nobody: the party item is the members' all the same.
    set_feed(&cloud, &lead, "nobody").await;

    cloud
        .post(
            "/v1/party/invitations",
            lead.token(),
            json!({"to": [mate.id, bystander.id]}),
        )
        .await
        .ok(200);
    // One member is not a pair yet.
    assert!(of_kind(&feed(&cloud, &lead).await, "party").is_empty());
    let held: anyplay_api::PartyInvitations = cloud
        .get("/v1/party/invitations", mate.token())
        .await
        .ok(200)
        .json();
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", held.invitations[0].id),
            mate.token(),
        )
        .await
        .ok(200);

    for member in [&lead, &mate] {
        let feed = feed(&cloud, member).await;
        let parties = of_kind(&feed, "party");
        assert_eq!(parties.len(), 1, "{}", member.handle);
        assert_eq!(parties[0].account.id.0, lead.id, "the leader owns it");
        let FeedEvent::Party { members } = &parties[0].event else {
            unreachable!()
        };
        let ids: Vec<u64> = members.iter().map(|m| m.id.0).collect();
        assert_eq!(ids, [lead.id, mate.id], "the leader first");
    }
    // A friend of the leader who is not in the party sees nothing of it, and cannot react.
    assert!(of_kind(&feed(&cloud, &bystander).await, "party").is_empty());
    let item = of_kind(&feed(&cloud, &lead).await, "party")[0].id;
    react(&cloud, &bystander, item, "wow")
        .await
        .is_error("not_found", 404);
    let reactions: Reactions = react(&cloud, &mate, item, "wow").await.ok(200).json();
    assert_eq!(reactions.mine, Some(Reaction::Wow));

    // A member who blocks the leader does not see the item any more.
    cloud
        .put(&format!("/v1/blocks/{}", lead.id), mate.token(), None)
        .await
        .ok(204);
    assert!(of_kind(&feed(&cloud, &mate).await, "party").is_empty());
    react(&cloud, &mate, item, "wow")
        .await
        .is_error("not_found", 404);
    assert_eq!(of_kind(&feed(&cloud, &lead).await, "party").len(), 1);
}

// Paging

#[tokio::test]
async fn pages_follow_next_until_the_end() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    for n in 0..7 {
        played_item(&cloud, &alec, &format!("GAME0010{n}")).await;
    }
    let first = feed_at(&cloud, &alec, "?limit=3").await;
    assert_eq!(first.items.len(), 3);
    assert_eq!(played(&first), ["GAME00106", "GAME00105", "GAME00104"]);
    let next = first.next.expect("more follow");
    assert_eq!(next, first.items[2].id, "next is the smallest ID held");
    let second = feed_at(&cloud, &alec, &format!("?limit=3&before={next}")).await;
    assert_eq!(played(&second), ["GAME00103", "GAME00102", "GAME00101"]);
    let last = feed_at(
        &cloud,
        &alec,
        &format!("?limit=3&before={}", second.next.unwrap()),
    )
    .await;
    assert_eq!(played(&last), ["GAME00100"]);
    assert_eq!(last.next, None);
    // A page that ends exactly at the end has no next.
    let all = feed_at(&cloud, &alec, "?limit=7").await;
    assert_eq!((all.items.len(), all.next), (7, None));
    // The default page holds all of them too; the ID of an item is a `before` that excludes it.
    let page = feed_at(&cloud, &alec, &format!("?before={}", all.items[0].id)).await;
    assert_eq!(page.items.len(), 6);
    assert_eq!(feed_at(&cloud, &alec, "?before=0").await.items.len(), 0);
}

#[tokio::test]
async fn a_bad_limit_or_before_is_refused_and_a_large_limit_is_clamped() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    for query in [
        "?limit=0",
        "?limit=-1",
        "?limit=many",
        "?before=-3",
        "?before=x",
    ] {
        cloud
            .get(&format!("/v1/feed{query}"), alec.token())
            .await
            .is_error("bad_request", 400);
    }
    for n in 0..55 {
        put_item(
            &cloud,
            &alec,
            NewItem::FirstPlayed {
                title_id: format!("GAME{n:05}"),
            },
        )
        .await;
    }
    let big = feed_at(&cloud, &alec, "?limit=500").await;
    assert_eq!(big.items.len(), 50);
    assert!(big.next.is_some());
    assert_eq!(
        feed(&cloud, &alec).await.items.len(),
        30,
        "the default is 30"
    );
    cloud
        .request(reqwest::Method::GET, "/v1/feed", None, None)
        .await
        .is_error("unauthorized", 401);
}

// Reactions

#[tokio::test]
async fn one_reaction_per_account_replaced_counted_and_taken_back() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let item = played_item(&cloud, &alec, GAME).await;

    let reactions: Reactions = react(&cloud, &sam, item, "nice").await.ok(200).json();
    assert_eq!(
        reactions,
        Reactions {
            reactions: ReactionCounts {
                nice: 1,
                ..Default::default()
            },
            mine: Some(Reaction::Nice),
        }
    );
    // The same again changes nothing; another reaction replaces the first.
    let again: Reactions = react(&cloud, &sam, item, "nice").await.ok(200).json();
    assert_eq!(again, reactions);
    let reactions: Reactions = react(&cloud, &sam, item, "wow").await.ok(200).json();
    assert_eq!((reactions.reactions.nice, reactions.reactions.wow), (0, 1));
    // The owner may react to their own item; counts are totals.
    let reactions: Reactions = react(&cloud, &alec, item, "wow").await.ok(200).json();
    assert_eq!(reactions.reactions.wow, 2);
    assert_eq!(reactions.mine, Some(Reaction::Wow));

    // Each sees their own in the feed.
    let seen = feed(&cloud, &sam).await;
    let item_seen = seen.items.iter().find(|i| i.id == item).unwrap();
    assert_eq!(item_seen.mine, Some(Reaction::Wow));
    assert_eq!(item_seen.reactions.wow, 2);

    let after: Reactions = unreact(&cloud, &sam, item).await.ok(200).json();
    assert_eq!((after.reactions.wow, after.mine), (1, None));
    // Taking back is idempotent.
    let after: Reactions = unreact(&cloud, &sam, item).await.ok(200).json();
    assert_eq!((after.reactions.wow, after.mine), (1, None));
    let seen = feed(&cloud, &sam).await;
    assert_eq!(seen.items.iter().find(|i| i.id == item).unwrap().mine, None);

    // Only the four exist.
    for bad in ["angry", "", "Nice"] {
        react(&cloud, &sam, item, bad)
            .await
            .is_error("bad_request", 400);
    }
    cloud
        .put(
            &format!("/v1/feed/{item}/reaction"),
            sam.token(),
            Some(json!({})),
        )
        .await
        .is_error("bad_request", 400);
    for good in ["nice", "gg", "wow", "same"] {
        react(&cloud, &sam, item, good).await.ok(200);
    }
}

#[tokio::test]
async fn an_item_that_may_not_be_seen_cannot_be_reacted_to() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let stranger = cloud.sign_up("Stranger").await;
    let item = played_item(&cloud, &alec, GAME).await;
    react(&cloud, &stranger, item, "gg")
        .await
        .is_error("not_found", 404);
    unreact(&cloud, &stranger, item)
        .await
        .is_error("not_found", 404);
    react(&cloud, &sam, 999_999, "gg")
        .await
        .is_error("not_found", 404);
    react(&cloud, &sam, 1 << 60, "gg")
        .await
        .is_error("not_found", 404);
    // Hiding the feed takes the right to react along.
    react(&cloud, &sam, item, "gg").await.ok(200);
    set_feed(&cloud, &alec, "nobody").await;
    react(&cloud, &sam, item, "same")
        .await
        .is_error("not_found", 404);
    unreact(&cloud, &sam, item).await.is_error("not_found", 404);
    react(&cloud, &alec, item, "same").await.ok(200);
    // The counts stay with the item for the owner.
    let seen = feed(&cloud, &alec).await;
    assert_eq!(
        seen.items
            .iter()
            .find(|i| i.id == item)
            .unwrap()
            .reactions
            .gg,
        1
    );
}

#[tokio::test]
async fn reactions_are_rate_limited_per_account() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.feed_reactions = 3).await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let item = played_item(&cloud, &alec, GAME).await;
    for _ in 0..3 {
        react(&cloud, &sam, item, "gg").await.ok(200);
    }
    let refused = react(&cloud, &sam, item, "gg").await;
    refused.is_error("rate_limited", 429);
    unreact(&cloud, &sam, item)
        .await
        .is_error("rate_limited", 429);
    // Reading the feed and the other accounts are not counted.
    feed(&cloud, &sam).await;
    react(&cloud, &alec, item, "gg").await.ok(200);
}

// Recording

#[tokio::test]
async fn old_trophy_history_makes_no_items_and_new_trophies_at_most_five() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let day = 24 * 60 * 60;
    // The history of a game, earned over the last weeks.
    let history: Vec<Value> = (0..20)
        .map(|n| trophy(n, "bronze", now() - 3 * day - u64::from(n)))
        .collect();
    upload(&cloud, &alec, GAME, history).await.ok(200);
    assert!(feed(&cloud, &alec).await.items.is_empty());

    // Seven new ones from the last hours: five items, in the order given.
    let fresh: Vec<Value> = (20..27)
        .map(|n| trophy(n, if n == 20 { "platinum" } else { "silver" }, now() - 600))
        .collect();
    upload(&cloud, &alec, GAME, fresh.clone()).await.ok(200);
    let page = feed(&cloud, &alec).await;
    let numbers: Vec<(u16, Grade)> = page
        .items
        .iter()
        .filter_map(|item| match &item.event {
            FeedEvent::Trophy { number, grade, .. } => Some((*number, *grade)),
            _ => None,
        })
        .collect();
    assert_eq!(
        numbers,
        [
            (24, Grade::Silver),
            (23, Grade::Silver),
            (22, Grade::Silver),
            (21, Grade::Silver),
            (20, Grade::Platinum)
        ],
        "newest first: the first five of the upload"
    );
    // The same upload again records nothing new, and so makes no items.
    upload(&cloud, &alec, GAME, fresh).await.ok(200);
    assert_eq!(feed(&cloud, &alec).await.items.len(), 5);
    // An old trophy among new ones makes none; one more fresh trophy makes one.
    upload(
        &cloud,
        &alec,
        OTHER,
        vec![
            trophy(1, "gold", now() - 2 * day),
            trophy(2, "gold", now() - 5),
        ],
    )
    .await
    .ok(200);
    let page = feed(&cloud, &alec).await;
    assert_eq!(page.items.len(), 6);
    assert!(matches!(
        &page.items[0].event,
        FeedEvent::Trophy { title_id, number: 2, grade: Grade::Gold, .. } if title_id == OTHER
    ));
    // The cloud keeps the JSON flat, with a kind.
    let raw = cloud
        .get("/v1/feed?limit=1", alec.token())
        .await
        .ok(200)
        .body;
    assert_eq!(raw["items"][0]["kind"], "trophy");
    assert_eq!(raw["items"][0]["grade"], "gold");
    assert!(raw["items"][0]["rarity"].is_null());
}

#[tokio::test]
async fn trophies_make_no_items_while_the_feed_is_hidden() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    set_feed(&cloud, &alec, "nobody").await;
    upload(&cloud, &alec, GAME, vec![trophy(1, "gold", now() - 5)])
        .await
        .ok(200);
    set_feed(&cloud, &alec, "friends").await;
    // Nothing was recorded meanwhile, so nothing appears when the feed is shown again.
    assert!(feed(&cloud, &alec).await.items.is_empty());
    upload(&cloud, &alec, GAME, vec![trophy(2, "gold", now() - 5)])
        .await
        .ok(200);
    assert_eq!(feed(&cloud, &alec).await.items.len(), 1);
}

#[tokio::test]
async fn becoming_friends_makes_an_item_for_each() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let carol = cloud.sign_up("Carol").await;
    // A request alone is not an item.
    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": "Sam"}),
        )
        .await
        .ok(200);
    assert!(feed(&cloud, &alec).await.items.is_empty());
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", alec.id),
            sam.token(),
        )
        .await
        .ok(204);
    // The answer to a request with a request of the other direction is a friendship at once.
    cloud
        .post(
            "/v1/friends/requests",
            carol.token(),
            json!({"handle": "Alec"}),
        )
        .await
        .ok(200);
    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": "Carol"}),
        )
        .await
        .ok(200);

    let for_alec = feed(&cloud, &alec).await;
    let friends = of_kind(&for_alec, "friend");
    assert_eq!(friends.len(), 4, "two friendships, an item for each side");
    let pairs: Vec<(u64, u64)> = friends
        .iter()
        .map(|item| match &item.event {
            FeedEvent::Friend { with } => (item.account.id.0, with.id.0),
            _ => unreachable!(),
        })
        .collect();
    for pair in [
        (alec.id, sam.id),
        (sam.id, alec.id),
        (alec.id, carol.id),
        (carol.id, alec.id),
    ] {
        assert!(pairs.contains(&pair), "{pair:?} in {pairs:?}");
    }
    // Sam sees his own item and those of his friend Alec, and nothing of Carol's.
    let for_sam = feed(&cloud, &sam).await;
    assert_eq!(of_kind(&for_sam, "friend").len(), 3);
    // Befriending again or being friends already adds nothing.
    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": "Sam"}),
        )
        .await
        .ok(200);
    assert_eq!(of_kind(&feed(&cloud, &alec).await, "friend").len(), 4);
}

#[tokio::test]
async fn a_title_is_played_for_the_first_time_once() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    play(&cloud, &alec, GAME).await;
    play(&cloud, &alec, GAME).await;
    cloud
        .put(
            "/v1/presence",
            alec.token(),
            Some(json!({"activity": null})),
        )
        .await
        .ok(204);
    play(&cloud, &alec, GAME).await;
    play(&cloud, &alec, OTHER).await;
    play(&cloud, &alec, GAME).await;
    assert_eq!(played(&feed(&cloud, &alec).await), [OTHER, GAME]);
    assert_eq!(played(&feed(&cloud, &sam).await), [OTHER, GAME]);
    // The memory outlives the items: after the retention took the old ones, the title stays
    // played.
    sqlx::query("DELETE FROM feed_items WHERE kind = 'first_played'")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    play(&cloud, &alec, GAME).await;
    assert!(played(&feed(&cloud, &alec).await).is_empty());
    // A title ID of another shape is no title.
    play(&cloud, &alec, "not a title").await;
    assert!(played(&feed(&cloud, &alec).await).is_empty());
}

#[tokio::test]
async fn a_hidden_feed_records_no_first_play_but_remembers_it() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    set_feed(&cloud, &alec, "nobody").await;
    play(&cloud, &alec, GAME).await;
    set_feed(&cloud, &alec, "friends").await;
    play(&cloud, &alec, GAME).await;
    play(&cloud, &alec, OTHER).await;
    assert_eq!(played(&feed(&cloud, &alec).await), [OTHER]);
}

#[tokio::test]
async fn an_account_gets_at_most_the_daily_number_of_items() {
    let cloud = TestCloud::with(|_| {}, |t| t.feed_items_per_day = 3).await;
    let alec = cloud.sign_up("Alec").await;
    for n in 0..6 {
        play(&cloud, &alec, &format!("GAME0002{n}")).await;
    }
    assert_eq!(
        played(&feed(&cloud, &alec).await),
        ["GAME00022", "GAME00021", "GAME00020"]
    );
    // An item of yesterday does not count.
    sqlx::query("UPDATE feed_items SET created = created - 172800")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    play(&cloud, &alec, "GAME00029").await;
    assert_eq!(played(&feed(&cloud, &alec).await).len(), 4);
}

// Retention and deletion

#[tokio::test]
async fn items_past_the_retention_go_with_their_reactions() {
    let cloud = TestCloud::with(|c| c.feed_retention_days = 2, |_| {}).await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let old = played_item(&cloud, &alec, GAME).await;
    let fresh = played_item(&cloud, &alec, OTHER).await;
    react(&cloud, &sam, old, "gg").await.ok(200);
    react(&cloud, &sam, fresh, "gg").await.ok(200);
    sqlx::query("UPDATE feed_items SET created = $2 WHERE id = $1")
        .bind(old as i64)
        .bind((now() - 3 * 86_400) as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    cloud.state.sweep().await.unwrap();
    let ids: Vec<u64> = feed(&cloud, &alec)
        .await
        .items
        .iter()
        .map(|i| i.id)
        .collect();
    assert!(!ids.contains(&old) && ids.contains(&fresh), "{ids:?}");
    let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM feed_reactions")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(left, 1, "the reaction to the old item went with it");
    react(&cloud, &sam, old, "gg")
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn deleting_an_account_removes_its_items_and_its_reactions() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    play(&cloud, &alec, GAME).await;
    play(&cloud, &sam, OTHER).await;
    let alec_item = of_kind(&feed(&cloud, &alec).await, "first_played")
        .iter()
        .find(|i| i.account.id.0 == alec.id)
        .unwrap()
        .id;
    let sam_item = of_kind(&feed(&cloud, &alec).await, "first_played")
        .iter()
        .find(|i| i.account.id.0 == sam.id)
        .unwrap()
        .id;
    react(&cloud, &sam, alec_item, "gg").await.ok(200);
    react(&cloud, &alec, sam_item, "wow").await.ok(200);
    react(&cloud, &sam, sam_item, "same").await.ok(200);

    cloud.delete_account_now(&alec).await;

    // Alec's items, his firsts and his reaction on Sam's item are gone; the friend item of Sam
    // about Alec went too, and Sam's own stay.
    assert_eq!(
        count(
            &cloud,
            "SELECT COUNT(*) FROM feed_items WHERE account_id = $1",
            alec.id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &cloud,
            "SELECT COUNT(*) FROM feed_items WHERE other_account = $1",
            alec.id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &cloud,
            "SELECT COUNT(*) FROM feed_firsts WHERE account_id = $1",
            alec.id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &cloud,
            "SELECT COUNT(*) FROM feed_reactions WHERE account_id = $1",
            alec.id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &cloud,
            "SELECT COUNT(*) FROM feed_reactions WHERE item_id = $1",
            alec_item
        )
        .await,
        0
    );
    let seen = feed(&cloud, &sam).await;
    assert_eq!(played(&seen), [OTHER]);
    let own = seen.items.iter().find(|i| i.id == sam_item).unwrap();
    assert_eq!(
        (own.reactions.same, own.reactions.wow, own.mine),
        (1, 0, Some(Reaction::Same)),
        "Alec's reaction to Sam's item went with Alec"
    );
}

#[tokio::test]
async fn the_export_holds_the_feed() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let item = played_item(&cloud, &sam, GAME).await;
    react(&cloud, &alec, item, "gg").await.ok(200);
    play(&cloud, &alec, OTHER).await;
    let export = cloud.get("/v1/me/export", alec.token()).await.ok(200).body;
    let kinds: Vec<&str> = export["feed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"first_played") && kinds.contains(&"friend"),
        "{kinds:?}"
    );
    assert_eq!(export["feed_reactions"][0]["reaction"], "gg");
    assert_eq!(export["feed_reactions"][0]["item"], item);
}

// Rarity

#[tokio::test]
async fn rarity_is_a_percent_with_one_decimal_and_null_below_ten_players() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    // Eleven other players of the title; two of them earned trophy 1.
    for k in 0..11u64 {
        let id = 9_000 + k;
        anyplay_cloud::db::insert_account(
            &cloud.state.db,
            id,
            &format!("Player{k}"),
            &format!("player{k}@example.com"),
            1,
        )
        .await
        .unwrap();
        let number = if k < 2 { 1 } else { 5 };
        sqlx::query(
            "INSERT INTO trophies (account_id, title_id, number, grade, earned) \
             VALUES ($1, $2, $3, 'bronze', 1)",
        )
        .bind(id as i64)
        .bind(GAME)
        .bind(number)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    }
    upload(
        &cloud,
        &alec,
        GAME,
        vec![trophy(1, "gold", now() - 5), trophy(2, "gold", now() - 4)],
    )
    .await
    .ok(200);
    // A title with Alec as its only player.
    upload(&cloud, &alec, OTHER, vec![trophy(1, "gold", now() - 3)])
        .await
        .ok(200);

    let page = feed(&cloud, &alec).await;
    let rarity = |title: &str, number: u16| -> Option<f32> {
        page.items
            .iter()
            .find_map(|item| match &item.event {
                FeedEvent::Trophy {
                    title_id,
                    number: n,
                    rarity,
                    ..
                } if title_id == title && *n == number => Some(*rarity),
                _ => None,
            })
            .expect("the item exists")
    };
    // Twelve players: three earned trophy 1 (25.0), only Alec trophy 2 (1/12 is 8.3).
    assert_eq!(rarity(GAME, 1), Some(25.0));
    assert_eq!(rarity(GAME, 2), Some(8.3));
    assert_eq!(rarity(OTHER, 1), None);
    let raw = cloud.get("/v1/feed", alec.token()).await.ok(200).body;
    let nulls = raw["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["rarity"].is_null())
        .count();
    assert_eq!(nulls, 1, "null, not absent or zero, below ten players");
}

// The profile page

#[tokio::test]
async fn a_profile_page_adds_trophies_and_links_by_what_the_caller_may_see() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let stranger = cloud.sign_up("Stranger").await;
    upload(
        &cloud,
        &alec,
        GAME,
        vec![
            trophy(1, "gold", 5),
            trophy(2, "bronze", 6),
            trophy(3, "bronze", 7),
        ],
    )
    .await
    .ok(200);
    for (provider, name, audience) in [
        ("discord", Some("alec#1"), "everyone"),
        ("steam", Some("AlecOnSteam"), "friends"),
    ] {
        sqlx::query(
            "INSERT INTO linked_accounts (account_id, provider, external_id, display_name, \
             audience, linked_at) VALUES ($1, $2, $3, $4, $5, 1)",
        )
        .bind(alec.id as i64)
        .bind(provider)
        .bind(format!("{provider}-id"))
        .bind(name)
        .bind(audience)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    }
    let page_of = |viewer: &User| {
        let path = format!("/v1/accounts/{}/profile", alec.id);
        let token = viewer.token().to_owned();
        let cloud = &cloud;
        async move { cloud.get(&path, &token).await.ok(200) }
    };

    let own = page_of(&alec).await;
    let page: ProfilePage = own.json();
    assert_eq!(page.profile.account.handle, "Alec");
    let counts = page.trophies.expect("own trophies");
    assert_eq!((counts.gold, counts.bronze, counts.silver), (1, 2, 0));
    assert_eq!(page.links.len(), 2);

    let friend = page_of(&sam).await;
    assert_eq!(friend.body["trophies"]["bronze"], 2);
    assert_eq!(friend.body["links"][0]["provider"], "discord");
    assert_eq!(friend.body["links"][0]["name"], "alec#1");
    assert_eq!(friend.body["links"][1]["provider"], "steam");
    // The same JSON reads as a plain profile.
    let plain: anyplay_api::Profile = friend.json();
    assert_eq!(plain.account.id.0, alec.id);

    // A stranger sees the trophies hidden (null) and only what is shown to everyone.
    let stranger_page = page_of(&stranger).await;
    assert!(stranger_page.body["trophies"].is_null());
    assert_eq!(stranger_page.body["links"].as_array().unwrap().len(), 1);
    assert_eq!(stranger_page.body["links"][0]["provider"], "discord");

    // Trophies follow their own visibility; links, never null, are empty when nothing is shown.
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "everyone"})),
        )
        .await
        .ok(200);
    assert_eq!(page_of(&stranger).await.body["trophies"]["gold"], 1);
    sqlx::query("UPDATE linked_accounts SET audience = 'nobody' WHERE account_id = $1")
        .bind(alec.id as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(page_of(&sam).await.body["links"], json!([]));
    assert_eq!(
        page_of(&alec).await.body["links"].as_array().unwrap().len(),
        2
    );

    // An account that does not exist is not found.
    cloud
        .get("/v1/accounts/987654/profile", alec.token())
        .await
        .is_error("not_found", 404);

    // Editing the profile still answers a plain profile.
    let patched = cloud
        .patch("/v1/me/profile", alec.token(), json!({"about": "Hi"}))
        .await
        .ok(200);
    assert_eq!(patched.body["about"], "Hi");
    assert!(patched.body.get("links").is_none() && patched.body.get("trophies").is_none());
}

// News

#[tokio::test]
async fn friends_hear_of_new_items_and_the_owner_of_a_reaction() {
    let cloud = TestCloud::start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let stranger = cloud.sign_up("Stranger").await;
    let mut alec_events = cloud.events(alec.token()).await;
    let mut sam_events = cloud.events(sam.token()).await;
    let mut stranger_events = cloud.events(stranger.token()).await;
    // Sam's connection told Alec his presence; read that first.
    for events in [&mut alec_events, &mut sam_events, &mut stranger_events] {
        while events
            .next_within(Duration::from_millis(300))
            .await
            .is_some()
        {}
    }
    let is_feed = |e: &Event| matches!(e, Event::Feed);
    let wait = Duration::from_millis(400);

    // A fresh trophy of Alec's: Sam hears, Alec and the stranger do not.
    upload(&cloud, &alec, GAME, vec![trophy(1, "gold", now() - 5)])
        .await
        .ok(200);
    sam_events.until(is_feed).await;
    alec_events.quiet(wait).await;
    stranger_events.quiet(wait).await;

    // An old trophy is no news.
    upload(&cloud, &alec, GAME, vec![trophy(2, "gold", 5)])
        .await
        .ok(200);
    sam_events.quiet(wait).await;

    // A reaction: the owner hears, the one who reacted does not.
    let item = feed(&cloud, &alec).await.items[0].id;
    react(&cloud, &sam, item, "gg").await.ok(200);
    alec_events.until(is_feed).await;
    sam_events.quiet(wait).await;
    // The same reaction again changes nothing and is no news.
    react(&cloud, &sam, item, "gg").await.ok(200);
    alec_events.quiet(wait).await;
    unreact(&cloud, &sam, item).await.ok(200);
    alec_events.until(is_feed).await;
    // The owner reacting to their own item tells no one.
    react(&cloud, &alec, item, "same").await.ok(200);
    alec_events.quiet(wait).await;

    // A first play and a new friendship reach the friends.
    play(&cloud, &sam, GAME).await;
    alec_events.until(is_feed).await;
    // A hidden feed sends no news, and no one is told of a friend added.
    set_feed(&cloud, &sam, "nobody").await;
    play(&cloud, &sam, OTHER).await;
    alec_events
        .until(|e| matches!(e, Event::Presence { .. }))
        .await;
    alec_events.quiet(wait).await;
}

#[tokio::test]
async fn a_party_item_is_news_for_the_members() {
    let cloud = TestCloud::start().await;
    let lead = cloud.sign_up("Lead").await;
    let mate = cloud.sign_up("Mate").await;
    befriend(&cloud, &lead, &mate).await;
    let mut mate_events = cloud.events(mate.token()).await;
    cloud
        .post(
            "/v1/party/invitations",
            lead.token(),
            json!({"to": [mate.id]}),
        )
        .await
        .ok(200);
    let held: anyplay_api::PartyInvitations = cloud
        .get("/v1/party/invitations", mate.token())
        .await
        .ok(200)
        .json();
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", held.invitations[0].id),
            mate.token(),
        )
        .await
        .ok(200);
    mate_events.until(|e| matches!(e, Event::Feed)).await;
}
