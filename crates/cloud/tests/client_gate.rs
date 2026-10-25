//! Retiring old launchers: `min_launcher_version` refuses a launcher below it with
//! `client_too_old` (426) on every `/v1/` path but `GET /v1/info`, and lets everything else pass.

mod common;

use anyplay_api::client::CLIENT_HEADER;
use common::{Reply, TestCloud};
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

async fn gated(minimum: &str) -> TestCloud {
    let minimum = minimum.to_owned();
    TestCloud::with(move |c| c.min_launcher_version = Some(minimum), |_| {}).await
}

/// A GET that names the client the way `client` says; `None` sends no header.
async fn get_as(cloud: &TestCloud, path: &str, token: Option<&str>, client: Option<&str>) -> Reply {
    let mut request = cloud.http.get(format!("http://{}{path}", cloud.base));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(client) = client {
        request = request.header(CLIENT_HEADER, client);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let text = response.text().await.unwrap();
    Reply {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
    }
}

fn is_refused(reply: &Reply) {
    reply.is_error("client_too_old", 426);
    assert_eq!(reply.body["minimum"], "0.3.0", "{}", reply.body);
    assert!(
        reply.body["message"]
            .as_str()
            .is_some_and(|m| m.contains("0.3.0")),
        "{}",
        reply.body
    );
}

#[tokio::test]
async fn a_launcher_below_the_minimum_is_refused_and_everyone_else_passes() {
    let cloud = gated("0.3.0").await;
    let a = cloud.sign_up("Alec").await;
    let friends = "/v1/friends";

    for old in [
        "launcher/0.2.9",
        "launcher/0.0.1",
        "launcher/0.3.0-beta",
        "launcher/0.3.0-rc.1",
    ] {
        is_refused(&get_as(&cloud, friends, Some(a.token()), Some(old)).await);
    }
    for fine in [
        "launcher/0.3.0",
        "launcher/0.3.1",
        "launcher/0.10.0",
        "launcher/1.0.0",
        "launcher/0.3.1-beta",
    ] {
        get_as(&cloud, friends, Some(a.token()), Some(fine))
            .await
            .ok(200);
    }

    // Fail open: no header, a value that does not read, another product.
    for open in [
        None,
        Some(""),
        Some("garbage"),
        Some("launcher"),
        Some("launcher/"),
        Some("launcher/0.2"),
        Some("launcher/v0.2.0"),
        Some("Launcher/0.2.0"),
        Some("launcher/0.2.0+build"),
        Some("launcher/0.0.0-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        Some("web/0.0.1"),
        Some("net/0.0.1"),
        Some("unknown-tool/0.0.1"),
    ] {
        get_as(&cloud, friends, Some(a.token()), open).await.ok(200);
    }
}

#[tokio::test]
async fn the_gate_comes_before_authentication_and_covers_every_method() {
    let cloud = gated("0.3.0").await;
    // No token at all: the answer is still the update, not "unauthorized".
    is_refused(&get_as(&cloud, "/v1/friends", None, Some("launcher/0.1.0")).await);
    is_refused(&get_as(&cloud, "/v1/no/such/path", None, Some("launcher/0.1.0")).await);
    let post = cloud
        .http
        .post(format!("http://{}/v1/auth/email", cloud.base))
        .header(CLIENT_HEADER, "launcher/0.1.0")
        .json(&serde_json::json!({"email": "a@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status().as_u16(), 426);
    // Without the header the same request reaches its handler.
    cloud
        .request(
            reqwest::Method::POST,
            "/v1/auth/email",
            None,
            Some(serde_json::json!({"email": "a@example.com"})),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn info_health_and_the_web_stay_open_to_an_old_launcher() {
    let cloud = gated("0.3.0").await;
    let old = Some("launcher/0.1.0");
    get_as(&cloud, "/v1/info", None, old).await.ok(200);
    get_as(&cloud, "/healthz", None, old).await.ok(200);
    // The account web is not under /v1/: its pages answer (here: not the update error).
    let web = get_as(&cloud, "/web/authorize", None, old).await;
    assert_ne!(web.status, 426, "{}", web.body);
}

#[tokio::test]
async fn the_event_handshake_is_refused_for_an_old_launcher() {
    let cloud = gated("0.3.0").await;
    let a = cloud.sign_up("Alec").await;
    let connect = |client: Option<&'static str>| {
        let base = cloud.base.clone();
        let token = a.token().to_owned();
        async move {
            let mut request = format!("ws://{base}/v1/events")
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().unwrap());
            if let Some(client) = client {
                request
                    .headers_mut()
                    .insert(CLIENT_HEADER, client.parse().unwrap());
            }
            tokio_tungstenite::connect_async(request).await
        }
    };
    match connect(Some("launcher/0.2.0")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), 426);
            let body: Value = serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
            assert_eq!(body["error"], "client_too_old");
            assert_eq!(body["minimum"], "0.3.0");
        }
        other => panic!(
            "expected the upgrade to be refused, got {:?}",
            other.map(|_| ())
        ),
    }
    assert!(connect(Some("launcher/0.3.0")).await.is_ok());
    assert!(connect(None).await.is_ok());
}

#[tokio::test]
async fn no_minimum_means_no_gate() {
    for setting in [None, Some(String::new()), Some("  ".to_owned())] {
        let cloud = TestCloud::with(move |c| c.min_launcher_version = setting, |_| {}).await;
        let a = cloud.sign_up("Alec").await;
        get_as(
            &cloud,
            "/v1/friends",
            Some(a.token()),
            Some("launcher/0.0.1"),
        )
        .await
        .ok(200);
    }
}

#[tokio::test]
async fn a_pre_release_of_the_minimum_is_older_and_the_refusal_is_counted() {
    let cloud = gated("0.3.0").await;
    let a = cloud.sign_up("Alec").await;
    is_refused(
        &get_as(
            &cloud,
            "/v1/friends",
            Some(a.token()),
            Some("launcher/0.3.0-beta"),
        )
        .await,
    );
    get_as(
        &cloud,
        "/v1/friends",
        Some(a.token()),
        Some("launcher/0.3.0"),
    )
    .await
    .ok(200);
    // The 426 shows in the request counters by route and status.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let metrics = listener.local_addr().unwrap();
    tokio::spawn(anyplay_cloud::serve_metrics(listener, cloud.state.clone()));
    let text = cloud
        .http
        .get(format!("http://{metrics}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        text.contains("anyplay_cloud_http_requests_total{route=\"/v1/friends\",status=\"426\"} 1"),
        "{text}"
    );
}
