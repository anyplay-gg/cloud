mod common;

use std::io::Cursor;

use anyplay_api::{Conversation, Message, Messages};
use anyplay_cloud::media::MediaStore as _;
use common::{Reply, TestCloud, User};
use serde_json::{Value, json};

const ADMIN: &str = "admin-secret";

fn png() -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(8, 6, image::Rgba([10, 20, 30, 255]));
    let mut out = Vec::new();
    picture
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn jpeg() -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(16, 16, image::Rgb([200, 100, 50]));
    let mut out = Vec::new();
    picture
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
        .unwrap();
    out
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A minimal MP4: `ftyp`, then `moov` with a movie header of `seconds` and a 1280 by 720 track.
fn mp4(seconds: u32) -> Vec<u8> {
    let mut mvhd = vec![0u8; 100];
    mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
    mvhd[16..20].copy_from_slice(&(seconds * 1000).to_be_bytes());
    let mut tkhd = vec![0u8; 84];
    tkhd[76..80].copy_from_slice(&(1280u32 << 16).to_be_bytes());
    tkhd[80..84].copy_from_slice(&(720u32 << 16).to_be_bytes());
    let mut moov = boxed(b"mvhd", &mvhd);
    moov.extend(boxed(b"trak", &boxed(b"tkhd", &tkhd)));
    let mut out = boxed(b"ftyp", b"isom\0\0\0\0isom");
    out.extend(boxed(b"moov", &moov));
    out.extend(boxed(b"mdat", &[1; 64]));
    out
}

async fn upload(
    cloud: &TestCloud,
    user: &User,
    conversation: &str,
    content_type: &str,
    body: Vec<u8>,
) -> Reply {
    let response = cloud
        .http
        .post(format!(
            "http://{}/v1/conversations/{conversation}/media",
            cloud.base
        ))
        .bearer_auth(user.token())
        .header("content-type", content_type)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let text = response.text().await.unwrap();
    Reply {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
    }
}

async fn download(
    cloud: &TestCloud,
    user: &str,
    id: &str,
) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
    let response = cloud
        .http
        .get(format!("http://{}/v1/media/{id}", cloud.base))
        .bearer_auth(user)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    (status, headers, response.bytes().await.unwrap().to_vec())
}

async fn open(cloud: &TestCloud, from: &User, with: &User) -> Conversation {
    cloud
        .post(
            "/v1/conversations",
            from.token(),
            json!({"accounts": [with.id]}),
        )
        .await
        .ok(200)
        .json()
}

async fn friends_cloud() -> (TestCloud, User, User, Conversation) {
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let conversation = open(&cloud, &ann, &ben).await;
    (cloud, ann, ben, conversation)
}

#[tokio::test]
async fn a_friend_gets_a_screenshot_as_a_media_message() {
    let (cloud, ann, ben, conversation) = friends_cloud().await;
    let bytes = png();
    let sent: Message = upload(&cloud, &ann, &conversation.id, "image/png", bytes.clone())
        .await
        .ok(200)
        .json();
    let media = sent.media.clone().expect("a media message");
    assert_eq!(sent.text, "");
    assert_eq!(
        (media.width, media.height, media.bytes),
        (Some(8), Some(6), bytes.len() as u64)
    );
    assert_eq!(media.expires - sent.created, 30 * 24 * 60 * 60);

    // Ben sees it in the thread and downloads it; the cloud serves it as what it is.
    let thread: Messages = cloud
        .get(
            &format!("/v1/conversations/{}/messages", conversation.id),
            ben.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(thread.messages[0].media.as_ref().unwrap().id, media.id);
    let (status, headers, body) = download(&cloud, ben.token(), &media.id).await;
    assert_eq!(status, 200);
    assert_eq!(body, bytes);
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    // So can its sender, and a JPEG goes through as well.
    assert_eq!(download(&cloud, ann.token(), &media.id).await.0, 200);
    upload(&cloud, &ann, &conversation.id, "image/jpeg", jpeg())
        .await
        .ok(200);
    // An edit has no text to change.
    cloud
        .patch(
            &format!(
                "/v1/conversations/{}/messages/{}",
                conversation.id, sent.seq
            ),
            ann.token(),
            json!({"text": "hi"}),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_clip_is_checked_for_its_length() {
    let (cloud, ann, _ben, conversation) = friends_cloud().await;
    let sent: Message = upload(&cloud, &ann, &conversation.id, "video/mp4", mp4(30))
        .await
        .ok(200)
        .json();
    let media = sent.media.unwrap();
    assert_eq!(media.duration_ms, Some(30_000));
    assert_eq!((media.width, media.height), (Some(1280), Some(720)));
    upload(&cloud, &ann, &conversation.id, "video/mp4", mp4(75))
        .await
        .is_error("too_large", 413);
    upload(&cloud, &ann, &conversation.id, "video/mp4", vec![9; 500])
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn only_friends_in_an_accepted_conversation_can_be_sent_to() {
    let cloud = TestCloud::start().await;
    let ann = cloud.sign_up("Ann").await;
    let ben = cloud.sign_up("Ben").await;
    // Strangers: the conversation is a message request at most.
    let conversation = open(&cloud, &ann, &ben).await;
    upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .is_error("forbidden", 403);
    // Friends, but a group is not one friend.
    let (cat, dan) = cloud.friends("Cat", "Dan").await;
    let eve = cloud.sign_up("Eve").await;
    cloud
        .post(
            "/v1/friends/requests",
            cat.token(),
            json!({"handle": eve.handle}),
        )
        .await
        .ok(200);
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", cat.id),
            eve.token(),
        )
        .await
        .ok(204);
    let group: Conversation = cloud
        .post(
            "/v1/conversations",
            cat.token(),
            json!({"accounts": [dan.id, eve.id], "name": "g"}),
        )
        .await
        .ok(200)
        .json();
    upload(&cloud, &cat, &group.id, "image/png", png())
        .await
        .is_error("forbidden", 403);
    // A block between friends stops it.
    let direct = open(&cloud, &cat, &dan).await;
    upload(&cloud, &cat, &direct.id, "image/png", png())
        .await
        .ok(200);
    cloud
        .put(&format!("/v1/blocks/{}", dan.id), cat.token(), None)
        .await
        .ok(204);
    let after = upload(&cloud, &cat, &direct.id, "image/png", png()).await;
    assert!(matches!(after.status, 403 | 404), "{}", after.body);
}

#[tokio::test]
async fn files_are_refused_by_type_size_and_allowance() {
    let (cloud, ann, _ben, conversation) = friends_cloud().await;
    // What the request says must be what the bytes are.
    upload(&cloud, &ann, &conversation.id, "video/mp4", png())
        .await
        .is_error("bad_request", 400);
    upload(&cloud, &ann, &conversation.id, "image/jpeg", png())
        .await
        .is_error("bad_request", 400);
    upload(
        &cloud,
        &ann,
        &conversation.id,
        "image/png",
        b"not a picture".to_vec(),
    )
    .await
    .is_error("bad_request", 400);
    upload(
        &cloud,
        &ann,
        &conversation.id,
        "text/html",
        b"<b>x</b>".to_vec(),
    )
    .await
    .is_error("bad_request", 400);
    // Over 10 MB is refused for a screenshot. The cloud answers before it reads the body, so a
    // client that is still sending may see its connection closed (Windows reports it as an
    // abort) instead of the answer; both mean the same.
    let big = cloud
        .http
        .post(format!(
            "http://{}/v1/conversations/{}/media",
            cloud.base, conversation.id
        ))
        .bearer_auth(ann.token())
        .header("content-type", "image/png")
        .body(vec![0u8; 10 * 1024 * 1024 + 1])
        .send()
        .await;
    if let Ok(response) = big {
        assert_eq!(response.status().as_u16(), 413);
    }
    // Nothing is left behind.
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM media")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    let room = png().len() as u64 + 10;
    let small = TestCloud::with(move |c| c.media_account_bytes = room, |_| {}).await;
    let (a, b) = small.friends("Ann", "Ben").await;
    let conversation = open(&small, &a, &b).await;
    upload(&small, &a, &conversation.id, "image/png", png())
        .await
        .ok(200);
    upload(&small, &a, &conversation.id, "image/png", png())
        .await
        .is_error("limit", 409);
}

#[tokio::test]
async fn only_the_two_friends_read_the_file() {
    let (cloud, ann, ben, conversation) = friends_cloud().await;
    let stranger = cloud.sign_up("Cat").await;
    let sent: Message = upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200)
        .json();
    let id = sent.media.unwrap().id;
    assert_eq!(download(&cloud, stranger.token(), &id).await.0, 404);
    assert_eq!(
        download(&cloud, ben.token(), "no-such-file-0000").await.0,
        404
    );
    assert_eq!(download(&cloud, ben.token(), &id).await.0, 200);
    // Without a token there is nothing.
    let anonymous = cloud
        .http
        .get(format!("http://{}/v1/media/{id}", cloud.base))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status().as_u16(), 401);
}

#[tokio::test]
async fn unsending_takes_the_file_away_but_keeps_it_for_a_report() {
    let (cloud, ann, ben, conversation) = friends_cloud().await;
    let sent: Message = upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200)
        .json();
    let id = sent.media.clone().unwrap().id;
    cloud
        .delete(
            &format!(
                "/v1/conversations/{}/messages/{}",
                conversation.id, sent.seq
            ),
            ann.token(),
        )
        .await
        .ok(200);
    assert_eq!(download(&cloud, ben.token(), &id).await.0, 404);
    assert_eq!(download(&cloud, ann.token(), &id).await.0, 404);
    let thread: Messages = cloud
        .get(
            &format!("/v1/conversations/{}/messages", conversation.id),
            ben.token(),
        )
        .await
        .ok(200)
        .json();
    assert!(thread.messages[0].media.is_none() && thread.messages[0].deleted);
    // The file is still on disk for a week, for a report.
    cloud.state.sweep().await.unwrap();
    assert!(cloud.state.media_store().open(&id).is_ok());
    // Ben reports the unsent message: the moderators can read the file.
    cloud
        .post(
            "/v1/reports",
            ben.token(),
            json!({"account": ann.id, "reason": "harassment",
                   "conversation": conversation.id, "seq": sent.seq}),
        )
        .await
        .ok(204);
    let held = cloud
        .http
        .get(format!("http://{}/v1/moderation/media/{id}", cloud.base))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap();
    assert_eq!(held.status().as_u16(), 200);
    assert_eq!(held.bytes().await.unwrap().to_vec(), png());
    // Once the hold has ended, the sweep deletes the file and the row.
    sqlx::query("UPDATE media SET hold_until_ms = 1")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    cloud.state.sweep().await.unwrap();
    assert!(cloud.state.media_store().open(&id).is_err());
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM media")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_report_keeps_a_live_file_past_its_expiry_and_the_sweep_deletes_the_rest() {
    let (cloud, ann, ben, conversation) = friends_cloud().await;
    let kept: Message = upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200)
        .json();
    let gone: Message = upload(&cloud, &ann, &conversation.id, "video/mp4", mp4(5))
        .await
        .ok(200)
        .json();
    let kept_id = kept.media.clone().unwrap().id;
    let gone_id = gone.media.unwrap().id;
    cloud
        .post(
            "/v1/reports",
            ben.token(),
            json!({"account": ann.id, "reason": "harassment",
                   "conversation": conversation.id, "seq": kept.seq}),
        )
        .await
        .ok(204);
    sqlx::query("UPDATE media SET expires_ms = 1")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    // Expired files cannot be read any more, by anyone.
    assert_eq!(download(&cloud, ben.token(), &kept_id).await.0, 404);
    cloud.state.sweep().await.unwrap();
    assert!(cloud.state.media_store().open(&gone_id).is_err());
    assert!(cloud.state.media_store().open(&kept_id).is_ok());
}

#[tokio::test]
async fn deleting_the_account_deletes_its_files() {
    let (cloud, ann, _ben, conversation) = friends_cloud().await;
    let sent: Message = upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200)
        .json();
    let id = sent.media.unwrap().id;
    let export = cloud.get("/v1/me/export", ann.token()).await.ok(200);
    assert_eq!(export.body["media_sent"][0]["id"], id.as_str());
    cloud.delete_account_now(&ann).await;
    assert!(cloud.state.media_store().open(&id).is_err());
}

#[tokio::test]
async fn unsending_does_not_free_the_allowance_while_the_file_is_on_disk() {
    let room = png().len() as u64 + 10;
    let cloud = TestCloud::with(move |c| c.media_account_bytes = room, |_| {}).await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let conversation = open(&cloud, &ann, &ben).await;
    let sent: Message = upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200)
        .json();
    cloud
        .delete(
            &format!(
                "/v1/conversations/{}/messages/{}",
                conversation.id, sent.seq
            ),
            ann.token(),
        )
        .await
        .ok(200);
    // The file is held for a report for a week, so it still counts.
    upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .is_error("limit", 409);
    // Once the sweep has deleted it, the room is free again.
    sqlx::query("UPDATE media SET hold_until_ms = 1")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    cloud.state.sweep().await.unwrap();
    upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200);
}

#[tokio::test]
async fn an_upload_without_a_length_is_held_to_the_allowance_too() {
    let room = png().len() as u64 + 10;
    let cloud = TestCloud::with(move |c| c.media_account_bytes = room, |_| {}).await;
    let (ann, ben) = cloud.friends("Ann", "Ben").await;
    let conversation = open(&cloud, &ann, &ben).await;
    upload(&cloud, &ann, &conversation.id, "image/png", png())
        .await
        .ok(200);
    // A chunked body has no Content-Length, so only the check after the upload can refuse it.
    let file = png();
    let mut request = format!(
        "POST /v1/conversations/{}/media HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\
         Content-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        conversation.id,
        cloud.base,
        ann.token()
    )
    .into_bytes();
    request.extend(format!("{:x}\r\n", file.len()).into_bytes());
    request.extend(&file);
    request.extend(b"\r\n0\r\n\r\n");
    let mut socket = tokio::net::TcpStream::connect(cloud.base.clone())
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut socket, &request)
        .await
        .unwrap();
    let mut answer = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut socket, &mut answer)
        .await
        .unwrap();
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 409"), "{answer}");
    let (files,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM media")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(files, 1);
}
