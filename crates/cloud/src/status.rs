//! The public status: `GET /v1/status` reports each part of the service as up, down or unknown,
//! and nothing else (no versions, hosts, counts or error texts).
//!
//! The answer is built at most once every [`TTL`], however many people ask, so a status page that
//! many players reload costs the cloud one database ping and a few probes every ten seconds.

use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::state::AppState;
use crate::util::now_ms;

/// How long an answer is reused.
const TTL: Duration = Duration::from_secs(10);
/// How long a probe of a relay or the website may take.
const PROBE: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Component {
    /// `api`, `database`, `signalling`, `relay` or `web`.
    pub id: &'static str,
    /// `up`, `down`, or `unknown` when this cloud does not watch the part.
    pub status: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Report {
    /// `operational`, `degraded` (a part other than the database is down) or `outage` (the
    /// database is down).
    pub status: &'static str,
    /// When the parts were checked, Unix milliseconds.
    pub checked: i64,
    pub components: Vec<Component>,
}

impl Report {
    /// The overall word for these parts.
    pub fn overall(components: &[Component]) -> &'static str {
        let down = |id: &str| components.iter().any(|c| c.id == id && c.status == "down");
        if down("database") {
            "outage"
        } else if components.iter().any(|c| c.status == "down") {
            "degraded"
        } else {
            "operational"
        }
    }
}

/// The last answer and when it was made.
#[derive(Default)]
pub struct StatusCache {
    last: Mutex<Option<(Instant, Report)>>,
}

impl StatusCache {
    /// The cached report, or a new one. Callers that arrive while it is being made wait for it
    /// instead of probing again.
    pub async fn get(&self, state: &AppState) -> Report {
        let mut last = self.last.lock().await;
        if let Some((at, report)) = last.as_ref()
            && at.elapsed() < TTL
        {
            return report.clone();
        }
        let report = check(state).await;
        *last = Some((Instant::now(), report.clone()));
        report
    }
}

fn word(up: bool) -> &'static str {
    if up { "up" } else { "down" }
}

/// Any answer below 500 counts: the part is there and talking.
async fn reaches(client: &reqwest::Client, url: &str) -> bool {
    matches!(
        tokio::time::timeout(PROBE, client.get(url).send()).await,
        Ok(Ok(answer)) if !answer.status().is_server_error()
    )
}

async fn check(state: &AppState) -> Report {
    let store = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await;
    let database = matches!(store, Ok(Ok(_)));
    let signalling = !state.draining.load(std::sync::atomic::Ordering::Relaxed)
        && state.news.nats_connected() != Some(false);

    let client = reqwest::Client::builder()
        .timeout(PROBE)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok();
    let mut relay = "unknown";
    let mut web = "unknown";
    if let Some(client) = &client {
        if !state.config.relays.is_empty() {
            let probes = state.config.relays.iter().map(|url| reaches(client, url));
            // The relay part is up when at least one relay answers; a second region that is
            // down while the first is up is a degraded service, not a lost one.
            relay = word(futures_util::future::join_all(probes).await.contains(&true));
        }
        if let Some(url) = &state.config.status_web_url {
            web = word(reaches(client, url).await);
        }
    }
    let components = vec![
        Component {
            id: "api",
            status: "up",
        },
        Component {
            id: "database",
            status: word(database),
        },
        Component {
            id: "signalling",
            status: word(signalling),
        },
        Component {
            id: "relay",
            status: relay,
        },
        Component {
            id: "web",
            status: web,
        },
    ];
    Report {
        status: Report::overall(&components),
        checked: now_ms(),
        components,
    }
}

/// `GET /v1/status`. Public, cacheable for a few seconds by anyone in between.
pub async fn status(State(state): State<AppState>) -> Response {
    let report = state.status.get(&state).await;
    let mut response = Json(report).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=10"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(database: &'static str, relay: &'static str) -> Vec<Component> {
        vec![
            Component {
                id: "database",
                status: database,
            },
            Component {
                id: "relay",
                status: relay,
            },
            Component {
                id: "web",
                status: "unknown",
            },
        ]
    }

    #[test]
    fn the_overall_word_follows_the_worst_part() {
        assert_eq!(Report::overall(&parts("up", "up")), "operational");
        assert_eq!(Report::overall(&parts("up", "unknown")), "operational");
        assert_eq!(Report::overall(&parts("up", "down")), "degraded");
        assert_eq!(Report::overall(&parts("down", "up")), "outage");
    }
}
