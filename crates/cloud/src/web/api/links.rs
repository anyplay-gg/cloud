//! Connections: the player's linked Discord and Steam accounts. The list, starting a link,
//! unlinking and the visibility of each, and the page route the provider sends the browser back
//! to (network `spec/portal-api.md`, "Connections").
//!
//! Linking needs a fresh session, at the start and again at the return; unlinking does not. The
//! state of a start lives in the browser session and is used once, so the address the provider
//! sends the browser to works for the browser that started it, once.

use anyplay_api::Audience;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::{ApiFail, ApiResult, Member, done};
use crate::error::AppError;
use crate::http::ApiJson;
use crate::links::{self, Identity, LinkError, Provider, Unconfirmed};
use crate::state::AppState;
use crate::util::{ct_eq, now, now_ms};
use crate::web::store::{self, PendingLink};
use crate::web::{WebError, existing, redirect_plain};

fn provider_of(name: &str) -> Result<Provider, ApiFail> {
    Provider::parse(name).ok_or_else(|| AppError::not_found().into())
}

/// `GET /me/links`
pub async fn list(State(state): State<AppState>, member: Member) -> ApiResult {
    let links = links::views(&state, member.id()).await?;
    Ok(Json(json!({ "links": links })).into_response())
}

/// `POST /me/links/{provider}/start`: the address to send the browser to.
pub async fn start(
    State(state): State<AppState>,
    mut member: Member,
    Path(name): Path<String>,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    let provider = provider_of(&name)?;
    let id = member.id();
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("links:start:{id}"),
            rates.link_starts,
            rates.link_start_window,
        )
        .map_err(AppError::rate_limited)?;
    member.require_fresh(&state)?;
    if !links::available(&state, provider) {
        return Err(ApiFail::new(
            StatusCode::CONFLICT,
            "provider_unavailable",
            "This cloud cannot link accounts of this service.",
        ));
    }
    if links::row(&state.db, id, provider).await?.is_some() {
        return Err(ApiFail::new(
            StatusCode::CONFLICT,
            "already_linked",
            "This account is linked already. Unlink it first to link another one.",
        ));
    }
    let (url, pending) = links::begin(&state, provider).ok_or_else(AppError::internal)?;
    member.session.data.link = Some(pending);
    store::save_data(&state.db, &member.session).await?;
    Ok(Json(json!({ "url": url })).into_response())
}

#[derive(Deserialize)]
pub struct VisibilityBody {
    visibility: Audience,
}

/// `PATCH /me/links/{provider}`: who sees the connection.
pub async fn set_visibility(
    State(state): State<AppState>,
    member: Member,
    Path(name): Path<String>,
    ApiJson(body): ApiJson<VisibilityBody>,
) -> ApiResult {
    let provider = provider_of(&name)?;
    let id = member.id();
    if !links::set_audience(&state.db, id, provider, body.visibility).await? {
        return Err(AppError::not_found().into());
    }
    let row = links::row(&state.db, id, provider).await?;
    Ok(Json(links::view(&state, provider, row.as_ref())).into_response())
}

/// `DELETE /me/links/{provider}`
pub async fn unlink(
    State(state): State<AppState>,
    member: Member,
    Path(name): Path<String>,
) -> ApiResult {
    let provider = provider_of(&name)?;
    links::delete(&state.db, member.id(), provider).await?;
    done()
}

// The return of the browser

/// The portal page the browser goes back to, with what came of the link.
fn back(provider: Provider, result: &str) -> Response {
    redirect_plain(&format!(
        "/account/connections?link={}&result={result}",
        provider.name()
    ))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The answer to the hop from the provider's site. The session cookie is `SameSite=Strict`, so a
/// browser leaves it out of a request that a cross-site page started, and the session cannot be
/// found. A page of ours that forwards to the same address makes the next request same-site.
/// Nothing is done here, and the query goes nowhere but into the address of our own page.
fn forward(provider: Provider, query: &str) -> Response {
    let target = format!("/web/links/{}/callback?{query}", provider.name());
    let target = escape(&target);
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\">\
         <meta http-equiv=\"refresh\" content=\"0;url={target}\">\
         <title>Finishing the connection</title></head>\
         <body><p>Finishing the connection. <a href=\"{target}\">Continue</a></p></body></html>"
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

/// `GET /web/links/{provider}/callback`: the provider sends the browser here. The answer is
/// always a redirect to the portal with a flag (see the spec), never an error page.
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let Some(provider) = Provider::parse(&name) else {
        return WebError::page(StatusCode::NOT_FOUND, "Not found", "There is nothing here.")
            .into_response();
    };
    let query = query.unwrap_or_default();
    let cross_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| site == "cross-site");
    if cross_site && query.len() <= MAX_QUERY {
        return forward(provider, &query);
    }
    let flag = match finish(&state, &headers, provider, &query).await {
        Ok(flag) => flag,
        Err(Early::SignedOut) => return redirect_plain("/sign-in"),
        Err(Early::Failed) => "failed",
    };
    back(provider, flag)
}

const MAX_QUERY: usize = 4096;

/// Where `finish` ends before it has a flag.
enum Early {
    /// There is no signed-in session: go to sign-in instead of the portal.
    SignedOut,
    /// The cloud failed; the player sees `failed`.
    Failed,
}

impl From<sqlx::Error> for Early {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "connection: the store failed");
        Early::Failed
    }
}

/// Finishes a link: the flag of the result.
async fn finish(
    state: &AppState,
    headers: &HeaderMap,
    provider: Provider,
    query: &str,
) -> Result<&'static str, Early> {
    let signed_out = || Early::SignedOut;
    let mut session = existing(state, headers)
        .await
        .map_err(|_| Early::Failed)?
        .ok_or_else(signed_out)?;
    let account = session.account.ok_or_else(signed_out)?;
    if state.ban_of(account).is_some() {
        return Ok("failed");
    }
    // The start is used up whatever comes of the return, so an address used twice finds nothing.
    let pending = session.data.link.take();
    if pending.is_some() {
        store::save_data(&state.db, &session).await?;
    }
    let params: Vec<(String, String)> = if query.len() <= MAX_QUERY {
        serde_urlencoded::from_str(query).unwrap_or_default()
    } else {
        Vec::new()
    };
    let param = |name: &str| {
        params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let Some(pending) = pending.filter(|p| matches_start(state, p, provider, param("state")))
    else {
        return Ok("expired");
    };
    let denied = match provider {
        Provider::Discord => param("error").is_some(),
        Provider::Steam => param("openid.mode") == Some("cancel"),
    };
    if denied {
        return Ok("denied");
    }
    if !session.fresh(state.tuning.fresh_for) {
        return Ok("fresh");
    }
    if links::row(&state.db, account, provider).await?.is_some() {
        return Ok("exists");
    }
    let identity = identity_of(state, provider, &pending, &params).await;
    let Ok(identity) = identity else {
        return Ok("failed");
    };
    Ok(
        match links::insert(&state.db, account, provider, &identity, now()).await {
            Ok(()) => "linked",
            Err(LinkError::Exists) => "exists",
            Err(LinkError::Taken) => "taken",
        },
    )
}

/// True when the return belongs to this start: the provider, the age and the state.
fn matches_start(
    state: &AppState,
    pending: &PendingLink,
    provider: Provider,
    sent: Option<&str>,
) -> bool {
    let age_ms = now_ms().saturating_sub(pending.started_ms);
    pending.provider == provider.name()
        && (0..=state.tuning.link_pending_ttl.as_millis() as i64).contains(&age_ms)
        && sent.is_some_and(|sent| ct_eq(sent.as_bytes(), pending.state.as_bytes()))
}

async fn identity_of(
    state: &AppState,
    provider: Provider,
    pending: &PendingLink,
    params: &[(String, String)],
) -> Result<Identity, Unconfirmed> {
    match provider {
        Provider::Discord => {
            let code = params
                .iter()
                .find(|(n, _)| n == "code")
                .map(|(_, v)| v.as_str())
                .ok_or(Unconfirmed)?;
            let verifier = pending.verifier.as_deref().ok_or(Unconfirmed)?;
            links::discord_identity(state, code, verifier).await
        }
        Provider::Steam => {
            let origin = &state.web.as_ref().ok_or(Unconfirmed)?.origin;
            let return_to =
                links::steam_return_to(&links::callback_url(origin, provider), &pending.state);
            links::steam_identity(state, params, &return_to).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_forwarding_page_escapes_the_query() {
        let response = forward(Provider::Steam, "a=\"><script>x</script>&b='");
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let page = String::from_utf8(body.to_vec()).unwrap();
        assert!(!page.contains("<script>"), "{page}");
        assert!(
            page.contains("&quot;&gt;&lt;script&gt;x&lt;/script&gt;&amp;b=&#39;"),
            "{page}"
        );
    }
}
