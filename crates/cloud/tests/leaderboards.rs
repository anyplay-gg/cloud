//! Leaderboards (network `spec/cloud-api.md`, "Leaderboards", ADR 0040): boards are made by the
//! first score, an entry is an account's best, who sees an entry follows the account's audience,
//! friendship and blocks, and a moderator decides order, bounds and what stays. Like the other
//! tests these run on SQLite, and on PostgreSQL as well when `ANYPLAY_TEST_DATABASE_URL` is set.

mod common;

use anyplay_api::{
    Audience, BoardOrder, BoardPage, BoardSettings, BoardStatus, BoardView, ScorePrivacy,
    ScoreSaved, TitleBoards,
};
use common::portal::Portal;
use common::{Reply, TestCloud, User};
use serde_json::json;

const ADMIN: &str = "an-admin-token-for-tests";
const TITLE: &str = "GAME00001";

async fn cloud() -> TestCloud {
    TestCloud::with(
        |c| {
            c.admin_token = Some(ADMIN.into());
            c.leaderboards_enabled = true;
        },
        |t| t.rates.admin_per_ip = 1_000_000,
    )
    .await
}

async fn submit_to(cloud: &TestCloud, user: &User, board: &str, score: i64) -> Reply {
    cloud
        .put(
            &format!("/v1/me/scores/{TITLE}/{board}"),
            user.token(),
            Some(json!({"score": score})),
        )
        .await
}

async fn submit(cloud: &TestCloud, user: &User, board: &str, score: i64) -> ScoreSaved {
    submit_to(cloud, user, board, score).await.ok(200).json()
}

async fn read(cloud: &TestCloud, user: &User, tail: &str) -> BoardPage {
    cloud
        .get(&format!("/v1/boards/{TITLE}/{tail}"), user.token())
        .await
        .ok(200)
        .json()
}

async fn show(cloud: &TestCloud, user: &User, audience: Audience) {
    let kept: ScorePrivacy = cloud
        .put(
            "/v1/me/score-privacy",
            user.token(),
            Some(json!({"scores": audience})),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(kept.scores, audience);
}

/// The handles of a page, in order.
fn handles(page: &BoardPage) -> Vec<&str> {
    page.entries
        .iter()
        .map(|e| e.account.handle.as_str())
        .collect()
}

fn ranks(page: &BoardPage) -> Vec<u32> {
    page.entries.iter().map(|e| e.rank).collect()
}

/// Accounts that show their scores to everyone, each with a score on board "7".
async fn public_players(cloud: &TestCloud, players: &[(&str, i64)]) -> Vec<User> {
    let mut users = Vec::new();
    for (name, score) in players {
        let user = cloud.sign_up(name).await;
        show(cloud, &user, Audience::Everyone).await;
        submit(cloud, &user, "7", *score).await;
        users.push(user);
    }
    users
}

async fn configure(cloud: &TestCloud, board: &str, body: serde_json::Value) -> BoardSettings {
    cloud
        .put(
            &format!("/v1/moderation/boards/{TITLE}/{board}"),
            ADMIN,
            Some(body),
        )
        .await
        .ok(200)
        .json()
}

#[tokio::test]
async fn everything_is_off_until_the_cloud_turns_it_on() {
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    submit_to(&cloud, &alec, "7", 10)
        .await
        .is_error("forbidden", 403);
    cloud
        .get(&format!("/v1/boards/{TITLE}/7"), alec.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .get(&format!("/v1/titles/{TITLE}/boards"), alec.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .put(
            &format!("/v1/moderation/boards/{TITLE}/7"),
            ADMIN,
            Some(json!({"order": "low"})),
        )
        .await
        .is_error("forbidden", 403);
    let visitor = Portal::new(&cloud);
    visitor
        .get(&format!("/boards/{TITLE}/7"))
        .await
        .error(404, "not_found");
    visitor
        .get(&format!("/titles/{TITLE}/boards"))
        .await
        .error(404, "not_found");
    // The choice of an audience is made before anything shows.
    show(&cloud, &alec, Audience::Everyone).await;
    let seen: ScorePrivacy = cloud
        .get("/v1/me/score-privacy", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(seen.scores, Audience::Everyone);
    // Nothing was made by the attempts above.
    let (boards,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM boards")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(boards, 0);
}

#[tokio::test]
async fn the_first_score_makes_a_board_and_an_entry_is_the_best_score() {
    let cloud = cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let first = submit(&cloud, &alec, "7", 100).await;
    assert!(first.accepted);
    assert_eq!((first.score, first.rank, first.total), (100, 1, 1));

    // Worse and equal scores change nothing; a better one replaces the entry.
    let worse = submit(&cloud, &alec, "7", 90).await;
    assert!(!worse.accepted);
    assert_eq!(worse.score, 100);
    let same = submit(&cloud, &alec, "7", 100).await;
    assert!(!same.accepted);
    assert_eq!(same.achieved, first.achieved);
    let better = submit(&cloud, &alec, "7", 120).await;
    assert!(better.accepted);
    assert_eq!(better.score, 120);

    let page = read(&cloud, &alec, "7").await;
    assert_eq!(
        (page.view, page.order, page.status, page.total),
        (BoardView::Top, BoardOrder::High, BoardStatus::Open, 1)
    );
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].score, 120);
    assert_eq!(page.me.as_ref().map(|m| m.rank), Some(1));
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scores")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 1, "one row per account and board, no history");

    // A board nobody scored on does not exist.
    cloud
        .get(&format!("/v1/boards/{TITLE}/8"), alec.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn the_order_of_a_board_is_data_and_changes_the_ranking_at_once() {
    let cloud = cloud().await;
    let players = public_players(&cloud, &[("Alec", 30), ("Bob", 10), ("Cleo", 20)]).await;
    let page = read(&cloud, &players[0], "7").await;
    assert_eq!(handles(&page), ["Alec", "Cleo", "Bob"]);

    let set = configure(&cloud, "7", json!({"order": "low"})).await;
    assert_eq!(set.order, BoardOrder::Low);
    assert_eq!((set.min_score, set.max_score), (0, 9_007_199_254_740_991));
    let page = read(&cloud, &players[0], "7").await;
    assert_eq!(page.order, BoardOrder::Low);
    assert_eq!(handles(&page), ["Bob", "Cleo", "Alec"]);

    // On a low board a lower score is the better one.
    let better = submit(&cloud, &players[0], "7", 5).await;
    assert!(better.accepted);
    assert_eq!(better.rank, 1);
    assert!(!submit(&cloud, &players[0], "7", 6).await.accepted);
}

#[tokio::test]
async fn equal_scores_share_a_rank_and_the_next_rank_skips() {
    let cloud = cloud().await;
    let players = public_players(
        &cloud,
        &[("Alec", 300), ("Bob", 200), ("Cleo", 200), ("Dana", 100)],
    )
    .await;
    let viewer = cloud.sign_up("Eve").await;
    let page = read(&cloud, &viewer, "7").await;
    assert_eq!(handles(&page), ["Alec", "Bob", "Cleo", "Dana"]);
    assert_eq!(ranks(&page), [1, 2, 2, 4]);
    assert_eq!(page.total, 4);
    assert!(page.me.is_none());

    // Pages: from position 1, two entries.
    let page = read(&cloud, &viewer, "7?start=1&count=2").await;
    assert_eq!(handles(&page), ["Bob", "Cleo"]);
    assert_eq!((page.start, page.total), (1, 4));
    let page = read(&cloud, &viewer, "7?start=3&count=10").await;
    assert_eq!(handles(&page), ["Dana"]);
    cloud
        .get(&format!("/v1/boards/{TITLE}/7?count=0"), viewer.token())
        .await
        .is_error("bad_request", 400);
    cloud
        .get(&format!("/v1/boards/{TITLE}/7?count=101"), viewer.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn scores_are_shown_to_friends_unless_the_player_chooses_otherwise() {
    let cloud = cloud().await;
    let (alec, bob) = cloud.friends("Alec", "Bob").await;
    let cleo = cloud.sign_up("Cleo").await;
    submit(&cloud, &alec, "7", 100).await;
    submit(&cloud, &bob, "7", 50).await;

    // The default is friends: a friend sees the entry, a stranger does not.
    let seen = cloud
        .get("/v1/me/score-privacy", alec.token())
        .await
        .ok(200)
        .json::<ScorePrivacy>();
    assert_eq!(seen.scores, Audience::Friends);
    let page = read(&cloud, &bob, "7").await;
    assert_eq!(handles(&page), ["Alec", "Bob"]);
    let page = read(&cloud, &cleo, "7").await;
    assert!(page.entries.is_empty() && page.total == 0);

    // Everyone: the stranger sees it.
    show(&cloud, &alec, Audience::Everyone).await;
    let page = read(&cloud, &cleo, "7").await;
    assert_eq!(handles(&page), ["Alec"]);

    // Nobody: not even a friend sees it, but the player always sees their own.
    show(&cloud, &alec, Audience::Nobody).await;
    let page = read(&cloud, &bob, "7").await;
    assert_eq!(handles(&page), ["Bob"]);
    let page = read(&cloud, &alec, "7").await;
    assert_eq!(handles(&page), ["Alec", "Bob"]);
    assert_eq!(page.me.map(|m| m.rank), Some(1));
}

#[tokio::test]
async fn a_hidden_player_is_not_a_gap_in_the_ranks() {
    let cloud = cloud().await;
    let (alec, bob) = cloud.friends("Alec", "Bob").await;
    let cleo = cloud.sign_up("Cleo").await;
    let dana = cloud.sign_up("Dana").await;
    show(&cloud, &cleo, Audience::Everyone).await;
    show(&cloud, &dana, Audience::Everyone).await;
    // Bob keeps the default (friends) and has the best score.
    submit(&cloud, &bob, "7", 900).await;
    submit(&cloud, &cleo, "7", 500).await;
    submit(&cloud, &dana, "7", 100).await;

    let stranger = read(&cloud, &alec, "7").await;
    // Alec is Bob's friend: he sees all three.
    assert_eq!(handles(&stranger), ["Bob", "Cleo", "Dana"]);
    let outsider = cloud.sign_up("Eve").await;
    let page = read(&cloud, &outsider, "7").await;
    assert_eq!(handles(&page), ["Cleo", "Dana"]);
    assert_eq!(ranks(&page), [1, 2], "no gap where Bob would be");
    assert_eq!(page.total, 2);
    // The player's own rank counts the entries they may see, and so does their total.
    let mine = submit(&cloud, &bob, "7", 900).await;
    assert_eq!((mine.rank, mine.total), (1, 3));
}

#[tokio::test]
async fn the_friends_view_holds_the_player_and_friends_who_show_their_scores() {
    let cloud = cloud().await;
    let (alec, bob) = cloud.friends("Alec", "Bob").await;
    let cleo = cloud.sign_up("Cleo").await;
    let dana = cloud.sign_up("Dana").await;
    // Cleo is a friend of Alec who shows scores to nobody; Dana shows to everyone, no friend.
    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": cleo.handle}),
        )
        .await
        .ok(200);
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", alec.id),
            cleo.token(),
        )
        .await
        .ok(204);
    show(&cloud, &cleo, Audience::Nobody).await;
    show(&cloud, &dana, Audience::Everyone).await;
    for (user, score) in [(&alec, 200), (&bob, 300), (&cleo, 400), (&dana, 500)] {
        submit(&cloud, user, "7", score).await;
    }
    let page = read(&cloud, &alec, "7/friends").await;
    assert_eq!(page.view, BoardView::Friends);
    assert_eq!(handles(&page), ["Bob", "Alec"]);
    assert_eq!(ranks(&page), [1, 2]);
    assert_eq!(page.total, 2);
    assert_eq!(page.me.map(|m| m.rank), Some(2));
    // The top view for Alec holds Dana (everyone) and Bob (a friend), and Alec; not Cleo.
    let top = read(&cloud, &alec, "7").await;
    assert_eq!(handles(&top), ["Dana", "Bob", "Alec"]);
}

#[tokio::test]
async fn the_lines_around_the_player_are_centred_on_them() {
    let cloud = cloud().await;
    let players = public_players(
        &cloud,
        &[
            ("Alec", 500),
            ("Bob", 400),
            ("Cleo", 300),
            ("Dana", 200),
            ("Eve", 100),
        ],
    )
    .await;
    let page = read(&cloud, &players[2], "7/around?count=3").await;
    assert_eq!(page.view, BoardView::Around);
    assert_eq!(handles(&page), ["Bob", "Cleo", "Dana"]);
    assert_eq!((page.start, page.total), (1, 5));
    assert_eq!(page.me.map(|m| m.rank), Some(3));
    // Near the top the window starts at the first place.
    let page = read(&cloud, &players[0], "7/around?count=5").await;
    assert_eq!(handles(&page).len(), 5);
    assert_eq!(page.start, 0);
    let page = read(&cloud, &players[4], "7/around?count=3").await;
    assert_eq!(handles(&page), ["Dana", "Eve"]);
    // Without an entry there is nothing around.
    let stranger = cloud.sign_up("Fay").await;
    let page = read(&cloud, &stranger, "7/around").await;
    assert!(page.entries.is_empty() && page.me.is_none());
    assert_eq!(page.total, 5);
}

#[tokio::test]
async fn a_block_hides_the_two_accounts_from_each_other() {
    let cloud = cloud().await;
    let players = public_players(&cloud, &[("Alec", 300), ("Bob", 200), ("Cleo", 100)]).await;
    let blocked = cloud
        .put(
            &format!("/v1/blocks/{}", players[1].id),
            players[0].token(),
            None,
        )
        .await;
    assert!(blocked.status / 100 == 2, "{}", blocked.body);
    let alec = read(&cloud, &players[0], "7").await;
    assert_eq!(handles(&alec), ["Alec", "Cleo"]);
    assert_eq!(ranks(&alec), [1, 2]);
    let bob = read(&cloud, &players[1], "7").await;
    assert_eq!(handles(&bob), ["Bob", "Cleo"]);
    let cleo = read(&cloud, &players[2], "7").await;
    assert_eq!(handles(&cleo), ["Alec", "Bob", "Cleo"]);
}

#[tokio::test]
async fn scores_outside_the_bounds_are_refused_and_a_moderator_can_change_them() {
    let cloud = cloud().await;
    let alec = cloud.sign_up("Alec").await;
    submit_to(&cloud, &alec, "7", -1)
        .await
        .is_error("bad_request", 400);
    submit_to(&cloud, &alec, "7", 9_007_199_254_740_992)
        .await
        .is_error("bad_request", 400);
    // A refused first score made no board.
    cloud
        .get(&format!("/v1/boards/{TITLE}/7"), alec.token())
        .await
        .is_error("not_found", 404);

    let set = configure(&cloud, "7", json!({"min_score": -100, "max_score": 100})).await;
    assert_eq!((set.min_score, set.max_score), (-100, 100));
    assert!(submit(&cloud, &alec, "7", -50).await.accepted);
    submit_to(&cloud, &alec, "7", 101)
        .await
        .is_error("bad_request", 400);

    // Bounds that make no sense are refused.
    cloud
        .put(
            &format!("/v1/moderation/boards/{TITLE}/7"),
            ADMIN,
            Some(json!({"min_score": 10, "max_score": 5})),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .put(
            &format!("/v1/moderation/boards/{TITLE}/7"),
            ADMIN,
            Some(json!({"max_score": 9_007_199_254_740_992_i64})),
        )
        .await
        .is_error("bad_request", 400);
    // A shape that cannot stand in a path is not a board.
    submit_to(&cloud, &alec, "a.b", 1)
        .await
        .is_error("bad_request", 400);
    cloud
        .put(
            "/v1/me/scores/not%20a%20title/7",
            alec.token(),
            Some(json!({"score": 1})),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_closed_board_takes_no_more_scores_and_stays_readable() {
    let cloud = cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    submit(&cloud, &alec, "7", 10).await;
    let set = configure(&cloud, "7", json!({"status": "closed"})).await;
    assert_eq!(set.status, BoardStatus::Closed);
    submit_to(&cloud, &alec, "7", 20)
        .await
        .is_error("forbidden", 403);
    submit_to(&cloud, &bob, "7", 20)
        .await
        .is_error("forbidden", 403);
    let page = read(&cloud, &alec, "7").await;
    assert_eq!(page.status, BoardStatus::Closed);
    assert_eq!(page.entries[0].score, 10);
    configure(&cloud, "7", json!({"status": "open"})).await;
    assert!(submit(&cloud, &alec, "7", 20).await.accepted);
}

#[tokio::test]
async fn a_moderator_removes_an_entry_and_it_stays_removed_until_restored() {
    let cloud = cloud().await;
    let players = public_players(&cloud, &[("Alec", 300), ("Bob", 200)]).await;
    let (alec, bob) = (&players[0], &players[1]);
    let path = format!("/v1/moderation/boards/{TITLE}/7/entries/{}", alec.id);

    // A player is no moderator.
    cloud
        .request(
            reqwest::Method::DELETE,
            &path,
            Some(bob.token()),
            Some(json!({"reason": "unfair"})),
        )
        .await
        .is_error("forbidden", 403);
    // A reason is needed.
    cloud
        .request(
            reqwest::Method::DELETE,
            &path,
            Some(ADMIN),
            Some(json!({"reason": "  "})),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .request(
            reqwest::Method::DELETE,
            &path,
            Some(ADMIN),
            Some(json!({"reason": "A score no game reaches"})),
        )
        .await
        .ok(204);
    let page = read(&cloud, bob, "7").await;
    assert_eq!(handles(&page), ["Bob"]);
    assert_eq!(page.total, 1);
    // The owner does not see it either, and cannot put a score back.
    let page = read(&cloud, alec, "7").await;
    assert_eq!(handles(&page), ["Bob"]);
    submit_to(&cloud, alec, "7", 999)
        .await
        .is_error("forbidden", 403);
    // Removing it twice, or an entry nobody has, finds nothing.
    cloud
        .request(
            reqwest::Method::DELETE,
            &path,
            Some(ADMIN),
            Some(json!({"reason": "again"})),
        )
        .await
        .is_error("not_found", 404);
    let (by, reason): (String, String) =
        sqlx::query_as("SELECT removed_by, removed_reason FROM scores WHERE account_id = $1")
            .bind(alec.id as i64)
            .fetch_one(&cloud.state.db)
            .await
            .unwrap();
    assert_eq!(
        (by.as_str(), reason.as_str()),
        ("operator", "A score no game reaches")
    );

    cloud
        .post(&format!("{path}/restore"), ADMIN, json!({}))
        .await
        .ok(204);
    cloud
        .post(&format!("{path}/restore"), ADMIN, json!({}))
        .await
        .is_error("not_found", 404);
    let page = read(&cloud, bob, "7").await;
    assert_eq!(handles(&page), ["Alec", "Bob"]);
    assert!(!submit(&cloud, alec, "7", 100).await.accepted);
    assert!(submit(&cloud, alec, "7", 400).await.accepted);
}

#[tokio::test]
async fn a_moderator_account_can_do_what_the_operator_does() {
    let cloud = cloud().await;
    let mallory = cloud.sign_up("Mallory").await;
    cloud
        .post(
            "/v1/moderation/moderators",
            ADMIN,
            json!({"handle": mallory.handle}),
        )
        .await
        .ok(200);
    let alec = cloud.sign_up("Alec").await;
    submit(&cloud, &alec, "7", 10).await;
    cloud
        .put(
            &format!("/v1/moderation/boards/{TITLE}/7"),
            mallory.token(),
            Some(json!({"order": "low"})),
        )
        .await
        .ok(200);
    cloud
        .request(
            reqwest::Method::DELETE,
            &format!("/v1/moderation/boards/{TITLE}/7/entries/{}", alec.id),
            Some(mallory.token()),
            Some(json!({"reason": "test"})),
        )
        .await
        .ok(204);
    let (by,): (String,) = sqlx::query_as("SELECT removed_by FROM scores")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(by, "Mallory");
}

#[tokio::test]
async fn the_limits_bound_what_a_client_can_make_up() {
    let cloud = TestCloud::with(
        |c| {
            c.admin_token = Some(ADMIN.into());
            c.leaderboards_enabled = true;
        },
        |t| {
            t.rates.admin_per_ip = 1_000_000;
            t.rates.score_submits = 20;
        },
    )
    .await;
    // An account makes at most 16 boards.
    let alec = cloud.sign_up("Alec").await;
    for n in 0..16 {
        submit(&cloud, &alec, &format!("b{n}"), 1).await;
    }
    submit_to(&cloud, &alec, "b16", 1)
        .await
        .is_error("limit", 409);
    cloud
        .get(&format!("/v1/boards/{TITLE}/b16"), alec.token())
        .await
        .is_error("not_found", 404);
    // But it can score on a board somebody else made.
    let bob = cloud.sign_up("Bob").await;
    submit(&cloud, &bob, "b0", 5).await;
    submit(&cloud, &bob, "b1", 5).await;

    // A title has at most 64 boards: moderators make the rest, then a player cannot add one.
    for n in 16..64 {
        configure(&cloud, &format!("b{n}"), json!({})).await;
    }
    let cleo = cloud.sign_up("Cleo").await;
    submit_to(&cloud, &cleo, "extra", 1)
        .await
        .is_error("limit", 409);
    cloud
        .put(
            &format!("/v1/moderation/boards/{TITLE}/extra"),
            ADMIN,
            Some(json!({})),
        )
        .await
        .is_error("limit", 409);

    // Twenty submissions per window, then rate limited.
    let dana = cloud.sign_up("Dana").await;
    for n in 0..20 {
        submit(&cloud, &dana, "b0", n).await;
    }
    let reply = submit_to(&cloud, &dana, "b0", 99).await;
    assert_eq!(reply.status, 429, "{}", reply.body);
    assert!(reply.headers.get("retry-after").is_some());
}

#[tokio::test]
async fn a_banned_account_is_in_no_view() {
    let cloud = cloud().await;
    let players = public_players(&cloud, &[("Alec", 300), ("Bob", 200)]).await;
    cloud
        .post(
            "/v1/moderation/actions",
            ADMIN,
            json!({"account": players[0].id, "kind": "ban", "rule": "spam", "reason": "A."}),
        )
        .await
        .ok(200);
    let page = read(&cloud, &players[1], "7").await;
    assert_eq!(handles(&page), ["Bob"]);
    assert_eq!(page.total, 1);
}

#[tokio::test]
async fn the_boards_of_a_title_list_what_the_viewer_sees() {
    let cloud = cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    show(&cloud, &alec, Audience::Everyone).await;
    submit(&cloud, &alec, "a", 1).await;
    submit(&cloud, &alec, "b", 1).await;
    submit(&cloud, &bob, "b", 2).await;
    // Bob's friends-only entry is the only one on "c": a stranger sees no such board.
    submit(&cloud, &bob, "c", 2).await;
    configure(&cloud, "b", json!({"order": "low"})).await;

    let list: TitleBoards = cloud
        .get(&format!("/v1/titles/{TITLE}/boards"), alec.token())
        .await
        .ok(200)
        .json();
    let seen: Vec<(&str, u32, BoardOrder)> = list
        .boards
        .iter()
        .map(|b| (b.board.as_str(), b.entries, b.order))
        .collect();
    assert_eq!(
        seen,
        [("a", 1, BoardOrder::High), ("b", 1, BoardOrder::Low)]
    );
    let list: TitleBoards = cloud
        .get(&format!("/v1/titles/{TITLE}/boards"), bob.token())
        .await
        .ok(200)
        .json();
    assert_eq!(list.boards.len(), 3);
    assert_eq!(list.title_id, TITLE);
    cloud
        .get("/v1/titles/not%20a%20title/boards", bob.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn the_web_shows_a_visitor_what_everyone_may_see() {
    let cloud = cloud().await;
    let (alec, bob) = cloud.friends("Alec", "Bob").await;
    let cleo = cloud.sign_up("Cleo").await;
    show(&cloud, &cleo, Audience::Everyone).await;
    submit(&cloud, &alec, "7", 300).await;
    submit(&cloud, &bob, "7", 200).await;
    submit(&cloud, &cleo, "7", 100).await;

    let visitor = Portal::new(&cloud);
    let seen = visitor.get(&format!("/boards/{TITLE}/7")).await.ok(200);
    assert_eq!(seen["titleId"], TITLE);
    assert_eq!(seen["view"], "top");
    assert_eq!(seen["order"], "high");
    assert_eq!(seen["total"], 1);
    assert_eq!(seen["entries"][0]["handle"], "Cleo");
    assert_eq!(seen["entries"][0]["rank"], 1);
    assert!(seen.get("me").is_none());
    // Only the handle: no account ID reaches the page.
    assert!(seen["entries"][0].get("account").is_none());
    assert!(seen["entries"][0].get("id").is_none());

    // The views that are about the viewer need a session.
    visitor
        .get(&format!("/boards/{TITLE}/7?view=around"))
        .await
        .error(401, "unauthorized");
    visitor
        .get(&format!("/boards/{TITLE}/7?view=friends"))
        .await
        .error(401, "unauthorized");
    visitor
        .get(&format!("/boards/{TITLE}/7?view=sideways"))
        .await
        .error(400, "bad_request");
    visitor
        .get(&format!("/boards/{TITLE}/nope"))
        .await
        .error(404, "not_found");
    visitor
        .get(&format!("/boards/{TITLE}/a.b"))
        .await
        .error(404, "not_found");

    // A signed-in friend sees more, and their own line.
    let browser = Portal::signed_in(&cloud, &bob).await;
    let seen = browser.get(&format!("/boards/{TITLE}/7")).await.ok(200);
    let names: Vec<&str> = seen["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["handle"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Alec", "Bob", "Cleo"]);
    assert_eq!(seen["me"]["handle"], "Bob");
    assert_eq!(seen["me"]["rank"], 2);
    let seen = browser
        .get(&format!("/boards/{TITLE}/7?view=friends"))
        .await
        .ok(200);
    assert_eq!(seen["entries"].as_array().unwrap().len(), 2);

    let list = visitor
        .get(&format!("/titles/{TITLE}/boards"))
        .await
        .ok(200);
    assert_eq!(list["titleId"], TITLE);
    assert_eq!(list["boards"][0]["board"], "7");
    assert_eq!(list["boards"][0]["entries"], 1);
}

#[tokio::test]
async fn scores_are_part_of_the_export_and_go_with_the_account() {
    let cloud = cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    show(&cloud, &bob, Audience::Everyone).await;
    submit(&cloud, &alec, "7", 42).await;
    submit(&cloud, &bob, "7", 7).await;
    let export = cloud.get("/v1/me/export", alec.token()).await.ok(200).body;
    assert_eq!(export["scores"][0]["board"], "7");
    assert_eq!(export["scores"][0]["score"], 42);
    assert_eq!(export["scores"][0]["title_id"], TITLE);
    assert_eq!(export["scores_shown_to"], "friends");

    cloud.delete_account_now(&alec).await;
    let page = read(&cloud, &bob, "7").await;
    assert_eq!(handles(&page), ["Bob"]);
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scores WHERE account_id = $1")
        .bind(alec.id as i64)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_board_stays_when_its_last_entry_goes() {
    let cloud = cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    submit(&cloud, &alec, "7", 1).await;
    configure(&cloud, "7", json!({"order": "low"})).await;
    cloud.delete_account_now(&alec).await;
    let page = read(&cloud, &bob, "7").await;
    assert_eq!(page.order, BoardOrder::Low, "the moderator's data stays");
    assert!(page.entries.is_empty());
}
