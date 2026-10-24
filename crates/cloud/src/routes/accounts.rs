//! The signed-in account and lookups of other accounts.

use anyplay_api::{Account, AccountId, Me};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{AppError, Result};
use crate::http::{Auth, parse_id};
use crate::routes::auth::me_of;
use crate::state::AppState;
use crate::util::now;
use crate::{db, live};

pub async fn me(State(state): State<AppState>, auth: Auth) -> Result<Json<Me>> {
    let account = db::account(&state.db, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(me_of(&account)))
}

pub async fn delete_me(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    state.delete_account(auth.account).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Everything the cloud stores about the account.
pub async fn export(State(state): State<AppState>, auth: Auth) -> Result<Json<Value>> {
    Ok(Json(export_json(&state, auth.account).await?))
}

/// The export of an account as JSON; the API and the account web hand it out.
pub async fn export_json(state: &AppState, account_id: u64) -> Result<Value> {
    let account = db::account(&state.db, account_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let devices = db::devices_of(&state.db, account_id).await?;
    let related = |rows: Vec<db::Related>| -> Vec<Value> {
        rows.into_iter()
            .map(|r| json!({"account": {"id": r.id(), "handle": r.handle}, "since": r.since()}))
            .collect()
    };
    let friends = db::friends_of(&state.db, account_id).await?;
    let incoming = db::incoming_requests(&state.db, account_id).await?;
    let outgoing = db::outgoing_requests(&state.db, account_id).await?;
    let blocked = db::blocked_by(&state.db, account_id).await?;
    let reports =
        crate::moderation::store::reports_by(&mut *state.db.acquire().await?, account_id).await?;
    let actions = crate::moderation::statements(state, account_id).await?;
    let ban = db::ban_of(&state.db, account_id).await?;
    let trophies = db::trophies_of(&state.db, account_id).await?;
    let privacy = db::privacy_of(&state.db, account_id).await?;
    let (about, favourites) = db::profiles::profile_of(&state.db, account_id).await?;
    let avatar = db::profiles::avatar_of(&state.db, account_id).await?;
    let playtime = db::profiles::playtime_of(&state.db, account_id).await?;
    let (conversations, messages, party) = {
        let mut conn = state.db.acquire().await?;
        let conversations = crate::chat::conversations_of(&mut conn, account_id).await?;
        let messages = crate::chat::written_by(&mut conn, account_id).await?;
        let party = match crate::party::party_id_of(&mut conn, account_id).await? {
            Some(id) => crate::party::party(&mut conn, &id).await?.map(|p| p.view()),
            None => None,
        };
        (conversations, messages, party)
    };
    let passkeys = crate::web::store::passkeys_of(&state.db, account_id).await?;
    let consent: (Option<i64>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT age_confirmed_at, terms_version, terms_accepted_at FROM accounts WHERE id = $1",
    )
    .bind(account_id as i64)
    .fetch_one(&state.db)
    .await?;
    let sessions = {
        let mut conn = state.db.acquire().await?;
        let mut views = Vec::new();
        for id in live::sessions_of(&mut conn, account_id).await? {
            if let Some(session) = live::session(&mut conn, &id).await? {
                views.push(session.view());
            }
        }
        views
    };
    Ok(json!({
        "exported": now(),
        "account": me_of(&account),
        "devices": devices.iter().map(|d| json!({
            "id": d.id(), "name": d.name, "platform": d.platform,
            "created": d.created, "last_used": d.last_used,
        })).collect::<Vec<_>>(),
        "friends": related(friends),
        "requests_in": related(incoming),
        "requests_out": related(outgoing),
        "blocked": related(blocked),
        "reports_filed": reports.iter().map(|r| json!({
            "account": {"id": r.reported, "handle": r.reported_handle},
            "reason": r.reason, "details": r.details, "created": r.created,
            "status": r.status, "outcome": r.outcome,
        })).collect::<Vec<_>>(),
        "actions_against_you": actions,
        "ban": ban.map(|b| json!({"until": b.until(), "reason": b.reason, "created": b.created})),
        "sessions": sessions,
        "trophies": trophies.iter().map(|t| json!({
            "title_id": t.title_id, "number": t.number, "grade": t.grade, "earned": t.earned,
        })).collect::<Vec<_>>(),
        "privacy": privacy,
        "passkeys": passkeys.iter().map(|k| json!({
            "name": k.name, "created": k.created, "last_used": k.last_used,
        })).collect::<Vec<_>>(),
        "consent": {
            "age_confirmed_at": consent.0, "terms_version": consent.1,
            "terms_accepted_at": consent.2,
        },
        "profile": {"about": about, "favourites": favourites},
        "avatar": avatar.map(|a| json!({
            "hash": a.hash, "updated": a.updated,
            "png": base64::engine::general_purpose::STANDARD.encode(&a.picture),
        })),
        "playtime": playtime,
        "party": party,
        "conversations": conversations,
        "messages": messages.iter().map(|(conversation, seq, text, created)| json!({
            "conversation": conversation, "seq": seq, "text": text, "created": created,
        })).collect::<Vec<_>>(),
    }))
}

pub async fn by_id(
    State(state): State<AppState>,
    _auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Account>> {
    let id = parse_id(&id)?;
    let account = db::account(&state.db, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(account_of(&account)))
}

#[derive(Deserialize)]
pub struct HandleQuery {
    handle: String,
}

pub async fn by_handle(
    State(state): State<AppState>,
    _auth: Auth,
    query: Result<Query<HandleQuery>, QueryRejection>,
) -> Result<Json<Account>> {
    let Query(query) = query.map_err(|_| AppError::bad_request("Give the handle to look up."))?;
    let account = db::account_by_handle(&state.db, &query.handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(account_of(&account)))
}

pub fn account_of(row: &db::AccountRow) -> Account {
    Account {
        id: AccountId(row.id()),
        handle: row.handle.clone(),
    }
}
