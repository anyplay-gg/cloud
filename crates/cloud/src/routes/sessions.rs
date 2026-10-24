//! Sessions and invitations. Sessions live in memory (see `hub`).

use std::collections::HashSet;
use std::time::Instant;

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
use crate::hub::{InvitationMem, MemberMem, Memory, SessionMem};
use crate::state::AppState;
use crate::util::{self, is_endpoint, is_http_url, is_label, now, random_uuid};

const MAX_SESSIONS_PER_ACCOUNT: usize = 4;
/// Invitations one session may have open, and one account may hold.
const MAX_INVITATIONS_PER_SESSION: usize = 128;
const MAX_INVITATIONS_PER_ACCOUNT: usize = 50;
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

fn session_count(hub: &Memory, account: u64) -> usize {
    hub.accounts.get(&account).map_or(0, |a| a.sessions.len())
}

fn address_of(hub: &mut Memory, state: &AppState, account: u64) -> Result<std::net::Ipv4Addr> {
    hub.lease(account, state.tuning.lease_ttl)
        .ok_or_else(AppError::internal)
}

fn member_accounts(hub: &Memory, id: &str) -> Vec<u64> {
    hub.sessions
        .get(id)
        .map(|s| s.members.iter().map(|m| m.account).collect())
        .unwrap_or_default()
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

    let mut hub = state.hub.lock().await;
    if session_count(&hub, auth.account) >= MAX_SESSIONS_PER_ACCOUNT {
        return Err(AppError::limit("You are in the most sessions allowed."));
    }
    let address = address_of(&mut hub, &state, auth.account)?;
    let id = random_uuid();
    hub.sessions.insert(
        id.clone(),
        SessionMem {
            id: id.clone(),
            title_id: request.title_id,
            title: request.title,
            version: request.version,
            build: request.build,
            leader: auth.account,
            max_members: request.max_members,
            visibility: request.visibility,
            properties: request.properties,
            members: vec![MemberMem {
                account: auth.account,
                handle,
                endpoint: request.endpoint,
                relay: request.relay,
                address,
                joined: now(),
            }],
            created: now(),
        },
    );
    hub.accounts
        .entry(auth.account)
        .or_default()
        .sessions
        .push(id.clone());
    state.arm_grace(&mut hub, auth.account);
    let view = hub.session_view(&id).ok_or_else(AppError::internal)?;
    drop(hub);
    state.refresh_presence([auth.account]).await;
    Ok(Json(view))
}

/// Whether the account may know about the session without being in it.
async fn can_see(state: &AppState, hub: &Memory, account: u64, id: &str) -> Result<bool> {
    let Some(session) = hub.sessions.get(id) else {
        return Ok(false);
    };
    if session.members.iter().any(|m| m.account == account) {
        return Ok(true);
    }
    if hub
        .invitations
        .values()
        .any(|i| i.to == account && i.session == id)
    {
        return Ok(true);
    }
    if session.visibility != Visibility::Friends {
        return Ok(false);
    }
    let members: Vec<u64> = session.members.iter().map(|m| m.account).collect();
    if db::blocked_between_any(&state.db, account, &members).await? {
        return Ok(false);
    }
    let friends: HashSet<u64> = db::friend_ids(&state.db, account)
        .await?
        .into_iter()
        .collect();
    Ok(members.iter().any(|m| friends.contains(m)))
}

pub async fn get(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Session>> {
    let hub = state.hub.lock().await;
    if !can_see(&state, &hub, auth.account, &id).await? {
        return Err(AppError::not_found());
    }
    hub.session_view(&id)
        .map(Json)
        .ok_or_else(AppError::not_found)
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

    let mut hub = state.hub.lock().await;
    let session = hub.sessions.get(&id).ok_or_else(AppError::not_found)?;
    let guard = || {
        AppError::version_mismatch(Expected {
            version: session.version.clone(),
            build: session.build.clone(),
        })
    };
    let matches = session.version == request.version && session.build == request.build;

    // A member joining again only moves its endpoint and relay.
    if session.members.iter().any(|m| m.account == auth.account) {
        if !matches {
            return Err(guard());
        }
        if let Some(session) = hub.sessions.get_mut(&id)
            && let Some(member) = session
                .members
                .iter_mut()
                .find(|m| m.account == auth.account)
        {
            member.endpoint = request.endpoint;
            member.relay = request.relay;
        }
        hub.announce_session(&id);
        return hub
            .session_view(&id)
            .map(Json)
            .ok_or_else(AppError::not_found);
    }

    // 1. Blocks, in either direction, between the joiner and any member.
    let members = member_accounts(&hub, &id);
    if db::blocked_between_any(&state.db, auth.account, &members).await? {
        return Err(AppError::forbidden("You cannot join this session."));
    }
    // 2. Visibility.
    let session = hub.sessions.get(&id).ok_or_else(AppError::not_found)?;
    let invited = hub.invitations.values().any(|i| {
        i.to == auth.account
            && i.session == id
            && request
                .invitation
                .as_ref()
                .is_none_or(|wanted| *wanted == i.id)
    });
    let allowed = match session.visibility {
        Visibility::Closed => false,
        Visibility::Invite => invited,
        Visibility::Friends => {
            invited || {
                let friends: HashSet<u64> = db::friend_ids(&state.db, auth.account)
                    .await?
                    .into_iter()
                    .collect();
                members.iter().any(|m| friends.contains(m))
            }
        }
    };
    if !allowed {
        return Err(AppError::not_found());
    }
    // 3. The version guard. 4. Capacity.
    let session = hub.sessions.get(&id).ok_or_else(AppError::not_found)?;
    if !(session.version == request.version && session.build == request.build) {
        return Err(AppError::version_mismatch(Expected {
            version: session.version.clone(),
            build: session.build.clone(),
        }));
    }
    if session.members.len() >= usize::from(session.max_members) {
        return Err(AppError::new(ErrorCode::Full, "The session is full."));
    }
    if session_count(&hub, auth.account) >= MAX_SESSIONS_PER_ACCOUNT {
        return Err(AppError::limit("You are in the most sessions allowed."));
    }

    let address = address_of(&mut hub, &state, auth.account)?;
    if let Some(session) = hub.sessions.get_mut(&id) {
        session.members.push(MemberMem {
            account: auth.account,
            handle,
            endpoint: request.endpoint,
            relay: request.relay,
            address,
            joined: now(),
        });
    }
    hub.accounts
        .entry(auth.account)
        .or_default()
        .sessions
        .push(id.clone());
    // The invitation is used up.
    let used: Vec<String> = hub
        .invitations
        .values()
        .filter(|i| i.to == auth.account && i.session == id)
        .map(|i| i.id.clone())
        .collect();
    for invitation in used {
        hub.remove_invitation(&invitation);
    }
    state.arm_grace(&mut hub, auth.account);
    hub.announce_session(&id);
    let dirty = member_accounts(&hub, &id);
    let view = hub.session_view(&id).ok_or_else(AppError::internal)?;
    drop(hub);
    state.refresh_presence(dirty).await;
    Ok(Json(view))
}

pub async fn leave(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let mut dirty = Vec::new();
    {
        let mut hub = state.hub.lock().await;
        if !hub.is_member(&id, auth.account) {
            return Err(AppError::not_found());
        }
        hub.remove_member(&id, auth.account, &mut dirty);
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
    let mut hub = state.hub.lock().await;
    let session = hub.sessions.get(&id).ok_or_else(AppError::not_found)?;
    if !session.members.iter().any(|m| m.account == auth.account) {
        return Err(AppError::not_found());
    }
    if session.leader != auth.account {
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
        && !session.members.iter().any(|m| m.account == leader.0)
    {
        return Err(AppError::bad_request("The new leader must be a member."));
    }
    if let Some(session) = hub.sessions.get_mut(&id) {
        if let Some(properties) = request.properties {
            session.properties = properties;
        }
        if let Some(visibility) = request.visibility {
            session.visibility = visibility;
        }
        if let Some(max) = request.max_members {
            session.max_members = max;
        }
        if let Some(leader) = request.leader {
            session.leader = leader.0;
        }
    }
    hub.announce_session(&id);
    let dirty = member_accounts(&hub, &id);
    let view = hub.session_view(&id).ok_or_else(AppError::internal)?;
    drop(hub);
    // Visibility and capacity decide what friends see of the session.
    state.refresh_presence(dirty).await;
    Ok(Json(view))
}

pub async fn remove_member(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, account)): Path<(String, String)>,
) -> Result<StatusCode> {
    let target = parse_id(&account)?;
    let mut dirty = Vec::new();
    {
        let mut hub = state.hub.lock().await;
        let session = hub.sessions.get(&id).ok_or_else(AppError::not_found)?;
        if !session.members.iter().any(|m| m.account == auth.account) {
            return Err(AppError::not_found());
        }
        if session.leader != auth.account {
            return Err(AppError::forbidden("Only the leader can remove members."));
        }
        if target == auth.account {
            return Err(AppError::bad_request("Leave the session instead."));
        }
        if !session.members.iter().any(|m| m.account == target) {
            return Err(AppError::not_found());
        }
        hub.remove_member(&id, target, &mut dirty);
    }
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
    let friends: HashSet<u64> = db::friend_ids(&state.db, auth.account)
        .await?
        .into_iter()
        .collect();

    let mut hub = state.hub.lock().await;
    let session = hub.sessions.get(&id).ok_or_else(AppError::not_found)?;
    if !session.members.iter().any(|m| m.account == auth.account) {
        return Err(AppError::not_found());
    }
    if session.visibility == Visibility::Closed {
        return Err(AppError::forbidden("This session is closed."));
    }
    let (title_id, title, version) = (
        session.title_id.clone(),
        session.title.clone(),
        session.version.clone(),
    );
    let mut sent = Vec::new();
    for to in request.to {
        let to_id = to.0;
        if sent.contains(&to) || !friends.contains(&to_id) || hub.is_member(&id, to_id) {
            continue;
        }
        if hub
            .invitations
            .values()
            .any(|i| i.to == to_id && i.session == id)
        {
            sent.push(to);
            continue;
        }
        let open_for_session = hub.invitations.values().filter(|i| i.session == id).count();
        let held = hub.invitations.values().filter(|i| i.to == to_id).count();
        if open_for_session >= MAX_INVITATIONS_PER_SESSION || held >= MAX_INVITATIONS_PER_ACCOUNT {
            continue;
        }
        let ttl = state.tuning.invitation_ttl;
        let invitation = InvitationMem {
            id: random_uuid(),
            from: auth.account,
            from_handle: handle.clone(),
            to: to_id,
            session: id.clone(),
            title_id: title_id.clone(),
            title: title.clone(),
            version: version.clone(),
            expires: util::now() + ttl.as_secs(),
            deadline: Instant::now() + ttl,
        };
        let view = hub.invitation_view(&invitation);
        hub.invitations.insert(invitation.id.clone(), invitation);
        hub.push(to_id, Event::Invitation { invitation: view });
        sent.push(to);
    }
    Ok(Json(Invited { sent }))
}

pub async fn decline(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let mut hub = state.hub.lock().await;
    match hub.invitations.get(&id) {
        Some(invitation) if invitation.to == auth.account => {
            hub.remove_invitation(&id);
            Ok(StatusCode::NO_CONTENT)
        }
        _ => Err(AppError::not_found()),
    }
}
