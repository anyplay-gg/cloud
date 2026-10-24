//! The account web and the hand-off to the app: sign in and create an account in a browser (a
//! cookie jar over plain HTTP), the one-time code to the loopback address with PKCE, the account
//! page, the link from the app and passkeys with a software authenticator.

mod common;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyplay_api::{AccountLink, BrowserTokenRequest, Device, Platform, SignedIn, path};
use anyplay_proof::{DeviceKey, Subject, pkce};
use common::{TestCloud, User};
use reqwest::header::{HeaderMap, SET_COOKIE};
use serde_json::{Value, json};
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

const REDIRECT: &str = "http://127.0.0.1:52000/callback";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// One answer of the web, with the body as text.
struct Page {
    status: u16,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl Page {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    fn location(&self) -> String {
        self.header("location")
            .unwrap_or_else(|| panic!("no redirect: {} {}", self.status, self.text()))
    }

    /// The CSRF token the page carries, in a form or in the `meta` tag.
    fn csrf(&self) -> String {
        let text = self.text();
        let marker = "name=\"csrf\" content=\"";
        let start = text.find(marker).expect("a csrf token") + marker.len();
        text[start..].split('"').next().unwrap().to_owned()
    }
}

fn form_request(
    request: reqwest::RequestBuilder,
    fields: &[(&str, &str)],
) -> reqwest::RequestBuilder {
    request
        .header("content-type", "application/x-www-form-urlencoded")
        .body(serde_urlencoded::to_string(fields).unwrap())
}

/// A browser: cookies, no redirects followed, nothing run.
struct Browser<'a> {
    cloud: &'a TestCloud,
    client: reqwest::Client,
    cookies: Mutex<HashMap<String, String>>,
    /// Headers every request carries, such as those a reverse proxy adds.
    extra: Vec<(&'static str, String)>,
}

impl<'a> Browser<'a> {
    fn with_headers(mut self, extra: &[(&'static str, &str)]) -> Self {
        self.extra = extra.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
        self
    }

    fn new(cloud: &'a TestCloud) -> Self {
        Self {
            extra: Vec::new(),
            cloud,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cookies: Mutex::new(HashMap::new()),
        }
    }

    fn cookie_header(&self) -> String {
        self.cookies
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Page {
        let mut request = request;
        for (name, value) in &self.extra {
            request = request.header(*name, value);
        }
        let cookie = self.cookie_header();
        let request = if cookie.is_empty() {
            request
        } else {
            request.header("cookie", cookie)
        };
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        for value in headers.get_all(SET_COOKIE) {
            let line = value.to_str().unwrap();
            let (pair, _) = line.split_once(';').unwrap_or((line, ""));
            let (name, value) = pair.split_once('=').unwrap();
            let mut jar = self.cookies.lock().unwrap();
            if value.is_empty() || line.contains("Max-Age=0") {
                jar.remove(name);
            } else {
                jar.insert(name.to_owned(), value.to_owned());
            }
        }
        let bytes = response.bytes().await.unwrap().to_vec();
        Page {
            status,
            headers,
            bytes,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.cloud.base)
    }

    async fn get(&self, path: &str) -> Page {
        self.send(self.client.get(self.url(path))).await
    }

    async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Page {
        self.send(form_request(self.client.post(self.url(path)), fields))
            .await
    }

    async fn post_json(&self, path: &str, body: Value, csrf: &str) -> Page {
        self.send(
            self.client
                .post(self.url(path))
                .header("x-csrf-token", csrf)
                .json(&body),
        )
        .await
    }

    /// Signs in on the account page by e-mail code. Returns the page it ends on.
    async fn sign_in_by_code(&self, email: &str) -> Page {
        let page = self.get("/web/sign-in").await;
        assert_eq!(page.status, 200);
        let sent = self
            .post_form(
                "/web/email",
                &[
                    ("csrf", &page.csrf_of_form()),
                    ("email", email),
                    ("intent", "sign_in"),
                ],
            )
            .await;
        assert_eq!(sent.status, 303, "{}", sent.text());
        assert_eq!(sent.location(), "/web/code");
        let code = self.cloud.mail.last_code(email).expect("a mail was sent");
        let entry = self.get("/web/code").await;
        self.post_form(
            "/web/code",
            &[("csrf", &entry.csrf_of_form()), ("code", &code)],
        )
        .await
    }
}

impl Page {
    /// The token in the first hidden `csrf` field of a form (or the meta tag when none).
    fn csrf_of_form(&self) -> String {
        let text = self.text();
        let marker = "name=\"csrf\" value=\"";
        match text.find(marker) {
            Some(start) => text[start + marker.len()..]
                .split('"')
                .next()
                .unwrap()
                .to_owned(),
            None => self.csrf(),
        }
    }
}

fn authorize_path(redirect: &str, state: &str, challenge: &str, intent: &str) -> String {
    format!(
        "/web/authorize?client=net&intent={intent}&redirect_uri={}&state={state}&code_challenge={challenge}&code_challenge_method=S256&device=Test%20PC",
        redirect.replace(':', "%3A").replace('/', "%2F")
    )
}

fn device(key: &DeviceKey) -> Device {
    Device {
        name: "Test PC".into(),
        platform: Platform::Linux,
        key: Some(key.public()),
    }
}

/// Signs an existing account in by API on a device that has `key`.
async fn sign_in_keyed(cloud: &TestCloud, email: &str, name: &str, key: &DeviceKey) -> SignedIn {
    let (challenge, code) = cloud.ask_for_code(email).await;
    let reply = cloud
        .request(
            reqwest::Method::POST,
            "/v1/auth/code",
            None,
            Some(json!(anyplay_api::CodeRequest {
                challenge: challenge.challenge,
                code,
                device: Device {
                    name: name.into(),
                    platform: Platform::Linux,
                    key: Some(key.public()),
                },
            })),
        )
        .await
        .ok(200);
    match reply.json() {
        anyplay_api::CodeAnswer::SignedIn { tokens, me } => SignedIn { tokens, me },
        other => panic!("expected a sign-in, got {other:?}"),
    }
}

async fn exchange(cloud: &TestCloud, code: &str, verifier: &str, key: &DeviceKey) -> common::Reply {
    cloud
        .request(
            reqwest::Method::POST,
            path::AUTH_BROWSER_TOKEN,
            None,
            Some(json!(BrowserTokenRequest {
                code: code.into(),
                verifier: verifier.into(),
                device: device(key),
            })),
        )
        .await
}

/// The code and state in the address the finished sign-in sends the browser to.
fn handoff(page: &Page) -> (String, String) {
    let text = page.text().replace("&amp;", "&");
    let start = text.find(REDIRECT).expect("the loopback address");
    let url = text[start..].split('"').next().unwrap();
    let query = url.split_once('?').unwrap().1;
    let get = |name: &str| {
        query
            .split('&')
            .find_map(|p| p.strip_prefix(&format!("{name}=")))
            .unwrap()
            .to_owned()
    };
    (get("code"), get("state"))
}

/// Runs the browser half of the sign-in for an existing account: authorize, e-mail, code.
async fn browser_sign_in(cloud: &TestCloud, email: &str, challenge: &str, state: &str) -> Page {
    let browser = Browser::new(cloud);
    let start = browser
        .get(&authorize_path(REDIRECT, state, challenge, "sign_in"))
        .await;
    assert_eq!(start.status, 303, "{}", start.text());
    assert_eq!(start.location(), "/web/sign-in");
    let ended = browser.sign_in_by_code(email).await;
    assert_eq!(ended.status, 303, "{}", ended.text());
    assert_eq!(ended.location(), "/web/next");
    browser.get("/web/next").await
}

const STATE: &str = "state-0123456789abcdef";

#[tokio::test]
async fn pages_carry_the_security_headers_and_load_nothing_from_elsewhere() {
    let cloud = TestCloud::start().await;
    let browser = Browser::new(&cloud);
    let page = browser.get("/web/sign-in").await;
    assert_eq!(page.status, 200);
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("default-src 'none'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("unsafe-inline"));
    assert!(!csp.contains("http"), "no outside origin: {csp}");
    assert_eq!(page.header("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(page.header("referrer-policy").unwrap(), "no-referrer");
    assert_eq!(page.header("x-frame-options").unwrap(), "DENY");
    assert!(page.header("cache-control").unwrap().contains("no-store"));
    let cookie = page.header("set-cookie").unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Path=/") && !cookie.contains("Domain"));

    let html = page.text();
    assert!(!html.contains("<script>"), "only script files of ours");
    assert!(!html.contains(" onclick=") && !html.contains(" style=\""));
    for part in html
        .split("src=\"")
        .skip(1)
        .chain(html.split("href=\"").skip(1))
    {
        let target = part.split('"').next().unwrap();
        assert!(
            target.starts_with('/') || target.starts_with('#'),
            "an outside address: {target}"
        );
    }

    for (name, kind) in [
        ("style.css", "text/css"),
        ("passkey.js", "text/javascript"),
        ("icon.svg", "image/svg+xml"),
        ("inter.woff2", "font/woff2"),
        ("inter-OFL.txt", "text/plain"),
    ] {
        let asset = browser.get(&format!("/web/assets/{name}")).await;
        assert_eq!(asset.status, 200, "{name}");
        assert!(
            asset.header("content-type").unwrap().starts_with(kind),
            "{name}"
        );
        assert!(asset.header("content-security-policy").is_some());
    }
    let font = browser.get("/web/assets/inter.woff2").await;
    assert_eq!(&font.bytes[..4], b"wOF2");
    assert_eq!(browser.get("/web/assets/nothing.js").await.status, 404);
}

#[tokio::test]
async fn info_names_the_account_web() {
    let cloud = TestCloud::start().await;
    let info: anyplay_api::Info = cloud
        .request(reqwest::Method::GET, "/v1/info", None, None)
        .await
        .ok(200)
        .json();
    let web = info.web.expect("the test cloud has a public address");
    assert!(web.starts_with("http://localhost:"));
}

#[tokio::test]
async fn a_cloud_without_a_public_address_has_no_web() {
    let cloud = TestCloud::with(|c| c.public_url = None, |_| {}).await;
    let browser = Browser::new(&cloud);
    assert_eq!(browser.get("/web/sign-in").await.status, 404);
    let info: anyplay_api::Info = cloud
        .request(reqwest::Method::GET, "/v1/info", None, None)
        .await
        .ok(200)
        .json();
    assert_eq!(info.web, None);
}

#[tokio::test]
async fn a_browser_sign_in_ends_with_a_one_time_code_for_the_app() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let verifier = pkce::new_verifier();
    let challenge = pkce::challenge(&verifier);
    let page = browser_sign_in(&cloud, &alec.email, &challenge, STATE).await;
    assert_eq!(page.status, 200);
    assert!(page.header("set-cookie").unwrap().contains("Max-Age=0"));
    let (code, state) = handoff(&page);
    assert_eq!(state, STATE, "the state comes back unchanged");

    let key = DeviceKey::generate();
    let signed_in: SignedIn = exchange(&cloud, &code, &verifier, &key)
        .await
        .ok(200)
        .json();
    assert_eq!(signed_in.me.handle, "Alec");
    assert_eq!(
        cloud.get("/v1/me", &signed_in.tokens.access).await.status,
        200
    );
    // The new device is bound to its key: its refresh token needs a proof.
    let refresh = &signed_in.tokens.refresh;
    let bare = cloud
        .request(
            reqwest::Method::POST,
            path::AUTH_REFRESH,
            None,
            Some(json!({"refresh": refresh})),
        )
        .await;
    bare.is_error("unauthorized", 401);
    let proof = anyplay_proof::make(
        &key,
        &Subject {
            method: "POST",
            path: path::AUTH_REFRESH,
            credential: refresh,
        },
        now(),
    );
    let ok = cloud
        .http
        .post(format!("http://{}{}", cloud.base, path::AUTH_REFRESH))
        .header(anyplay_proof::HEADER, proof)
        .json(&json!({"refresh": refresh}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status().as_u16(), 200);

    // The code was used up.
    exchange(&cloud, &code, &verifier, &key)
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn a_wrong_verifier_fails_and_uses_the_code_up() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let verifier = pkce::new_verifier();
    let page = browser_sign_in(&cloud, &alec.email, &pkce::challenge(&verifier), STATE).await;
    let (code, _) = handoff(&page);
    let key = DeviceKey::generate();
    // Somebody who saw the redirect has the code but not the verifier.
    exchange(&cloud, &code, &pkce::new_verifier(), &key)
        .await
        .is_error("code_wrong", 400);
    // The right verifier comes too late: one try only.
    exchange(&cloud, &code, &verifier, &key)
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn an_expired_code_fails() {
    let cloud = TestCloud::with(|_| {}, |t| t.link_ttl = Duration::from_millis(200)).await;
    let alec = cloud.sign_up("Alec").await;
    let verifier = pkce::new_verifier();
    let page = browser_sign_in(&cloud, &alec.email, &pkce::challenge(&verifier), STATE).await;
    let (code, _) = handoff(&page);
    tokio::time::sleep(Duration::from_millis(400)).await;
    exchange(&cloud, &code, &verifier, &DeviceKey::generate())
        .await
        .is_error("code_expired", 400);
}

#[tokio::test]
async fn a_redirect_to_anything_but_loopback_is_refused() {
    let cloud = TestCloud::start().await;
    let challenge = pkce::challenge(&pkce::new_verifier());
    for bad in [
        "https://evil.example/callback",
        "http://evil.example:52000/callback",
        "http://localhost:52000/callback",
        "http://127.0.0.1.evil.example:52000/callback",
        "http://127.0.0.1:52000@evil.example/callback",
        "http://127.0.0.1:80/callback",
        "http://127.0.0.1:52000/callback?next=https://evil.example",
        "anyplay://callback",
        "javascript:alert(1)",
    ] {
        let browser = Browser::new(&cloud);
        let page = browser
            .get(&authorize_path(bad, STATE, &challenge, "sign_in"))
            .await;
        assert_eq!(page.status, 400, "{bad}");
        assert!(page.header("set-cookie").is_none(), "{bad}: no session");
        assert!(!page.text().contains("evil.example"), "{bad}");
    }
    // Other parts of the request must be in order too.
    let browser = Browser::new(&cloud);
    for path in [
        format!(
            "/web/authorize?client=net&redirect_uri=http%3A%2F%2F127.0.0.1%3A52000%2Fcallback&state=short&code_challenge={challenge}&code_challenge_method=S256"
        ),
        format!(
            "/web/authorize?client=net&redirect_uri=http%3A%2F%2F127.0.0.1%3A52000%2Fcallback&state={STATE}&code_challenge={challenge}&code_challenge_method=plain"
        ),
        format!(
            "/web/authorize?client=net&redirect_uri=http%3A%2F%2F127.0.0.1%3A52000%2Fcallback&state={STATE}&code_challenge=short&code_challenge_method=S256"
        ),
        format!(
            "/web/authorize?client=other&redirect_uri=http%3A%2F%2F127.0.0.1%3A52000%2Fcallback&state={STATE}&code_challenge={challenge}&code_challenge_method=S256"
        ),
        "/web/authorize".to_owned(),
    ] {
        assert_eq!(browser.get(&path).await.status, 400, "{path}");
    }
}

#[tokio::test]
async fn a_new_address_creates_an_account_and_the_cloud_records_age_and_terms() {
    let cloud = TestCloud::with(|c| c.terms_version = Some("2026-10".into()), |_| {}).await;
    let verifier = pkce::new_verifier();
    let browser = Browser::new(&cloud);
    browser
        .get(&authorize_path(
            REDIRECT,
            STATE,
            &pkce::challenge(&verifier),
            "create",
        ))
        .await;
    let page = browser.get("/web/create").await;
    assert!(page.text().contains("Create account"));
    let sent = browser
        .post_form(
            "/web/email",
            &[
                ("csrf", &page.csrf_of_form()),
                ("email", "New.Player@Example.com"),
                ("intent", "create"),
            ],
        )
        .await;
    assert_eq!(sent.location(), "/web/code");
    let code = cloud.mail.last_code("new.player@example.com").unwrap();
    let entry = browser.get("/web/code").await;
    let verified = browser
        .post_form(
            "/web/code",
            &[("csrf", &entry.csrf_of_form()), ("code", &code)],
        )
        .await;
    assert_eq!(verified.location(), "/web/details");
    let details = browser.get("/web/details").await;
    assert!(details.text().contains("version 2026-10"));
    let csrf = details.csrf_of_form();

    // The rules of the API apply: a reserved handle, a missing age, missing terms.
    for (fields, expect) in [
        (
            vec![("handle", "admin"), ("age", "yes"), ("terms", "yes")],
            "reserved",
        ),
        (vec![("handle", "Newbie"), ("terms", "yes")], "old enough"),
        (
            vec![("handle", "Newbie"), ("age", "yes")],
            "Accept the terms",
        ),
        (
            vec![("handle", "a"), ("age", "yes"), ("terms", "yes")],
            "3 to 16",
        ),
    ] {
        let mut form = vec![("csrf", csrf.as_str())];
        form.extend(fields);
        let page = browser.post_form("/web/details", &form).await;
        assert_eq!(page.status, 400);
        assert!(page.text().contains(expect), "{expect}: {}", page.text());
        assert!(page.text().contains("id=\"problem\""));
        assert!(page.text().contains("aria-invalid=\"true\""));
    }

    let created = browser
        .post_form(
            "/web/details",
            &[
                ("csrf", &csrf),
                ("handle", "Newbie"),
                ("age", "yes"),
                ("terms", "yes"),
            ],
        )
        .await;
    assert_eq!(created.status, 303, "{}", created.text());
    assert_eq!(created.location(), "/web/next");
    let (code, state) = handoff(&browser.get("/web/next").await);
    assert_eq!(state, STATE);
    let signed_in: SignedIn = exchange(&cloud, &code, &verifier, &DeviceKey::generate())
        .await
        .ok(200)
        .json();
    assert_eq!(signed_in.me.handle, "Newbie");
    assert_eq!(signed_in.me.email, "new.player@example.com");

    let row: (Option<i64>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT age_confirmed_at, terms_version, terms_accepted_at FROM accounts WHERE id = $1",
    )
    .bind(signed_in.me.id.0 as i64)
    .fetch_one(&cloud.state.db)
    .await
    .unwrap();
    let now = now() as i64;
    assert!(row.0.is_some_and(|t| (now - 5..=now + 5).contains(&t)));
    assert_eq!(row.1.as_deref(), Some("2026-10"));
    assert!(row.2.is_some());
}

#[tokio::test]
async fn a_new_version_of_the_terms_is_asked_for_at_the_next_web_sign_in() {
    let cloud = TestCloud::with(|c| c.terms_version = Some("2".into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    // An account made through the API has accepted nothing; one that accepted version 1 has to
    // accept version 2.
    sqlx::query("UPDATE accounts SET terms_version = '1', terms_accepted_at = 1 WHERE id = $1")
        .bind(alec.id as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let browser = Browser::new(&cloud);
    let ended = browser.sign_in_by_code(&alec.email).await;
    assert_eq!(ended.location(), "/web/terms");
    let terms = browser.get("/web/terms").await;
    assert!(terms.text().contains("version 2"));
    // Without accepting there is no way on.
    let refused = browser
        .post_form("/web/terms", &[("csrf", &terms.csrf_of_form())])
        .await;
    assert_eq!(refused.status, 400);
    assert_eq!(browser.get("/web/account").await.location(), "/web/sign-in");

    let accepted = browser
        .post_form(
            "/web/terms",
            &[("csrf", &terms.csrf_of_form()), ("accept", "yes")],
        )
        .await;
    assert_eq!(accepted.location(), "/web/account");
    assert_eq!(browser.get("/web/account").await.status, 200);
    let version: (Option<String>,) =
        sqlx::query_as("SELECT terms_version FROM accounts WHERE id = $1")
            .bind(alec.id as i64)
            .fetch_one(&cloud.state.db)
            .await
            .unwrap();
    assert_eq!(version.0.as_deref(), Some("2"));
    // The next sign-in does not ask again.
    let again = Browser::new(&cloud);
    assert_eq!(
        again.sign_in_by_code(&alec.email).await.location(),
        "/web/account"
    );
}

#[tokio::test]
async fn a_wrong_code_is_explained_and_too_many_end_the_challenge() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let browser = Browser::new(&cloud);
    let page = browser.get("/web/sign-in").await;
    browser
        .post_form(
            "/web/email",
            &[
                ("csrf", &page.csrf_of_form()),
                ("email", &alec.email),
                ("intent", "sign_in"),
            ],
        )
        .await;
    let entry = browser.get("/web/code").await;
    let wrong = browser
        .post_form(
            "/web/code",
            &[("csrf", &entry.csrf_of_form()), ("code", "000000")],
        )
        .await;
    assert_eq!(wrong.status, 400);
    assert!(wrong.text().contains("That code is wrong"));
    assert!(wrong.text().contains("autocomplete=\"one-time-code\""));
}

#[tokio::test]
async fn forms_need_their_token_and_the_right_origin() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let mailed = cloud.mail.last_code(&alec.email);
    let browser = Browser::new(&cloud);
    let page = browser.get("/web/sign-in").await;

    let wrong = browser
        .post_form(
            "/web/email",
            &[
                ("csrf", "wrong"),
                ("email", &alec.email),
                ("intent", "sign_in"),
            ],
        )
        .await;
    assert_eq!(wrong.status, 403);
    // No session at all.
    let stranger = Browser::new(&cloud);
    let none = stranger
        .post_form(
            "/web/email",
            &[("csrf", &page.csrf_of_form()), ("email", &alec.email)],
        )
        .await;
    assert_eq!(none.status, 403);
    // Another site made the browser send it.
    for (name, value) in [
        ("origin", "https://evil.example"),
        ("sec-fetch-site", "cross-site"),
    ] {
        let request = form_request(
            browser
                .client
                .post(browser.url("/web/email"))
                .header(name, value),
            &[("csrf", &page.csrf_of_form()), ("email", &alec.email)],
        );
        let cross = browser.send(request).await;
        assert_eq!(cross.status, 403, "{name}");
    }
    assert_eq!(
        cloud.mail.last_code(&alec.email),
        mailed,
        "no mail went out"
    );
}

/// A signed-in browser on the account page, and the devices of the account.
async fn account_browser<'a>(cloud: &'a TestCloud, user: &User) -> Browser<'a> {
    let browser = Browser::new(cloud);
    let ended = browser.sign_in_by_code(&user.email).await;
    assert_eq!(ended.location(), "/web/account", "{}", ended.text());
    browser
}

#[tokio::test]
async fn the_account_page_shows_the_account_and_signs_devices_out() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let second = cloud.sign_in(&alec.email, "Laptop <b>").await;
    let browser = account_browser(&cloud, &alec).await;
    let page = browser.get("/web/account").await;
    assert_eq!(page.status, 200);
    let html = page.text();
    assert!(html.contains("alec@example.com") && html.contains("Alec"));
    assert!(html.contains("Laptop &lt;b&gt;"), "names are escaped");
    assert!(!html.contains("Laptop <b>"));
    assert!(html.contains("No passkeys yet"));
    let csrf = page.csrf();

    // A device signs out.
    let gone = browser
        .post_form(
            &format!("/web/account/devices/{}/sign-out", second.tokens.device),
            &[("csrf", &csrf)],
        )
        .await;
    assert_eq!(gone.location(), "/web/account");
    assert_eq!(cloud.get("/v1/me", &second.tokens.access).await.status, 401);
    assert_eq!(cloud.get("/v1/me", alec.token()).await.status, 200);
    assert!(
        browser
            .get("/web/account")
            .await
            .text()
            .contains("Device signed out.")
    );

    // Another account's device is not found.
    let sam = cloud.sign_up("Sam").await;
    let missing = browser
        .post_form(
            &format!("/web/account/devices/{}/sign-out", sam.tokens.device),
            &[("csrf", &csrf)],
        )
        .await;
    assert_eq!(missing.status, 404);
    assert_eq!(cloud.get("/v1/me", sam.token()).await.status, 200);
}

#[tokio::test]
async fn signing_out_everywhere_ends_devices_and_browsers() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let other = cloud.sign_in(&alec.email, "Laptop").await;
    let browser = account_browser(&cloud, &alec).await;
    let second_browser = account_browser(&cloud, &alec).await;
    let csrf = browser.get("/web/account").await.csrf();
    let done = browser
        .post_form("/web/account/sign-out-everywhere", &[("csrf", &csrf)])
        .await;
    assert_eq!(done.status, 200);
    assert!(done.text().contains("Signed out everywhere"));
    assert_eq!(cloud.get("/v1/me", alec.token()).await.status, 401);
    assert_eq!(cloud.get("/v1/me", &other.tokens.access).await.status, 401);
    assert_eq!(browser.get("/web/account").await.location(), "/web/sign-in");
    assert_eq!(
        second_browser.get("/web/account").await.location(),
        "/web/sign-in"
    );
}

#[tokio::test]
async fn sensitive_actions_need_a_recent_sign_in() {
    let cloud = TestCloud::with(|_| {}, |t| t.fresh_for = Duration::from_millis(50)).await;
    let alec = cloud.sign_up("Alec").await;
    let browser = account_browser(&cloud, &alec).await;
    let csrf = browser.get("/web/account").await.csrf();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let refused = browser
        .post_form("/web/account/sign-out-everywhere", &[("csrf", &csrf)])
        .await;
    assert_eq!(refused.status, 403);
    assert!(refused.text().contains("Sign in again"));
    assert_eq!(cloud.get("/v1/me", alec.token()).await.status, 200);
    assert_eq!(browser.get("/web/account/delete").await.status, 403);
    // Reading is fine.
    assert_eq!(browser.get("/web/account").await.status, 200);
}

#[tokio::test]
async fn the_data_export_and_the_deletion_of_the_account() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let browser = account_browser(&cloud, &alec).await;
    let export = browser.get("/web/account/export").await;
    assert_eq!(export.status, 200);
    assert!(
        export
            .header("content-disposition")
            .unwrap()
            .contains("attachment")
    );
    assert_eq!(export.json()["account"]["handle"], "Alec");
    assert_eq!(export.json()["passkeys"], json!([]));
    assert!(export.json()["consent"].get("terms_version").is_some());

    let confirm = browser.get("/web/account/delete").await;
    assert_eq!(confirm.status, 200);
    let csrf = confirm.csrf_of_form();
    let wrong = browser
        .post_form("/web/account/delete", &[("csrf", &csrf), ("handle", "Sam")])
        .await;
    assert_eq!(wrong.status, 400);
    assert_eq!(cloud.get("/v1/me", alec.token()).await.status, 200);

    // Deleting waits for the grace period: devices are signed out, the account stays, and this
    // browser stays signed in.
    let scheduled = browser
        .post_form(
            "/web/account/delete",
            &[("csrf", &csrf), ("handle", "alec")],
        )
        .await;
    assert_eq!(scheduled.status, 200);
    assert!(scheduled.text().contains("Account deletion scheduled"));
    assert!(scheduled.text().contains("30 days"));
    assert_eq!(cloud.get("/v1/me", alec.token()).await.status, 401);
    assert_eq!(browser.get("/web/account").await.status, 200);
    assert!(
        anyplay_cloud::db::deletion_of(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_some()
    );
    // When the time has come the sweep deletes it.
    sqlx::query("UPDATE accounts SET deletion_at = 1 WHERE id = $1")
        .bind(alec.id as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    cloud.state.sweep().await.unwrap();
    assert!(
        anyplay_cloud::db::account(&cloud.state.db, alec.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(browser.get("/web/account").await.location(), "/web/sign-in");
}

#[tokio::test]
async fn the_account_link_opens_the_web_once_and_only_for_the_device_key() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let key = DeviceKey::generate();
    let device = sign_in_keyed(&cloud, &alec.email, "Keyed PC", &key).await;
    let access = device.tokens.access.clone();

    let ask = |proof: Option<String>| {
        let mut request = cloud
            .http
            .post(format!("http://{}{}", cloud.base, path::AUTH_BROWSER_LINK))
            .bearer_auth(&access);
        if let Some(proof) = proof {
            request = request.header(anyplay_proof::HEADER, proof);
        }
        async move {
            let response = request.send().await.unwrap();
            (response.status().as_u16(), response.text().await.unwrap())
        }
    };
    let proof = |key: &DeviceKey| {
        anyplay_proof::make(
            key,
            &Subject {
                method: "POST",
                path: path::AUTH_BROWSER_LINK,
                credential: &access,
            },
            now(),
        )
    };
    // A stolen access token without the key gets no link.
    assert_eq!(ask(None).await.0, 401);
    assert_eq!(ask(Some(proof(&DeviceKey::generate()))).await.0, 401);
    // A legacy device has no key to prove.
    let legacy = cloud.sign_in(&alec.email, "Legacy PC").await;
    let legacy_reply = cloud
        .post_empty(path::AUTH_BROWSER_LINK, &legacy.tokens.access)
        .await;
    assert_eq!(legacy_reply.status, 401);

    let (status, body) = ask(Some(proof(&key))).await;
    assert_eq!(status, 200, "{body}");
    let link: AccountLink = serde_json::from_str(&body).unwrap();
    let url = Url::parse(&link.url).unwrap();
    assert!(link.url.starts_with("http://localhost:"));
    assert_eq!(url.path(), "/web/link");
    let target = format!("{}?{}", url.path(), url.query().unwrap());

    // Fetching the address changes nothing; the button signs in.
    let browser = Browser::new(&cloud);
    let page = browser.get(&target).await;
    assert_eq!(page.status, 200);
    assert!(page.text().contains("Open account"));
    assert!(
        browser.get(&target).await.status == 200,
        "a preview does not use it up"
    );
    assert_eq!(browser.get("/web/account").await.location(), "/web/sign-in");
    let code = url
        .query_pairs()
        .find(|(k, _)| k == "c")
        .unwrap()
        .1
        .into_owned();
    let opened = browser
        .post_form(
            "/web/link",
            &[("csrf", &page.csrf_of_form()), ("code", &code)],
        )
        .await;
    assert_eq!(opened.location(), "/web/account");
    assert_eq!(browser.get("/web/account").await.status, 200);

    // Used once.
    let again = Browser::new(&cloud);
    let page = again.get("/web/sign-in").await;
    let reused = again
        .post_form(
            "/web/link",
            &[("csrf", &page.csrf_of_form()), ("code", &code)],
        )
        .await;
    assert_eq!(reused.status, 400);
    assert_eq!(again.get("/web/account").await.location(), "/web/sign-in");

    // The device signing out kills a link it made.
    let (_, body) = ask(Some(proof(&key))).await;
    let link: AccountLink = serde_json::from_str(&body).unwrap();
    cloud
        .state
        .sign_out_device(device.tokens.device)
        .await
        .unwrap();
    let late = Browser::new(&cloud);
    let url = Url::parse(&link.url).unwrap();
    let target = format!("{}?{}", url.path(), url.query().unwrap());
    let page = late.get(&target).await;
    let code = url
        .query_pairs()
        .find(|(k, _)| k == "c")
        .unwrap()
        .1
        .into_owned();
    let refused = late
        .post_form(
            "/web/link",
            &[("csrf", &page.csrf_of_form()), ("code", &code)],
        )
        .await;
    assert_eq!(refused.status, 400);
}

// Passkeys

fn origin_of(cloud: &TestCloud) -> Url {
    let web = cloud.state.web.as_ref().unwrap();
    Url::parse(&web.origin).unwrap()
}

/// Registers a passkey on the signed-in browser's account with a software authenticator.
/// Returns the credential ID (base64url) and the user handle of the account.
async fn add_passkey(
    cloud: &TestCloud,
    browser: &Browser<'_>,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    name: &str,
) -> (String, String) {
    let csrf = browser.get("/web/account").await.csrf();
    let started = browser
        .post_json("/web/account/passkeys/start", json!({}), &csrf)
        .await;
    assert_eq!(started.status, 200, "{}", started.text());
    let started = started.json();
    let options = started["options"].clone();
    assert_eq!(
        options["publicKey"]["authenticatorSelection"]["residentKey"],
        "preferred"
    );
    assert_eq!(
        options["publicKey"]["authenticatorSelection"]["userVerification"],
        "required"
    );
    let user_handle = options["publicKey"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let creation: CreationChallengeResponse = serde_json::from_value(options).unwrap();
    let credential = authenticator
        .do_registration(origin_of(cloud), creation)
        .expect("the software authenticator makes a passkey");
    let raw_id = serde_json::to_value(&credential).unwrap()["rawId"]
        .as_str()
        .unwrap()
        .to_owned();
    let finished = browser
        .post_json(
            "/web/account/passkeys/finish",
            json!({"id": started["id"], "name": name, "credential": credential}),
            &csrf,
        )
        .await;
    assert_eq!(finished.status, 200, "{}", finished.text());
    (raw_id, user_handle)
}

/// What an authenticator that stores discoverable credentials would answer: the software one
/// needs the credential named, and it does not send the user handle.
fn discoverable_answer(
    cloud: &TestCloud,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    mut options: Value,
    raw_id: &str,
    user_handle: &str,
) -> Value {
    options["publicKey"]["allowCredentials"] = json!([{"type": "public-key", "id": raw_id}]);
    let request: RequestChallengeResponse = serde_json::from_value(options).unwrap();
    let assertion = authenticator
        .do_authentication(origin_of(cloud), request)
        .expect("the software authenticator signs");
    let mut assertion = serde_json::to_value(&assertion).unwrap();
    assertion["response"]["userHandle"] = json!(user_handle);
    assertion
}

#[tokio::test]
async fn a_passkey_is_added_and_signs_in_without_an_address() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let browser = account_browser(&cloud, &alec).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (raw_id, user_handle) =
        add_passkey(&cloud, &browser, &mut authenticator, "My laptop").await;

    let page = browser.get("/web/account").await.text();
    assert!(page.contains("My laptop") && page.contains("Passkey added."));
    let row = sqlx::query_as::<_, (String,)>("SELECT name FROM passkeys WHERE account_id = $1")
        .bind(alec.id as i64)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(row.0, "My laptop");

    // A fresh browser signs in with it.
    let other = Browser::new(&cloud);
    let sign_in = other.get("/web/sign-in").await;
    let csrf = sign_in.csrf();
    let started = other
        .post_json("/web/passkey/sign-in/start", json!({}), &csrf)
        .await;
    assert_eq!(started.status, 200, "{}", started.text());
    let started = started.json();
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &user_handle,
    );
    let finished = other
        .post_json(
            "/web/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer.clone()}),
            &csrf,
        )
        .await;
    assert_eq!(finished.status, 200, "{}", finished.text());
    assert_eq!(finished.json()["next"], "/web/account");
    let account = other.get("/web/account").await;
    assert_eq!(account.status, 200);
    assert!(account.text().contains("alec@example.com"));

    // The response cannot be used a second time: the ceremony was taken.
    let replay = Browser::new(&cloud);
    let page = replay.get("/web/sign-in").await;
    let replayed = replay
        .post_json(
            "/web/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer}),
            &page.csrf(),
        )
        .await;
    assert_eq!(replayed.status, 401);
    assert_eq!(replay.get("/web/account").await.location(), "/web/sign-in");

    // The signature counter and last use moved on.
    let used: (Option<i64>,) =
        sqlx::query_as("SELECT last_used FROM passkeys WHERE account_id = $1")
            .bind(alec.id as i64)
            .fetch_one(&cloud.state.db)
            .await
            .unwrap();
    assert!(used.0.is_some());
}

#[tokio::test]
async fn a_passkey_that_belongs_to_another_user_handle_is_refused() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let browser = account_browser(&cloud, &alec).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (raw_id, _) = add_passkey(&cloud, &browser, &mut authenticator, "Alec's key").await;
    let sam_browser = account_browser(&cloud, &sam).await;
    let mut sam_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (_, sam_handle) =
        add_passkey(&cloud, &sam_browser, &mut sam_authenticator, "Sam's key").await;

    // Alec's credential, presented with Sam's user handle.
    let other = Browser::new(&cloud);
    let csrf = other.get("/web/sign-in").await.csrf();
    let started = other
        .post_json("/web/passkey/sign-in/start", json!({}), &csrf)
        .await
        .json();
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &sam_handle,
    );
    let refused = other
        .post_json(
            "/web/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer}),
            &csrf,
        )
        .await;
    assert_eq!(refused.status, 401);
    assert_eq!(other.get("/web/account").await.location(), "/web/sign-in");
}

#[tokio::test]
async fn a_passkey_signs_in_for_the_app_too_and_can_be_removed() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let browser = account_browser(&cloud, &alec).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (raw_id, user_handle) = add_passkey(&cloud, &browser, &mut authenticator, "Key").await;

    // The app opens the browser; the player uses the passkey.
    let verifier = pkce::new_verifier();
    let other = Browser::new(&cloud);
    other
        .get(&authorize_path(
            REDIRECT,
            STATE,
            &pkce::challenge(&verifier),
            "sign_in",
        ))
        .await;
    let csrf = other.get("/web/sign-in").await.csrf();
    let started = other
        .post_json("/web/passkey/sign-in/start", json!({}), &csrf)
        .await
        .json();
    let answer = discoverable_answer(
        &cloud,
        &mut authenticator,
        started["options"].clone(),
        &raw_id,
        &user_handle,
    );
    let finished = other
        .post_json(
            "/web/passkey/sign-in/finish",
            json!({"id": started["id"], "credential": answer}),
            &csrf,
        )
        .await;
    assert_eq!(finished.json()["next"], "/web/next", "{}", finished.text());
    let (code, state) = handoff(&other.get("/web/next").await);
    assert_eq!(state, STATE);
    let signed_in: SignedIn = exchange(&cloud, &code, &verifier, &DeviceKey::generate())
        .await
        .ok(200)
        .json();
    assert_eq!(signed_in.me.handle, "Alec");

    // Removing it needs the token and removes it.
    let page = browser.get("/web/account").await;
    let id = raw_id.clone();
    let bad = browser
        .post_form(
            &format!("/web/account/passkeys/{id}/remove"),
            &[("csrf", "nope")],
        )
        .await;
    assert_eq!(bad.status, 403);
    let removed = browser
        .post_form(
            &format!("/web/account/passkeys/{id}/remove"),
            &[("csrf", &page.csrf())],
        )
        .await;
    assert_eq!(removed.location(), "/web/account");
    assert!(
        browser
            .get("/web/account")
            .await
            .text()
            .contains("Passkey removed.")
    );
    let left: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM passkeys")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(left.0, 0);
}

#[tokio::test]
async fn passkey_requests_need_a_session_a_token_and_a_recent_sign_in() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let stranger = Browser::new(&cloud);
    assert_eq!(
        stranger
            .post_json("/web/account/passkeys/start", json!({}), "x")
            .await
            .status,
        401
    );
    let browser = account_browser(&cloud, &alec).await;
    assert_eq!(
        browser
            .post_json("/web/account/passkeys/start", json!({}), "wrong")
            .await
            .status,
        401
    );
    // The sign-in ceremony does not hand out options without the page's token either.
    let anon = Browser::new(&cloud);
    anon.get("/web/sign-in").await;
    assert_eq!(
        anon.post_json("/web/passkey/sign-in/start", json!({}), "wrong")
            .await
            .status,
        401
    );
}

// Moderation and the proxy

const ADMIN: &str = "admin-token-0123456789";

async fn ban(cloud: &TestCloud, handle: &str, reason: &str) {
    cloud
        .request(
            reqwest::Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": handle, "rule": "cheating", "reason": reason})),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn a_banned_player_sees_the_statement_on_the_web_and_can_appeal() {
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    ban(&cloud, "Alec", "Aimbot in the arena.").await;

    let browser = Browser::new(&cloud);
    let ended = browser.sign_in_by_code(&alec.email).await;
    assert_eq!(ended.location(), "/web/banned", "{}", ended.text());
    let page = browser.get("/web/banned").await;
    assert_eq!(page.status, 403);
    let text = page.text();
    assert!(text.contains("This account is banned"));
    assert!(text.contains("Cheating") && text.contains("Aimbot in the arena."));
    assert!(text.contains("The ban is for good."));
    assert!(
        text.contains("action=\"/web/appeal\""),
        "the player can appeal"
    );
    assert!(
        !text.contains("appeal_token"),
        "the token never reaches the page"
    );
    // A banned player has no account page.
    assert_eq!(browser.get("/web/account").await.status, 303);

    // An empty appeal is explained; a real one is filed once.
    let csrf = page.csrf_of_form();
    let empty = browser
        .post_form("/web/appeal", &[("csrf", &csrf), ("text", "  ")])
        .await;
    assert_eq!(empty.status, 400);
    assert!(empty.text().contains("1 to 1,000 characters"));
    let sent = browser
        .post_form(
            "/web/appeal",
            &[("csrf", &csrf), ("text", "I did not cheat.")],
        )
        .await;
    assert_eq!(sent.location(), "/web/banned");
    let after = browser.get("/web/banned").await.text();
    assert!(after.contains("Your appeal") && after.contains("A moderator is looking"));
    assert!(!after.contains("action=\"/web/appeal\""));
    let statements = anyplay_cloud::moderation::statements(&cloud.state, alec.id)
        .await
        .unwrap();
    assert_eq!(
        statements[0].appeal.text.as_deref(),
        Some("I did not cheat.")
    );
    // A form without the token of its session is refused.
    let thief = Browser::new(&cloud);
    thief.get("/web/sign-in").await;
    assert_eq!(
        thief
            .post_form("/web/appeal", &[("csrf", "x"), ("text", "hi")])
            .await
            .status,
        403
    );
}

#[tokio::test]
async fn a_banned_address_gets_no_new_account_on_the_web() {
    let cloud = TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    ban(&cloud, "Alec", "Aimbot.").await;
    cloud.state.delete_account(alec.id).await.unwrap();

    // The address is banned for good, even with its account gone: sign in and create account
    // both end at the statement, with no way to appeal for an account that is not there.
    for intent in ["sign_in", "create"] {
        let browser = Browser::new(&cloud);
        let start = if intent == "create" {
            "/web/create"
        } else {
            "/web/sign-in"
        };
        let page = browser.get(start).await;
        let sent = browser
            .post_form(
                "/web/email",
                &[
                    ("csrf", &page.csrf_of_form()),
                    ("email", &alec.email),
                    ("intent", intent),
                ],
            )
            .await;
        assert_eq!(sent.location(), "/web/code");
        let code = cloud.mail.last_code(&alec.email).unwrap();
        let entry = browser.get("/web/code").await;
        let ended = browser
            .post_form(
                "/web/code",
                &[("csrf", &entry.csrf_of_form()), ("code", &code)],
            )
            .await;
        assert_eq!(ended.status, 403, "{intent}: {}", ended.text());
        let text = ended.text();
        assert!(
            text.contains("This account is banned") && text.contains("Aimbot."),
            "{intent}"
        );
        assert!(!text.contains("/web/appeal"));
        // No account came of it.
        assert!(
            anyplay_cloud::db::account_by_email(&cloud.state.db, &alec.email)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(browser.get("/web/details").await.status, 303);
    }
}

#[tokio::test]
async fn an_account_made_on_the_web_is_new_to_the_moderation_rules() {
    // The test clouds switch the rule off; here it is on, as in a real cloud.
    let cloud = TestCloud::with(|c| c.new_account_hours = 24, |_| {}).await;
    let browser = Browser::new(&cloud);
    let page = browser.get("/web/create").await;
    browser
        .post_form(
            "/web/email",
            &[
                ("csrf", &page.csrf_of_form()),
                ("email", "web@example.com"),
                ("intent", "create"),
            ],
        )
        .await;
    let code = cloud.mail.last_code("web@example.com").unwrap();
    let entry = browser.get("/web/code").await;
    browser
        .post_form(
            "/web/code",
            &[("csrf", &entry.csrf_of_form()), ("code", &code)],
        )
        .await;
    let details = browser.get("/web/details").await;
    let made = browser
        .post_form(
            "/web/details",
            &[
                ("csrf", &details.csrf_of_form()),
                ("handle", "Webby"),
                ("age", "yes"),
            ],
        )
        .await;
    assert_eq!(made.location(), "/web/account");
    let account = anyplay_cloud::db::account_by_email(&cloud.state.db, "web@example.com")
        .await
        .unwrap()
        .unwrap();
    // Like any account made today, it may not write to strangers yet.
    let refused = anyplay_cloud::moderation::may_write_to_strangers(&cloud.state, account.id())
        .await
        .unwrap_err();
    assert_eq!(refused.body.error, anyplay_api::ErrorCode::Restricted);
}

#[tokio::test]
async fn the_web_works_behind_a_proxy_that_terminates_tls() {
    let cloud = TestCloud::with(
        |c| {
            c.public_url = Some("https://anyplaynet.com".into());
            c.trust_proxy = true;
        },
        |_| {},
    )
    .await;
    let alec = cloud.sign_up("Alec").await;
    let proxied = [
        ("host", "anyplaynet.com"),
        ("x-forwarded-for", "203.0.113.9"),
        ("x-forwarded-proto", "https"),
        ("origin", "https://anyplaynet.com"),
    ];
    let browser = Browser::new(&cloud).with_headers(&proxied);
    let page = browser.get("/web/sign-in").await;
    assert_eq!(page.status, 200);
    let cookie = page.header("set-cookie").unwrap();
    assert!(cookie.starts_with("__Host-anyplay="), "{cookie}");
    assert!(cookie.contains("Secure") && cookie.contains("Path=/") && !cookie.contains("Domain"));
    assert!(page.header("strict-transport-security").is_some());

    // The forms post with the public origin, and the sign-in works.
    let ended = browser.sign_in_by_code(&alec.email).await;
    assert_eq!(ended.location(), "/web/account", "{}", ended.text());
    assert_eq!(browser.get("/web/account").await.status, 200);

    // The origin is the public one: a page of another origin cannot post, nor can one that
    // names this machine's own address.
    for origin in ["https://evil.example", "http://127.0.0.1:8080"] {
        let other = Browser::new(&cloud)
            .with_headers(&[("origin", origin), ("x-forwarded-for", "203.0.113.9")]);
        let page = other.get("/web/sign-in").await;
        let refused = other
            .post_form(
                "/web/email",
                &[("csrf", &page.csrf_of_form()), ("email", &alec.email)],
            )
            .await;
        assert_eq!(refused.status, 403, "{origin}");
    }
    let info: anyplay_api::Info = cloud
        .request(reqwest::Method::GET, "/v1/info", None, None)
        .await
        .ok(200)
        .json();
    assert_eq!(info.web.as_deref(), Some("https://anyplaynet.com"));
}
