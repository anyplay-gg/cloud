//! Join codes (network `spec/cloud-api.md`, "Join codes"): a code stands for the player who
//! issued it, and redeeming it makes an ordinary invitation from that player. Nothing here
//! joins anybody. The codes themselves are in [`crate::joins`].

use anyplay_api::{
    ErrorCode, Event, JoinCode, JoinKind, JoinView, MAX_PARTY_MEMBERS, Redeemed, Visibility,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};

use crate::db;
use crate::error::{AppError, Result};
use crate::http::Auth;
use crate::joins::{self, Offer};
use crate::live::{self, InvitationRow};
use crate::party;
use crate::routes::{parties, sessions};
use crate::state::AppState;
use crate::util::{now, now_ms, random_uuid};

/// Counts one use of the named limit for the account.
fn limit(state: &AppState, what: &str, account: u64, allowed: u32) -> Result<()> {
    state
        .limiter
        .hit(
            &format!("join-codes:{what}:{account}"),
            allowed,
            state.tuning.rates.join_code_window,
        )
        .map_err(AppError::rate_limited)
}

/// The origin the join page is on: the account web, or else the cloud's own address as the
/// request named it.
fn origin(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(web) = state.config.web_origin() {
        return web;
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty() && h.len() <= 255 && h.bytes().all(|b| b.is_ascii_graphic()))
        .map_or_else(|| state.config.listen.to_string(), str::to_owned);
    let proto = if state.config.trust_proxy
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            == Some("https")
    {
        "https"
    } else {
        "http"
    };
    format!("{proto}://{host}")
}

/// `POST /v1/join-codes`: a new code for the caller, replacing the old one. `not_found` when the
/// caller offers nothing to join.
pub async fn issue(
    State(state): State<AppState>,
    auth: Auth,
    headers: HeaderMap,
) -> Result<Json<JoinCode>> {
    if joins::offered(&mut *state.db.acquire().await?, auth.account)
        .await?
        .is_none()
    {
        return Err(AppError::not_found());
    }
    limit(
        &state,
        "new",
        auth.account,
        state.tuning.rates.join_codes_new,
    )?;
    let code = joins::generate()?;
    let created = now();
    let expires = created + state.config.join_code_minutes * 60;
    let mut tx = state.begin_write().await?;
    joins::replace(&mut tx, auth.account, &code, expires, created).await?;
    tx.commit().await?;
    let url = format!("{}/join/{code}", origin(&state, &headers));
    Ok(Json(JoinCode { code, url, expires }))
}

/// `DELETE /v1/join-codes/current`: the caller's code stops working. Harmless without one.
pub async fn revoke(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    joins::revoke(&mut *state.db.acquire().await?, auth.account).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/join-codes/{code}/redeem`: an invitation to what the issuer offers, for the
/// caller. The caller is not made a member of anything.
pub async fn redeem(
    State(state): State<AppState>,
    auth: Auth,
    Path(code): Path<String>,
) -> Result<Json<Redeemed>> {
    limit(
        &state,
        "redeem",
        auth.account,
        state.tuning.rates.join_codes_redeemed,
    )?;
    let code = joins::normalize(&code).ok_or_else(AppError::not_found)?;
    let issuer = joins::owner(&mut *state.db.acquire().await?, &code, now())
        .await?
        .ok_or_else(AppError::not_found)?;
    if issuer == auth.account {
        return Err(AppError::bad_request(
            "This is your own join code. Send it to someone else.",
        ));
    }
    let from = db::account(&state.db, issuer)
        .await?
        .ok_or_else(AppError::not_found)?
        .handle;

    // Everything that decides runs under the lock of the session or party, as in the join and
    // the invitation this one stands in for.
    let ttl = state.tuning.invitation_ttl;
    let mut tx = state.begin_write().await?;
    if let Some(id) = joins::offered_session(&mut tx, issuer).await? {
        let session = live::lock_session(&mut tx, &id)
            .await?
            .ok_or_else(AppError::not_found)?;
        if session.visibility() == Visibility::Closed || !session.is_member(issuer) {
            return Err(AppError::not_found());
        }
        // A block stays private: it looks like a code that opens nothing.
        if db::blocked_between_any(&mut tx, auth.account, &session.member_ids()).await? {
            return Err(AppError::not_found());
        }
        if session.is_member(auth.account) {
            return Err(AppError::limit("You are in this session already."));
        }
        if let Some(held) =
            live::invitation_from(&mut tx, auth.account, &id, issuer, now_ms()).await?
        {
            return Ok(Json(Redeemed::Session(held.view())));
        }
        if session.members.len() >= session.max_members() {
            return Err(AppError::new(ErrorCode::Full, "The session is full."));
        }
        let (open_for_session, held) = live::count_invitations(&mut tx, &id, auth.account).await?;
        if open_for_session >= sessions::MAX_INVITATIONS_PER_SESSION
            || held >= sessions::MAX_INVITATIONS_PER_ACCOUNT
        {
            return Err(AppError::limit("Too many invitations are open. Try later."));
        }
        let invitation = InvitationRow {
            id: random_uuid(),
            from_id: issuer as i64,
            from_handle: from,
            to_id: auth.account as i64,
            session_id: id,
            title_id: session.row.title_id.clone(),
            title: session.row.title.clone(),
            version: session.row.version.clone(),
            expires: (now() + ttl.as_secs()) as i64,
        };
        live::insert_invitation(&mut tx, &invitation, now_ms() + ttl.as_millis() as i64).await?;
        tx.commit().await?;
        let view = invitation.view();
        sessions::push_invitations(&state, vec![invitation]);
        return Ok(Json(Redeemed::Session(view)));
    }

    let Some(id) = joins::offered_party(&mut tx, issuer).await? else {
        return Err(AppError::not_found());
    };
    let party = party::lock_party(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if party.leader() != issuer {
        return Err(AppError::not_found());
    }
    if party::blocked_with_member(&mut tx, &id, auth.account).await? {
        return Err(AppError::not_found());
    }
    if party.is_member(auth.account) {
        return Err(AppError::limit("You are in this party already."));
    }
    if party::party_id_of(&mut tx, auth.account).await?.is_some() {
        return Err(AppError::limit(
            "You are in a party already. Leave it first.",
        ));
    }
    let members = party.members.len() as u16;
    if let Some(held) = party::invitation_to_party(&mut tx, auth.account, &id, now_ms()).await? {
        return Ok(Json(Redeemed::Party(held.view(members))));
    }
    if party.is_full() {
        return Err(AppError::new(ErrorCode::Full, "The party is full."));
    }
    let (open_for_party, held) = party::count_invitations(&mut tx, &id, auth.account).await?;
    if open_for_party >= parties::MAX_INVITATIONS_PER_PARTY
        || held >= parties::MAX_INVITATIONS_PER_ACCOUNT
    {
        return Err(AppError::limit("Too many invitations are open. Try later."));
    }
    let invitation = party::InvitationRow {
        id: random_uuid(),
        party_id: id,
        from_id: issuer as i64,
        from_handle: from,
        to_id: auth.account as i64,
        expires: (now() + ttl.as_secs()) as i64,
        deadline_ms: now_ms() + ttl.as_millis() as i64,
        call: 0,
    };
    if !party::insert_invitation(&mut tx, &invitation).await? {
        return Err(AppError::limit("You hold an invitation to this party."));
    }
    tx.commit().await?;
    let view = invitation.view(members);
    state.news.push(
        auth.account,
        &Event::PartyInvitation {
            invitation: view.clone(),
        },
    );
    Ok(Json(Redeemed::Party(view)))
}

/// What the page of a code shows: who invites, to what, and how many are there. Nothing else,
/// and one answer (`not_found`) for every code that opens nothing. No viewer is needed or
/// looked at. Used by the portal's `GET /join/{code}`.
pub async fn view(state: &AppState, code: &str) -> Result<JoinView> {
    let code = joins::normalize(code).ok_or_else(AppError::not_found)?;
    let mut conn = state.db.acquire().await?;
    let issuer = joins::owner(&mut conn, &code, now())
        .await?
        .ok_or_else(AppError::not_found)?;
    let offer = joins::offered(&mut conn, issuer)
        .await?
        .ok_or_else(AppError::not_found)?;
    drop(conn);
    let handle = db::account(&state.db, issuer)
        .await?
        .ok_or_else(AppError::not_found)?
        .handle;
    Ok(match offer {
        Offer::Session(session) => JoinView {
            handle,
            title: Some(session.row.title.clone()),
            members: session.members.len() as u16,
            max_members: session.row.max_members as u16,
            kind: JoinKind::Session,
        },
        Offer::Party(party) => JoinView {
            handle,
            title: None,
            members: party.members.len() as u16,
            max_members: MAX_PARTY_MEMBERS,
            kind: JoinKind::Party,
        },
    })
}
