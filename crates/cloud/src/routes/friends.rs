//! Friends, requests and blocks.
//!
//! A change between two accounts runs in one store transaction that holds both accounts (see
//! [`hold`]): the limits it checks count what the other changes to either account have written,
//! on this node and every other, while changes between other accounts go on at the same time.

use anyplay_api::{
    Account, AccountId, Event, Friend, FriendLists, FriendRequest, FriendRequestAnswer,
    FriendState, Request,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::AnyConnection;

use crate::db::{self, Related};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::live;
use crate::state::{AppState, WriteTx};
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

/// How long a change waits for another change to the same account on PostgreSQL before it is
/// refused. Without a limit, a burst at one popular account would hold a store connection per
/// waiting request and could take the whole pool.
const LOCK_TIMEOUT: &str = "SET LOCAL lock_timeout = '2s'";

/// Starts a transaction that holds accounts `a` and `b` until it ends: another change that holds
/// either of them waits for it. Every change to friends, requests and blocks must start here, or
/// the limits it checks no longer count what the others write. On PostgreSQL it locks the two
/// accounts' rows, in the order of their IDs so that two changes never wait for each other, and a
/// change that waits longer than [`LOCK_TIMEOUT`] is answered `rate_limited`; SQLite has one
/// writer, which the transaction already is. Fails with `not_found` when either account is gone,
/// unless `missing_ok`.
async fn hold(state: &AppState, a: u64, b: u64, missing_ok: bool) -> Result<WriteTx> {
    let mut tx = state.begin_write().await?;
    let postgres = state.config.database_url.is_some();
    if postgres {
        sqlx::query(LOCK_TIMEOUT).execute(&mut *tx).await?;
    }
    match db::lock_accounts(&mut tx, a, b, postgres).await {
        Ok(true) => Ok(tx),
        Ok(false) if missing_ok => Ok(tx),
        Ok(false) => Err(AppError::not_found()),
        Err(error) if is_lock_timeout(&error) => Err(AppError::rate_limited(1)),
        Err(error) => Err(error.into()),
    }
}

/// PostgreSQL's `lock_not_available`, which a `lock_timeout` raises.
fn is_lock_timeout(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(e) if e.code().as_deref() == Some("55P03"))
}

/// The friend lists with the friends' presence. All of it is read on one store connection: under
/// load a handler waits for each connection it asks the pool for, and this one is asked for by
/// every player who comes back after a reconnect.
pub async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<FriendLists>> {
    let mut conn = state.db.acquire().await?;
    let friends = db::friends_of(&mut *conn, auth.account).await?;
    let incoming = db::incoming_requests(&mut *conn, auth.account).await?;
    let outgoing = db::outgoing_requests(&mut *conn, auth.account).await?;
    let blocked = db::blocked_by(&mut *conn, auth.account).await?;
    let mut presence = live::presence_of_friends(&mut conn, auth.account).await?;
    drop(conn);
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
    let friends = Json(FriendRequestAnswer {
        state: FriendState::Friends,
    });
    let rules = crate::moderation::friend_request_rules(&state, me).await?;
    let mut tx = hold(&state, me, them, false).await?;
    // A block stays private: the answer is the same and nothing happens.
    if db::has_blocked(&mut *tx, them, me).await? {
        return Ok(requested);
    }
    if db::has_blocked(&mut *tx, me, them).await? {
        return Err(AppError::forbidden(
            "You blocked this account. Unblock it first.",
        ));
    }
    if db::are_friends(&mut *tx, me, them).await? {
        return Ok(friends);
    }
    if db::request_exists(&mut tx, them, me).await? {
        check_friend_limits(&mut tx, me, them).await?;
        db::make_friends(&mut tx, me, them, now()).await?;
        tx.commit().await?;
        notify(&state, &[me, them]);
        state.record_friends(me, them).await;
        return Ok(friends);
    }
    if db::request_exists(&mut tx, me, them).await? {
        return Ok(requested);
    }
    // A muted account sends none, a new one only a few.
    rules.check(&mut tx).await?;
    if db::count_friends(&mut tx, me).await? >= MAX_FRIENDS {
        return Err(AppError::limit("You have the most friends allowed."));
    }
    if db::count_outgoing(&mut tx, me).await? >= MAX_OUTGOING {
        return Err(AppError::limit("You have too many open friend requests."));
    }
    if db::count_incoming(&mut tx, them).await? >= MAX_INCOMING {
        return Err(AppError::limit("This account has too many open requests."));
    }
    db::insert_request(&mut tx, me, them, now()).await?;
    rules.record(&mut tx).await?;
    tx.commit().await?;
    notify(&state, &[me, them]);
    crate::push::notify_from(&state, them, me, crate::push::Note::friend_request);
    Ok(requested)
}

async fn check_friend_limits(tx: &mut AnyConnection, a: u64, b: u64) -> Result<()> {
    for account in [a, b] {
        if db::count_friends(tx, account).await? >= MAX_FRIENDS {
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
    let mut tx = hold(&state, auth.account, them, false).await?;
    if !db::request_exists(&mut tx, them, auth.account).await? {
        return Err(AppError::not_found());
    }
    check_friend_limits(&mut tx, auth.account, them).await?;
    db::make_friends(&mut tx, auth.account, them, now()).await?;
    tx.commit().await?;
    notify(&state, &[auth.account, them]);
    state.record_friends(auth.account, them).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Declines an incoming request or withdraws an outgoing one.
pub async fn cancel(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    let mut tx = hold(&state, auth.account, them, false).await?;
    let removed = db::delete_request(&mut tx, them, auth.account).await?
        || db::delete_request(&mut tx, auth.account, them).await?;
    if !removed {
        return Err(AppError::not_found());
    }
    tx.commit().await?;
    notify(&state, &[auth.account, them]);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unfriend(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let them = parse_id(&id)?;
    let mut tx = hold(&state, auth.account, them, false).await?;
    if !db::remove_friendship(&mut tx, auth.account, them).await? {
        return Err(AppError::not_found());
    }
    tx.commit().await?;
    notify(&state, &[auth.account, them]);
    // Neither sees the other's presence from now on, and invitations between them end.
    let gone = live::take_between(&mut *state.db.acquire().await?, auth.account, them).await?;
    state.invitations_gone(&gone);
    state.forget_missed_between(auth.account, them).await?;
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
    if db::account(&state.db, them).await?.is_none() {
        return Err(AppError::not_found());
    }
    let mut tx = hold(&state, auth.account, them, false).await?;
    if !db::has_blocked(&mut *tx, auth.account, them).await?
        && db::count_blocks(&mut tx, auth.account).await? >= MAX_BLOCKS
    {
        return Err(AppError::limit(
            "You have blocked the most accounts allowed.",
        ));
    }
    let had_relation = db::block(&mut tx, auth.account, them, now()).await?;
    tx.commit().await?;
    let gone = live::take_between(&mut *state.db.acquire().await?, auth.account, them).await?;
    state.invitations_gone(&gone);
    let gone =
        crate::party::take_between(&mut *state.db.acquire().await?, auth.account, them).await?;
    state.party_invitations_gone(&gone);
    state.forget_missed_between(auth.account, them).await?;
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
    let mut tx = hold(&state, auth.account, them, true).await?;
    let removed = db::unblock(&mut tx, auth.account, them).await?;
    tx.commit().await?;
    if removed {
        notify(&state, &[auth.account]);
    }
    Ok(StatusCode::NO_CONTENT)
}
