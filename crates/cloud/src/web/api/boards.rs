//! What the portal shows of leaderboards: a board page and the boards of a title, read-only
//! (network `spec/portal-api.md`, "Leaderboards").
//!
//! The rules are those of `/v1` ([`crate::routes::boards`], [`crate::leaderboards`]): a visitor
//! who is not signed in sees only the entries whose owners show their scores to everyone, a
//! signed-in browser also its friends' and its own. With the feature off every read answers
//! `not_found`. Entries name a player by handle only.

use anyplay_api::{
    BOARD_PAGE_DEFAULT, BOARD_PAGE_MAX, BoardEntry, BoardOrder, BoardStatus, BoardSummary,
    BoardView, is_board_id, is_title_id,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use super::{ApiResult, limit_public, social};
use crate::error::AppError;
use crate::http::ClientIp;
use crate::routes::boards;
use crate::state::AppState;

/// A line of a board, in the portal's spelling.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Line {
    rank: u32,
    handle: String,
    score: i64,
    achieved: u64,
}

impl From<BoardEntry> for Line {
    fn from(entry: BoardEntry) -> Self {
        Self {
            rank: entry.rank,
            handle: entry.account.handle,
            score: entry.score,
            achieved: entry.achieved,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    title_id: String,
    board: String,
    order: BoardOrder,
    status: BoardStatus,
    view: BoardView,
    total: u32,
    start: u32,
    entries: Vec<Line>,
    #[serde(skip_serializing_if = "Option::is_none")]
    me: Option<Line>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Boards {
    title_id: String,
    boards: Vec<BoardSummary>,
}

#[derive(Deserialize)]
pub struct ViewQuery {
    view: Option<String>,
    start: Option<u32>,
    count: Option<u32>,
}

/// `GET /boards/{title}/{board}?view=&start=&count=`
pub async fn board(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path((title, board)): Path<(String, String)>,
    query: Result<Query<ViewQuery>, QueryRejection>,
) -> ApiResult {
    limit_public(&state, ip)?;
    if !state.config.leaderboards_enabled || !is_title_id(&title) || !is_board_id(&board) {
        return Err(AppError::not_found().into());
    }
    let Query(query) =
        query.map_err(|_| AppError::bad_request("Give `view`, `start` and `count` as text."))?;
    let view = match query.view.as_deref() {
        None | Some("top") => BoardView::Top,
        Some("around") => BoardView::Around,
        Some("friends") => BoardView::Friends,
        Some(_) => {
            return Err(AppError::bad_request("`view` is top, around or friends.").into());
        }
    };
    let count = query.count.unwrap_or(BOARD_PAGE_DEFAULT);
    if count == 0 || count > BOARD_PAGE_MAX {
        return Err(AppError::bad_request(format!("`count` is 1 to {BOARD_PAGE_MAX}.")).into());
    }
    let viewer = social::viewer(&state, &headers).await;
    if view != BoardView::Top && viewer.is_none() {
        return Err(AppError::unauthorized("Sign in to see this.").into());
    }
    let page = boards::page(
        &state,
        viewer.unwrap_or(0),
        &title,
        &board,
        view,
        query.start.unwrap_or(0),
        count,
    )
    .await?;
    Ok(Json(Page {
        title_id: page.title_id,
        board: page.board,
        order: page.order,
        status: page.status,
        view: page.view,
        total: page.total,
        start: page.start,
        entries: page.entries.into_iter().map(Line::from).collect(),
        me: page.me.map(Line::from),
    })
    .into_response())
}

/// `GET /titles/{title}/boards`
pub async fn of_title(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(title): Path<String>,
) -> ApiResult {
    limit_public(&state, ip)?;
    if !state.config.leaderboards_enabled || !is_title_id(&title) {
        return Err(AppError::not_found().into());
    }
    let viewer = social::viewer(&state, &headers).await;
    let found = boards::boards(&state, viewer.unwrap_or(0), &title).await?;
    Ok(Json(Boards {
        title_id: found.title_id,
        boards: found.boards,
    })
    .into_response())
}
