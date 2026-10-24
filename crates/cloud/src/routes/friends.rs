//! Friends, requests and blocks.

use anyplay_api::{
    Account, AccountId, Event, Friend, FriendLists, FriendRequest, FriendRequestAnswer,
    FriendState, Request,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::db::{self, Related};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::live;
use crate::state::AppState;
use crate::util::now;

const MAX_FRIENDS: i64 = 500;
const MAX_OUTGOING: i64 = 100;
const MAX_INCOMING: i64 = 500;
const MAX_BLOCKS: i64 = 1000;

fn account(row: &Related) -> Account {
    Account {
        id: AccountId(row.id()),
        handle: row.handle.clone(),
    }
}

/// Tells accounts that their friend lists changed.
fn notify(state: &AppState, accounts: &[u64]) {
    for account in accounts {
        state.news.push(*account, &Event::Friends);
    }
}

pub async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<FriendLists>> {
    let friends = db::friends_of(&state.db, auth.account).await?;
    let incoming = db::incoming_requests(&state.db, auth.account).await?;
    let outgoing = db::outgoing_requests(&state.db, auth.account).await?;
    let blocked = db::blocked_by(&state.db, auth.account).await?;
    let mut presence =
        live::presence_of_friends(&mut *state.db.acquire().await?, auth.account).await?;
    Ok(Json(FriendLists {
        friends: friends
            .iter()
            .map(|f| Friend {
                account: account(f),
                since: f.since(),
                presence: presence.remove(&f.id()).unwrap_or_default(),
            })
            .collect(),
        incoming: incoming
            .iter()
            .map(|r| Request {
                account: account(r),
                since: r.since(),
            })
            .collect(),
        outgoing: outgoing
            .iter()
            .map(|r| Request {
                account: account(r),
                since: r.since(),
            })
            .collect(),
        blocked: blocked.iter().map(account).collect(),
    }))
}

pub async fn request(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<FriendRequest>,
) -> Result<Json<FriendRequestAnswer>> {
    let me = auth.account;
    let _social = state.social.lock().await;
    let target = db::account_by_handle(&state.db, &request.handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    let them = target.id();
    if them == me {
        return Err(AppError::bad_request("You cannot add yourself."));
    }
    let requested = Json(FriendRequestAnswer {
        state: FriendState::Requested,
    });
    // A block stays private: the answer is the same and nothing happens.
    if db::has_blocked(&state.db, them, me).await? {
        return Ok(requested);
    }
    if db::has_blocked(&state.db, me, them).await? {
        return Err(AppError::forbidden(
            "You blocked this account. Unblock it first.",
        ));
    }
    let friends = Json(FriendRequestAnswer {
        state: FriendState::Friends,
    });
    if db::are_friends(&state.db, me, them).await? {
        return Ok(friends);
    }
    if db::request_exists(&state.db, them, me).await? {
        check_friend_limits(&state, me, them).await?;
        db::make_friends(&state.db, me, them, now()).await?;
        notify(&state, &[me, them]);
        return Ok(friends);
    }
    if db::request_exists(&state.db, me, them).await? {
        return Ok(requested);
    }
    if db::count_friends(&state.db, me).await? >= MAX_FRIENDS {
        return Err(AppError::limit("You have the most friends allowed."));
    }
    if db::count_outgoing(&state.db, me).await? >= MAX_OUTGOING {
        return Err(AppError::limit("You have too many open friend requests."));
    }
    if db::count_incoming(&state.db, them).await? >= MAX_INCOMING {
        return Err(AppError::limit("This account has too many open requests."));
    }
    db::insert_request(&state.db, me, them, now()).await?;
    notify(&state, &[me, them]);
    Ok(requested)
}

async fn check_friend_limits(state: &AppState, a: u64, b: u64) -> Result<()> {
    for account in [a, b] {
        if db::count_friends(&state.db, account).await? >= MAX_FRIENDS {
            return Err(AppError::limit("The most friends allowed is reached."));
        }
    }
    Ok(())
}

pub async fn accept(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    let _social = state.social.lock().await;
    if !db::request_exists(&state.db, them, auth.account).await? {
        return Err(AppError::not_found());
    }
    check_friend_limits(&state, auth.account, them).await?;
    db::make_friends(&state.db, auth.account, them, now()).await?;
    notify(&state, &[auth.account, them]);
    Ok(StatusCode::NO_CONTENT)
}

/// Declines an incoming request or withdraws an outgoing one.
pub async fn cancel(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    let _social = state.social.lock().await;
    let removed = db::delete_request(&state.db, them, auth.account).await?
        || db::delete_request(&state.db, auth.account, them).await?;
    if !removed {
        return Err(AppError::not_found());
    }
    notify(&state, &[auth.account, them]);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unfriend(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    let _social = state.social.lock().await;
    if !db::remove_friendship(&state.db, auth.account, them).await? {
        return Err(AppError::not_found());
    }
    notify(&state, &[auth.account, them]);
    // Neither sees the other's presence from now on, and invitations between them end.
    let gone = live::take_between(&mut *state.db.acquire().await?, auth.account, them).await?;
    state.invitations_gone(&gone);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn block(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    if them == auth.account {
        return Err(AppError::bad_request("You cannot block yourself."));
    }
    let _social = state.social.lock().await;
    if db::account(&state.db, them).await?.is_none() {
        return Err(AppError::not_found());
    }
    if !db::has_blocked(&state.db, auth.account, them).await?
        && db::count_blocks(&state.db, auth.account).await? >= MAX_BLOCKS
    {
        return Err(AppError::limit(
            "You have blocked the most accounts allowed.",
        ));
    }
    let had_relation = db::block(&state.db, auth.account, them, now()).await?;
    let gone = live::take_between(&mut *state.db.acquire().await?, auth.account, them).await?;
    state.invitations_gone(&gone);
    let gone =
        crate::party::take_between(&mut *state.db.acquire().await?, auth.account, them).await?;
    state.party_invitations_gone(&gone);
    state.news.push(auth.account, &Event::Friends);
    // The blocked account learns only what it would see anyway: a friend or request is gone.
    if had_relation {
        state.news.push(them, &Event::Friends);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unblock(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    let _social = state.social.lock().await;
    if db::unblock(&state.db, auth.account, them).await? {
        notify(&state, &[auth.account]);
    }
    Ok(StatusCode::NO_CONTENT)
}
