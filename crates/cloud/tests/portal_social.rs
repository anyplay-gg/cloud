//! The social side of the portal API: friends, the player's own profile, privacy and avatar,
//! trophies, and the public profile of any player, per viewer.

mod common;

use common::portal::{API, Portal};
use common::{TestCloud, User};
use reqwest::Method;
use serde_json::{Value, json};

// Friends

#[tokio::test]
async fn friend_requests_are_sent_accepted_declined_and_ended_in_the_portal() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let cleo = cloud.sign_up("Cleo").await;
    let a = Portal::signed_in(&cloud, &alec).await;
    let b = Portal::signed_in(&cloud, &bob).await;
    let c = Portal::signed_in(&cloud, &cleo).await;

    let sent = a
        .post("/friends/requests", json!({"handle": "bob"}))
        .await
        .ok(200);
    assert_eq!(sent["state"], "requested");
    a.post("/friends/requests", json!({"handle": "nobody"}))
        .await
        .error(404, "not_found");
    a.post("/friends/requests", json!({"handle": "Alec"}))
        .await
        .error(400, "bad_request");

    let lists = b.get("/friends").await.ok(200);
    assert_eq!(lists["incoming"][0]["account"]["handle"], "Alec");
    assert_eq!(
        a.get("/friends").await.ok(200)["outgoing"][0]["account"]["handle"],
        "Bob"
    );
    b.post(&format!("/friends/requests/{}/accept", alec.id), json!({}))
        .await
        .ok(200);
    let lists = a.get("/friends").await.ok(200);
    assert_eq!(lists["friends"][0]["account"]["handle"], "Bob");
    assert_eq!(lists["friends"][0]["presence"]["online"], false);
    assert_eq!(lists["incoming"], json!([]));

    // Decline and withdraw.
    c.post("/friends/requests", json!({"handle": "Alec"}))
        .await
        .ok(200);
    a.delete(&format!("/friends/requests/{}", cleo.id))
        .await
        .ok(200);
    assert_eq!(c.get("/friends").await.ok(200)["outgoing"], json!([]));
    a.post("/friends/requests", json!({"handle": "Cleo"}))
        .await
        .ok(200);
    c.delete(&format!("/friends/requests/{}", alec.id))
        .await
        .ok(200);
    a.delete(&format!("/friends/requests/{}", cleo.id))
        .await
        .error(404, "not_found");

    // Block, unblock, end the friendship.
    a.send(Method::PUT, &format!("/blocks/{}", cleo.id), json!({}))
        .await
        .ok(200);
    assert_eq!(
        a.get("/friends").await.ok(200)["blocked"][0]["handle"],
        "Cleo"
    );
    a.delete(&format!("/blocks/{}", cleo.id)).await.ok(200);
    a.delete(&format!("/friends/{}", bob.id)).await.ok(200);
    a.delete(&format!("/friends/{}", bob.id))
        .await
        .error(404, "not_found");
    assert_eq!(b.get("/friends").await.ok(200)["friends"], json!([]));

    // A signed-out browser sees none of it.
    Portal::new(&cloud)
        .get("/friends")
        .await
        .error(401, "unauthorized");
}

// Profiles and trophies

fn png() -> Vec<u8> {
    let picture =
        image::RgbaImage::from_fn(64, 64, |x, y| image::Rgba([x as u8, y as u8, 200, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[tokio::test]
async fn the_player_edits_their_own_profile_privacy_and_avatar() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let portal = Portal::signed_in(&cloud, &alec).await;

    let profile = portal.get("/me/profile").await.ok(200);
    assert_eq!(profile["account"]["handle"], "Alec");
    assert_eq!(profile["about"], Value::Null);
    let updated = portal
        .send(
            Method::PATCH,
            "/me/profile",
            json!({"about": "I play at night.", "favourites": ["PPSA01234"]}),
        )
        .await
        .ok(200);
    assert_eq!(updated["about"], "I play at night.");
    assert_eq!(
        portal.get("/me/profile").await.ok(200)["favourites"],
        json!(["PPSA01234"])
    );
    portal
        .send(
            Method::PATCH,
            "/me/profile",
            json!({"about": "see https://example.com"}),
        )
        .await
        .error(400, "bad_request");
    portal
        .send(
            Method::PATCH,
            "/me/profile",
            json!({"favourites": ["not a title"]}),
        )
        .await
        .error(400, "bad_request");

    let privacy = portal.get("/me/privacy").await.ok(200);
    assert_eq!(privacy["about"], "friends");
    let set = portal
        .send(
            Method::PUT,
            "/me/privacy",
            json!({"about": "everyone", "trophies": "nobody", "favourites": "friends",
                   "avatar": "everyone", "playtime": "friends"}),
        )
        .await
        .ok(200);
    assert_eq!(set["about"], "everyone");
    assert_eq!(
        portal.get("/me/privacy").await.ok(200)["trophies"],
        "nobody"
    );

    // The picture is a PNG or JPEG body; a form cannot send those either.
    let upload = |bytes: Vec<u8>, content_type: &'static str| {
        portal.dispatch(
            portal
                .client
                .put(portal.url(&format!("{API}/me/avatar")))
                .header("x-csrf-token", portal.token())
                .header("content-type", content_type)
                .body(bytes),
        )
    };
    let hash = upload(png(), "image/png").await.ok(200)["avatar"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(portal.get("/me/profile").await.ok(200)["avatar"], hash);
    upload(png(), "text/plain")
        .await
        .error(415, "unsupported_media_type");
    upload(b"not a picture".to_vec(), "image/png")
        .await
        .error(400, "bad_request");
    // The picture is served to the page.
    let served = cloud
        .request(Method::GET, &format!("/v1/avatars/{hash}"), None, None)
        .await;
    assert_eq!(served.status, 200);
    portal.delete("/me/avatar").await.ok(200);
    assert_eq!(
        portal.get("/me/profile").await.ok(200)["avatar"],
        Value::Null
    );
}

async fn upload_trophies(cloud: &TestCloud, user: &User, title: &str, trophies: Value) {
    cloud
        .put(
            &format!("/v1/me/trophies/{title}"),
            user.token(),
            Some(json!({"trophies": trophies})),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn a_public_profile_shows_each_viewer_what_the_player_shows_them() {
    let cloud = TestCloud::start().await;
    let (alec, bob) = cloud.friends("Alec", "Bob").await;
    let cleo = cloud.sign_up("Cleo").await;
    let dave = cloud.sign_up("Dave").await;
    cloud
        .patch(
            "/v1/me/profile",
            alec.token(),
            json!({"about": "I play at night.", "favourites": ["PPSA01234"]}),
        )
        .await
        .ok(200);
    // Everyone sees the text; friends the favourites; nobody the trophies.
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(
                json!({"about": "everyone", "favourites": "friends", "trophies": "nobody",
                        "avatar": "friends", "playtime": "friends"}),
            ),
        )
        .await
        .ok(200);
    upload_trophies(
        &cloud,
        &alec,
        "PPSA01234",
        json!([{"number": 1, "grade": "bronze", "earned": 1_700_000_000u64}]),
    )
    .await;

    // Signed out: a stranger.
    let out = Portal::new(&cloud);
    let seen = out.get("/users/ALEC").await.ok(200);
    assert_eq!(seen["account"]["handle"], "Alec");
    assert_eq!(seen["about"], "I play at night.");
    assert_eq!(seen["favourites"], Value::Null);
    assert_eq!(seen["trophies"], Value::Null);
    assert_eq!(seen["viewer"], Value::Null);
    out.get("/users/alec/trophies")
        .await
        .error(404, "not_found");

    // A signed-in stranger sees the same, and how they stand to the player.
    let stranger = Portal::signed_in(&cloud, &cleo).await;
    let seen = stranger.get("/users/alec").await.ok(200);
    assert_eq!(seen["about"], "I play at night.");
    assert_eq!(seen["favourites"], Value::Null);
    assert_eq!(
        seen["viewer"],
        json!({"relation": "none", "requested": null, "blocked": false})
    );
    stranger
        .post("/friends/requests", json!({"handle": "Alec"}))
        .await
        .ok(200);
    assert_eq!(
        stranger.get("/users/alec").await.ok(200)["viewer"]["requested"],
        "outgoing"
    );
    let alecs = Portal::signed_in(&cloud, &alec).await;
    assert_eq!(
        alecs.get("/users/cleo").await.ok(200)["viewer"]["requested"],
        "incoming"
    );

    // A friend sees the favourites too.
    let friend = Portal::signed_in(&cloud, &bob).await;
    let seen = friend.get("/users/alec").await.ok(200);
    assert_eq!(seen["favourites"], json!(["PPSA01234"]));
    assert_eq!(seen["viewer"]["relation"], "friend");
    assert_eq!(seen["trophies"], Value::Null);

    // The player sees everything of themselves, trophies included.
    let own = alecs.get("/users/alec").await.ok(200);
    assert_eq!(own["viewer"]["relation"], "self");
    assert_eq!(own["trophies"]["bronze"], 1);
    assert_eq!(own["favourites"], json!(["PPSA01234"]));

    // Once the trophies are shown to friends, the friend sees them and the stranger does not.
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "friends"})),
        )
        .await
        .ok(200);
    assert_eq!(
        friend.get("/users/alec").await.ok(200)["trophies"]["bronze"],
        1
    );
    assert_eq!(
        stranger.get("/users/alec").await.ok(200)["trophies"],
        Value::Null
    );

    // A block does not show on the page: whoever is blocked, either way, sees exactly what a
    // visitor who is not signed in sees, and no relation. (Less would tell them of the block.)
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(
                json!({"about": "everyone", "favourites": "friends", "trophies": "everyone",
                        "avatar": "everyone", "playtime": "everyone"}),
            ),
        )
        .await
        .ok(200);
    let without_viewer = |mut page: Value| {
        page.as_object_mut().unwrap().remove("viewer");
        page
    };
    let public = without_viewer(out.get("/users/alec").await.ok(200));
    assert_eq!(public["about"], "I play at night.");
    assert_eq!(public["trophies"]["bronze"], 1);
    assert_eq!(public["favourites"], Value::Null);
    let public_trophies = out.get("/users/alec/trophies").await.ok(200);

    alecs
        .send(Method::PUT, &format!("/blocks/{}", dave.id), json!({}))
        .await
        .ok(200);
    let blocked_viewer = Portal::signed_in(&cloud, &dave).await;
    let seen = blocked_viewer.get("/users/alec").await.ok(200);
    assert_eq!(seen["viewer"]["relation"], "none");
    assert_eq!(seen["viewer"]["blocked"], false, "Dave did not block Alec");
    assert_eq!(
        without_viewer(seen),
        public,
        "the same as a signed-out visitor"
    );
    assert_eq!(
        blocked_viewer.get("/users/alec/trophies").await.ok(200),
        public_trophies
    );
    // Alec's own page of Dave shows the block he made.
    assert_eq!(
        alecs.get("/users/dave").await.ok(200)["viewer"]["blocked"],
        true
    );
    // The other way round: Dave blocks Alec. Alec still sees what a stranger of Dave sees.
    blocked_viewer
        .send(Method::PUT, &format!("/blocks/{}", alec.id), json!({}))
        .await
        .ok(200);
    let daves = blocked_viewer.get("/users/alec").await.ok(200);
    assert_eq!(daves["viewer"]["blocked"], true);
    assert_eq!(without_viewer(daves), public);

    // Nobody by that name, and names that cannot be handles.
    out.get("/users/nobody").await.error(404, "not_found");
    out.get("/users/way-too-long-for-a-handle")
        .await
        .error(404, "not_found");
    out.get("/users/a%20b").await.error(404, "not_found");
}

#[tokio::test]
async fn trophies_are_counts_per_title_and_follow_the_privacy() {
    let cloud = TestCloud::start().await;
    let (alec, bob) = cloud.friends("Alec", "Bob").await;
    let t = 1_700_000_000u64;
    upload_trophies(
        &cloud,
        &alec,
        "PPSA01234",
        json!([
            {"number": 1, "grade": "bronze", "earned": t},
            {"number": 2, "grade": "gold", "earned": t + 10},
            {"number": 3, "grade": "bronze", "earned": t + 20},
        ]),
    )
    .await;
    upload_trophies(
        &cloud,
        &alec,
        "PPSA09999",
        json!([{"number": 1, "grade": "platinum", "earned": t + 5}]),
    )
    .await;

    let own = Portal::signed_in(&cloud, &alec).await;
    let mine = own.get("/me/trophies").await.ok(200);
    assert_eq!(
        mine["counts"],
        json!({"bronze": 2, "silver": 0, "gold": 1, "platinum": 1})
    );
    let titles = mine["titles"].as_array().unwrap();
    assert_eq!(titles.len(), 2);
    // The title with the latest trophy first; only counts, never a name or a text.
    assert_eq!(titles[0]["title_id"], "PPSA01234");
    assert_eq!(titles[0]["earned"], 3);
    assert_eq!(titles[0]["last_earned"], t + 20);
    assert_eq!(titles[0]["counts"]["gold"], 1);
    assert_eq!(titles[1]["title_id"], "PPSA09999");
    assert!(!mine.to_string().contains("name"));

    // Friends see them by default; strangers and the signed-out do not.
    let friend = Portal::signed_in(&cloud, &bob).await;
    assert_eq!(
        friend.get("/users/alec/trophies").await.ok(200)["counts"]["platinum"],
        1
    );
    let cleo = cloud.sign_up("Cleo").await;
    let stranger = Portal::signed_in(&cloud, &cleo).await;
    stranger
        .get("/users/alec/trophies")
        .await
        .error(404, "not_found");
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "everyone"})),
        )
        .await
        .ok(200);
    assert_eq!(
        Portal::new(&cloud)
            .get("/users/alec/trophies")
            .await
            .ok(200)["titles"][0]["title_id"],
        "PPSA01234"
    );
    Portal::new(&cloud)
        .get("/me/trophies")
        .await
        .error(401, "unauthorized");
}

#[tokio::test]
async fn public_reads_are_limited_per_address() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.portal_public = 3).await;
    cloud.sign_up("Alec").await;
    let portal = Portal::new(&cloud);
    for _ in 0..3 {
        portal.get("/users/alec").await.ok(200);
    }
    let limited = portal.get("/users/alec").await;
    limited.error(429, "rate_limited");
    assert!(limited.header("retry-after").is_some());
}

#[tokio::test]
async fn trophies_are_counted_by_the_store_not_read_one_by_one() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let t = 1_700_000_000u64;
    let many: Vec<Value> = (0..200u64)
        .map(|n| {
            json!({"number": n, "grade": if n % 4 == 0 { "silver" } else { "bronze" }, "earned": t + n})
        })
        .collect();
    upload_trophies(&cloud, &alec, "PPSA01234", json!(many)).await;
    upload_trophies(
        &cloud,
        &alec,
        "PPSA09999",
        json!([{"number": 1, "grade": "gold", "earned": t}]),
    )
    .await;
    // Two grades in one title and one in the other: three rows, however many records.
    let rows = anyplay_cloud::db::trophy_summary(&cloud.state.db, alec.id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    let own = Portal::signed_in(&cloud, &alec).await;
    let mine = own.get("/me/trophies").await.ok(200);
    assert_eq!(
        mine["counts"],
        json!({"bronze": 150, "silver": 50, "gold": 1, "platinum": 0})
    );
    assert_eq!(mine["titles"][0]["title_id"], "PPSA01234");
    assert_eq!(mine["titles"][0]["earned"], 200);
    assert_eq!(mine["titles"][0]["last_earned"], t + 199);
}
