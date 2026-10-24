//! Conversations and messages: one-to-one, group and party chats, message requests and read
//! marks. Messages are stored to be delivered ([ADR 0011] in the network repository) and
//! numbered per conversation inside the transaction that stores them (see `chat`). The events
//! are pointers: a client that hears `message` fetches the messages after the number it has.

use std::collections::HashMap;

use anyplay_api::{
    AccountId, AddToConversation, Added, Conversation, ConversationKind, Conversations,
    CreateConversation, ErrorCode, Event, MAX_CONVERSATION_NAME, MAX_GROUP_MEMBERS,
    MAX_MESSAGES_PAGE, MarkRead, Message, Messages, SendMessage, is_message_text,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::chat::{self, ACCEPTED, ConversationRow, DECLINED, REQUEST};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::live::Conn;
use crate::social::may_write_to_strangers;
use crate::state::AppState;
use crate::util::{now, now_ms, random_uuid};
use crate::{db, util};

/// Messages a stranger may send into a request before the receiver answers.
const MAX_UNANSWERED: i64 = 5;
/// Requests an account may hold, and groups it may be in.
const MAX_REQUESTS: i64 = 100;
const MAX_GROUPS: i64 = 50;
const DEFAULT_PAGE: u32 = 50;

async fn handle_of(state: &AppState, account: u64) -> Result<String> {
    Ok(db::account(&state.db, account)
        .await?
        .ok_or_else(AppError::not_found)?
        .handle)
}

fn check_name(name: &Option<String>) -> Result<()> {
    if let Some(name) = name
        && !util::is_label(name, MAX_CONVERSATION_NAME)
    {
        return Err(AppError::bad_request(
            "A group name has 1 to 64 printable characters.",
        ));
    }
    Ok(())
}

/// Tells each account.
fn push_all(state: &AppState, accounts: impl IntoIterator<Item = u64>, event: &Event) {
    for account in accounts {
        state.news.push(account, event);
    }
}

pub async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<Conversations>> {
    let all = chat::conversations_of(&mut *state.db.acquire().await?, auth.account).await?;
    let (requests, conversations) = all.into_iter().partition(|c| c.request);
    Ok(Json(Conversations {
        conversations,
        requests,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Conversation>> {
    let conversation = chat::conversation_of(&mut *state.db.acquire().await?, auth.account, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(conversation))
}

/// Opens a conversation: with one account the direct conversation (made when there is none),
/// with several a group.
pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<CreateConversation>,
) -> Result<Json<Conversation>> {
    let mut others: Vec<u64> = Vec::new();
    for account in &request.accounts {
        if account.0 == auth.account {
            return Err(AppError::bad_request("You are in it already."));
        }
        if !others.contains(&account.0) {
            others.push(account.0);
        }
    }
    check_name(&request.name)?;
    let id = match others.len() {
        0 => return Err(AppError::bad_request("Name at least one account.")),
        1 => open_direct(&state, auth.account, others[0]).await?,
        n if n < usize::from(MAX_GROUP_MEMBERS) => {
            open_group(&state, auth.account, others, request.name).await?
        }
        _ => {
            return Err(AppError::bad_request(
                "A group has at most 32 members, you included.",
            ));
        }
    };
    let conversation = chat::conversation_of(&mut *state.db.acquire().await?, auth.account, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok(Json(conversation))
}

async fn open_direct(state: &AppState, me: u64, other: u64) -> Result<String> {
    if db::account(&state.db, other).await?.is_none() {
        return Err(AppError::not_found());
    }
    let (friends, existing) = {
        let mut conn = state.db.acquire().await?;
        if chat::has_blocked(&mut conn, me, other).await? {
            return Err(AppError::forbidden(
                "You blocked this account. Unblock it first.",
            ));
        }
        (
            chat::are_friends(&mut conn, me, other).await?,
            chat::direct_between(&mut conn, me, other).await?,
        )
    };
    if existing.is_none() && !friends {
        if !may_write_to_strangers(state, me).await? {
            return Err(AppError::forbidden(
                "A new account cannot write to people who are not its friends yet.",
            ));
        }
        let rates = &state.tuning.rates;
        state
            .limiter
            .hit(
                &format!("strangers:{me}"),
                rates.strangers,
                rates.strangers_window,
            )
            .map_err(AppError::rate_limited)?;
        if chat::count_requests(&mut *state.db.acquire().await?, other).await? >= MAX_REQUESTS {
            return Err(AppError::limit("This account has too many open requests."));
        }
    }

    let created = now();
    let row = ConversationRow {
        id: random_uuid(),
        kind: chat::kind_text(ConversationKind::Direct).to_owned(),
        name: None,
        party_id: None,
        pair_low: Some(me.min(other) as i64),
        pair_high: Some(me.max(other) as i64),
        last_seq: 0,
        created: created as i64,
        updated_ms: now_ms(),
    };
    let mut tx = state.db.begin().await?;
    // The insert is the first write: when the pair has a conversation already, it does nothing.
    if chat::insert_conversation(&mut tx, &row).await? {
        chat::add_member(&mut tx, &row.id, me, ACCEPTED, 0, created).await?;
        let theirs = if chat::are_friends(&mut tx, me, other).await? {
            ACCEPTED
        } else {
            REQUEST
        };
        chat::add_member(&mut tx, &row.id, other, theirs, 0, created).await?;
        tx.commit().await?;
        return Ok(row.id);
    }
    let id = chat::direct_between(&mut tx, me, other)
        .await?
        .ok_or_else(AppError::internal)?;
    // Opening a conversation accepts it. What was sent while it was declined stays unseen.
    let conversation = chat::lock_conversation(&mut tx, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    let members = chat::members(&mut tx, &id).await?;
    if let Some(mine) = members.iter().find(|m| m.id() == me)
        && mine.state != ACCEPTED
    {
        let from = (mine.state == DECLINED).then(|| conversation.last());
        chat::set_state(&mut tx, &id, me, ACCEPTED, from).await?;
    }
    tx.commit().await?;
    Ok(id)
}

async fn open_group(
    state: &AppState,
    me: u64,
    others: Vec<u64>,
    name: Option<String>,
) -> Result<String> {
    let created = now();
    let row = ConversationRow {
        id: random_uuid(),
        kind: chat::kind_text(ConversationKind::Group).to_owned(),
        name,
        party_id: None,
        pair_low: None,
        pair_high: None,
        last_seq: 0,
        created: created as i64,
        updated_ms: now_ms(),
    };
    let mut tx = state.db.begin().await?;
    chat::insert_conversation(&mut tx, &row).await?;
    if chat::count_groups(&mut tx, me).await? >= MAX_GROUPS {
        return Err(AppError::limit("You are in the most groups allowed."));
    }
    for other in &others {
        if !chat::account_exists(&mut tx, *other).await? {
            return Err(AppError::not_found());
        }
        if !chat::are_friends(&mut tx, me, *other).await? {
            return Err(AppError::forbidden("You can add friends only."));
        }
        if chat::blocked_either(&mut tx, me, *other).await? {
            return Err(AppError::forbidden("You cannot add this account."));
        }
        if chat::count_groups(&mut tx, *other).await? >= MAX_GROUPS {
            return Err(AppError::limit(
                "An account you named is in the most groups allowed.",
            ));
        }
    }
    chat::add_member(&mut tx, &row.id, me, ACCEPTED, 0, created).await?;
    for other in &others {
        chat::add_member(&mut tx, &row.id, *other, ACCEPTED, 0, created).await?;
    }
    tx.commit().await?;
    push_all(
        state,
        others.iter().copied().chain([me]),
        &Event::Conversation { id: row.id.clone() },
    );
    Ok(row.id)
}

pub async fn add_members(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<AddToConversation>,
) -> Result<Json<Added>> {
    if request.accounts.len() >= usize::from(MAX_GROUP_MEMBERS) {
        return Err(AppError::bad_request("Add at most 31 accounts at once."));
    }
    let mut tx = state.db.begin().await?;
    let conversation = chat::lock_conversation(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let mut members = chat::members(&mut tx, &id).await?;
    if !members
        .iter()
        .any(|m| m.id() == auth.account && m.state == ACCEPTED)
    {
        return Err(AppError::not_found());
    }
    if conversation.kind() != ConversationKind::Group {
        return Err(AppError::bad_request("Only groups take new members."));
    }
    if members.len() >= usize::from(MAX_GROUP_MEMBERS) {
        return Err(AppError::new(ErrorCode::Full, "The group is full."));
    }
    let mut added = Vec::new();
    for account in request.accounts {
        let other = account.0;
        if members.len() + added.len() >= usize::from(MAX_GROUP_MEMBERS)
            || added.contains(&account)
            || members.iter().any(|m| m.id() == other)
            || !chat::are_friends(&mut tx, auth.account, other).await?
            || !chat::blocked_members(&mut tx, &id, other).await?.is_empty()
            || chat::count_groups(&mut tx, other).await? >= MAX_GROUPS
        {
            continue;
        }
        if chat::add_member(&mut tx, &id, other, ACCEPTED, conversation.last(), now()).await? {
            added.push(account);
        }
    }
    tx.commit().await?;

    if !added.is_empty() {
        members = chat::members(&mut *state.db.acquire().await?, &id).await?;
        push_all(
            &state,
            members.iter().map(|m| m.id()),
            &Event::Conversation { id },
        );
    }
    Ok(Json(Added { added }))
}

/// Reads the caller's membership: its conversation, all members and its own row. A declined
/// member sees nothing.
async fn membership(
    conn: &mut Conn,
    id: &str,
    account: u64,
) -> Result<(ConversationRow, Vec<chat::MemberRow>, chat::MemberRow)> {
    let conversation = chat::conversation(conn, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let members = chat::members(conn, id).await?;
    let me = members
        .iter()
        .find(|m| m.id() == account && m.state != DECLINED)
        .cloned()
        .ok_or_else(AppError::not_found)?;
    Ok((conversation, members, me))
}

pub async fn send(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<SendMessage>,
) -> Result<Json<Message>> {
    if !is_message_text(&request.text) {
        return Err(AppError::bad_request(
            "A message has 1 to 2000 characters and is not only white space.",
        ));
    }
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("messages:{}", auth.account),
            rates.messages,
            rates.messages_window,
        )
        .map_err(AppError::rate_limited)?;
    let handle = handle_of(&state, auth.account).await?;
    let created_ms = now_ms();

    let mut tx = state.db.begin().await?;
    // The row first: writers on every node take turns here, and the number below is theirs alone.
    let conversation = chat::lock_conversation(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let members = chat::members(&mut tx, &id).await?;
    let me = members
        .iter()
        .find(|m| m.id() == auth.account && m.state != DECLINED)
        .ok_or_else(AppError::not_found)?;
    let mut states: HashMap<u64, i64> = members.iter().map(|m| (m.id(), m.state)).collect();
    if conversation.kind() == ConversationKind::Direct {
        let other = members
            .iter()
            .find(|m| m.id() != auth.account)
            .ok_or_else(AppError::not_found)?;
        if chat::has_blocked(&mut tx, auth.account, other.id()).await? {
            return Err(AppError::forbidden(
                "You blocked this account. Unblock it first.",
            ));
        }
        // Answering a request accepts it, and a friendship turns any request into a chat.
        if me.state == REQUEST {
            chat::set_state(&mut tx, &id, auth.account, ACCEPTED, None).await?;
            states.insert(auth.account, ACCEPTED);
        }
        if other.state == REQUEST && chat::are_friends(&mut tx, auth.account, other.id()).await? {
            chat::set_state(&mut tx, &id, other.id(), ACCEPTED, None).await?;
            states.insert(other.id(), ACCEPTED);
        }
        if states.get(&other.id()) == Some(&REQUEST)
            && chat::count_from(&mut tx, &id, auth.account).await? >= MAX_UNANSWERED
        {
            return Err(AppError::limit("Wait for an answer before you write more."));
        }
    }
    let seq = chat::next_seq(&mut tx, &id, created_ms).await?;
    chat::insert_message(&mut tx, &id, seq, auth.account, &request.text, created_ms).await?;
    chat::set_read(&mut tx, &id, auth.account, seq).await?;
    // A block in either direction keeps the message from the other side for good.
    let blocked = chat::blocked_members(&mut tx, &id, auth.account).await?;
    for account in &blocked {
        chat::hide_message(&mut tx, &id, seq, *account).await?;
    }
    tx.commit().await?;

    let pointer = Event::Message {
        conversation: id.clone(),
        seq,
    };
    for (account, member_state) in states {
        if account == auth.account {
            state.news.push(account, &pointer);
        } else if blocked.contains(&account) {
            continue;
        } else if member_state == ACCEPTED {
            state.news.push(account, &pointer);
        } else if member_state == REQUEST && seq == 1 {
            // Told once; the rest wait in the request.
            state.news.push(
                account,
                &Event::MessageRequest {
                    conversation: id.clone(),
                },
            );
        }
    }
    Ok(Json(Message {
        conversation: id,
        seq,
        sender: anyplay_api::Account {
            id: AccountId(auth.account),
            handle,
        },
        text: request.text,
        created: (created_ms / 1000) as u64,
    }))
}

#[derive(Deserialize)]
pub struct MessagesQuery {
    after: Option<u64>,
    before: Option<u64>,
    limit: Option<u32>,
}

/// The messages after a number, before one, or the newest, at most 100 of them. A member sees
/// the messages from when it joined, minus the ones a block kept from it.
pub async fn messages(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    query: Result<Query<MessagesQuery>, QueryRejection>,
) -> Result<Json<Messages>> {
    let Query(query) = query.map_err(|_| AppError::bad_request("The query is not valid."))?;
    if query.after.is_some() && query.before.is_some() {
        return Err(AppError::bad_request("Give after or before, not both."));
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE)
        .clamp(1, MAX_MESSAGES_PAGE);
    let mut conn = state.db.acquire().await?;
    let (conversation, _, me) = membership(&mut conn, &id, auth.account).await?;
    let floor = me.from_seq.max(0) as u64;
    let messages = match query.before {
        Some(before) => {
            chat::messages_before(&mut conn, &id, auth.account, before, floor, limit).await?
        }
        None if query.after.is_some() => {
            let above = query.after.unwrap_or(0).max(floor);
            chat::messages_after(&mut conn, &id, auth.account, above, limit).await?
        }
        None => {
            let below = conversation.last().saturating_add(1);
            chat::messages_before(&mut conn, &id, auth.account, below, floor, limit).await?
        }
    };
    Ok(Json(Messages {
        messages,
        last: conversation.last(),
    }))
}

pub async fn mark_read(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<MarkRead>,
) -> Result<StatusCode> {
    let mut conn = state.db.acquire().await?;
    let (conversation, members, me) = membership(&mut conn, &id, auth.account).await?;
    let seq = request.seq.min(conversation.last());
    if chat::set_read(&mut conn, &id, auth.account, seq).await? {
        let event = Event::Read {
            conversation: id,
            account: AccountId(auth.account),
            seq,
        };
        // A member who has not accepted the conversation keeps its marks to its own devices.
        if me.state == ACCEPTED {
            let told = members
                .iter()
                .filter(|m| m.state == ACCEPTED)
                .map(|m| m.id());
            push_all(&state, told, &event);
        } else {
            state.news.push(auth.account, &event);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn accept(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Conversation>> {
    let mut tx = state.db.begin().await?;
    chat::lock_conversation(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let members = chat::members(&mut tx, &id).await?;
    let me = members
        .iter()
        .find(|m| m.id() == auth.account && m.state != DECLINED)
        .ok_or_else(AppError::not_found)?;
    if me.state == REQUEST {
        chat::set_state(&mut tx, &id, auth.account, ACCEPTED, None).await?;
    }
    tx.commit().await?;
    state
        .news
        .push(auth.account, &Event::Conversation { id: id.clone() });
    let conversation = chat::conversation_of(&mut *state.db.acquire().await?, auth.account, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok(Json(conversation))
}

/// Leaves a group, or declines a message request. A conversation with a friend stays, and a
/// party's chat ends with the party.
pub async fn leave(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let mut tx = state.db.begin().await?;
    let conversation = chat::lock_conversation(&mut tx, &id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let members = chat::members(&mut tx, &id).await?;
    let me = members
        .iter()
        .find(|m| m.id() == auth.account && m.state != DECLINED)
        .ok_or_else(AppError::not_found)?;
    let mut remaining: Vec<u64> = Vec::new();
    match conversation.kind() {
        ConversationKind::Party => {
            return Err(AppError::bad_request("Leave the party instead."));
        }
        ConversationKind::Direct => {
            let other = members
                .iter()
                .find(|m| m.id() != auth.account)
                .ok_or_else(AppError::not_found)?;
            let request = me.state == REQUEST
                && !chat::are_friends(&mut tx, auth.account, other.id()).await?;
            if !request {
                return Err(AppError::bad_request(
                    "A conversation with a friend cannot be left.",
                ));
            }
            chat::set_state(&mut tx, &id, auth.account, DECLINED, None).await?;
        }
        ConversationKind::Group => {
            chat::delete_member(&mut tx, &id, auth.account).await?;
            remaining = members
                .iter()
                .map(|m| m.id())
                .filter(|m| *m != auth.account)
                .collect();
            if remaining.is_empty() {
                chat::delete_conversation(&mut tx, &id).await?;
            }
        }
    }
    tx.commit().await?;

    state
        .news
        .push(auth.account, &Event::ConversationGone { id: id.clone() });
    push_all(&state, remaining, &Event::Conversation { id });
    Ok(StatusCode::NO_CONTENT)
}
