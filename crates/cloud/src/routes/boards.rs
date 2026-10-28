//! Leaderboards: submitting a score, reading a board, the account's audience and the
//! moderator's calls (network `spec/cloud-api.md`, "Leaderboards", ADR 0041). The store and the
//! rules of who sees what are in [`crate::leaderboards`].
//!
//! The cloud offers them only when `leaderboards_enabled` is set; until then every route answers
//! `forbidden`.

use anyplay_api::{
    BOARD_PAGE_DEFAULT, BOARD_PAGE_MAX, BoardChange, BoardPage, BoardSettings, BoardView,
    EntryRemoval, ScorePrivacy, ScoreSaved, ScoreSubmit, TitleBoards, is_board_id, is_title_id,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::leaderboards::{self, ConfigRefusal, Refusal};
use crate::moderation::access::Staff;
use crate::state::AppState;
use crate::util::now;

/// The error of a feature that is switched off.
pub fn off() -> AppError {
    AppError::forbidden("This cloud has leaderboards off.")
}

fn enabled(state: &AppState) -> Result<()> {
    if state.config.leaderboards_enabled {
        Ok(())
    } else {
        Err(off())
    }
}

/// Checks that the title and the board can stand in a path.
pub fn check_ids(title: &str, board: &str) -> Result<()> {
    if !is_title_id(title) {
        return Err(AppError::bad_request("That is not a title ID."));
    }
    if !is_board_id(board) {
        return Err(AppError::bad_request(
            "A board ID has 1 to 64 letters, digits, underscores and hyphens.",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct PageQuery {
    start: Option<u32>,
    count: Option<u32>,
}

/// The `start` and `count` of a request: 25 entries when none is asked for, at most 100.
pub fn window(query: Result<Query<PageQuery>, QueryRejection>) -> Result<(u32, u32)> {
    let Query(query) =
        query.map_err(|_| AppError::bad_request("Give `start` and `count` as numbers."))?;
    let count = query.count.unwrap_or(BOARD_PAGE_DEFAULT);
    if count == 0 || count > BOARD_PAGE_MAX {
        return Err(AppError::bad_request(format!(
            "`count` is 1 to {BOARD_PAGE_MAX}."
        )));
    }
    Ok((query.start.unwrap_or(0), count))
}

/// A range of a board as `viewer` (0 for a visitor) sees it. `not_found` when there is no such
/// board.
pub async fn page(
    state: &AppState,
    viewer: u64,
    title: &str,
    board_id: &str,
    view: BoardView,
    start: u32,
    count: u32,
) -> Result<BoardPage> {
    let mut conn = state.db.acquire().await?;
    let board = leaderboards::board(&mut conn, title, board_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let range = leaderboards::read(
        &mut conn,
        &board,
        title,
        board_id,
        viewer,
        view,
        start,
        count,
        now(),
    )
    .await?;
    Ok(BoardPage {
        title_id: title.to_owned(),
        board: board_id.to_owned(),
        order: board.order,
        status: board.status,
        view,
        total: range.total,
        start: range.start,
        entries: range.entries,
        me: range.me,
    })
}

/// `PUT /v1/me/scores/{title}/{board}`
pub async fn submit(
    State(state): State<AppState>,
    auth: Auth,
    Path((title, board)): Path<(String, String)>,
    ApiJson(body): ApiJson<ScoreSubmit>,
) -> Result<Json<ScoreSaved>> {
    enabled(&state)?;
    check_ids(&title, &board)?;
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("scores:put:{}", auth.account),
            rates.score_submits,
            rates.score_window,
        )
        .map_err(AppError::rate_limited)?;
    let now = now();
    let saved = leaderboards::submit(
        &state.writer,
        &state.db,
        auth.account,
        &title,
        &board,
        body.score,
        now,
    )
    .await?
    .map_err(refused)?;
    // Where the account stands now: its line in the top view, which counts what it may see.
    let mut conn = state.db.acquire().await?;
    let kept = leaderboards::board(&mut conn, &title, &board)
        .await?
        .ok_or_else(AppError::not_found)?;
    let range = leaderboards::read(
        &mut conn,
        &kept,
        &title,
        &board,
        auth.account,
        BoardView::Top,
        0,
        0,
        now,
    )
    .await?;
    Ok(Json(ScoreSaved {
        accepted: saved.accepted,
        score: saved.score,
        achieved: saved.achieved,
        rank: range.me.map_or(0, |me| me.rank),
        total: range.total,
    }))
}

fn refused(refusal: Refusal) -> AppError {
    match refusal {
        Refusal::Closed => AppError::forbidden("This board takes no more scores."),
        Refusal::Removed => AppError::forbidden("A moderator removed your score on this board."),
        Refusal::OutOfRange => {
            AppError::bad_request("That score is outside what this board takes.")
        }
        Refusal::TitleFull => AppError::limit(format!(
            "A title has at most {} boards.",
            anyplay_api::BOARDS_PER_TITLE_MAX
        )),
        Refusal::MakeLimit => AppError::limit(format!(
            "An account may make at most {} boards.",
            anyplay_api::BOARDS_PER_ACCOUNT_MAKE
        )),
        Refusal::ScoreLimit => AppError::limit(format!(
            "An account keeps scores on at most {} boards.",
            anyplay_api::SCORES_PER_ACCOUNT_MAX
        )),
    }
}

async fn read_view(
    state: AppState,
    auth: Auth,
    title: String,
    board: String,
    view: BoardView,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<BoardPage>> {
    enabled(&state)?;
    check_ids(&title, &board)?;
    let (start, count) = window(query)?;
    Ok(Json(
        page(&state, auth.account, &title, &board, view, start, count).await?,
    ))
}

/// `GET /v1/boards/{title}/{board}`
pub async fn top(
    State(state): State<AppState>,
    auth: Auth,
    Path((title, board)): Path<(String, String)>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<BoardPage>> {
    read_view(state, auth, title, board, BoardView::Top, query).await
}

/// `GET /v1/boards/{title}/{board}/around`
pub async fn around(
    State(state): State<AppState>,
    auth: Auth,
    Path((title, board)): Path<(String, String)>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<BoardPage>> {
    read_view(state, auth, title, board, BoardView::Around, query).await
}

/// `GET /v1/boards/{title}/{board}/friends`
pub async fn friends(
    State(state): State<AppState>,
    auth: Auth,
    Path((title, board)): Path<(String, String)>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<BoardPage>> {
    read_view(state, auth, title, board, BoardView::Friends, query).await
}

/// The boards of a title the viewer (0 for a visitor) sees an entry of.
pub async fn boards(state: &AppState, viewer: u64, title: &str) -> Result<TitleBoards> {
    let mut conn = state.db.acquire().await?;
    Ok(TitleBoards {
        title_id: title.to_owned(),
        boards: leaderboards::boards_of(&mut conn, title, viewer, now()).await?,
    })
}

/// `GET /v1/titles/{title}/boards`
pub async fn of_title(
    State(state): State<AppState>,
    auth: Auth,
    Path(title): Path<String>,
) -> Result<Json<TitleBoards>> {
    enabled(&state)?;
    if !is_title_id(&title) {
        return Err(AppError::bad_request("That is not a title ID."));
    }
    Ok(Json(boards(&state, auth.account, &title).await?))
}

/// `GET /v1/me/score-privacy`
pub async fn get_privacy(State(state): State<AppState>, auth: Auth) -> Result<Json<ScorePrivacy>> {
    let mut conn = state.db.acquire().await?;
    Ok(Json(ScorePrivacy {
        scores: leaderboards::audience(&mut conn, auth.account).await?,
    }))
}

/// `PUT /v1/me/score-privacy`. Works while the feature is off, so that a player's choice is made
/// before the cloud shows anything.
pub async fn set_privacy(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(body): ApiJson<ScorePrivacy>,
) -> Result<Json<ScorePrivacy>> {
    let mut conn = state.db.acquire().await?;
    leaderboards::set_audience(&mut conn, auth.account, body.scores).await?;
    Ok(Json(body))
}

// Moderators

/// `PUT /v1/moderation/boards/{title}/{board}`
pub async fn configure(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path((title, board)): Path<(String, String)>,
    ApiJson(change): ApiJson<BoardChange>,
) -> Result<Json<BoardSettings>> {
    enabled(&state)?;
    check_ids(&title, &board)?;
    let settings = leaderboards::configure(
        &state.writer,
        &state.db,
        &title,
        &board,
        &change,
        now(),
    )
    .await?
    .map_err(|refusal| match refusal {
        ConfigRefusal::Bounds => AppError::bad_request(
            "The lowest score must not be above the highest, and both stay within ±(2^53 - 1).",
        ),
        ConfigRefusal::TitleFull => refused(Refusal::TitleFull),
    })?;
    tracing::info!(
        target: "anyplay::leaderboards",
        by = actor.handle(), title = %title, board = %board,
        order = ?settings.order, status = ?settings.status,
        min = settings.min_score, max = settings.max_score,
        "a moderator set a board"
    );
    Ok(Json(settings))
}

fn reason(removal: &EntryRemoval) -> Result<&str> {
    let text = removal.reason.trim();
    if text.is_empty() || text.chars().count() > 500 || text.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "Give a reason of 1 to 500 characters, without control characters.",
        ));
    }
    Ok(text)
}

/// `DELETE /v1/moderation/boards/{title}/{board}/entries/{account}`
pub async fn remove(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path((title, board, account)): Path<(String, String, String)>,
    ApiJson(removal): ApiJson<EntryRemoval>,
) -> Result<StatusCode> {
    enabled(&state)?;
    check_ids(&title, &board)?;
    let account = parse_id(&account)?;
    let reason = reason(&removal)?;
    let mut conn = state.db.acquire().await?;
    if !leaderboards::remove(
        &mut conn,
        &title,
        &board,
        account,
        actor.handle(),
        reason,
        now(),
    )
    .await?
    {
        return Err(AppError::not_found());
    }
    tracing::info!(
        target: "anyplay::leaderboards",
        by = actor.handle(), title = %title, board = %board, account,
        "a moderator removed an entry"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/moderation/boards/{title}/{board}/entries/{account}/restore`
pub async fn restore(
    State(state): State<AppState>,
    Staff(actor): Staff,
    Path((title, board, account)): Path<(String, String, String)>,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> Result<StatusCode> {
    enabled(&state)?;
    check_ids(&title, &board)?;
    let account = parse_id(&account)?;
    let mut conn = state.db.acquire().await?;
    if !leaderboards::restore(&mut conn, &title, &board, account).await? {
        return Err(AppError::not_found());
    }
    tracing::info!(
        target: "anyplay::leaderboards",
        by = actor.handle(), title = %title, board = %board, account,
        "a moderator restored an entry"
    );
    Ok(StatusCode::NO_CONTENT)
}
