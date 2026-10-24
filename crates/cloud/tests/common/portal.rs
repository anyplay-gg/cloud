//! A browser on the web portal, for the tests of its JSON API: a cookie jar, the CSRF token of its
//! session, and `fetch`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use reqwest::Method;
use serde_json::{Value, json};

use super::{TestCloud, User};

pub const API: &str = "/web/api/v1";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// One answer of the portal API.
pub struct Answer {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub bytes: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    /// Asserts the status and returns the JSON body.
    pub fn ok(&self, status: u16) -> Value {
        assert_eq!(self.status, status, "body: {}", self.text());
        self.json()
    }

    /// Asserts an error answer with this status and code.
    pub fn error(&self, status: u16, code: &str) -> Value {
        assert_eq!(self.status, status, "body: {}", self.text());
        let body = self.json();
        assert_eq!(body["error"], code, "body: {body}");
        assert!(body["message"].is_string(), "a message: {body}");
        body
    }
}

/// A browser on the portal: a cookie jar, the CSRF token of its session, and `fetch`.
pub struct Portal<'a> {
    pub cloud: &'a TestCloud,
    pub client: reqwest::Client,
    pub cookies: Mutex<HashMap<String, String>>,
    pub csrf: Mutex<Option<String>>,
}

impl<'a> Portal<'a> {
    pub fn new(cloud: &'a TestCloud) -> Self {
        Self {
            cloud,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cookies: Mutex::new(HashMap::new()),
            csrf: Mutex::new(None),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.cloud.base)
    }

    pub fn token(&self) -> String {
        self.csrf.lock().unwrap().clone().unwrap_or_default()
    }

    /// Sends `request` with the cookies, and keeps the cookies it sets.
    pub async fn dispatch(&self, request: reqwest::RequestBuilder) -> Answer {
        let cookie = self
            .cookies
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let request = if cookie.is_empty() {
            request
        } else {
            request.header("cookie", cookie)
        };
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        for value in headers.get_all(reqwest::header::SET_COOKIE) {
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
        let answer = Answer {
            status,
            headers,
            bytes,
        };
        // Like the page: whatever view comes back carries the token to use from now on.
        if let Some(csrf) = answer.json().get("csrf").and_then(Value::as_str) {
            *self.csrf.lock().unwrap() = Some(csrf.to_owned());
        }
        answer
    }

    /// A read.
    pub async fn get(&self, path: &str) -> Answer {
        self.dispatch(self.client.get(self.url(&format!("{API}{path}"))))
            .await
    }

    /// A write the way the page sends it: JSON, with the CSRF token.
    pub async fn send(&self, method: Method, path: &str, body: Value) -> Answer {
        self.dispatch(
            self.client
                .request(method, self.url(&format!("{API}{path}")))
                .header("x-csrf-token", self.token())
                .json(&body),
        )
        .await
    }

    pub async fn post(&self, path: &str, body: Value) -> Answer {
        self.send(Method::POST, path, body).await
    }

    pub async fn delete(&self, path: &str) -> Answer {
        self.send(Method::DELETE, path, json!({})).await
    }

    /// Reads the session, which starts one when the browser has none.
    pub async fn session(&self) -> Value {
        self.get("/session").await.ok(200)
    }

    /// Signs in by e-mail code, or creates the account when the address is new. Returns the
    /// answer to the code.
    pub async fn code_sign_in(&self, email: &str) -> Value {
        self.session().await;
        let sent = self
            .post("/sign-in/email", json!({"email": email}))
            .await
            .ok(200);
        assert_eq!(sent["flow"]["step"], "code");
        let code = self.cloud.mail.last_code(email).expect("a mail was sent");
        self.post("/sign-in/code", json!({"code": code}))
            .await
            .ok(200)
    }

    /// A browser signed in as an account that exists.
    pub async fn signed_in(cloud: &'a TestCloud, user: &User) -> Self {
        let portal = Self::new(cloud);
        let view = portal.code_sign_in(&user.email).await;
        assert_eq!(view["flow"]["step"], "done", "{view}");
        assert_eq!(view["signed_in"], true);
        portal
    }

    /// Makes the session old enough that sensitive actions need a step-up.
    pub async fn age(&self, cloud: &TestCloud) {
        sqlx::query("UPDATE web_sessions SET created_ms = created_ms - 3600000, data = '{}' WHERE account_id IS NOT NULL")
            .execute(&cloud.state.db)
            .await
            .unwrap();
    }
}
