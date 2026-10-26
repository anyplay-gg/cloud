mod common;

use anyplay_api::{Event, LfgAsk, LfgMine, Listing, Listings, PartyInvitations};
use common::{TestCloud, User};
use serde_json::json;

const TITLE: &str = "GAME00001";

fn listing(slots: u8) -> serde_json::Value {
    json!({"title_id": TITLE, "mode": "online", "language": "en", "note": "chill runs", "slots": slots})
}

async fn post(cloud: &TestCloud, user: &User, body: serde_json::Value) -> common::Reply {
    cloud.post("/v1/lfg", user.token(), body).await
}

async fn board(cloud: &TestCloud, user: &User, query: &str) -> Vec<Listing> {
    cloud
        .get(&format!("/v1/lfg?title={TITLE}{query}"), user.token())
        .await
        .ok(200)
        .json::<Listings>()
        .listings
}

#[tokio::test]
async fn a_listing_shows_on_the_board_of_its_game_for_others() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    let made: Listing = post(&cloud, &ann, listing(2)).await.ok(200).json();
    assert_eq!((made.slots, made.filled), (2, 0));
    assert_eq!(made.expires - made.created, 3600);

    let seen = board(&cloud, &ben, "").await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].id, made.id);
    assert_eq!(seen[0].poster.id.0, ann.id);
    assert!(seen[0].requests.is_empty());
    // Not on anyone's board for another game, and not on the poster's own.
    let other: Listings = cloud
        .get("/v1/lfg?title=GAME00002", ben.token())
        .await
        .ok(200)
        .json();
    assert!(other.listings.is_empty());
    assert!(board(&cloud, &ann, "").await.is_empty());
    // Filters.
    assert_eq!(board(&cloud, &ben, "&mode=online").await.len(), 1);
    assert!(board(&cloud, &ben, "&mode=lan").await.is_empty());
    assert_eq!(board(&cloud, &ben, "&language=en").await.len(), 1);
    assert!(board(&cloud, &ben, "&language=de").await.is_empty());
    // The board is read for one game at a time.
    cloud
        .get("/v1/lfg", ben.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn bad_listings_and_limits_are_refused() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    for body in [
        json!({"title_id": TITLE, "mode": "online", "slots": 0}),
        json!({"title_id": TITLE, "mode": "online", "slots": 8}),
        json!({"title_id": TITLE, "mode": "nonsense", "slots": 1}),
        json!({"title_id": TITLE, "mode": "lan", "slots": 1, "language": "English"}),
        json!({"title_id": TITLE, "mode": "lan", "slots": 1, "note": "join example.com now"}),
        json!({"title_id": TITLE, "mode": "lan", "slots": 1, "note": "x".repeat(141)}),
        json!({"title_id": "bad id!", "mode": "lan", "slots": 1}),
    ] {
        post(&cloud, &ann, body).await.is_error("bad_request", 400);
    }
    post(&cloud, &ann, listing(1)).await.ok(200);
    // One open listing per game.
    post(&cloud, &ann, listing(1)).await.is_error("limit", 409);
    // Three open listings at most.
    for n in 2..=3 {
        post(
            &cloud,
            &ann,
            json!({"title_id": format!("GAME0000{n}"), "mode": "lan", "slots": 1}),
        )
        .await
        .ok(200);
    }
    post(
        &cloud,
        &ann,
        json!({"title_id": "GAME00009", "mode": "lan", "slots": 1}),
    )
    .await
    .is_error("limit", 409);
}

#[tokio::test]
async fn a_block_hides_listings_both_ways() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    post(&cloud, &ann, listing(1)).await.ok(200);
    assert_eq!(board(&cloud, &ben, "").await.len(), 1);
    cloud
        .put(&format!("/v1/blocks/{}", ben.id), ann.token(), None)
        .await
        .ok(204);
    assert!(board(&cloud, &ben, "").await.is_empty());
    // And the blocked cannot ask.
    let id = {
        let mine: LfgMine = cloud.get("/v1/lfg/mine", ann.token()).await.ok(200).json();
        mine.posted[0].id.clone()
    };
    cloud
        .post_empty(&format!("/v1/lfg/{id}/ask"), ben.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn asking_and_accepting_lead_to_a_party_invitation() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    let cat = cloud.sign_up("Cat").await;
    let mut ann_events = cloud.events(ann.token()).await;
    let mut ben_events = cloud.events(ben.token()).await;
    let made: Listing = post(&cloud, &ann, listing(1)).await.ok(200).json();

    // Ben asks; Ann hears; asking twice changes nothing.
    let asked: Listing = cloud
        .post_empty(&format!("/v1/lfg/{}/ask", made.id), ben.token())
        .await
        .ok(200)
        .json();
    assert_eq!(asked.ask, Some(LfgAsk::Pending));
    cloud
        .post_empty(&format!("/v1/lfg/{}/ask", made.id), ben.token())
        .await
        .ok(200);
    let event = ann_events
        .until(|e| matches!(e, Event::LfgRequest { .. }))
        .await;
    let Event::LfgRequest { from, title_id, .. } = event else {
        unreachable!()
    };
    assert_eq!((from.id.0, title_id.as_str()), (ben.id, TITLE));
    ann_events
        .quiet(std::time::Duration::from_millis(200))
        .await;

    // The board shows Ben his own request; Ann sees the request on hers.
    assert_eq!(board(&cloud, &ben, "").await[0].ask, Some(LfgAsk::Pending));
    let mine: LfgMine = cloud.get("/v1/lfg/mine", ann.token()).await.ok(200).json();
    assert_eq!(mine.posted[0].requests.len(), 1);
    let theirs: LfgMine = cloud.get("/v1/lfg/mine", ben.token()).await.ok(200).json();
    assert_eq!(theirs.asked.len(), 1);

    // Only the poster accepts; Cat never asked.
    cloud
        .post_empty(
            &format!("/v1/lfg/{}/requests/{}/accept", made.id, ben.id),
            ben.token(),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post_empty(
            &format!("/v1/lfg/{}/requests/{}/accept", made.id, cat.id),
            ann.token(),
        )
        .await
        .is_error("not_found", 404);

    let accepted: Listing = cloud
        .post_empty(
            &format!("/v1/lfg/{}/requests/{}/accept", made.id, ben.id),
            ann.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(accepted.filled, 1);
    assert!(accepted.requests[0].accepted);
    let event = ben_events
        .until(|e| matches!(e, Event::LfgAccepted { .. }))
        .await;
    let Event::LfgAccepted { by, .. } = event else {
        unreachable!()
    };
    assert_eq!(by.id.0, ann.id);
    // They are not friends, and the invitation is there anyway.
    let invitations: PartyInvitations = cloud
        .get("/v1/party/invitations", ben.token())
        .await
        .ok(200)
        .json();
    assert_eq!(invitations.invitations.len(), 1);
    assert_eq!(invitations.invitations[0].from.id.0, ann.id);
    cloud
        .post_empty(
            &format!(
                "/v1/party/invitations/{}/accept",
                invitations.invitations[0].id
            ),
            ben.token(),
        )
        .await
        .ok(200);

    // The only place is taken, so the listing is closed for Cat.
    assert!(board(&cloud, &cat, "").await.is_empty());
    cloud
        .post_empty(&format!("/v1/lfg/{}/ask", made.id), cat.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn declining_withdrawing_and_closing() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    let cat = cloud.sign_up("Cat").await;
    let made: Listing = post(&cloud, &ann, listing(3)).await.ok(200).json();
    for user in [&ben, &cat] {
        cloud
            .post_empty(&format!("/v1/lfg/{}/ask", made.id), user.token())
            .await
            .ok(200);
    }
    // Declining takes the request away and tells nobody.
    let mut ben_events = cloud.events(ben.token()).await;
    let after: Listing = cloud
        .delete(
            &format!("/v1/lfg/{}/requests/{}", made.id, ben.id),
            ann.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(after.requests.len(), 1);
    ben_events
        .quiet(std::time::Duration::from_millis(300))
        .await;
    assert_eq!(board(&cloud, &ben, "").await.len(), 1);
    assert_eq!(board(&cloud, &ben, "").await[0].ask, None);
    // Cat withdraws.
    cloud
        .delete(&format!("/v1/lfg/{}/ask", made.id), cat.token())
        .await
        .ok(204);
    let mine: LfgMine = cloud.get("/v1/lfg/mine", ann.token()).await.ok(200).json();
    assert!(mine.posted[0].requests.is_empty());
    // Only the poster closes.
    cloud
        .delete(&format!("/v1/lfg/{}", made.id), ben.token())
        .await
        .is_error("not_found", 404);
    cloud
        .delete(&format!("/v1/lfg/{}", made.id), ann.token())
        .await
        .ok(204);
    assert!(board(&cloud, &ben, "").await.is_empty());
    // You cannot ask for your own listing.
    let own: Listing = post(&cloud, &ann, listing(1)).await.ok(200).json();
    cloud
        .post_empty(&format!("/v1/lfg/{}/ask", own.id), ann.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_listing_runs_out_and_the_sweep_deletes_it() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    let made: Listing = post(&cloud, &ann, listing(1)).await.ok(200).json();
    sqlx::query("UPDATE lfg_listings SET expires_ms = 1 WHERE id = $1")
        .bind(&made.id)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    assert!(board(&cloud, &ben, "").await.is_empty());
    cloud
        .post_empty(&format!("/v1/lfg/{}/ask", made.id), ben.token())
        .await
        .is_error("not_found", 404);
    cloud.state.sweep().await.unwrap();
    let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM lfg_listings")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_listing_can_be_reported_and_a_ban_closes_it() {
    const ADMIN: &str = "admin-secret";
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    let made: Listing = post(&cloud, &ann, listing(1)).await.ok(200).json();
    // The report must name the listing's poster.
    cloud
        .post(
            "/v1/reports",
            ben.token(),
            json!({"account": ben.id, "reason": "spam", "listing": made.id}),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .post(
            "/v1/reports",
            ben.token(),
            json!({"account": ann.id, "reason": "spam", "listing": "nothing"}),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            "/v1/reports",
            ben.token(),
            json!({"account": ann.id, "reason": "spam", "listing": made.id}),
        )
        .await
        .ok(204);
    let (content,): (String,) = sqlx::query_as("SELECT content FROM reports")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert!(
        content.contains("chill runs") && content.contains(TITLE),
        "{content}"
    );

    // A ban takes the listing off the board.
    cloud
        .request(
            reqwest::Method::POST,
            "/admin/v1/actions",
            Some(ADMIN),
            Some(json!({"handle": ann.handle, "kind": "ban", "rule": "spam", "reason": "Spam."})),
        )
        .await
        .ok(200);
    assert!(board(&cloud, &ben, "").await.is_empty());
}
