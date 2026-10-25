//! Tokens for Discord's Social SDK (network `spec/cloud-api.md`, "Discord Social SDK").
//!
//! The SDK in a player's net service authorises the player and gets a one-time code. The cloud is
//! the confidential client: it trades the code (with the player's PKCE verifier) for tokens using
//! the application's secret, and hands them straight back. It keeps nothing: no token, no
//! Discord ID, no log line with either. Refreshing works the same way.
//!
//! When the account already has a Discord connection, the identity behind a fresh token must be
//! the connected one; otherwise the tokens are revoked and the request refused.

use std::sync::OnceLock;
use std::time::Duration;

use anyplay_api::{DiscordCodeExchange, DiscordRefresh, DiscordTokens};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::links::{self, Provider};
use crate::state::AppState;

/// The redirect address the SDK reports on a desktop; registered with the Discord application.
pub const SDK_REDIRECT: &str = "http://127.0.0.1/callback";

/// The most an answer of Discord may weigh.
const MAX_ANSWER: usize = 16 * 1024;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("anyplay-cloud/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("the HTTP client builds")
    })
}

#[derive(Deserialize)]
struct Grant {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct User {
    id: String,
}

fn is_pkce_verifier(text: &str) -> bool {
    (43..=128).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

fn is_token_like(text: &str, longest: usize) -> bool {
    !text.is_empty() && text.len() <= longest && text.bytes().all(|b| b.is_ascii_graphic())
}

fn post_form(url: String, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
    client()
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(serde_urlencoded::to_string(fields).unwrap_or_default())
}

fn unavailable() -> AppError {
    tracing::warn!("Discord SDK tokens: Discord could not be reached or answered badly");
    AppError::new(
        anyplay_api::ErrorCode::Internal,
        "Discord could not be reached. Try again later.",
    )
}

fn refused() -> AppError {
    AppError::new(
        anyplay_api::ErrorCode::GrantRefused,
        "Discord refused the code or the refresh token. Connect again.",
    )
}

/// Reads a body of Discord's, if it fits.
async fn body(mut response: reqwest::Response) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_ANSWER as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_ANSWER {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

/// Posts a form to Discord's token endpoint and reads the grant.
async fn token_request(state: &AppState, fields: &[(&str, &str)]) -> Result<Grant> {
    let (client_id, secret) = state.config.discord().ok_or_else(AppError::not_found)?;
    let mut form = vec![("client_id", client_id), ("client_secret", secret)];
    form.extend_from_slice(fields);
    let api = &state.tuning.links.discord_api;
    let response = post_form(format!("{api}/oauth2/token"), &form)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(error = %error.without_url(), "Discord SDK tokens: request failed");
            unavailable()
        })?;
    let status = response.status();
    if status.as_u16() == 429 {
        let wait = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(30, |s| s.ceil() as u64);
        return Err(AppError::rate_limited(wait));
    }
    if status.as_u16() == 401 {
        // The application's own credentials are wrong: the cloud is misconfigured, not the player.
        tracing::error!("Discord SDK tokens: Discord refused the cloud's client credentials");
        return Err(AppError::internal());
    }
    if status.is_client_error() {
        tracing::info!(%status, "Discord SDK tokens: grant refused");
        return Err(refused());
    }
    if !status.is_success() {
        tracing::warn!(%status, "Discord SDK tokens: Discord failed");
        return Err(unavailable());
    }
    let bytes = body(response).await.ok_or_else(unavailable)?;
    let grant: Grant = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if grant.access_token.is_empty() || grant.refresh_token.is_empty() {
        return Err(unavailable());
    }
    Ok(grant)
}

fn tokens(grant: Grant) -> DiscordTokens {
    DiscordTokens {
        access: grant.access_token,
        refresh: grant.refresh_token,
        expires_in: grant.expires_in,
        scope: grant.scope,
    }
}

/// Lets Discord end a token. Best effort.
async fn revoke(state: &AppState, token: &str) {
    let Some((client_id, secret)) = state.config.discord() else {
        return;
    };
    let api = &state.tuning.links.discord_api;
    let _ = post_form(
        format!("{api}/oauth2/token/revoke"),
        &[
            ("token", token),
            ("client_id", client_id),
            ("client_secret", secret),
        ],
    )
    .send()
    .await;
}

/// Trades the SDK's code for tokens. `account` is who asked.
pub async fn exchange(
    state: &AppState,
    account: u64,
    request: &DiscordCodeExchange,
) -> Result<DiscordTokens> {
    let allowed = redirects(state);
    if !allowed.contains(&request.redirect_uri) {
        return Err(AppError::bad_request(
            "The redirect address is not one the Discord application registered.",
        ));
    }
    if !is_token_like(&request.code, 512) {
        return Err(AppError::bad_request("The code is not valid."));
    }
    if !is_pkce_verifier(&request.code_verifier) {
        return Err(AppError::bad_request(
            "The code verifier must be 43 to 128 URL-safe characters.",
        ));
    }
    let grant = token_request(
        state,
        &[
            ("grant_type", "authorization_code"),
            ("code", &request.code),
            ("redirect_uri", &request.redirect_uri),
            ("code_verifier", &request.code_verifier),
        ],
    )
    .await?;
    if let Err(error) = check_identity(state, account, &grant.access_token).await {
        revoke(state, &grant.access_token).await;
        revoke(state, &grant.refresh_token).await;
        return Err(error);
    }
    Ok(tokens(grant))
}

/// Trades a refresh token for new tokens.
pub async fn refresh(state: &AppState, request: &DiscordRefresh) -> Result<DiscordTokens> {
    if !is_token_like(&request.refresh, 512) {
        return Err(AppError::bad_request("The refresh token is not valid."));
    }
    let grant = token_request(
        state,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &request.refresh),
        ],
    )
    .await?;
    Ok(tokens(grant))
}

/// The redirect addresses a code may have been issued for: the SDK's, and the portal's.
fn redirects(state: &AppState) -> Vec<String> {
    let mut all = vec![SDK_REDIRECT.to_owned()];
    if let Some(web) = &state.web {
        all.push(links::callback_url(&web.origin, Provider::Discord));
    }
    all
}

/// The Discord account behind `access` must be the one linked to `account`, when there is one.
/// An account without a connection may connect any Discord account: the net service holds the
/// session and the cloud stores nothing.
async fn check_identity(state: &AppState, account: u64, access: &str) -> Result<()> {
    let Some(linked) = links::row(&state.db, account, Provider::Discord).await? else {
        return Ok(());
    };
    let api = &state.tuning.links.discord_api;
    let response = client()
        .get(format!("{api}/users/@me"))
        .bearer_auth(access)
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(not_the_linked_account());
    }
    if !status.is_success() {
        return Err(unavailable());
    }
    let bytes = body(response).await.ok_or_else(unavailable)?;
    let user: User = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if user.id == linked.external_id {
        Ok(())
    } else {
        Err(not_the_linked_account())
    }
}

fn not_the_linked_account() -> AppError {
    AppError::forbidden("This is not the Discord account linked to your AnyPlay account.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verifier_is_43_to_128_url_safe_characters() {
        assert!(is_pkce_verifier(&"a".repeat(43)));
        assert!(is_pkce_verifier(&"aB3-._~".repeat(18)));
        assert!(!is_pkce_verifier(&"a".repeat(42)));
        assert!(!is_pkce_verifier(&"a".repeat(129)));
        assert!(!is_pkce_verifier(&format!("{}!", "a".repeat(50))));
    }

    #[test]
    fn a_code_is_printable_ascii_and_short() {
        assert!(is_token_like("abc.DEF-123", 512));
        assert!(!is_token_like("", 512));
        assert!(!is_token_like("has space", 512));
        assert!(!is_token_like(&"a".repeat(513), 512));
    }
}
