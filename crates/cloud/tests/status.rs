//! `GET /v1/status`: public, one entry per part, nothing about the inside.

mod common;

use common::TestCloud;
use reqwest::Method;
use serde_json::Value;

async fn status(cloud: &TestCloud) -> common::Reply {
    cloud
        .request(Method::GET, "/v1/status", None, None)
        .await
        .ok(200)
}

fn part<'a>(body: &'a Value, id: &str) -> &'a str {
    body["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no component {id} in {body}"))["status"]
        .as_str()
        .unwrap()
}

#[tokio::test]
async fn an_unwatched_part_is_unknown_and_the_rest_is_up() {
    let cloud = TestCloud::with(|c| c.relays.clear(), |_| {}).await;
    let reply = status(&cloud).await;
    assert_eq!(reply.body["status"], "operational", "{}", reply.body);
    assert_eq!(part(&reply.body, "api"), "up");
    assert_eq!(part(&reply.body, "database"), "up");
    assert_eq!(part(&reply.body, "signalling"), "up");
    assert_eq!(part(&reply.body, "relay"), "unknown");
    assert_eq!(part(&reply.body, "web"), "unknown");
    // Nothing but the parts and the time: no version, host or count.
    let mut keys: Vec<&String> = reply.body.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["checked", "components", "status"]);
    for c in reply.body["components"].as_array().unwrap() {
        let mut keys: Vec<&String> = c.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["id", "status"]);
    }
    assert_eq!(
        reply.headers.get("cache-control").unwrap(),
        "public, max-age=10"
    );
}

#[tokio::test]
async fn the_answer_is_reused_for_a_few_seconds() {
    let cloud = TestCloud::with(|c| c.relays.clear(), |_| {}).await;
    let first = status(&cloud).await;
    let second = status(&cloud).await;
    assert_eq!(first.body["checked"], second.body["checked"]);
}

#[tokio::test]
async fn a_website_that_answers_is_up_and_one_that_does_not_is_down() {
    let site = TestCloud::start().await;
    let url = format!("http://{}/healthz", site.base);
    let up = TestCloud::with(
        move |c| {
            c.relays.clear();
            c.status_web_url = Some(url);
        },
        |_| {},
    )
    .await;
    assert_eq!(part(&status(&up).await.body, "web"), "up");

    let down = TestCloud::with(
        |c| {
            c.relays.clear();
            // Nothing listens on port 1.
            c.status_web_url = Some("http://127.0.0.1:1/".into());
        },
        |_| {},
    )
    .await;
    let reply = status(&down).await;
    assert_eq!(part(&reply.body, "web"), "down");
    assert_eq!(reply.body["status"], "degraded");
    assert_eq!(part(&reply.body, "database"), "up");
}
