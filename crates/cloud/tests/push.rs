//! Notifications: subscribing a browser through the portal API, and the Web Push the cloud sends
//! to a push service that runs in this process. The "browser" decrypts what arrives.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyplay_cloud::push;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use common::portal::Portal;
use common::{TestCloud, User};
use openssl::bn::BigNumContext;
use openssl::ec::{EcGroup, EcKey, PointConversionForm};
use openssl::nid::Nid;
use openssl::pkey::Private;
use reqwest::Method;
use serde_json::{Value, json};

struct Received {
    name: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Default)]
struct Service {
    received: Vec<Received>,
}

type Shared = Arc<Mutex<Service>>;

/// A push service. A path named `gone` answers 410, `broken` answers 500, any other 201.
async fn accept(
    State(service): State<Shared>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    service.lock().unwrap().received.push(Received {
        name: name.clone(),
        headers,
        body: body.to_vec(),
    });
    match name.as_str() {
        "gone" => StatusCode::GONE,
        "broken" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CREATED,
    }
}

async fn push_service() -> (String, Shared) {
    let service = Shared::default();
    let app = Router::new()
        .route("/push/{name}", post(accept))
        .with_state(service.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{address}/push"), service)
}

/// A browser's subscription keys.
struct Browser {
    key: EcKey<Private>,
    auth: [u8; 16],
}

impl Browser {
    fn new() -> Self {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let mut auth = [0u8; 16];
        openssl::rand::rand_bytes(&mut auth).unwrap();
        Self {
            key: EcKey::generate(&group).unwrap(),
            auth,
        }
    }

    fn keys(&self) -> Value {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let mut ctx = BigNumContext::new().unwrap();
        let public = self
            .key
            .public_key()
            .to_bytes(&group, PointConversionForm::UNCOMPRESSED, &mut ctx)
            .unwrap();
        json!({"p256dh": B64.encode(public), "auth": B64.encode(self.auth)})
    }

    fn open(&self, received: &Received) -> Value {
        let plain = push::decrypt(&received.body, &self.key, &self.auth).unwrap();
        serde_json::from_slice(&plain).unwrap()
    }
}

async fn cloud(configured: bool) -> TestCloud {
    let (private, _) = push::generate_key().unwrap();
    TestCloud::with(
        move |c| {
            if configured {
                c.vapid_private_key = Some(private);
                c.vapid_subject = Some("mailto:ops@example.com".into());
            }
        },
        |t| t.push_any_host = true,
    )
    .await
}

async fn member<'a>(cloud: &'a TestCloud, name: &str) -> (User, Portal<'a>) {
    let user = cloud.sign_up(name).await;
    let portal = Portal::signed_in(cloud, &user).await;
    (user, portal)
}

async fn subscribe(
    portal: &Portal<'_>,
    endpoint: &str,
    browser: &Browser,
    kinds: Option<Value>,
) -> Value {
    let mut body = json!({"endpoint": endpoint, "keys": browser.keys(), "label": "Test PC"});
    if let Some(kinds) = kinds {
        body["kinds"] = kinds;
    }
    portal
        .send(Method::PUT, "/push/subscription", body)
        .await
        .ok(200)
}

async fn unsubscribe(portal: &Portal<'_>, endpoint: &str) {
    portal
        .send(
            Method::DELETE,
            "/push/subscription",
            json!({"endpoint": endpoint}),
        )
        .await
        .ok(200);
}

async fn arrived(service: &Shared, count: usize) {
    for _ in 0..100 {
        if service.lock().unwrap().received.len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "expected {count} pushes, got {}",
        service.lock().unwrap().received.len()
    );
}

/// Nothing more arrives for a moment.
async fn quiet(service: &Shared, count: usize) {
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(service.lock().unwrap().received.len(), count);
}

async fn subscriptions(portal: &Portal<'_>) -> Vec<Value> {
    portal.get("/push").await.ok(200)["subscriptions"]
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn a_cloud_without_a_key_says_so_and_refuses_subscriptions() {
    let cloud = cloud(false).await;
    let (_, portal) = member(&cloud, "Alec").await;
    let state = portal.get("/push").await.ok(200);
    assert_eq!(state["enabled"], false);
    assert!(state["public_key"].is_null());
    assert_eq!(state["subscriptions"], json!([]));
    portal
        .send(
            Method::PUT,
            "/push/subscription",
            json!({"endpoint": "http://127.0.0.1:1/push/a", "keys": Browser::new().keys()}),
        )
        .await
        .error(403, "forbidden");
    // Leaving is always possible.
    unsubscribe(&portal, "http://127.0.0.1:1/push/a").await;
    // And it needs a session.
    let stranger = Portal::new(&cloud);
    stranger.session().await;
    stranger.get("/push").await.error(401, "unauthorized");
}

#[tokio::test]
async fn a_browser_subscribes_changes_and_leaves() {
    let cloud = cloud(true).await;
    let (service_url, _) = push_service().await;
    let (alec, portal) = member(&cloud, "Alec").await;
    let state = portal.get("/push").await.ok(200);
    assert_eq!(state["enabled"], true);
    let key = B64.decode(state["public_key"].as_str().unwrap()).unwrap();
    assert_eq!((key.len(), key[0]), (65, 4));

    let browser = Browser::new();
    let endpoint = format!("{service_url}/a");
    let made = subscribe(&portal, &endpoint, &browser, None).await;
    assert_eq!(made["label"], "Test PC");
    assert_eq!(
        made["kinds"],
        json!(["friend_request", "party_invite", "message"])
    );
    assert!(made["created_at"].as_u64().unwrap() > 0);
    assert!(made.get("endpoint").is_none() && made.get("keys").is_none());

    // The same address again changes the row.
    let changed = subscribe(&portal, &endpoint, &browser, Some(json!(["message"]))).await;
    assert_eq!(changed["id"], made["id"]);
    assert_eq!(changed["kinds"], json!(["message"]));
    assert_eq!(subscriptions(&portal).await.len(), 1);

    // The export names the device and nothing secret.
    let export = portal.get("/account/export").await.ok(200);
    assert_eq!(export["notification_devices"][0]["label"], "Test PC");
    assert!(!export.to_string().contains(&endpoint));
    assert!(
        !export
            .to_string()
            .contains(browser.keys()["auth"].as_str().unwrap())
    );

    // Another account cannot remove it.
    let (_, other) = member(&cloud, "Bob").await;
    unsubscribe(&other, &endpoint).await;
    assert_eq!(subscriptions(&portal).await.len(), 1);

    unsubscribe(&portal, &endpoint).await;
    assert!(subscriptions(&portal).await.is_empty());
    unsubscribe(&portal, &endpoint).await;

    // Deleting the account takes the rows with it.
    subscribe(&portal, &endpoint, &browser, None).await;
    cloud.delete_account_now(&alec).await;
    let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM push_subscriptions")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_subscription_is_checked() {
    let cloud = cloud(true).await;
    let (_, portal) = member(&cloud, "Alec").await;
    let browser = Browser::new();
    let keys = browser.keys();
    let attempt = |extra: Value| {
        let mut body = json!({"endpoint": "http://127.0.0.1:1/push/a", "keys": keys});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        portal.send(Method::PUT, "/push/subscription", body)
    };
    attempt(json!({"endpoint": "not an address"}))
        .await
        .error(400, "bad_request");
    attempt(json!({"endpoint": "ftp://127.0.0.1/x"}))
        .await
        .error(400, "bad_request");
    attempt(json!({"keys": {"p256dh": "AAAA", "auth": keys["auth"]}}))
        .await
        .error(400, "bad_request");
    attempt(json!({"keys": {"p256dh": keys["p256dh"], "auth": "AAAA"}}))
        .await
        .error(400, "bad_request");
    attempt(json!({"kinds": ["everything"]}))
        .await
        .error(400, "bad_request");
    attempt(json!({"label": "x".repeat(61)}))
        .await
        .error(400, "bad_request");
    attempt(json!({"label": "tab\there"}))
        .await
        .error(400, "bad_request");
    assert!(subscriptions(&portal).await.is_empty());
}

#[tokio::test]
async fn an_account_holds_ten_browsers_and_an_address_moves_to_the_one_that_signed_in() {
    let cloud = cloud(true).await;
    let (_, alec) = member(&cloud, "Alec").await;
    let browser = Browser::new();
    for n in 0..push::MAX_SUBSCRIPTIONS {
        subscribe(
            &alec,
            &format!("http://127.0.0.1:1/push/{n}"),
            &browser,
            None,
        )
        .await;
    }
    alec.send(
        Method::PUT,
        "/push/subscription",
        json!({"endpoint": "http://127.0.0.1:1/push/extra", "keys": browser.keys()}),
    )
    .await
    .error(409, "limit");
    // Changing one of the ten is not a new one.
    subscribe(
        &alec,
        "http://127.0.0.1:1/push/3",
        &browser,
        Some(json!([])),
    )
    .await;

    let (_, bob) = member(&cloud, "Bob").await;
    let moved = subscribe(&bob, "http://127.0.0.1:1/push/3", &browser, None).await;
    assert_eq!(subscriptions(&bob).await.len(), 1);
    assert_eq!(subscriptions(&alec).await.len(), 9);
    assert_eq!(subscriptions(&bob).await[0]["id"], moved["id"]);
}

#[tokio::test]
async fn friend_requests_messages_and_invitations_reach_the_browser() {
    let cloud = cloud(true).await;
    let (service_url, service) = push_service().await;
    let (alec, alec_portal) = member(&cloud, "Alec").await;
    let (bob, bob_portal) = member(&cloud, "Bob").await;
    let browser = Browser::new();
    subscribe(&bob_portal, &format!("{service_url}/bob"), &browser, None).await;

    // A friend request.
    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": bob.handle}),
        )
        .await
        .ok(200);
    arrived(&service, 1).await;
    {
        let service = service.lock().unwrap();
        let first = &service.received[0];
        assert_eq!(first.name, "bob");
        let header = |name: &str| first.headers[name].to_str().unwrap().to_owned();
        assert_eq!(header("content-encoding"), "aes128gcm");
        assert_eq!(header("ttl"), (3 * 24 * 60 * 60).to_string());
        assert_eq!(header("urgency"), "normal");
        let authorization = header("authorization");
        assert!(authorization.starts_with("vapid t="), "{authorization}");
        let public = cloud.state.push.as_ref().unwrap().public_key();
        assert!(authorization.ends_with(&format!(", k={public}")));
        let note = browser.open(first);
        assert_eq!(note["kind"], "friend_request");
        assert_eq!(note["body"], "Alec sent you a friend request");
        assert_eq!(note["url"], "/account/friends");
    }

    // Becoming friends is not a request: nothing more.
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", alec.id),
            bob.token(),
        )
        .await
        .ok(204);
    quiet(&service, 1).await;

    // A message, with no text in the push.
    let conversation: anyplay_api::Conversation = cloud
        .post(
            "/v1/conversations",
            alec.token(),
            json!({"accounts": [bob.id]}),
        )
        .await
        .ok(200)
        .json();
    cloud
        .post(
            &format!("/v1/conversations/{}/messages", conversation.id),
            alec.token(),
            json!({"text": "secret words"}),
        )
        .await
        .ok(200);
    arrived(&service, 2).await;
    {
        let service = service.lock().unwrap();
        let note = browser.open(&service.received[1]);
        assert_eq!(note["kind"], "message");
        assert_eq!(note["body"], "Alec sent you a message");
        assert!(!note.to_string().contains("secret"));
        assert_eq!(service.received[1].headers["urgency"], "high");
    }

    // The sender is not told of their own message.
    subscribe(
        &alec_portal,
        &format!("{service_url}/alec"),
        &Browser::new(),
        None,
    )
    .await;
    cloud
        .post(
            &format!("/v1/conversations/{}/messages", conversation.id),
            alec.token(),
            json!({"text": "again"}),
        )
        .await
        .ok(200);
    arrived(&service, 3).await;
    quiet(&service, 3).await;
    assert_eq!(service.lock().unwrap().received[2].name, "bob");

    // A party invitation.
    cloud
        .post(
            "/v1/party/invitations",
            alec.token(),
            json!({"to": [bob.id]}),
        )
        .await
        .ok(200);
    arrived(&service, 4).await;
    let note = browser.open(&service.lock().unwrap().received[3]);
    assert_eq!(note["kind"], "party_invite");
    assert_eq!(note["body"], "Alec invited you to a party");
}

#[tokio::test]
async fn only_the_wanted_kinds_are_sent_and_a_blocked_sender_sends_nothing() {
    let cloud = cloud(true).await;
    let (service_url, service) = push_service().await;
    let (alec, _) = member(&cloud, "Alec").await;
    let (bob, bob_portal) = member(&cloud, "Bob").await;
    let (carol, _) = member(&cloud, "Carol").await;
    let browser = Browser::new();
    let endpoint = format!("{service_url}/bob");
    subscribe(&bob_portal, &endpoint, &browser, Some(json!(["message"]))).await;

    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": bob.handle}),
        )
        .await
        .ok(200);
    quiet(&service, 0).await;

    // Bob wants friend requests now, and blocks Carol.
    subscribe(
        &bob_portal,
        &endpoint,
        &browser,
        Some(json!(["friend_request"])),
    )
    .await;
    cloud
        .put(&format!("/v1/blocks/{}", carol.id), bob.token(), None)
        .await
        .ok(204);
    cloud
        .post(
            "/v1/friends/requests",
            carol.token(),
            json!({"handle": bob.handle}),
        )
        .await
        .ok(200);
    quiet(&service, 0).await;
}

#[tokio::test]
async fn a_subscription_the_push_service_reports_gone_is_removed_and_a_broken_one_after_five() {
    let cloud = cloud(true).await;
    let (service_url, service) = push_service().await;
    let (alec, _) = member(&cloud, "Alec").await;
    let (bob, bob_portal) = member(&cloud, "Bob").await;
    let browser = Browser::new();
    subscribe(&bob_portal, &format!("{service_url}/gone"), &browser, None).await;
    subscribe(
        &bob_portal,
        &format!("{service_url}/broken"),
        &browser,
        None,
    )
    .await;
    subscribe(&bob_portal, &format!("{service_url}/fine"), &browser, None).await;

    cloud
        .post(
            "/v1/friends/requests",
            alec.token(),
            json!({"handle": bob.handle}),
        )
        .await
        .ok(200);
    arrived(&service, 3).await;
    for _ in 0..50 {
        if subscriptions(&bob_portal).await.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        subscriptions(&bob_portal).await.len(),
        2,
        "410 removes at once"
    );

    // Four more sends to the broken one make five failures in a row. As friends, every message of
    // the conversation is told, not only the first of a request.
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", alec.id),
            bob.token(),
        )
        .await
        .ok(204);
    let conversation: anyplay_api::Conversation = cloud
        .post(
            "/v1/conversations",
            alec.token(),
            json!({"accounts": [bob.id]}),
        )
        .await
        .ok(200)
        .json();
    let mut sent = 3;
    for n in 0..4 {
        cloud
            .post(
                &format!("/v1/conversations/{}/messages", conversation.id),
                alec.token(),
                json!({"text": format!("hello {n}")}),
            )
            .await
            .ok(200);
        sent += 2;
        arrived(&service, sent).await;
        // Let the outcome of the sends reach the store.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let (rows,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM push_subscriptions WHERE endpoint LIKE '%/broken'")
            .fetch_one(&cloud.state.db)
            .await
            .unwrap();
    assert_eq!(rows, 0, "five failures remove it");
    assert_eq!(subscriptions(&bob_portal).await.len(), 1);
}
