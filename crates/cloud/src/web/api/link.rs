//! The link the app's account button opens (`POST /v1/auth/browser/link`), as JSON for the
//! account web. The page asks first (`GET`, which changes nothing) and the button uses the code
//! up (`POST`), so a program that fetches the address cannot sign anybody in.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde_json::json;

use super::flow::session_view;
use super::{ApiResult, Visitor};
use crate::db;
use crate::error::AppError;
use crate::http::{ApiJson, ClientIp};
use crate::state::AppState;
use crate::web::native::{LINK, LinkGrant};
use crate::web::{Visit, limit_ip_api, sign_in_session, store, with_cookie};

/// `GET /link/{code}`: whose account the link opens. The code stays valid.
pub async fn peek(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(code): Path<String>,
) -> ApiResult {
    limit_ip_api(&state, "link", ip, state.tuning.rates.code_per_ip)?;
    let data = store::flow_peek(&state.db, LINK, &code)
        .await?
        .ok_or_else(AppError::not_found)?;
    let grant: LinkGrant = serde_json::from_str(&data).map_err(|_| AppError::not_found())?;
    let account = db::account(&state.db, grant.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(json!({"handle": account.handle})).into_response())
}

/// `POST /link/{code}`: uses the code up and signs this browser in as the account. The answer
/// is the session view of the new session. A banned account is not signed in: the session
/// learns of the ban, and the page goes on to the statement (`GET /ban`).
pub async fn open(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    visitor: Visitor,
    Path(code): Path<String>,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    limit_ip_api(&state, "link", ip, state.tuning.rates.code_per_ip)?;
    let mut session = visitor.session;
    let data = store::flow_take(&state.db, LINK, &code)
        .await?
        .ok_or_else(AppError::not_found)?;
    let grant: LinkGrant = serde_json::from_str(&data).map_err(|_| AppError::not_found())?;
    // The device that asked must still be signed in, and the account not banned.
    if db::device_owner(&state.db, grant.device).await? != Some(grant.account) {
        return Err(AppError::not_found().into());
    }
    if let Some(until) = state.ban_of(grant.account) {
        session.data.banned = Some(grant.account);
        store::save_data(&state.db, &session).await?;
        return Err(
            crate::moderation::banned_error(&state, grant.account, until)
                .await
                .into(),
        );
    }
    store::end_session(&state.db, &session).await?;
    let mut visit: Visit =
        sign_in_session(&state, grant.account, store::SessionData::default()).await?;
    let view = session_view(&state, &mut visit.session).await?;
    Ok(with_cookie(&state, &visit, Json(view).into_response()))
}
