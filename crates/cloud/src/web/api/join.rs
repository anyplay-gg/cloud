//! The page of a join code as JSON (`GET /join/{code}`), for the account web. It needs no
//! session, changes nothing and shows only what the join page shows.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;

use super::{ApiResult, limit_public};
use crate::http::ClientIp;
use crate::routes::join_codes;
use crate::state::AppState;

/// `GET /join/{code}`: who invites, to what, and how many are there. Counted per client address,
/// like the public profiles.
pub async fn show(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(code): Path<String>,
) -> ApiResult {
    limit_public(&state, ip)?;
    Ok(Json(join_codes::view(&state, &code).await?).into_response())
}
