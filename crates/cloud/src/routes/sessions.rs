//! Sessions and invitations. They live in the store (see `live`): every change starts by
//! locking the session's row, so changes from any node take turns.

use std::collections::HashSet;

use anyplay_api::{
    CreateSession, ErrorCode, Event, Expected, Invite, Invited, JoinSession, MAX_MEMBERS,
    MAX_PROPERTIES, Session, UpdateSession, Visibility,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::live::{self, InvitationRow, MemberRow, SessionRow, SessionState};
use crate::state::AppState;
use crate::util::{self, is_endpoint, is_http_url, is_label, now, now_ms, random_uuid};

const MAX_SESSIONS_PER_ACCOUNT: i64 = 4;
/// Invitations one session may have open, and one account may hold.
const MAX_INVITATIONS_PER_SESSION: i64 = 128;
const MAX_INVITATIONS_PER_ACCOUNT: i64 = 50;
/// Accounts one invite request may name.
const MAX_INVITE_BATCH: usize = 64;

fn check_properties(properties: &str) -> Result<()> {
    // Base64 is 4 characters per 3 bytes; refuse early so a huge string is never decoded.
    if properties.len() > MAX_PROPERTIES.div_ceil(3) * 4 {
        return Err(AppError::too_large("The session properties are too large."));
    }
    let decoded = STANDARD
        .decode(properties)
        .map_err(|_| AppError::bad_request("The properties are not standard base64."))?;
    if decoded.len() > MAX_PROPERTIES {
        return Err(AppError::too_large("The session properties are too large."));
    }
    Ok(())
}

fn check_max_members(max: u16) -> Result<()> {
    if (2..=MAX_MEMBERS).contains(&max) {
        Ok(())
    } else {
        Err(AppError::bad_request("max_members is 2 to 64."))
    }
}

fn check_endpoint(endpoint: &str, relay: &Option<String>) -> Result<()> {
    if !is_endpoint(endpoint) {
        return Err(AppError::bad_request(
            "The endpoint is 64 lowercase hex digits.",
        ));
    }
    if let Some(relay) = relay
        && !is_http_url(relay)
    {
        return Err(AppError::bad_request(
            "The relay is an http or https URL of at most 255 characters.",
        ));
    }
    Ok(())
}

async fn handle_of(state: &AppState, account: u64) -> Result<String> {
    Ok(db::account(&state.db, account)
        .await?
        .ok_or_else(AppError::not_found)?
        .handle)
}

async fn address_of(state: &AppState, account: u64) -> Result<String> {
    Ok(state
        .address_of(account)
        .await?
        .ok_or_else(AppError::internal)?
        .to_string())
}

/// Starts the grace periods of a member without an event connection, so it leaves in time.
async fn arm_grace(state: &AppState, account: u64) -> Result<()> {
    Ok(live::arm_grace(&mut *state.db.acquire().await?, account, now_ms()).await?)
}

async fn friends_of(state: &AppState, account: u64) -> Result<HashSet<u64>> {
    Ok(db::friend_ids(&state.db, account)
        .await?
        .into_iter()
        .collect())
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<CreateSession>,
) -> Result<Json<Session>> {
    if !is_label(&request.title_id, 64)
        || !is_label(&request.title, 128)
        || !is_label(&request.version, 32)
        || !is_label(&request.build, 64)
    {
        return Err(AppError::bad_request(
            "title_id, title, version and build must be short printable texts.",
        ));
    }
    check_max_members(request.max_members)?;
    check_endpoint(&request.endpoint, &request.relay)?;
    check_properties(&request.properties)?;
    let handle = handle_of(&state, auth.account).await?;
    let address = address_of(&state, auth.account).await?;

    let created = now() as i64;
    let row = SessionRow {
        id: random_uuid(),
        title_id: request.title_id,
        title: request.title,
        version: request.version,
        build: request.build,
        leader: auth.account as i64,
        max_members: i64::from(request.max_members),
        visibility: live::visibility_text(request.visibility).to_owned(),
        properties: request.properties,
        created,
    };
    let member = MemberRow {
        account_id: auth.account as i64,
        handle,
        endpoint: request.endpoint,
        relay: request.relay,
        address,
        joined: created,
    };
    let mut tx = state.db.begin().await?;
    // A write first, so that SQLite takes its write lock before the count reads.
    live::insert_session(&mut tx, &row).await?;
    if live::count_sessions_of(&mut tx, auth.account).await? >= MAX_SESSIONS_PER_ACCOUNT {
        return Err(AppError::limit("You are in the most sessions allowed."));
    }
    live::add_member(&mut tx, &row.id, &member).await?;
    tx.commit().await?;

    arm_grace(&state, auth.account).await?;
    let view = SessionState {
        row,
        members: vec![member],
    }
    .view();
    state.refresh_presence([auth.account]).await;
    Ok(Json(view))
}

/// Whether the account may know about the session without being in it.
async fn can_see(state: &AppState, session: &SessionState, account: u64) -> Result<bool> {
    if session.is_member(account) {
        return Ok(true);
    }
    let mut conn = state.db.acquire().await?;
    if live::is_invited(&mut conn, account, &session.row.id, None).await? {
        return Ok(true);
    }
    if session.visibility() != Visibility::Friends {
        return Ok(false);
    }
    let members = session.member_ids();
    if db::blocked_between_any(&state.db, account, &members).await? {
        return Ok(false);
    }
    let friends = friends_of(state, account).await?;
    Ok(members.iter().any(|m| friends.contains(m)))
}

pub async fn get(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Session>> {
    let session = live::session(&mut *state.db.acquire().await?, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !can_see(&state, &session, auth.account).await? {
        return Err(AppError::not_found());
    }
    Ok(Json(session.view()))
}

pub async fn join(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<JoinSession>,
) -> Result<Json<Session>> {
    if !is_label(&request.version, 32) || !is_label(&request.build, 64) {
        return Err(AppError::bad_request(
            "version and build must be short printable texts.",
        ));
    }
    check_endpoint(&request.endpoint, &request.relay)?;
    let handle = handle_of(&state, auth.account).await?;
    let friends = friends_of(&state, auth.account).await?;
    let address = address_of(&state, auth.account).await?;

    let mut tx = state.db.begin().await?;
    let mut session = live::lock_session(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let guard = |session: &SessionState| {
        AppError::version_mismatch(Expected {
            version: session.row.version.clone(),
            build: session.row.build.clone(),
        })
    };
    let matches = session.row.version == request.version && session.row.build == request.build;

    // A member joining again only moves its endpoint and relay.
    if session.is_member(auth.account) {
        if !matches {
            return Err(guard(&session));
        }
        live::move_member(
            &mut tx,
            &id,
            auth.account,
            &request.endpoint,
            request.relay.as_deref(),
        )
        .await?;
        tx.commit().await?;
        if let Some(member) = session
            .members
            .iter_mut()
            .find(|m| m.account_id as u64 == auth.account)
        {
            member.endpoint = request.endpoint;
            member.relay = request.relay;
        }
        state.announce(&session);
        return Ok(Json(session.view()));
    }

    // 1. Blocks, in either direction, between the joiner and any member.
    let members = session.member_ids();
    if db::blocked_between_any(&state.db, auth.account, &members).await? {
        return Err(AppError::forbidden("You cannot join this session."));
    }
    // 2. Visibility.
    let invited =
        live::is_invited(&mut tx, auth.account, &id, request.invitation.as_deref()).await?;
    let allowed = match session.visibility() {
        Visibility::Closed => false,
        Visibility::Invite => invited,
        Visibility::Friends => invited || members.iter().any(|m| friends.contains(m)),
    };
    if !allowed {
        return Err(AppError::not_found());
    }
    // 3. The version guard. 4. Capacity.
    if !matches {
        return Err(guard(&session));
    }
    if session.members.len() >= session.max_members() {
        return Err(AppError::new(ErrorCode::Full, "The session is full."));
    }
    if live::count_sessions_of(&mut tx, auth.account).await? >= MAX_SESSIONS_PER_ACCOUNT {
        return Err(AppError::limit("You are in the most sessions allowed."));
    }

    let member = MemberRow {
        account_id: auth.account as i64,
        handle,
        endpoint: request.endpoint,
        relay: request.relay,
        address,
        joined: now() as i64,
    };
    live::add_member(&mut tx, &id, &member).await?;
    // The invitation is used up.
    let used = live::take_used(&mut tx, auth.account, &id).await?;
    tx.commit().await?;

    session.members.push(member);
    state.invitations_gone(&used);
    arm_grace(&state, auth.account).await?;
    state.announce(&session);
    state.refresh_presence(session.member_ids()).await;
    Ok(Json(session.view()))
}

pub async fn leave(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let dirty = state.remove_member(&id, auth.account).await?;
    if dirty.is_empty() {
        return Err(AppError::not_found());
    }
    state.refresh_presence(dirty).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn update(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<UpdateSession>,
) -> Result<Json<Session>> {
    if let Some(properties) = &request.properties {
        check_properties(properties)?;
    }
    if let Some(max) = request.max_members {
        check_max_members(max)?;
    }
    let mut tx = state.db.begin().await?;
    let mut session = live::lock_session(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !session.is_member(auth.account) {
        return Err(AppError::not_found());
    }
    if session.leader() != auth.account {
        return Err(AppError::forbidden(
            "Only the leader can change the session.",
        ));
    }
    if let Some(max) = request.max_members
        && usize::from(max) < session.members.len()
    {
        return Err(AppError::bad_request(
            "max_members is below the number of members.",
        ));
    }
    if let Some(leader) = request.leader
        && !session.is_member(leader.0)
    {
        return Err(AppError::bad_request("The new leader must be a member."));
    }
    if let Some(properties) = request.properties {
        session.row.properties = properties;
    }
    if let Some(visibility) = request.visibility {
        session.row.visibility = live::visibility_text(visibility).to_owned();
    }
    if let Some(max) = request.max_members {
        session.row.max_members = i64::from(max);
    }
    if let Some(leader) = request.leader {
        session.row.leader = leader.0 as i64;
    }
    live::update_session(&mut tx, &session.row).await?;
    tx.commit().await?;

    state.announce(&session);
    // Visibility and capacity decide what friends see of the session.
    state.refresh_presence(session.member_ids()).await;
    Ok(Json(session.view()))
}

pub async fn remove_member(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, account)): Path<(String, String)>,
) -> Result<StatusCode> {
    let target = parse_id(&account)?;
    {
        let session = live::session(&mut *state.db.acquire().await?, &id)
            .await?
            .ok_or_else(AppError::not_found)?;
        if !session.is_member(auth.account) {
            return Err(AppError::not_found());
        }
        if session.leader() != auth.account {
            return Err(AppError::forbidden("Only the leader can remove members."));
        }
        if target == auth.account {
            return Err(AppError::bad_request("Leave the session instead."));
        }
        if !session.is_member(target) {
            return Err(AppError::not_found());
        }
    }
    let dirty = state.remove_member(&id, target).await?;
    state.refresh_presence(dirty).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn invite(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Invite>,
) -> Result<Json<Invited>> {
    if request.to.len() > MAX_INVITE_BATCH {
        return Err(AppError::bad_request("Invite at most 64 accounts at once."));
    }
    let handle = handle_of(&state, auth.account).await?;
    let friends = friends_of(&state, auth.account).await?;

    let mut tx = state.db.begin().await?;
    let session = live::lock_session(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !session.is_member(auth.account) {
        return Err(AppError::not_found());
    }
    if session.visibility() == Visibility::Closed {
        return Err(AppError::forbidden("This session is closed."));
    }
    let ttl = state.tuning.invitation_ttl;
    let mut sent = Vec::new();
    let mut new = Vec::new();
    for to in request.to {
        let to_id = to.0;
        if sent.contains(&to) || !friends.contains(&to_id) || session.is_member(to_id) {
            continue;
        }
        if live::is_invited(&mut tx, to_id, &id, None).await? {
            sent.push(to);
            continue;
        }
        let (open_for_session, held) = live::count_invitations(&mut tx, &id, to_id).await?;
        if open_for_session >= MAX_INVITATIONS_PER_SESSION || held >= MAX_INVITATIONS_PER_ACCOUNT {
            continue;
        }
        let invitation = InvitationRow {
            id: random_uuid(),
            from_id: auth.account as i64,
            from_handle: handle.clone(),
            to_id: to_id as i64,
            session_id: id.clone(),
            title_id: session.row.title_id.clone(),
            title: session.row.title.clone(),
            version: session.row.version.clone(),
            expires: (util::now() + ttl.as_secs()) as i64,
        };
        live::insert_invitation(&mut tx, &invitation, now_ms() + ttl.as_millis() as i64).await?;
        new.push(invitation);
        sent.push(to);
    }
    tx.commit().await?;

    for invitation in new {
        state.news.push(
            invitation.to(),
            &Event::Invitation {
                invitation: invitation.view(),
            },
        );
    }
    Ok(Json(Invited { sent }))
}

pub async fn decline(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let gone = live::take_declined(&mut *state.db.acquire().await?, &id, auth.account).await?;
    if gone.is_empty() {
        return Err(AppError::not_found());
    }
    state.invitations_gone(&gone);
    Ok(StatusCode::NO_CONTENT)
}
