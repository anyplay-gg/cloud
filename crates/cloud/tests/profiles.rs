mod common;

use std::io::Write as _;

use anyplay_api::{
    AvatarSet, Cards, MAX_ABOUT, MAX_AVATAR_UPLOAD, MAX_DAY_MINUTES, MAX_PLAYTIME_UPLOAD,
    PlaytimeAdded, Privacy, Profile, TitlePlaytime,
};
use base64::Engine as _;
use common::{Reply, TestCloud, User};
use image::{ImageEncoder as _, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

// A cloud whose avatar limit does not get in the way of the tests that upload often.
async fn start() -> TestCloud {
    TestCloud::with(|_| {}, |t| t.rates.avatar_uploads = 1000).await
}

fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86_400;
    day_text(days as i64)
}

/// `YYYY-MM-DD` of a day number, after Howard Hinnant's civil_from_days.
fn day_text(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

// Pictures

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let picture = RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .write_image(
            picture.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    bytes
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let picture = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([(x % 256) as u8, (y % 256) as u8, 200, 128])
    });
    let mut bytes = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out
}

/// A PNG that claims `width` by `height` pixels at this depth and colour type; its data is all
/// zeros, which compresses about a thousand to one, so the file is small.
fn bomb(width: u32, height: u32, depth: u8, colour: u8, bytes_per_pixel_bits: u64) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[depth, colour, 0, 0, 0]);
    let row = vec![0u8; 1 + (u64::from(width) * bytes_per_pixel_bits).div_ceil(8) as usize];
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    for _ in 0..height {
        zlib.write_all(&row).unwrap();
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &header));
    out.extend(chunk(b"IDAT", &zlib.finish().unwrap()));
    out.extend(chunk(b"IEND", b""));
    out
}

/// The chunk names of a PNG, in order.
fn chunks(png: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        names.push(String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned());
        at += 12 + len;
    }
    names
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// Requests with a body that is not JSON

struct Raw {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Raw {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("body {:?} (status {}): {e}", self.body.len(), self.status))
    }

    fn is_error(&self, code: &str, status: u16) {
        assert_eq!(
            self.status,
            status,
            "body: {}",
            String::from_utf8_lossy(&self.body)
        );
        let body: Value = serde_json::from_slice(&self.body).unwrap();
        assert_eq!(body["error"], code, "body: {body}");
    }

    fn ok(self, status: u16) -> Self {
        assert_eq!(
            self.status,
            status,
            "body: {}",
            String::from_utf8_lossy(&self.body)
        );
        self
    }
}

async fn send(request: reqwest::RequestBuilder) -> Raw {
    let response = request.send().await.unwrap();
    Raw {
        status: response.status().as_u16(),
        headers: response.headers().clone(),
        body: response.bytes().await.unwrap().to_vec(),
    }
}

async fn put_avatar(cloud: &TestCloud, user: &User, content_type: &str, body: Vec<u8>) -> Raw {
    send(
        cloud
            .http
            .put(format!("http://{}/v1/me/avatar", cloud.base))
            .bearer_auth(user.token())
            .header("content-type", content_type)
            .body(body),
    )
    .await
}

async fn upload_avatar(cloud: &TestCloud, user: &User, body: Vec<u8>) -> String {
    let set: AvatarSet = put_avatar(cloud, user, "image/png", body)
        .await
        .ok(200)
        .json();
    set.avatar
}

async fn picture(cloud: &TestCloud, hash: &str) -> Raw {
    send(
        cloud
            .http
            .get(format!("http://{}/v1/avatars/{hash}", cloud.base)),
    )
    .await
}

// Profiles

async fn profile_of(cloud: &TestCloud, viewer: &User, owner: &User) -> Profile {
    cloud
        .get(
            &format!("/v1/accounts/{}/profile", owner.id),
            viewer.token(),
        )
        .await
        .ok(200)
        .json()
}

async fn set_profile(cloud: &TestCloud, user: &User, body: Value) -> Reply {
    cloud.patch("/v1/me/profile", user.token(), body).await
}

async fn upload_playtime(cloud: &TestCloud, user: &User, entries: Vec<Value>) -> Reply {
    cloud
        .put(
            "/v1/me/playtime",
            user.token(),
            Some(json!({"entries": entries})),
        )
        .await
}

fn entry(title: &str, day: &str, minutes: u32) -> Value {
    json!({"title_id": title, "day": day, "minutes": minutes})
}

/// Fills every part of the profile of `user`.
async fn fill(cloud: &TestCloud, user: &User) -> String {
    set_profile(
        cloud,
        user,
        json!({"about": "Speedruns and sunsets", "favourites": ["GAME00001", "GAME00002"]}),
    )
    .await
    .ok(200);
    let hash = upload_avatar(cloud, user, png(64, 64)).await;
    upload_playtime(cloud, user, vec![entry("GAME00001", &today(), 90)])
        .await
        .ok(200);
    hash
}

/// Which of about, favourites, avatar and playtime a profile shows.
fn shown(profile: &Profile) -> [bool; 4] {
    [
        profile.about.is_some(),
        profile.favourites.is_some(),
        profile.avatar.is_some(),
        profile.playtime.is_some(),
    ]
}

const PARTS: [&str; 4] = ["about", "favourites", "avatar", "playtime"];

#[tokio::test]
async fn each_part_follows_its_audience_for_self_friend_stranger_and_blocked() {
    let cloud = start().await;
    let (alec, sam) = cloud.friends("Alec", "Sam").await;
    let stranger = cloud.sign_up("Stranger").await;
    let blocked = cloud.sign_up("Blocked").await;
    let blocker = cloud.sign_up("Blocker").await;
    let hash = fill(&cloud, &alec).await;
    cloud
        .put(&format!("/v1/blocks/{}", blocked.id), alec.token(), None)
        .await
        .ok(204);
    cloud
        .put(&format!("/v1/blocks/{}", alec.id), blocker.token(), None)
        .await
        .ok(204);

    // Before anything is set, everything is for friends.
    let privacy: Privacy = cloud
        .get("/v1/me/privacy", alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(privacy, Privacy::default());
    assert_eq!(shown(&profile_of(&cloud, &sam, &alec).await), [true; 4]);
    assert_eq!(
        shown(&profile_of(&cloud, &stranger, &alec).await),
        [false; 4]
    );

    for (n, part) in PARTS.iter().enumerate() {
        for audience in ["friends", "everyone", "nobody"] {
            // The part under test has the audience, the others are shut.
            let mut body = json!({"trophies": "nobody"});
            for other in PARTS {
                body[other] = json!(if other == *part { audience } else { "nobody" });
            }
            cloud
                .put("/v1/me/privacy", alec.token(), Some(body))
                .await
                .ok(200);

            let only = |yes: bool| {
                let mut parts = [false; 4];
                parts[n] = yes;
                parts
            };
            let context = format!("{part}: {audience}");
            assert_eq!(
                shown(&profile_of(&cloud, &alec, &alec).await),
                [true; 4],
                "self, {context}"
            );
            assert_eq!(
                shown(&profile_of(&cloud, &sam, &alec).await),
                only(audience != "nobody"),
                "friend, {context}"
            );
            assert_eq!(
                shown(&profile_of(&cloud, &stranger, &alec).await),
                only(audience == "everyone"),
                "stranger, {context}"
            );
            for viewer in [&blocked, &blocker] {
                assert_eq!(
                    shown(&profile_of(&cloud, viewer, &alec).await),
                    [false; 4],
                    "blocked, {context}"
                );
            }

            // The gamer card shows the avatar by the same rule, and nothing else is added.
            let card = |viewer: &User| {
                let path = format!("/v1/cards?accounts={}", alec.id);
                let token = viewer.token().to_owned();
                let cloud = &cloud;
                async move {
                    let reply = cloud.get(&path, &token).await.ok(200);
                    let cards: Cards = reply.json();
                    (cards.cards[0].avatar.clone(), reply.body)
                }
            };
            let (own, body) = card(&alec).await;
            assert_eq!(own.as_deref(), Some(hash.as_str()));
            let mut keys: Vec<_> = body["cards"][0]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            keys.sort();
            assert_eq!(keys, ["account", "avatar", "trophies"], "{context}");
            let (friend, _) = card(&sam).await;
            let (other, _) = card(&stranger).await;
            let (shut, _) = card(&blocked).await;
            let avatar_part = *part == "avatar";
            assert_eq!(
                friend.is_some(),
                avatar_part && audience != "nobody",
                "card, friend, {context}"
            );
            assert_eq!(
                other.is_some(),
                avatar_part && audience == "everyone",
                "card, stranger, {context}"
            );
            assert_eq!(shut, None, "card, blocked, {context}");
        }
    }
}

#[tokio::test]
async fn privacy_written_before_profiles_keeps_working_and_new_parts_start_at_friends() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"trophies": "everyone"})),
        )
        .await
        .ok(200);
    let privacy: Value = cloud.get("/v1/me/privacy", alec.token()).await.ok(200).body;
    assert_eq!(
        privacy,
        json!({"trophies": "everyone", "about": "friends", "favourites": "friends",
               "avatar": "friends", "playtime": "friends", "feed": "friends"})
    );
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"about": "public"})),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn the_text_and_favourites_are_checked_and_changed_part_by_part() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    let empty = profile_of(&cloud, &alec, &alec).await;
    assert_eq!(empty.account.handle, "Alec");
    assert_eq!(shown(&empty), [false; 4]);

    let set: Profile = set_profile(
        &cloud,
        &alec,
        json!({"about": "  Hello, I play late.  ", "favourites": ["GAME00002", "GAME00001"]}),
    )
    .await
    .ok(200)
    .json();
    assert_eq!(set.about.as_deref(), Some("Hello, I play late."));
    assert_eq!(
        set.favourites,
        Some(vec!["GAME00002".into(), "GAME00001".into()])
    );

    // A part left out stays.
    let set: Profile = set_profile(&cloud, &alec, json!({"about": "New text"}))
        .await
        .ok(200)
        .json();
    assert_eq!(set.about.as_deref(), Some("New text"));
    assert_eq!(set.favourites.as_ref().map(Vec::len), Some(2));
    let set: Profile = set_profile(&cloud, &alec, json!({"favourites": []}))
        .await
        .ok(200)
        .json();
    assert_eq!(set.about.as_deref(), Some("New text"));
    assert_eq!(set.favourites, None);
    let set: Profile = set_profile(&cloud, &alec, json!({"about": ""}))
        .await
        .ok(200)
        .json();
    assert_eq!(shown(&set), [false; 4]);

    // The longest text passes, one more does not; characters count, not bytes.
    set_profile(&cloud, &alec, json!({"about": "ä".repeat(MAX_ABOUT)}))
        .await
        .ok(200);
    for about in [
        "ä".repeat(MAX_ABOUT + 1),
        "two\nlines".into(),
        "bell\u{7}".into(),
        "flip \u{202e}text".into(),
        "see https://example.com".into(),
        "www.example".into(),
        "add me: someone@example.org".into(),
        "mailto:someone@example.org".into(),
        "example.com".into(),
    ] {
        set_profile(&cloud, &alec, json!({"about": about}))
            .await
            .is_error("bad_request", 400);
    }
    set_profile(&cloud, &alec, json!({"about": "Version 1.5 and e.g. this"}))
        .await
        .ok(200);

    for favourites in [
        json!(["GAME00001", "GAME00002", "GAME00003", "GAME00004"]),
        json!(["GAME00001", "GAME00001"]),
        json!(["../admin"]),
        json!(["a b"]),
        json!([""]),
    ] {
        set_profile(&cloud, &alec, json!({"favourites": favourites}))
            .await
            .is_error("bad_request", 400);
    }
    set_profile(&cloud, &alec, json!({"about": 5}))
        .await
        .is_error("bad_request", 400);
    // Nothing of the refused requests was kept.
    let kept = profile_of(&cloud, &alec, &alec).await;
    assert_eq!(kept.about.as_deref(), Some("Version 1.5 and e.g. this"));
    cloud
        .request(
            reqwest::Method::PATCH,
            "/v1/me/profile",
            None,
            Some(json!({})),
        )
        .await
        .is_error("unauthorized", 401);
    cloud
        .get("/v1/accounts/987654/profile", alec.token())
        .await
        .is_error("not_found", 404);
}

// Avatars

#[tokio::test]
async fn an_avatar_becomes_a_square_png_served_by_its_hash_with_long_cache_headers() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;

    let hash = put_avatar(&cloud, &alec, "image/jpeg", jpeg(300, 120))
        .await
        .ok(200)
        .json::<AvatarSet>()
        .avatar;
    assert!(anyplay_api::is_avatar_hash(&hash), "{hash}");
    assert_eq!(
        profile_of(&cloud, &alec, &alec).await.avatar.as_deref(),
        Some(hash.as_str())
    );

    let served = picture(&cloud, &hash).await.ok(200);
    let header = |name: &str| {
        served
            .headers
            .get(name)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(header("content-type"), "image/png");
    assert_eq!(
        header("cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(header("etag"), format!("\"{hash}\""));
    assert_eq!(header("x-content-type-options"), "nosniff");
    let decoded = image::load_from_memory(&served.body).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (256, 256));
    assert_eq!(
        Sha256::digest(&served.body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        hash,
        "the hash is the SHA-256 of the served bytes"
    );

    // The same picture again is the same address; a revalidation costs no body.
    let again_hash = put_avatar(&cloud, &alec, "image/jpeg", jpeg(300, 120))
        .await
        .ok(200)
        .json::<AvatarSet>()
        .avatar;
    assert_eq!(again_hash, hash);
    let again = send(
        cloud
            .http
            .get(format!("http://{}/v1/avatars/{hash}", cloud.base))
            .header("if-none-match", format!("\"{hash}\"")),
    )
    .await;
    assert_eq!(again.status, 304);
    assert!(again.body.is_empty());
    assert_eq!(
        again.headers.get("cache-control").unwrap(),
        "public, max-age=31536000, immutable"
    );
}

#[tokio::test]
async fn a_new_avatar_replaces_the_old_one_and_removal_leaves_none() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    let first = upload_avatar(&cloud, &alec, png(100, 100)).await;
    picture(&cloud, &first).await.ok(200);

    let second = upload_avatar(&cloud, &alec, dark_png()).await;
    assert_ne!(first, second);
    picture(&cloud, &first).await.is_error("not_found", 404);
    picture(&cloud, &second).await.ok(200);
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM avatars")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 1, "the old picture is gone from the store");

    cloud.delete("/v1/me/avatar", alec.token()).await.ok(204);
    picture(&cloud, &second).await.is_error("not_found", 404);
    assert_eq!(profile_of(&cloud, &alec, &alec).await.avatar, None);
    // Removing none is fine.
    cloud.delete("/v1/me/avatar", alec.token()).await.ok(204);

    // Not a hash, or one that does not exist.
    picture(&cloud, "../../etc/passwd")
        .await
        .is_error("not_found", 404);
    picture(&cloud, &"A".repeat(64))
        .await
        .is_error("not_found", 404);
    picture(&cloud, &"0".repeat(64))
        .await
        .is_error("not_found", 404);
}

fn dark_png() -> Vec<u8> {
    let picture = RgbImage::from_pixel(80, 80, Rgb([10, 10, 10]));
    let mut bytes = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

#[tokio::test]
async fn pictures_that_are_too_big_not_pictures_or_bombs_are_refused() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;

    // Over 1 MB.
    let big = vec![0x89; MAX_AVATAR_UPLOAD + 1];
    // The cloud answers from the headers and closes without reading the body, so the client may
    // see the connection close before it has sent all of it. Either way the picture is refused.
    let refused = cloud
        .http
        .put(format!("http://{}/v1/me/avatar", cloud.base))
        .bearer_auth(alec.token())
        .header("content-type", "image/png")
        .body(big)
        .send()
        .await;
    if let Ok(response) = refused {
        assert_eq!(response.status().as_u16(), 413);
    }

    // Not a picture, whatever it says it is.
    put_avatar(&cloud, &alec, "image/png", b"hello".to_vec())
        .await
        .is_error("bad_request", 400);
    put_avatar(&cloud, &alec, "image/png", vec![])
        .await
        .is_error("bad_request", 400);
    put_avatar(&cloud, &alec, "image/jpeg", png(8, 8))
        .await
        .is_error("bad_request", 400);
    put_avatar(&cloud, &alec, "image/png", jpeg(8, 8))
        .await
        .is_error("bad_request", 400);
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0, 0x3b]);
    put_avatar(&cloud, &alec, "image/gif", gif.clone())
        .await
        .is_error("bad_request", 400);
    put_avatar(&cloud, &alec, "image/png", gif)
        .await
        .is_error("bad_request", 400);
    put_avatar(&cloud, &alec, "image/webp", b"RIFF....WEBPVP8 ".to_vec())
        .await
        .is_error("bad_request", 400);
    put_avatar(&cloud, &alec, "text/plain", png(8, 8))
        .await
        .is_error("bad_request", 400);
    send(
        cloud
            .http
            .put(format!("http://{}/v1/me/avatar", cloud.base))
            .bearer_auth(alec.token())
            .body(png(8, 8)),
    )
    .await
    .is_error("bad_request", 400);
    // A cut-off picture.
    let whole = png(64, 64);
    put_avatar(
        &cloud,
        &alec,
        "image/png",
        whole[..whole.len() / 2].to_vec(),
    )
    .await
    .is_error("bad_request", 400);
    // No token.
    send(
        cloud
            .http
            .put(format!("http://{}/v1/me/avatar", cloud.base))
            .header("content-type", "image/png")
            .body(png(8, 8)),
    )
    .await
    .is_error("unauthorized", 401);

    // A small file that claims a huge picture: 20,000 by 20,000 pixels in under 100 KB.
    let wide = bomb(20_000, 20_000, 1, 0, 1);
    assert!(
        wide.len() < MAX_AVATAR_UPLOAD,
        "the bomb is a small file: {}",
        wide.len()
    );
    put_avatar(&cloud, &alec, "image/png", wide)
        .await
        .is_error("too_large", 413);
    // Within the side limit but needing 134 MB to decode.
    let heavy = bomb(4096, 4096, 16, 6, 64);
    assert!(
        heavy.len() < MAX_AVATAR_UPLOAD,
        "the bomb is a small file: {}",
        heavy.len()
    );
    put_avatar(&cloud, &alec, "image/png", heavy)
        .await
        .is_error("too_large", 413);
    // The same in a JPEG: a real header with its size changed to 8,000 by 8,000.
    let mut huge = jpeg(16, 16);
    let sof = huge.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    huge[sof + 5..sof + 9].copy_from_slice(&[0x1F, 0x40, 0x1F, 0x40]);
    put_avatar(&cloud, &alec, "image/jpeg", huge)
        .await
        .is_error("too_large", 413);

    // Nothing was kept, and the cloud still decodes a good picture afterwards.
    assert_eq!(profile_of(&cloud, &alec, &alec).await.avatar, None);
    upload_avatar(&cloud, &alec, png(4096, 16)).await;
}

#[tokio::test]
async fn nothing_of_the_original_file_survives_the_new_picture() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;

    // A PNG with text chunks and an unknown chunk before its end.
    let mut original = png(40, 40);
    let end = original.len() - 12;
    let mut extra = chunk(b"tEXt", b"Comment\0SECRET-PNG-TEXT");
    extra.extend(chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0SECRET-PNG-XMP"));
    extra.extend(chunk(b"eXIf", b"MM\0*SECRET-PNG-EXIF"));
    extra.extend(chunk(b"zzZz", b"SECRET-PNG-PRIVATE"));
    original.splice(end..end, extra);
    assert!(chunks(&original).contains(&"tEXt".to_string()));
    let hash = upload_avatar(&cloud, &alec, original.clone()).await;
    let made = picture(&cloud, &hash).await.ok(200).body;
    for secret in [&b"SECRET"[..], b"Comment", b"Exif", b"XML:com"] {
        assert!(
            !contains(&made, secret),
            "{}",
            String::from_utf8_lossy(secret)
        );
    }
    let names = chunks(&made);
    assert_eq!(names.first().map(String::as_str), Some("IHDR"));
    assert_eq!(names.last().map(String::as_str), Some("IEND"));
    assert!(
        names
            .iter()
            .all(|n| ["IHDR", "IDAT", "IEND"].contains(&n.as_str())),
        "only the header, the data and the end: {names:?}"
    );

    // A JPEG with an EXIF block, a comment and an XMP block after its start.
    let original = jpeg(64, 64);
    let segment = |marker: u8, body: &[u8]| {
        let mut out = vec![0xFF, marker];
        out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(body);
        out
    };
    let mut with_metadata = original[..2].to_vec();
    with_metadata.extend(segment(0xE1, b"Exif\0\0SECRET-JPEG-EXIF"));
    with_metadata.extend(segment(0xFE, b"SECRET-JPEG-COMMENT"));
    with_metadata.extend(segment(
        0xE1,
        b"http://ns.adobe.com/xap/1.0/\0SECRET-JPEG-XMP",
    ));
    with_metadata.extend_from_slice(&original[2..]);
    let hash = put_avatar(&cloud, &alec, "image/jpeg", with_metadata)
        .await
        .ok(200)
        .json::<AvatarSet>()
        .avatar;
    let made = picture(&cloud, &hash).await.ok(200).body;
    assert!(!contains(&made, b"SECRET"));
    assert!(!contains(&made, b"Exif"));
    assert!(!contains(&made, b"adobe"));
    assert_eq!(chunks(&made).first().map(String::as_str), Some("IHDR"));
    // The picture is the same as without the metadata.
    let plain = put_avatar(&cloud, &alec, "image/jpeg", original)
        .await
        .ok(200)
        .json::<AvatarSet>();
    assert_eq!(plain.avatar, hash);
}

#[tokio::test]
async fn avatar_uploads_are_rate_limited_per_account() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.avatar_uploads = 2).await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    put_avatar(&cloud, &alec, "image/png", png(8, 8))
        .await
        .ok(200);
    // A refused picture counts as well: decoding is what costs.
    put_avatar(&cloud, &alec, "image/png", b"no".to_vec())
        .await
        .is_error("bad_request", 400);
    let limited = put_avatar(&cloud, &alec, "image/png", png(8, 8)).await;
    limited.is_error("rate_limited", 429);
    assert!(limited.headers.contains_key("retry-after"));
    // Others are not held up, and nothing else of the account is.
    put_avatar(&cloud, &sam, "image/png", png(8, 8))
        .await
        .ok(200);
    cloud.get("/v1/me", alec.token()).await.ok(200);
    set_profile(&cloud, &alec, json!({"about": "still works"}))
        .await
        .ok(200);
}

#[tokio::test]
async fn a_moderator_can_remove_an_avatar() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    let hash = upload_avatar(&cloud, &alec, png(32, 32)).await;
    let removed = anyplay_cloud::db::profiles::remove_avatar(&cloud.state.db, alec.id)
        .await
        .unwrap();
    assert!(removed);
    picture(&cloud, &hash).await.is_error("not_found", 404);
    assert_eq!(profile_of(&cloud, &alec, &alec).await.avatar, None);
    assert!(
        !anyplay_cloud::db::profiles::remove_avatar(&cloud.state.db, alec.id)
            .await
            .unwrap()
    );
}

// Playtime

#[tokio::test]
async fn playtime_keeps_the_highest_minutes_per_title_and_day() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    let (d1, d2) = ("2026-10-01", "2026-10-02");
    let total = |profile: &Profile| profile.playtime.clone().unwrap_or_default();
    let minutes = |title: &str, minutes: u64| TitlePlaytime {
        title_id: title.into(),
        minutes,
    };

    let added: PlaytimeAdded = upload_playtime(
        &cloud,
        &alec,
        vec![
            entry("GAME00001", d1, 60),
            entry("GAME00001", d2, 30),
            entry("GAME00002", d1, 200),
        ],
    )
    .await
    .ok(200)
    .json();
    assert_eq!(added.changed, 3);
    assert_eq!(
        total(&profile_of(&cloud, &alec, &alec).await),
        vec![minutes("GAME00002", 200), minutes("GAME00001", 90)]
    );

    // The same upload again changes nothing and adds nothing.
    let added: PlaytimeAdded = upload_playtime(
        &cloud,
        &alec,
        vec![
            entry("GAME00001", d1, 60),
            entry("GAME00001", d2, 30),
            entry("GAME00002", d1, 200),
        ],
    )
    .await
    .ok(200)
    .json();
    assert_eq!(added.changed, 0);
    assert_eq!(
        total(&profile_of(&cloud, &alec, &alec).await)[1],
        minutes("GAME00001", 90)
    );

    // A lower number for a day is ignored, a higher one replaces it, the order does not matter,
    // and one entry twice in a call counts once with its higher number.
    let added: PlaytimeAdded = upload_playtime(
        &cloud,
        &alec,
        vec![
            entry("GAME00001", d1, 10),
            entry("GAME00001", d2, 45),
            entry("GAME00002", d1, 150),
            entry("GAME00003", d1, 5),
            entry("GAME00003", d1, 20),
            entry("GAME00003", d1, 7),
        ],
    )
    .await
    .ok(200)
    .json();
    assert_eq!(
        added.changed, 2,
        "day 2 of GAME00001 rose and GAME00003 is new"
    );
    assert_eq!(
        total(&profile_of(&cloud, &alec, &alec).await),
        vec![
            minutes("GAME00002", 200),
            minutes("GAME00001", 105),
            minutes("GAME00003", 20)
        ]
    );

    // An empty upload is fine, a full day and a day ahead pass.
    upload_playtime(&cloud, &alec, vec![]).await.ok(200);
    upload_playtime(
        &cloud,
        &alec,
        vec![entry("GAME00004", &today(), MAX_DAY_MINUTES)],
    )
    .await
    .ok(200);
}

#[tokio::test]
async fn bad_playtime_is_refused_whole() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    let good = entry("GAME00001", "2026-10-01", 30);
    let tomorrow_plus = day_text(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400
            + 2,
    );
    for bad in [
        entry("GAME00001", "2026-02-30", 5),
        entry("GAME00001", "26-10-01", 5),
        entry("GAME00001", "2026-10-01T10:00", 5),
        entry("GAME00001", "1999-12-31", 5),
        entry("GAME00001", &tomorrow_plus, 5),
        entry("GAME00001", "2026-10-01", MAX_DAY_MINUTES + 1),
        entry("../x", "2026-10-01", 5),
        entry("", "2026-10-01", 5),
        json!({"title_id": "GAME00001", "day": "2026-10-01", "minutes": -1}),
        json!({"title_id": "GAME00001", "day": "2026-10-01"}),
    ] {
        upload_playtime(&cloud, &alec, vec![good.clone(), bad.clone()])
            .await
            .is_error("bad_request", 400);
    }
    let many: Vec<_> = (0..=MAX_PLAYTIME_UPLOAD)
        .map(|n| entry("GAME00001", "2026-10-01", n as u32 % 10))
        .collect();
    upload_playtime(&cloud, &alec, many)
        .await
        .is_error("too_large", 413);
    assert_eq!(profile_of(&cloud, &alec, &alec).await.playtime, None);
    cloud
        .request(
            reqwest::Method::PUT,
            "/v1/me/playtime",
            None,
            Some(json!({"entries": []})),
        )
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn export_holds_the_profile_and_deletion_removes_it() {
    let cloud = start().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let hash = fill(&cloud, &alec).await;
    upload_playtime(&cloud, &alec, vec![entry("GAME00001", "2026-10-01", 15)])
        .await
        .ok(200);
    cloud
        .put(
            "/v1/me/privacy",
            alec.token(),
            Some(json!({"about": "everyone", "playtime": "nobody"})),
        )
        .await
        .ok(200);
    // Another account's data is not touched by the deletion. Its picture is another one: the same
    // picture has the same address for both.
    fill(&cloud, &sam).await;
    upload_avatar(&cloud, &sam, dark_png()).await;

    let export = cloud.get("/v1/me/export", alec.token()).await.ok(200).body;
    assert_eq!(
        export["privacy"],
        json!({"trophies": "friends", "about": "everyone", "favourites": "friends",
               "avatar": "friends", "playtime": "nobody", "feed": "friends"})
    );
    assert_eq!(
        export["profile"],
        json!({"about": "Speedruns and sunsets", "favourites": ["GAME00001", "GAME00002"]})
    );
    assert_eq!(export["avatar"]["hash"], hash);
    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(export["avatar"]["png"].as_str().unwrap())
        .unwrap();
    assert_eq!(png_bytes, picture(&cloud, &hash).await.body);
    assert_eq!(
        export["playtime"],
        json!([
            {"title_id": "GAME00001", "day": "2026-10-01", "minutes": 15},
            {"title_id": "GAME00001", "day": today(), "minutes": 90},
        ])
    );

    cloud.delete_account_now(&alec).await;
    picture(&cloud, &hash).await.is_error("not_found", 404);
    let db = &cloud.state.db;
    for table in ["profiles", "avatars", "playtime", "privacy"] {
        let (mine,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE account_id = {}",
            alec.id
        )))
        .fetch_one(db)
        .await
        .unwrap();
        assert_eq!(mine, 0, "{table}");
    }
    for table in ["profiles", "avatars", "playtime"] {
        let (others,): (i64,) =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(db)
                .await
                .unwrap();
        assert!(others > 0, "{table} of the other account is still there");
    }
}
