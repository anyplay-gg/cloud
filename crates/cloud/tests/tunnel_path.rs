//! `POST /v1/tunnels/path`: a net service says which kind of path a tunnel settled on, and the
//! cloud only counts it.

mod common;

use common::TestCloud;
use serde_json::json;

async fn metrics(cloud: &TestCloud) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(anyplay_cloud::serve_metrics(listener, cloud.state.clone()));
    cloud
        .http
        .get(format!("http://{address}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn reports_are_counted_by_kind_and_nothing_is_stored() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    for body in [
        json!({"kind": "direct"}),
        json!({"kind": "direct", "upgraded": true}),
        json!({"kind": "relay"}),
    ] {
        cloud
            .post("/v1/tunnels/path", alec.token(), body)
            .await
            .ok(204);
    }
    let text = metrics(&cloud).await;
    assert!(
        text.contains("anyplay_cloud_tunnel_paths_total{kind=\"direct\"} 2"),
        "{text}"
    );
    assert!(text.contains("anyplay_cloud_tunnel_paths_total{kind=\"relay\"} 1"));
    assert!(text.contains("anyplay_cloud_tunnel_upgrades_total 1"));
}

#[tokio::test]
async fn a_bad_report_is_refused_and_a_report_needs_sign_in() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    for body in [
        json!({"kind": "wifi"}),
        json!({"kind": "relay", "upgraded": true}),
        json!({"kind": "direct", "address": "10.0.0.1"}),
        json!({}),
    ] {
        let reply = cloud.post("/v1/tunnels/path", alec.token(), body).await;
        assert_eq!(reply.status, 400, "{}", reply.body);
    }
    cloud
        .request(
            reqwest::Method::POST,
            "/v1/tunnels/path",
            None,
            Some(json!({"kind": "direct"})),
        )
        .await
        .ok(401);
    let text = metrics(&cloud).await;
    assert!(text.contains("anyplay_cloud_tunnel_paths_total{kind=\"direct\"} 0"));
}
