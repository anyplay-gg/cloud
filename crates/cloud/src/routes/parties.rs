//! Parties: up to 8 friends who stay together across games. The leader invites, members leave at
//! any time, and the leader moves everyone into one of its sessions with the invitations that
//! sessions already have. Every change starts by locking the party's row (see `party`).

use std::collections::HashSet;

use anyplay_api::{
    AccountId, ErrorCode, Event, Invite, Invited, MoveParty, Party, PartyEndpoint,
    PartyInvitations, UpdateParty, Visibility,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::chat::{self, ACCEPTED, ConversationRow};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::live;
use crate::party::{self, InvitationRow, MemberRow, PartyRow, PartyState};
use crate::routes::sessions;
use crate::state::AppState;
use crate::util::{is_endpoint, is_http_url, now, now_ms, random_uuid};

/// Accounts one invite request may name.
const MAX_INVITE_BATCH: usize = 16;
/// Invitations one party may have open, and one account may hold.
const MAX_INVITATIONS_PER_PARTY: i64 = 16;
const MAX_INVITATIONS_PER_ACCOUNT: i64 = 20;

async fn handle_of(state: &AppState, account: u64) -> Result<String> {
    Ok(crate::db::account(&state.db, account)
        .await?
        .ok_or_else(AppError::not_found)?
        .handle)
}

/// Starts the grace period of a member without an event connection, so it leaves in time.
async fn arm_grace(state: &AppState, account: u64) -> Result<()> {
    Ok(live::arm_grace(&mut *state.db.acquire().await?, account, now_ms()).await?)
}

/// A new party with `account` as its only member and leader, and its chat.
async fn make_party(state: &AppState, account: u64, handle: String) -> Result<PartyState> {
    let created = now() as i64;
    let row = PartyRow {
        id: random_uuid(),
        leader: account as i64,
        created,
        revision: 0,
    };
    let member = MemberRow::new(account, handle, created);
    let conversation = ConversationRow {
        id: random_uuid(),
        kind: chat::kind_text(anyplay_api::ConversationKind::Party).to_owned(),
        name: None,
        party_id: Some(row.id.clone()),
        pair_low: None,
        pair_high: None,
        last_seq: 0,
        created,
        updated_ms: now_ms(),
    };
    let mut tx = state.db.begin().await?;
    party::insert_party(&mut tx, &row).await?;
    if !party::add_member(&mut tx, &row.id, &member).await? {
        return Err(AppError::limit("You are in a party already."));
    }
    chat::insert_conversation(&mut tx, &conversation).await?;
    chat::add_member(
        &mut tx,
        &conversation.id,
        account,
        ACCEPTED,
        0,
        created as u64,
    )
    .await?;
    tx.commit().await?;
    arm_grace(state, account).await?;
    Ok(PartyState {
        row,
        members: vec![member],
        conversation: conversation.id,
    })
}

/// The party an account is in, read fresh.
async fn party_of(state: &AppState, account: u64) -> Result<Option<PartyState>> {
    let mut conn = state.db.acquire().await?;
    match party::party_id_of(&mut conn, account).await? {
        Some(id) => Ok(party::party(&mut conn, &id).await?),
        None => Ok(None),
    }
}

pub async fn get(State(state): State<AppState>, auth: Auth) -> Result<Json<Party>> {
    let party = party_of(&state, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(party.view()))
}

pub async fn create(State(state): State<AppState>, auth: Auth) -> Result<Json<Party>> {
    let handle = handle_of(&state, auth.account).await?;
    let party = make_party(&state, auth.account, handle).await?;
    state.announce_party(&party);
    Ok(Json(party.view()))
}

pub async fn invite(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<Invite>,
) -> Result<Json<Invited>> {
    if request.to.len() > MAX_INVITE_BATCH {
        return Err(AppError::bad_request("Invite at most 16 accounts at once."));
    }
    let handle = handle_of(&state, auth.account).await?;
    let friends: HashSet<u64> = crate::db::friend_ids(&state.db, auth.account)
        .await?
        .into_iter()
        .collect();
    // Inviting without a party makes one.
    let id = match party_of(&state, auth.account).await? {
        Some(party) => party.row.id,
        None => match make_party(&state, auth.account, handle.clone()).await {
            Ok(party) => party.row.id,
            // Another request of the same account got there first.
            Err(error) if error.body.error == ErrorCode::Limit => {
                party_of(&state, auth.account).await?.ok_or(error)?.row.id
            }
            Err(error) => return Err(error),
        },
    };

    let ttl = state.tuning.invitation_ttl;
    let mut tx = state.db.begin().await?;
    let party = party::lock_party(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !party.is_member(auth.account) {
        return Err(AppError::not_found());
    }
    if party.leader() != auth.account {
        return Err(AppError::forbidden("Only the leader can invite."));
    }
    if party.is_full() {
        return Err(AppError::new(ErrorCode::Full, "The party is full."));
    }
    let mut sent = Vec::new();
    let mut new = Vec::new();
    for to in request.to {
        let to_id = to.0;
        if sent.contains(&to) || !friends.contains(&to_id) || party.is_member(to_id) {
            continue;
        }
        if party::blocked_with_member(&mut tx, &id, to_id).await? {
            continue;
        }
        if party::is_invited(&mut tx, to_id, &id).await? {
            sent.push(to);
            continue;
        }
        let (open_for_party, held) = party::count_invitations(&mut tx, &id, to_id).await?;
        if open_for_party >= MAX_INVITATIONS_PER_PARTY || held >= MAX_INVITATIONS_PER_ACCOUNT {
            continue;
        }
        let invitation = InvitationRow {
            id: random_uuid(),
            party_id: id.clone(),
            from_id: auth.account as i64,
            from_handle: handle.clone(),
            to_id: to_id as i64,
            expires: (now() + ttl.as_secs()) as i64,
            deadline_ms: now_ms() + ttl.as_millis() as i64,
        };
        if party::insert_invitation(&mut tx, &invitation).await? {
            new.push(invitation);
        }
        sent.push(to);
    }
    tx.commit().await?;

    for invitation in new {
        state.news.push(
            invitation.to(),
            &Event::PartyInvitation {
                invitation: invitation.view(party.members.len() as u16),
            },
        );
    }
    Ok(Json(Invited { sent }))
}

/// The invitations the caller holds, for a client that comes back after a break.
pub async fn invitations(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Json<PartyInvitations>> {
    let mut conn = state.db.acquire().await?;
    let rows = party::invitations_of(&mut conn, auth.account, now_ms()).await?;
    let mut invitations = Vec::new();
    for row in rows {
        let members = party::party(&mut conn, &row.party_id)
            .await?
            .map_or(0, |p| p.members.len() as u16);
        invitations.push(row.view(members));
    }
    Ok(Json(PartyInvitations { invitations }))
}

pub async fn accept(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Party>> {
    let handle = handle_of(&state, auth.account).await?;
    let Some(row) =
        party::invitation(&mut *state.db.acquire().await?, &id, auth.account, now_ms()).await?
    else {
        return Err(AppError::not_found());
    };
    let mut tx = state.db.begin().await?;
    // The party row first, so the capacity below is read from a state nobody else is changing.
    let mut party = party::lock_party(&mut tx, &row.party_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if party::invitation(&mut tx, &id, auth.account, now_ms())
        .await?
        .is_none()
    {
        return Err(AppError::not_found());
    }
    if party::party_id_of(&mut tx, auth.account).await?.is_some() {
        return Err(AppError::limit(
            "You are in a party already. Leave it first.",
        ));
    }
    if party.is_full() {
        return Err(AppError::new(ErrorCode::Full, "The party is full."));
    }
    if party::blocked_with_member(&mut tx, &party.row.id, auth.account).await? {
        return Err(AppError::forbidden("You cannot join this party."));
    }
    let member = MemberRow::new(auth.account, handle, now() as i64);
    if !party::add_member(&mut tx, &party.row.id, &member).await? {
        return Err(AppError::limit(
            "You are in a party already. Leave it first.",
        ));
    }
    // The chat shows what is written from now on.
    let conversation = chat::lock_conversation(&mut tx, &party.conversation)
        .await?
        .ok_or_else(AppError::internal)?;
    chat::add_member(
        &mut tx,
        &party.conversation,
        auth.account,
        ACCEPTED,
        conversation.last(),
        now(),
    )
    .await?;
    // The account is in a party: its invitations, this one included, are used up.
    let used = party::take_held(&mut tx, auth.account).await?;
    tx.commit().await?;

    party.members.push(member);
    state.party_invitations_gone(&used);
    arm_grace(&state, auth.account).await?;
    state.announce_party(&party);
    Ok(Json(party.view()))
}

pub async fn decline(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let gone = party::take_declined(&mut *state.db.acquire().await?, &id, auth.account).await?;
    if gone.is_empty() {
        return Err(AppError::not_found());
    }
    state.party_invitations_gone(&gone);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn leave(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    let id = party::party_id_of(&mut *state.db.acquire().await?, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !state.remove_party_member(&id, auth.account).await? {
        return Err(AppError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn kick(
    State(state): State<AppState>,
    auth: Auth,
    Path(account): Path<String>,
) -> Result<StatusCode> {
    let target = parse_id(&account)?;
    let party = party_of(&state, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    if party.leader() != auth.account {
        return Err(AppError::forbidden("Only the leader can remove members."));
    }
    if target == auth.account {
        return Err(AppError::bad_request("Leave the party instead."));
    }
    if !party.is_member(target) {
        return Err(AppError::not_found());
    }
    state.remove_party_member(&party.row.id, target).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn update(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<UpdateParty>,
) -> Result<Json<Party>> {
    let id = party::party_id_of(&mut *state.db.acquire().await?, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let mut tx = state.db.begin().await?;
    let mut party = party::lock_party(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !party.is_member(auth.account) {
        return Err(AppError::not_found());
    }
    if party.leader() != auth.account {
        return Err(AppError::forbidden("Only the leader can change the party."));
    }
    if let Some(leader) = request.leader {
        if !party.is_member(leader.0) {
            return Err(AppError::bad_request("The new leader must be a member."));
        }
        party.row.leader = leader.0 as i64;
        party::set_leader(&mut tx, &id, leader.0).await?;
    }
    tx.commit().await?;
    state.announce_party(&party);
    Ok(Json(party.view()))
}

/// A member's net service says where it can be reached for voice. The revision rises and the
/// members hear of it only when something changed: the change is rolled back otherwise, which
/// takes the revision the lock gave it back too. Other nodes get the event as for any party
/// change: `announce_party` pushes it to every member, and the news layer carries it over NATS.
pub async fn set_endpoint(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<PartyEndpoint>,
) -> Result<Json<Party>> {
    if !is_endpoint(&request.endpoint) {
        return Err(AppError::bad_request(
            "The endpoint is 64 lowercase hex digits.",
        ));
    }
    if let Some(relay) = &request.relay
        && !(is_http_url(relay) && relay.starts_with("https://"))
    {
        return Err(AppError::bad_request(
            "The relay is an https URL of at most 255 characters.",
        ));
    }
    let id = party::party_id_of(&mut *state.db.acquire().await?, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let mut tx = state.db.begin().await?;
    let mut party = party::lock_party(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let Some(me) = party.member(auth.account) else {
        return Err(AppError::not_found());
    };
    if party
        .members
        .iter()
        .any(|m| m.account_id as u64 != auth.account && m.endpoint == request.endpoint)
    {
        return Err(AppError::limit(
            "Another member of the party has this endpoint.",
        ));
    }
    if me.endpoint == request.endpoint
        && me.relay == request.relay
        && (me.voice != 0) == request.voice
    {
        // Nothing changed: dropping the transaction takes the revision back too.
        drop(tx);
        party.row.revision -= 1;
        return Ok(Json(party.view()));
    }
    party::set_endpoint(
        &mut tx,
        &id,
        auth.account,
        &request.endpoint,
        request.relay.as_deref(),
        request.voice,
    )
    .await?;
    tx.commit().await?;
    if let Some(m) = party
        .members
        .iter_mut()
        .find(|m| m.account_id as u64 == auth.account)
    {
        m.endpoint = request.endpoint;
        m.relay = request.relay;
        m.voice = i64::from(request.voice);
    }
    state.announce_party(&party);
    Ok(Json(party.view()))
}

/// The leader invites every other member into one of its sessions.
pub async fn move_to_session(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<MoveParty>,
) -> Result<Json<Invited>> {
    let party = party_of(&state, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    if party.leader() != auth.account {
        return Err(AppError::forbidden(
            "Only the leader can move the party into a session.",
        ));
    }
    let handle = handle_of(&state, auth.account).await?;
    let others: Vec<AccountId> = party
        .member_ids()
        .into_iter()
        .filter(|m| *m != auth.account)
        .map(AccountId)
        .collect();

    let mut tx = state.db.begin().await?;
    let session = live::lock_session(&mut tx, &request.session)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !session.is_member(auth.account) {
        return Err(AppError::not_found());
    }
    if session.visibility() == Visibility::Closed {
        return Err(AppError::forbidden("This session is closed."));
    }
    // Party members need not be friends of the leader's friends; the invitation lets them in.
    let (sent, new) = sessions::open_invitations(
        &state,
        &mut tx,
        &session,
        (auth.account, &handle),
        others,
        |_| true,
    )
    .await?;
    tx.commit().await?;
    sessions::push_invitations(&state, new);
    Ok(Json(Invited { sent }))
}
