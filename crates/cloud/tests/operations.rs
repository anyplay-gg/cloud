mod common;

use std::time::Duration;

use anyplay_api::{Event, Relays, close};
use anyplay_cloud::limits::Rates;
use anyplay_ticket::{Kind, Verifier};
use common::{Events, TestCloud};
use reqwest::Method;
use serde_json::{Value, json};

const ADMIN_TOKEN: &str = "an-admin-token-for-tests";

async fn with_admin() -> TestCloud {
    TestCloud::with(|c| c.admin_token = Some(ADMIN_TOKEN.into()), |_| {}).await
}

async fn admin(
    cloud: &TestCloud,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> common::Reply {
    cloud.request(method, path, Some(ADMIN_TOKEN), body).await
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn relay_tickets_are_bound_to_the_endpoint_and_verify_with_the_public_key() {
    let cloud = TestCloud::start().await;
    let a = cloud.sign_up("Alec").await;
    let endpoint = "ab".repeat(32);
    let relays: Relays = cloud
        .post(
            "/v1/relays/tickets",
            a.token(),
            json!({"endpoint": endpoint}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(relays.relays.len(), 2);
    assert_eq!(relays.relays[0].url, "https://relay-a.example");
    assert_eq!(relays.relays[1].url, "https://relay-b.example");

    // A relay knows only the public key, as the cloud wrote it to the data directory.
    let public = std::fs::read_to_string(cloud.state.config.data_dir.join("signing.pub")).unwrap();
    let verifier = Verifier::from_base64(&public).unwrap();
    for relay in &relays.relays {
        let claims = verifier.verify(&relay.token, Kind::Relay, now()).unwrap();
        assert_eq!(claims.endpoint.as_deref(), Some(endpoint.as_str()));
        assert_eq!(claims.account, a.id);
        assert_eq!(claims.expires, relay.expires);
        let day = 24 * 60 * 60;
        assert!(relay.expires >= now() + day - 5 && relay.expires <= now() + day + 5);
        // It is no access token.
        assert!(verifier.verify(&relay.token, Kind::Access, now()).is_err());
    }
    // An access token is no relay ticket.
    assert!(verifier.verify(a.token(), Kind::Relay, now()).is_err());

    cloud
        .post("/v1/relays/tickets", a.token(), json!({"endpoint": "XYZ"}))
        .await
        .is_error("bad_request", 400);
    cloud
        .post("/v1/relays/tickets", "x", json!({}))
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn relay_tickets_are_limited_per_account_not_per_endpoint() {
    let cloud = TestCloud::with(
        |_| {},
        |tuning| {
            tuning.rates.relay_tickets_per_account = 3;
            tuning.rates.relay_ticket_window = Duration::from_secs(1);
        },
    )
    .await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Bea").await;
    let ask = |token: String, n: usize| {
        let cloud = &cloud;
        async move {
            // A new endpoint every time: the limit does not care.
            let endpoint = format!("{n:02x}").repeat(32);
            cloud
                .post("/v1/relays/tickets", &token, json!({"endpoint": endpoint}))
                .await
        }
    };
    for n in 0..3 {
        ask(a.token().to_owned(), n).await.ok(200);
    }
    let refused = ask(a.token().to_owned(), 3).await;
    refused.is_error("rate_limited", 429);
    assert_eq!(refused.headers["retry-after"], "1");
    // Another account has its own allowance.
    ask(b.token().to_owned(), 0).await.ok(200);

    // When the window ends the account may ask again.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    ask(a.token().to_owned(), 4).await.ok(200);
}

#[tokio::test]
async fn too_many_requests_get_429_with_retry_after() {
    let cloud = TestCloud::with(
        |_| {},
        |t| {
            t.rates = Rates::default();
            t.rates.account = 5;
        },
    )
    .await;
    // auth/email: 3 per address.
    for _ in 0..3 {
        cloud
            .request(
                Method::POST,
                "/v1/auth/email",
                None,
                Some(json!({"email": "spam@example.com"})),
            )
            .await
            .ok(200);
    }
    let reply = cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": "Spam@Example.com"})),
        )
        .await;
    reply.is_error("rate_limited", 429);
    let wait: u64 = reply.headers["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=15 * 60).contains(&wait));
    // Another address is fine until the IP runs out (10 per 15 minutes).
    for n in 0..6 {
        cloud
            .request(
                Method::POST,
                "/v1/auth/email",
                None,
                Some(json!({"email": format!("n{n}@example.com")})),
            )
            .await
            .ok(200);
    }
    cloud
        .request(
            Method::POST,
            "/v1/auth/email",
            None,
            Some(json!({"email": "last@example.com"})),
        )
        .await
        .is_error("rate_limited", 429);
}

#[tokio::test]
async fn an_account_may_make_a_limited_number_of_requests_per_minute() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.account = 5).await;
    let a = cloud.sign_up("Alec").await;
    for _ in 0..5 {
        cloud.get("/v1/me", a.token()).await.ok(200);
    }
    let reply = cloud.get("/v1/me", a.token()).await;
    reply.is_error("rate_limited", 429);
    assert!(reply.headers.contains_key("retry-after"));
    let b = cloud.sign_up("Sam").await;
    cloud.get("/v1/me", b.token()).await.ok(200);
}

#[tokio::test]
async fn the_client_address_comes_from_the_proxy_only_when_trusted() {
    for trusted in [false, true] {
        let cloud = TestCloud::with(
            |c| c.trust_proxy = trusted,
            |t| {
                t.rates.email_per_ip = 2;
            },
        )
        .await;
        let ask = |n: u32, forwarded: &str| {
            let cloud = &cloud;
            let forwarded = forwarded.to_owned();
            async move {
                cloud
                    .http
                    .post(format!("http://{}/v1/auth/email", cloud.base))
                    .header("x-forwarded-for", forwarded)
                    .json(&json!({"email": format!("p{n}@example.com")}))
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .as_u16()
            }
        };
        assert_eq!(ask(1, "10.0.0.1, 192.168.0.1").await, 200);
        assert_eq!(ask(2, "10.0.0.1").await, 200);
        // A third address from the same first hop is over the limit...
        assert_eq!(ask(3, "10.0.0.1").await, 429, "trusted = {trusted}");
        // ...but a different first hop is a different client only when the proxy is trusted.
        let other = ask(4, "10.0.0.2").await;
        assert_eq!(
            other,
            if trusted { 200 } else { 429 },
            "trusted = {trusted}"
        );
    }
}

#[tokio::test]
async fn deleting_an_account_removes_it_everywhere_and_tells_the_others() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut eb = cloud.events(b.token()).await;
    let mut ea = cloud.events(a.token()).await;
    assert!(matches!(eb.next().await, Event::Presence { .. }));
    cloud
        .post(
            "/v1/reports",
            b.token(),
            json!({"account": a.id, "reason": "spam", "details": "x"}),
        )
        .await
        .ok(204);

    cloud.delete("/v1/me", a.token()).await.ok(204);
    assert_eq!(ea.next().await, Event::SignedOut);
    assert_eq!(ea.close_code().await, Some(close::SIGNED_OUT));
    let mut saw_offline = false;
    let mut saw_friends = false;
    while !(saw_offline && saw_friends) {
        match eb.next().await {
            Event::Presence { presence, .. } => saw_offline = !presence.online,
            Event::Friends => saw_friends = true,
            other => panic!("unexpected {other:?}"),
        }
    }

    cloud
        .get("/v1/me", a.token())
        .await
        .is_error("unauthorized", 401);
    cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": a.tokens.refresh})),
        )
        .await
        .is_error("unauthorized", 401);
    cloud
        .get(&format!("/v1/accounts/{}", a.id), b.token())
        .await
        .is_error("not_found", 404);
    let lists: anyplay_api::FriendLists = cloud.get("/v1/friends", b.token()).await.ok(200).json();
    assert!(lists.friends.is_empty());

    // The handle and the address are free again.
    let again = cloud.sign_up("Alec").await;
    assert_ne!(again.id, a.id);
}

#[tokio::test]
async fn the_export_holds_everything_stored_about_the_account() {
    let cloud = TestCloud::start().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let c = cloud.sign_up("Kim").await;
    cloud.sign_in(&a.email, "laptop").await;
    cloud
        .put(&format!("/v1/blocks/{}", c.id), a.token(), None)
        .await
        .ok(204);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": c.id, "reason": "name", "details": "odd"}),
        )
        .await
        .ok(204);
    cloud
        .post(
            "/v1/friends/requests",
            a.token(),
            json!({"handle": "nobody"}),
        )
        .await
        .is_error("not_found", 404);

    let export = cloud.get("/v1/me/export", a.token()).await.ok(200).body;
    assert_eq!(export["account"]["handle"], "Alec");
    assert_eq!(export["account"]["email"], "alec@example.com");
    assert_eq!(export["devices"].as_array().unwrap().len(), 2);
    assert_eq!(export["friends"][0]["account"]["id"], b.id);
    assert_eq!(export["blocked"][0]["account"]["handle"], "Kim");
    assert_eq!(export["reports_filed"][0]["reason"], "name");
    assert_eq!(export["ban"], Value::Null);
    // No secret leaves with it.
    let text = export.to_string();
    assert!(!text.contains(&a.tokens.refresh));
}

#[tokio::test]
async fn reports_are_stored_and_validated() {
    let cloud = with_admin().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": b.id, "reason": "cheating", "details": "wall hacks"}),
        )
        .await
        .ok(204);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": b.id, "reason": "cheating"}),
        )
        .await
        .ok(204);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": b.id, "reason": "rude", "details": ""}),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": a.id, "reason": "spam"}),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": 4242, "reason": "spam"}),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": b.id, "reason": "other", "details": "x".repeat(1001)}),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": b.id, "reason": "other", "details": "ä".repeat(1000)}),
        )
        .await
        .ok(204);

    let reports = admin(&cloud, Method::GET, "/admin/v1/reports", None)
        .await
        .ok(200)
        .body;
    let reports = reports.as_array().unwrap();
    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0]["reporter"]["handle"], "Alec");
    assert_eq!(reports[0]["reported"]["handle"], "Sam");
    // Reports vanish with their reporter.
    cloud.delete("/v1/me", a.token()).await.ok(204);
    let reports = admin(&cloud, Method::GET, "/admin/v1/reports", None)
        .await
        .ok(200)
        .body;
    assert!(reports.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_ban_takes_effect_at_once_and_follows_the_e_mail_address() {
    let cloud = with_admin().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let mut eb = cloud.events(b.token()).await;
    assert!(matches!(ea.next().await, Event::Presence { .. }));
    cloud.get("/v1/me", b.token()).await.ok(200);

    admin(
        &cloud,
        Method::POST,
        "/admin/v1/bans",
        Some(json!({"handle": "sam", "reason": "cheating"})),
    )
    .await
    .ok(200);
    // The very next request fails, long before the access token would expire.
    let reply = cloud.get("/v1/me", b.token()).await;
    reply.is_error("banned", 403);
    assert_eq!(reply.body.get("until"), None, "a permanent ban has no end");
    assert_eq!(eb.next().await, Event::Banned { until: None });
    assert_eq!(eb.close_code().await, Some(close::BANNED));
    // His friends see him go.
    let Event::Presence { presence, .. } = ea.until(|e| matches!(e, Event::Presence { .. })).await
    else {
        unreachable!()
    };
    assert!(!presence.online);
    // He cannot reconnect, refresh or sign in again.
    let mut again = Events::connect_raw(&cloud.base, b.token()).await.unwrap();
    assert_eq!(
        again.next_within(Duration::from_secs(2)).await,
        Some(Event::Banned { until: None })
    );
    assert_eq!(again.close_code().await, Some(close::BANNED));
    cloud
        .request(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": b.tokens.refresh})),
        )
        .await
        .is_error("banned", 403);
    let (challenge, code) = cloud.ask_for_code(&b.email).await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("banned", 403);

    // Deleting the account does not lift the ban from the address... but nobody else is touched.
    cloud.get("/v1/me", a.token()).await.ok(200);

    admin(&cloud, Method::DELETE, "/admin/v1/bans/sam", None)
        .await
        .ok(204);
    admin(&cloud, Method::DELETE, "/admin/v1/bans/sam", None)
        .await
        .is_error("not_found", 404);
    cloud.get("/v1/me", b.token()).await.ok(200);
}

#[tokio::test]
async fn a_banned_person_cannot_start_over_with_the_same_address() {
    let cloud = with_admin().await;
    let b = cloud.sign_up("Sam").await;
    admin(
        &cloud,
        Method::POST,
        "/admin/v1/bans",
        Some(json!({"handle": "Sam", "days": 3})),
    )
    .await
    .ok(200);
    let reply = cloud.get("/v1/me", b.token()).await;
    reply.is_error("banned", 403);
    let until = reply.body["until"].as_u64().unwrap();
    let three_days = 3 * 24 * 60 * 60;
    assert!(until >= now() + three_days - 5 && until <= now() + three_days + 5);

    // The ban outlives the account.
    let (challenge, code) = cloud.ask_for_code(&b.email).await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("banned", 403);
    // Another address is a different matter.
    cloud.sign_up("Other").await;
    // Lifting the ban lets the person back in.
    admin(&cloud, Method::DELETE, "/admin/v1/bans/Sam", None)
        .await
        .ok(204);
    cloud.sign_in(&b.email, "pc").await;
}

#[tokio::test]
async fn a_ban_stays_when_the_account_is_gone() {
    let cloud = with_admin().await;
    let b = cloud.sign_up("Sam").await;
    admin(
        &cloud,
        Method::POST,
        "/admin/v1/bans",
        Some(json!({"handle": "Sam"})),
    )
    .await
    .ok(200);
    // A banned account cannot delete itself.
    cloud
        .delete("/v1/me", b.token())
        .await
        .is_error("banned", 403);
    // If the account row disappears anyway, the hash of the address still keeps the person out.
    anyplay_cloud::db::delete_account(&cloud.state.db, b.id)
        .await
        .unwrap();
    let (challenge, code) = cloud.ask_for_code(&b.email).await;
    cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .is_error("banned", 403);
}

#[tokio::test]
async fn a_banned_member_leaves_sessions_at_once() {
    let cloud = with_admin().await;
    let (a, b) = cloud.friends("Alec", "Sam").await;
    let mut ea = cloud.events(a.token()).await;
    let _eb = cloud.events(b.token()).await;
    let session: anyplay_api::Session = cloud
        .post(
            "/v1/sessions",
            a.token(),
            json!({"title_id": "G", "title": "G", "version": "1", "build": "1", "max_members": 4,
                   "visibility": "friends", "endpoint": "aa".repeat(32)}),
        )
        .await
        .ok(200)
        .json();
    cloud
        .post(
            &format!("/v1/sessions/{}/join", session.id),
            b.token(),
            json!({"version": "1", "build": "1", "endpoint": "bb".repeat(32)}),
        )
        .await
        .ok(200);
    let Event::Session { session: both } = ea.until(|e| matches!(e, Event::Session { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(both.members.len(), 2);
    admin(
        &cloud,
        Method::POST,
        "/admin/v1/bans",
        Some(json!({"handle": "Sam"})),
    )
    .await
    .ok(200);
    let Event::Session { session: alone } = ea.until(|e| matches!(e, Event::Session { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(alone.members.len(), 1);
}

#[tokio::test]
async fn the_admin_routes_need_the_token_and_exist_only_when_it_is_set() {
    let cloud = with_admin().await;
    cloud.sign_up("Sam").await;
    cloud
        .request(
            Method::POST,
            "/admin/v1/bans",
            None,
            Some(json!({"handle": "Sam"})),
        )
        .await
        .is_error("unauthorized", 401);
    cloud
        .request(
            Method::POST,
            "/admin/v1/bans",
            Some("wrong-token-wrong-token"),
            Some(json!({"handle": "Sam"})),
        )
        .await
        .is_error("unauthorized", 401);
    admin(
        &cloud,
        Method::POST,
        "/admin/v1/bans",
        Some(json!({"handle": "nobody"})),
    )
    .await
    .is_error("not_found", 404);
    admin(
        &cloud,
        Method::POST,
        "/admin/v1/bans",
        Some(json!({"handle": "Sam", "days": 0})),
    )
    .await
    .is_error("bad_request", 400);

    let plain = TestCloud::start().await;
    plain
        .request(Method::GET, "/admin/v1/reports", Some(ADMIN_TOKEN), None)
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_client_that_does_not_answer_pings_is_dropped() {
    let cloud = TestCloud::with(|_| {}, |t| t.ping_interval = Duration::from_millis(100)).await;
    let a = cloud.sign_up("Alec").await;
    let b = cloud.sign_up("Sam").await;
    cloud
        .post("/v1/friends/requests", a.token(), json!({"handle": "Sam"}))
        .await
        .ok(200);
    cloud
        .post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
        .await
        .ok(204);
    let mut ea = cloud.events(a.token()).await;
    let mut silent = cloud.events(b.token()).await;
    let _ = ea.until(|e| matches!(e, Event::Presence { .. })).await;
    // The test client answers pings only while it reads, and `silent` is never read; Alec keeps reading.
    let Event::Presence { presence, .. } = ea.until(|e| matches!(e, Event::Presence { .. })).await
    else {
        unreachable!()
    };
    assert!(!presence.online, "Sam was dropped for not answering");
    assert!(silent.ends_within(Duration::from_secs(3)).await);
}

#[tokio::test]
async fn a_client_that_answers_pings_stays() {
    let cloud = TestCloud::with(|_| {}, |t| t.ping_interval = Duration::from_millis(100)).await;
    let a = cloud.sign_up("Alec").await;
    let mut events = cloud.events(a.token()).await;
    // Reading lets the client library answer each ping.
    events.quiet(Duration::from_millis(700)).await;
    cloud.get("/v1/me", a.token()).await.ok(200);
}

#[tokio::test]
async fn a_new_signing_key_is_made_on_first_start_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (first, created) = anyplay_cloud::load_signing_key(dir.path()).unwrap();
    assert!(created);
    let (second, created) = anyplay_cloud::load_signing_key(dir.path()).unwrap();
    assert!(!created);
    assert_eq!(first.verifier(), second.verifier());
    let public = std::fs::read_to_string(dir.path().join("signing.pub")).unwrap();
    assert_eq!(public.trim(), first.verifier().to_base64());
}

#[test]
fn a_signing_key_file_is_only_read() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    let signer = anyplay_ticket::Signer::generate();
    std::fs::write(
        &key,
        format!(
            "{}
",
            signer.secret_base64()
        ),
    )
    .unwrap();
    let config = anyplay_cloud::Config {
        signing_key_file: Some(key.clone()),
        data_dir: dir.path().join("data"),
        ..anyplay_cloud::Config::default()
    };
    let (loaded, created) = anyplay_cloud::signing_key(&config).unwrap();
    assert!(!created);
    assert_eq!(loaded.verifier(), signer.verifier());
    let seen: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(seen, ["key"], "no signing.pub, no data folder");

    // A missing file is an error that names the setting, and nothing is created.
    std::fs::remove_file(&key).unwrap();
    let error = anyplay_cloud::signing_key(&config).unwrap_err();
    assert!(
        format!("{error:#}").contains("signing_key_file"),
        "{error:#}"
    );
    assert!(!key.exists());
    // So is a file that holds no key.
    std::fs::write(&key, "not a key").unwrap();
    assert!(anyplay_cloud::signing_key(&config).is_err());
}

#[tokio::test]
async fn shutting_down_closes_event_connections_and_stops_serving() {
    // This test builds its own client, so it cannot count on another test having set the
    // crypto provider first.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let config = anyplay_cloud::Config {
        data_dir: dir.path().to_path_buf(),
        drain_seconds: 0,
        ..Default::default()
    };
    let mail = std::sync::Arc::new(anyplay_cloud::mail::MemoryMailer::default());
    let state = anyplay_cloud::open(config, common::fast_tuning(), mail.clone())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(anyplay_cloud::serve(listener, state, async move {
        let _ = stopped.await;
    }));

    let http = reqwest::Client::new();
    let post = |path: &str, body: Value| {
        let http = http.clone();
        let url = format!("http://{base}{path}");
        async move {
            http.post(url)
                .json(&body)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let challenge = post("/v1/auth/email", json!({"email": "a@example.com"})).await;
    let code = mail.last_code("a@example.com").unwrap();
    let answer = post(
        "/v1/auth/code",
        json!({"challenge": challenge["challenge"], "code": code, "device": {"name": "pc", "platform": "linux"}}),
    )
    .await;
    let signed = post(
        "/v1/auth/signup",
        json!({"signup": answer["signup"], "handle": "Alec", "age_confirmed": true,
               "device": {"name": "pc", "platform": "linux"}}),
    )
    .await;
    let mut events = Events::connect(&base, signed["tokens"]["access"].as_str().unwrap())
        .await
        .unwrap();

    stop.send(()).unwrap();
    assert_eq!(events.close_code().await, Some(1001));
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the server stops")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn health_and_readiness_answer_and_metrics_count_requests_by_route() {
    let cloud = TestCloud::start().await;
    let health = cloud
        .http
        .get(format!("http://{}/healthz", cloud.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    let ready = cloud
        .http
        .get(format!("http://{}/readyz", cloud.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    let alec = cloud.sign_up("Alec").await;
    let _events = cloud.events(alec.token()).await;
    cloud.get("/v1/me", alec.token()).await.ok(200);

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
    assert!(text.contains("anyplay_cloud_http_requests_total{route=\"/v1/me\",status=\"200\"} 1"));
    assert!(
        text.contains("anyplay_cloud_http_requests_total{route=\"/healthz\",status=\"200\"} 1")
    );
    assert!(text.contains("anyplay_cloud_event_connections 1"));
    // The metrics stay off the public port.
    cloud
        .get("/metrics", alec.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_node_told_to_stop_reports_itself_not_ready_and_closes_connections_after_the_drain() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let config = anyplay_cloud::Config {
        data_dir: dir.path().to_path_buf(),
        drain_seconds: 1,
        ..Default::default()
    };
    let mail = std::sync::Arc::new(anyplay_cloud::mail::MemoryMailer::default());
    let state = anyplay_cloud::open(config, common::fast_tuning(), mail.clone())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(anyplay_cloud::serve(listener, state.clone(), async move {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::new();
    let challenge: Value = http
        .post(format!("http://{base}/v1/auth/email"))
        .json(&json!({"email": "a@example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = mail.last_code("a@example.com").unwrap();
    let answer: Value = http
        .post(format!("http://{base}/v1/auth/code"))
        .json(&json!({"challenge": challenge["challenge"], "code": code,
                      "device": {"name": "pc", "platform": "linux"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let signed: Value = http
        .post(format!("http://{base}/v1/auth/signup"))
        .json(
            &json!({"signup": answer["signup"], "handle": "Alec", "age_confirmed": true,
                      "device": {"name": "pc", "platform": "linux"}}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut events = Events::connect(&base, signed["tokens"]["access"].as_str().unwrap())
        .await
        .unwrap();

    stop.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ready = http
        .get(format!("http://{base}/readyz"))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 503, "a draining node takes no new players");
    assert!(
        !events.ends_within(Duration::from_millis(500)).await,
        "players keep their connection during the drain"
    );
    assert_eq!(events.close_code().await, Some(close::GOING_AWAY));
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the server stops")
        .unwrap()
        .unwrap();
}
