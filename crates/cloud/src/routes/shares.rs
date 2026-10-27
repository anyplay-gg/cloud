//! Share play: a host streams the running game to one member of their party (network
//! `spec/share-play.md`, "Cloud API"). The store and the endings are in [`crate::share`].
//!
//! The cloud offers it only when `share_enabled` is set; until then every route answers
//! `forbidden`. Nothing here sees a picture, a sound or a pad state: only who shares with whom,
//! the title, the mode and the path.

use anyplay_api::{
    AcceptShare, CreateShare, Event, Share, ShareGoneReason, ShareInvitations, ShareMode,
    SharePath, UpdateShare,
};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::share::{self, ShareRow};
use crate::state::AppState;
use crate::util::{is_endpoint, is_http_url, now_ms, random_uuid};
use crate::{live, party};

fn enabled(state: &AppState) -> Result<()> {
    if state.config.share_enabled {
        Ok(())
    } else {
        Err(AppError::forbidden("This cloud has share play off."))
    }
}

fn check_endpoint(endpoint: &str, relay: Option<&str>) -> Result<()> {
    if !is_endpoint(endpoint) {
        return Err(AppError::bad_request(
            "The endpoint is 64 lowercase hex digits.",
        ));
    }
    if let Some(relay) = relay
        && !(is_http_url(relay) && relay.starts_with("https://"))
    {
        return Err(AppError::bad_request(
            "The relay is an https URL of at most 255 characters.",
        ));
    }
    Ok(())
}

fn known_mode(mode: ShareMode) -> Result<()> {
    if mode == ShareMode::Unknown {
        return Err(AppError::bad_request(
            "The mode is watch, hand_over or together.",
        ));
    }
    Ok(())
}

/// The share as the caller sees it, for an answer.
fn view(row: &ShareRow, viewer: u64) -> Result<Share> {
    row.view(viewer).ok_or_else(AppError::not_found)
}

/// Tells the two parties of a share that it changed. The guest of a share that is not accepted
/// yet hears of the changed invitation instead: it is not in the share.
fn announce(state: &AppState, row: &ShareRow) {
    for account in [row.host(), row.guest()] {
        if account == row.guest() && !row.is_active() {
            state.news.push(
                account,
                &Event::ShareInvitation {
                    invitation: row.invitation_view(),
                },
            );
        } else if let Some(share) = row.view(account) {
            state.news.push(
                account,
                &Event::Share {
                    share: Box::new(share),
                },
            );
        }
    }
}

/// The host invites a member of their party.
pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<CreateShare>,
) -> Result<Json<Share>> {
    enabled(&state)?;
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("share-start:{}", auth.account),
            rates.share_starts,
            rates.share_window,
        )
        .map_err(AppError::rate_limited)?;
    check_endpoint(&request.endpoint, request.relay.as_deref())?;
    known_mode(request.mode)?;
    let host = auth.account;
    let guest = request.to.0;
    if guest == host {
        return Err(AppError::bad_request("You cannot share with yourself."));
    }

    let mut tx = state.begin_write().await?;
    let host_party = party::party_id_of(&mut tx, host).await?;
    let guest_party = party::party_id_of(&mut tx, guest).await?;
    let (Some(id), Some(other)) = (host_party, guest_party) else {
        return Err(AppError::forbidden(
            "Share play is for members of the same party.",
        ));
    };
    if id != other {
        return Err(AppError::forbidden(
            "Share play is for members of the same party.",
        ));
    }
    if crate::db::blocked_between_any(&mut tx, host, &[guest]).await?
        || state.ban_of(guest).is_some()
    {
        return Err(AppError::forbidden("You cannot share with this account."));
    }
    let Some(activity) = share::activity_of(&mut tx, host).await? else {
        return Err(AppError::not_found());
    };
    let party = party::party(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let handle = |account: u64| party.member(account).map(|m| m.handle.clone());
    let (Some(host_handle), Some(guest_handle)) = (handle(host), handle(guest)) else {
        return Err(AppError::forbidden(
            "Share play is for members of the same party.",
        ));
    };

    let now = now_ms();
    let row = ShareRow {
        id: random_uuid(),
        invitation: random_uuid(),
        host_id: host as i64,
        host_handle,
        guest_id: guest as i64,
        guest_handle,
        title_id: activity.title_id,
        title: activity.title,
        mode: share::mode_str(request.mode).to_owned(),
        state: "invited".to_owned(),
        host_endpoint: request.endpoint,
        host_relay: request.relay,
        guest_endpoint: String::new(),
        guest_relay: None,
        path: None,
        created: (now / 1000),
        revision: 0,
        deadline_ms: now + state.tuning.share_invitation_ttl.as_millis() as i64,
        relay_since_ms: None,
    };
    if !share::insert(&mut tx, &row).await? {
        // Dropping the transaction takes the share row back.
        return Err(AppError::limit(
            "You or the other player are in a share already.",
        ));
    }
    // An account in a share without an event connection leaves it in time.
    live::arm_grace(&mut tx, host, now).await?;
    live::arm_grace(&mut tx, guest, now).await?;
    tx.commit().await?;

    state.news.push(
        guest,
        &Event::ShareInvitation {
            invitation: row.invitation_view(),
        },
    );
    let mine = view(&row, host)?;
    state.news.push(
        host,
        &Event::Share {
            share: Box::new(mine.clone()),
        },
    );
    Ok(Json(mine))
}

/// The share the caller is in, as the caller sees it.
pub async fn get(State(state): State<AppState>, auth: Auth) -> Result<Json<Share>> {
    enabled(&state)?;
    let row = share::share_of(&mut *state.db.acquire().await?, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(view(&row, auth.account)?))
}

/// The invited guest accepts and names its own endpoint.
pub async fn accept(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<AcceptShare>,
) -> Result<Json<Share>> {
    enabled(&state)?;
    check_endpoint(&request.endpoint, request.relay.as_deref())?;
    let mut tx = state.begin_write().await?;
    let Some(row) = share::lock_of(&mut tx, auth.account).await? else {
        return Err(AppError::not_found());
    };
    if row.guest() != auth.account {
        return Err(AppError::forbidden("Only the invited player can accept."));
    }
    if row.is_active() {
        return Err(AppError::bad_request("You accepted this share already."));
    }
    if row.deadline_ms <= now_ms() {
        drop(tx);
        state.end_share(&row.id, ShareGoneReason::Timeout).await?;
        return Err(AppError::not_found());
    }
    if row.host_endpoint == request.endpoint {
        return Err(AppError::bad_request(
            "The two sides need endpoints of their own.",
        ));
    }
    share::set_accepted(
        &mut tx,
        &row.id,
        &request.endpoint,
        request.relay.as_deref(),
    )
    .await?;
    tx.commit().await?;

    let mut row = row;
    row.state = "active".to_owned();
    row.guest_endpoint = request.endpoint;
    row.guest_relay = request.relay;
    state.news.push(
        row.guest(),
        &Event::ShareInvitationGone {
            id: row.invitation.clone(),
        },
    );
    announce(&state, &row);
    Ok(Json(view(&row, auth.account)?))
}

/// The host ends the share, or the guest declines or leaves it.
pub async fn end(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    enabled(&state)?;
    let row = share::share_of(&mut *state.db.acquire().await?, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let reason = if row.host() == auth.account {
        ShareGoneReason::Ended
    } else if row.is_active() {
        ShareGoneReason::Left
    } else {
        ShareGoneReason::Declined
    };
    state.end_share(&row.id, reason).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The host's net service tells what the share shows: the mode it applied, the path it takes.
pub async fn update(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<UpdateShare>,
) -> Result<Json<Share>> {
    enabled(&state)?;
    if request.mode.is_none() && request.path.is_none() {
        return Err(AppError::bad_request("Send a mode or a path."));
    }
    if let Some(mode) = request.mode {
        known_mode(mode)?;
    }
    if request.path == Some(SharePath::Unknown) {
        return Err(AppError::bad_request("The path is direct or relay."));
    }
    let now = now_ms();
    let mut tx = state.begin_write().await?;
    let Some(mut row) = share::lock_of(&mut tx, auth.account).await? else {
        return Err(AppError::not_found());
    };
    if row.host() != auth.account {
        return Err(AppError::forbidden("Only the host changes the share."));
    }
    if let Some(mode) = request.mode {
        share::set_mode(&mut tx, &row.id, mode).await?;
        row.mode = share::mode_str(mode).to_owned();
    }
    if let Some(path) = request.path {
        // Relayed time counts from the report that says so to the one that says otherwise.
        let since = match (path, row.relay_since_ms) {
            (SharePath::Relay, Some(since)) => Some(since),
            (SharePath::Relay, None) => Some(now),
            _ => None,
        };
        if path != SharePath::Relay
            && let Some(was) = row.relay_since_ms
        {
            share::add_relay_use(&mut tx, row.host(), was, now).await?;
        }
        share::set_path(&mut tx, &row.id, path, since).await?;
        row.path = Some(share::path_str(path).to_owned());
        row.relay_since_ms = since;
    }
    tx.commit().await?;
    announce(&state, &row);
    Ok(Json(view(&row, auth.account)?))
}

/// The invitations the caller holds.
pub async fn invitations(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Json<ShareInvitations>> {
    enabled(&state)?;
    let rows =
        share::invitations_of(&mut *state.db.acquire().await?, auth.account, now_ms()).await?;
    Ok(Json(ShareInvitations {
        invitations: rows.iter().map(ShareRow::invitation_view).collect(),
    }))
}
