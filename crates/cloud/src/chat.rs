//! Conversations and messages in the store.
//!
//! Every message has a number that counts up by one in its conversation. A writer takes the next
//! number with an UPDATE of the conversation's row ([`lock_conversation`] first, then
//! [`next_seq`]) inside the transaction that stores the message. The row lock (PostgreSQL) or
//! the write lock (SQLite) is held until that transaction ends, so writers on any nodes take
//! turns, numbers are never used twice, and a message with a lower number is always committed
//! before one with a higher number. A client that has everything up to number N therefore misses
//! nothing by asking for what comes after N (`docs/adr/0013` in the network repository).
//!
//! A message can change after it was numbered: an edit, an unsend or a reaction takes the next
//! `rev` of the conversation the same way (`docs/adr/0030`), under the same lock, and stamps the
//! message with it. A client asks for the messages with a `rev` above the one it holds.
//!
//! Messages stay for a limited time and are deleted by [`sweep_retention`]
//! (`docs/adr/0011` in the network repository). Written once with `$1` parameters for SQLite and
//! Postgres, like [`crate::live`].

use anyplay_api::{
    Account, AccountId, Conversation, ConversationKind, MAX_QUOTE_CHARS, Message, MessageReaction,
    Quote, ReactionOn, ReadMark,
};
use sqlx::FromRow;

use crate::live::Conn;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

/// The member has not accepted the conversation: a message request.
pub const REQUEST: i64 = 0;
pub const ACCEPTED: i64 = 1;
/// The member declined; nothing more is shown to it, and the others are not told.
pub const DECLINED: i64 = 2;

pub fn kind_text(kind: ConversationKind) -> &'static str {
    match kind {
        ConversationKind::Direct => "direct",
        ConversationKind::Group => "group",
        ConversationKind::Party => "party",
    }
}

fn kind_from(text: &str) -> ConversationKind {
    match text {
        "direct" => ConversationKind::Direct,
        "party" => ConversationKind::Party,
        _ => ConversationKind::Group,
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct ConversationRow {
    pub id: String,
    pub kind: String,
    pub name: Option<String>,
    pub party_id: Option<String>,
    pub pair_low: Option<i64>,
    pub pair_high: Option<i64>,
    pub last_seq: i64,
    pub created: i64,
    pub updated_ms: i64,
    pub rev: i64,
}

impl ConversationRow {
    pub fn kind(&self) -> ConversationKind {
        kind_from(&self.kind)
    }

    pub fn last(&self) -> u64 {
        u(self.last_seq)
    }

    pub fn rev(&self) -> u64 {
        u(self.rev)
    }
}

const CONVERSATION_COLUMNS: &str =
    "id, kind, name, party_id, pair_low, pair_high, last_seq, created, updated_ms, rev";

#[derive(Clone, Debug, FromRow)]
pub struct MemberRow {
    pub account_id: i64,
    pub handle: String,
    pub state: i64,
    pub from_seq: i64,
    pub read_seq: i64,
    pub muted: i64,
    pub pinned: i64,
}

impl MemberRow {
    pub fn id(&self) -> u64 {
        u(self.account_id)
    }
}

pub async fn conversation(conn: &mut Conn, id: &str) -> sqlx::Result<Option<ConversationRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CONVERSATION_COLUMNS} FROM conversations WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
}

/// Starts a change to a conversation inside a transaction: touches its row, which keeps every
/// other change to it waiting until this transaction ends, and reads it.
pub async fn lock_conversation(conn: &mut Conn, id: &str) -> sqlx::Result<Option<ConversationRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE conversations SET last_seq = last_seq WHERE id = $1 RETURNING {CONVERSATION_COLUMNS}"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
}

/// Takes the next change number. Call it after [`lock_conversation`] in the transaction that
/// changes a message, and stamp the message with it ([`stamp_message`]).
pub async fn next_rev(conn: &mut Conn, id: &str) -> sqlx::Result<u64> {
    let (rev,): (i64,) =
        sqlx::query_as("UPDATE conversations SET rev = rev + 1 WHERE id = $1 RETURNING rev")
            .bind(id)
            .fetch_one(conn)
            .await?;
    Ok(u(rev))
}

/// Takes the next message number. Call it after [`lock_conversation`] in the transaction that
/// stores the message.
pub async fn next_seq(conn: &mut Conn, id: &str, now_ms: i64) -> sqlx::Result<u64> {
    let (seq,): (i64,) = sqlx::query_as(
        "UPDATE conversations SET last_seq = last_seq + 1, updated_ms = $2 WHERE id = $1 \
         RETURNING last_seq",
    )
    .bind(id)
    .bind(now_ms)
    .fetch_one(conn)
    .await?;
    Ok(u(seq))
}

pub async fn members(conn: &mut Conn, id: &str) -> sqlx::Result<Vec<MemberRow>> {
    sqlx::query_as(
        "SELECT m.account_id, a.handle, m.state, m.from_seq, m.read_seq, m.muted, m.pinned \
         FROM conversation_members m JOIN accounts a ON a.id = m.account_id \
         WHERE m.conversation_id = $1 ORDER BY m.joined, m.account_id",
    )
    .bind(id)
    .fetch_all(conn)
    .await
}

/// A new conversation. False for a direct conversation that exists already for the pair.
pub async fn insert_conversation(conn: &mut Conn, row: &ConversationRow) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO conversations (id, kind, name, party_id, pair_low, pair_high, last_seq, \
         created, updated_ms) VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $8) ON CONFLICT DO NOTHING",
    )
    .bind(&row.id)
    .bind(&row.kind)
    .bind(&row.name)
    .bind(&row.party_id)
    .bind(row.pair_low)
    .bind(row.pair_high)
    .bind(row.created)
    .bind(row.updated_ms)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn direct_between(conn: &mut Conn, a: u64, b: u64) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM conversations WHERE pair_low = $1 AND pair_high = $2")
            .bind(i(a.min(b)))
            .bind(i(a.max(b)))
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(id,)| id))
}

/// Adds a member who sees messages after `from_seq` only. Nothing happens when it is one already.
pub async fn add_member(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
    state: i64,
    from_seq: u64,
    now: u64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO conversation_members (conversation_id, account_id, state, from_seq, read_seq, joined) \
         VALUES ($1, $2, $3, $4, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(conversation)
    .bind(i(account))
    .bind(state)
    .bind(i(from_seq))
    .bind(i(now))
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn delete_member(conn: &mut Conn, conversation: &str, account: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND account_id = $2")
        .bind(conversation)
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(())
}

/// Changes a member's state. Moving to `ACCEPTED` from `DECLINED` starts the member's view at
/// `from_seq`, so what was sent while it had declined stays unseen.
pub async fn set_state(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
    state: i64,
    from_seq: Option<u64>,
) -> sqlx::Result<()> {
    match from_seq {
        Some(from) => {
            sqlx::query(
                "UPDATE conversation_members SET state = $3, from_seq = $4, read_seq = $4 \
                 WHERE conversation_id = $1 AND account_id = $2",
            )
            .bind(conversation)
            .bind(i(account))
            .bind(state)
            .bind(i(from))
            .execute(conn)
            .await?;
        }
        None => {
            sqlx::query(
                "UPDATE conversation_members SET state = $3 \
                 WHERE conversation_id = $1 AND account_id = $2",
            )
            .bind(conversation)
            .bind(i(account))
            .bind(state)
            .execute(conn)
            .await?;
        }
    }
    Ok(())
}

/// Moves a member's read mark forward, and with it the delivered mark, which is never below it.
/// False when the read mark was there already.
pub async fn set_read(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
    seq: u64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE conversation_members SET read_seq = $3, \
         delivered_seq = CASE WHEN delivered_seq < $3 THEN $3 ELSE delivered_seq END \
         WHERE conversation_id = $1 AND account_id = $2 AND read_seq < $3",
    )
    .bind(conversation)
    .bind(i(account))
    .bind(i(seq))
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Moves a member's delivered mark forward. False when it was there already.
pub async fn set_delivered(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
    seq: u64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE conversation_members SET delivered_seq = $3 \
         WHERE conversation_id = $1 AND account_id = $2 AND delivered_seq < $3",
    )
    .bind(conversation)
    .bind(i(account))
    .bind(i(seq))
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Sets what the member chose for itself; a `None` stays.
pub async fn set_settings(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
    muted: Option<bool>,
    pinned: Option<bool>,
) -> sqlx::Result<()> {
    if let Some(muted) = muted {
        sqlx::query(
            "UPDATE conversation_members SET muted = $3 WHERE conversation_id = $1 AND account_id = $2",
        )
        .bind(conversation)
        .bind(i(account))
        .bind(i64::from(muted))
        .execute(&mut *conn)
        .await?;
    }
    if let Some(pinned) = pinned {
        sqlx::query(
            "UPDATE conversation_members SET pinned = $3 WHERE conversation_id = $1 AND account_id = $2",
        )
        .bind(conversation)
        .bind(i(account))
        .bind(i64::from(pinned))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// How many conversations an account pinned.
pub async fn count_pinned(conn: &mut Conn, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM conversation_members WHERE account_id = $1 AND pinned = 1 AND state <> 2",
    )
    .bind(i(account))
    .fetch_one(conn)
    .await?;
    Ok(n)
}

pub async fn delete_conversation(conn: &mut Conn, id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    sender: u64,
    body: &str,
    created_ms: i64,
    reply: Option<u64>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO messages (conversation_id, seq, sender, body, created_ms, reply_seq) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(i(sender))
    .bind(body)
    .bind(created_ms)
    .bind(reply.map(i))
    .execute(conn)
    .await?;
    Ok(())
}

/// A message as it is stored, for the rules about changing it.
#[derive(Clone, Debug, FromRow)]
pub struct StoredMessage {
    pub sender: i64,
    pub body: String,
    pub created_ms: i64,
    pub deleted_ms: Option<i64>,
}

impl StoredMessage {
    pub fn sender(&self) -> u64 {
        u(self.sender)
    }

    pub fn deleted(&self) -> bool {
        self.deleted_ms.is_some()
    }
}

pub async fn stored_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
) -> sqlx::Result<Option<StoredMessage>> {
    sqlx::query_as(
        "SELECT sender, body, created_ms, deleted_ms FROM messages \
         WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(conversation)
    .bind(i(seq))
    .fetch_optional(conn)
    .await
}

/// The members a block kept this message from.
pub async fn hidden_from(conn: &mut Conn, conversation: &str, seq: u64) -> sqlx::Result<Vec<u64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT account_id FROM message_hidden WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(conversation)
    .bind(i(seq))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(a,)| u(a)).collect())
}

/// Marks a message as changed at `rev`.
pub async fn stamp_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    rev: u64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE messages SET rev = $3 WHERE conversation_id = $1 AND seq = $2")
        .bind(conversation)
        .bind(i(seq))
        .bind(i(rev))
        .execute(conn)
        .await?;
    Ok(())
}

/// Keeps the text a message is about to lose for a report (see [`held_text`]).
pub async fn hold_text(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    body: &str,
    now_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO message_holds (conversation_id, seq, body, held_ms) VALUES ($1, $2, $3, $4) \
         ON CONFLICT DO NOTHING",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(body)
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(())
}

/// Every held text of a message, oldest first, that was held no earlier than `not_before_ms`.
pub async fn held_texts(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    not_before_ms: i64,
) -> sqlx::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT body FROM message_holds WHERE conversation_id = $1 AND seq = $2 AND held_ms >= $3 \
         ORDER BY held_ms",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(not_before_ms)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(b,)| b).collect())
}

pub async fn edit_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    body: &str,
    now_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE messages SET body = $3, edited_ms = $4 WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(body)
    .bind(now_ms)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the text and the reactions of a message and marks it unsent.
pub async fn unsend_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    now_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE messages SET body = '', deleted_ms = $3 WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(now_ms)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM message_reactions WHERE conversation_id = $1 AND seq = $2")
        .bind(conversation)
        .bind(i(seq))
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn reaction_of(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    account: u64,
) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT reaction FROM message_reactions \
         WHERE conversation_id = $1 AND seq = $2 AND account_id = $3",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(i(account))
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(r,)| r))
}

/// Sets or, with `None`, takes back an account's reaction to a message.
pub async fn set_reaction(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    account: u64,
    reaction: Option<MessageReaction>,
    now_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM message_reactions WHERE conversation_id = $1 AND seq = $2 AND account_id = $3",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(i(account))
    .execute(&mut *conn)
    .await?;
    if let Some(reaction) = reaction {
        sqlx::query(
            "INSERT INTO message_reactions (conversation_id, seq, account_id, reaction, created_ms) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(conversation)
        .bind(i(seq))
        .bind(i(account))
        .bind(reaction.as_str())
        .bind(now_ms)
        .execute(conn)
        .await?;
    }
    Ok(())
}

/// Records that a block kept a message from an account.
pub async fn hide_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    account: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO message_hidden (conversation_id, seq, account_id) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(i(account))
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, FromRow)]
struct MessageRow {
    seq: i64,
    sender: i64,
    handle: String,
    body: String,
    created_ms: i64,
    edited_ms: Option<i64>,
    deleted_ms: Option<i64>,
    rev: i64,
    q_seq: Option<i64>,
    q_sender: Option<i64>,
    q_handle: Option<String>,
    q_body: Option<String>,
    q_deleted_ms: Option<i64>,
}

fn message(conversation: &str, row: MessageRow) -> Message {
    let reply = match (row.q_seq, row.q_sender, row.q_handle) {
        (Some(seq), Some(sender), Some(handle)) => {
            let deleted = row.q_deleted_ms.is_some();
            Some(Quote {
                seq: u(seq),
                sender: Account {
                    id: AccountId(u(sender)),
                    handle,
                },
                text: if deleted {
                    String::new()
                } else {
                    row.q_body
                        .unwrap_or_default()
                        .chars()
                        .take(MAX_QUOTE_CHARS)
                        .collect()
                },
                deleted,
            })
        }
        _ => None,
    };
    Message {
        conversation: conversation.to_owned(),
        seq: u(row.seq),
        sender: Account {
            id: AccountId(u(row.sender)),
            handle: row.handle,
        },
        text: row.body,
        created: u(row.created_ms / 1000),
        reply,
        edited: row.edited_ms.map(|t| u(t / 1000)),
        deleted: row.deleted_ms.is_some(),
        reactions: Vec::new(),
        rev: u(row.rev),
    }
}

/// The messages `viewer` may see: not kept away by a block, and not from before `$3`, where its
/// view starts. The quoted message of a reply is joined in under the same two rules.
const MESSAGE_SELECT: &str = "SELECT msg.seq, msg.sender, a.handle, msg.body, msg.created_ms, \
     msg.edited_ms, msg.deleted_ms, msg.rev, \
     q.seq AS q_seq, q.sender AS q_sender, qa.handle AS q_handle, q.body AS q_body, \
     q.deleted_ms AS q_deleted_ms \
     FROM messages msg JOIN accounts a ON a.id = msg.sender \
     LEFT JOIN messages q ON q.conversation_id = msg.conversation_id AND q.seq = msg.reply_seq \
     AND q.seq > $3 AND NOT EXISTS (SELECT 1 FROM message_hidden hq \
     WHERE hq.conversation_id = q.conversation_id AND hq.seq = q.seq AND hq.account_id = $2) \
     LEFT JOIN accounts qa ON qa.id = q.sender \
     WHERE msg.conversation_id = $1 AND msg.seq > $3 \
     AND NOT EXISTS (SELECT 1 FROM message_hidden h WHERE h.conversation_id = msg.conversation_id \
     AND h.seq = msg.seq AND h.account_id = $2)";

/// Which messages a fetch wants, besides the ones `viewer` may see.
enum Window {
    After(u64),
    Before(u64),
    ChangedAfter(u64),
}

async fn fetch(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    floor: u64,
    window: Window,
    limit: u32,
) -> sqlx::Result<Vec<Message>> {
    let (filter, bound) = match window {
        Window::After(n) => ("AND msg.seq > $4 ORDER BY msg.seq ASC", n),
        Window::Before(n) => ("AND msg.seq < $4 ORDER BY msg.seq DESC", n),
        Window::ChangedAfter(n) => ("AND msg.rev > $4 ORDER BY msg.rev ASC, msg.seq ASC", n),
    };
    let rows: Vec<MessageRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{MESSAGE_SELECT} {filter} LIMIT $5"
    )))
    .bind(conversation)
    .bind(i(viewer))
    .bind(i(floor))
    .bind(i(bound))
    .bind(i64::from(limit))
    .fetch_all(&mut *conn)
    .await?;
    let mut messages: Vec<Message> = rows.into_iter().map(|r| message(conversation, r)).collect();
    if matches!(window, Window::Before(_)) {
        messages.reverse();
    }
    add_reactions(conn, conversation, viewer, &mut messages).await?;
    Ok(messages)
}

/// Puts the reactions on the messages, leaving out those of accounts a block lies between the
/// viewer and.
async fn add_reactions(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    messages: &mut [Message],
) -> sqlx::Result<()> {
    let (Some(low), Some(high)) = (
        messages.iter().map(|m| m.seq).min(),
        messages.iter().map(|m| m.seq).max(),
    ) else {
        return Ok(());
    };
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT seq, account_id, reaction FROM message_reactions \
         WHERE conversation_id = $1 AND seq >= $2 AND seq <= $3 ORDER BY created_ms, account_id",
    )
    .bind(conversation)
    .bind(i(low))
    .bind(i(high))
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let blocked = blocked_members(conn, conversation, viewer).await?;
    for message in messages.iter_mut() {
        let of: Vec<&(i64, i64, String)> = rows
            .iter()
            .filter(|(seq, account, _)| u(*seq) == message.seq && !blocked.contains(&u(*account)))
            .collect();
        message.reactions = MessageReaction::ALL
            .into_iter()
            .filter_map(|reaction| {
                let accounts: Vec<AccountId> = of
                    .iter()
                    .filter(|(_, _, name)| name == reaction.as_str())
                    .map(|(_, account, _)| AccountId(u(*account)))
                    .collect();
                (!accounts.is_empty()).then_some(ReactionOn { reaction, accounts })
            })
            .collect();
    }
    Ok(())
}

/// The messages with a number above `above` that `viewer` may see, oldest first. `floor` is
/// where the viewer's view of the conversation starts.
pub async fn messages_after(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    floor: u64,
    above: u64,
    limit: u32,
) -> sqlx::Result<Vec<Message>> {
    fetch(
        conn,
        conversation,
        viewer,
        floor,
        Window::After(above.max(floor)),
        limit,
    )
    .await
}

/// The newest messages with a number below `below` that `viewer` may see and that come after
/// `floor`, oldest first.
pub async fn messages_before(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    floor: u64,
    below: u64,
    limit: u32,
) -> sqlx::Result<Vec<Message>> {
    fetch(
        conn,
        conversation,
        viewer,
        floor,
        Window::Before(below),
        limit,
    )
    .await
}

/// The messages that changed after `rev` and that `viewer` may see, in the order of the change.
pub async fn messages_changed(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    floor: u64,
    rev: u64,
    limit: u32,
) -> sqlx::Result<Vec<Message>> {
    fetch(
        conn,
        conversation,
        viewer,
        floor,
        Window::ChangedAfter(rev),
        limit,
    )
    .await
}

/// How many messages `sender` wrote in a conversation.
pub async fn count_from(conn: &mut Conn, conversation: &str, sender: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM messages WHERE conversation_id = $1 AND sender = $2")
            .bind(conversation)
            .bind(i(sender))
            .fetch_one(conn)
            .await?;
    Ok(n)
}

// Relations the conversation rules ask about

pub async fn account_exists(conn: &mut Conn, account: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
        .bind(i(account))
        .fetch_optional(conn)
        .await?;
    Ok(row.is_some())
}

pub async fn are_friends(conn: &mut Conn, a: u64, b: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM friends WHERE low = $1 AND high = $2")
        .bind(i(a.min(b)))
        .bind(i(a.max(b)))
        .fetch_optional(conn)
        .await?;
    Ok(row.is_some())
}

pub async fn has_blocked(conn: &mut Conn, blocker: u64, blocked: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM blocks WHERE blocker = $1 AND blocked = $2")
            .bind(i(blocker))
            .bind(i(blocked))
            .fetch_optional(conn)
            .await?;
    Ok(row.is_some())
}

/// True when either account blocked the other.
pub async fn blocked_either(conn: &mut Conn, a: u64, b: u64) -> sqlx::Result<bool> {
    Ok(has_blocked(conn, a, b).await? || has_blocked(conn, b, a).await?)
}

/// The members of a conversation that blocked `account` or that `account` blocked.
pub async fn blocked_members(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
) -> sqlx::Result<Vec<u64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT m.account_id FROM conversation_members m WHERE m.conversation_id = $1 \
         AND EXISTS (SELECT 1 FROM blocks b WHERE (b.blocker = $2 AND b.blocked = m.account_id) \
         OR (b.blocker = m.account_id AND b.blocked = $2))",
    )
    .bind(conversation)
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(a,)| u(a)).collect())
}

/// How many requests an account holds that it has neither accepted nor declined.
pub async fn count_requests(conn: &mut Conn, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM conversation_members WHERE account_id = $1 AND state = 0",
    )
    .bind(i(account))
    .fetch_one(conn)
    .await?;
    Ok(n)
}

/// How many group conversations an account is in.
pub async fn count_groups(conn: &mut Conn, account: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM conversation_members m JOIN conversations c ON c.id = m.conversation_id \
         WHERE m.account_id = $1 AND c.kind = 'group'",
    )
    .bind(i(account))
    .fetch_one(conn)
    .await?;
    Ok(n)
}

// Views

/// A conversation as an account sees it, before it is turned into the API's type.
#[derive(Clone, Debug, FromRow)]
struct Listed {
    id: String,
    kind: String,
    name: Option<String>,
    party_id: Option<String>,
    last_seq: i64,
    updated_ms: i64,
    read_seq: i64,
    /// 1 while the account has not accepted it and the other side is no friend.
    request: i64,
    muted: i64,
    pinned: i64,
    rev: i64,
    unread: i64,
}

#[derive(Clone, Debug, FromRow)]
struct Listed2 {
    conversation_id: String,
    account_id: i64,
    handle: String,
    state: i64,
    read_seq: i64,
    delivered_seq: i64,
}

/// `unread` counts the messages of others above the account's read mark that it can see and
/// that were not unsent.
const LISTED: &str = "SELECT c.id, c.kind, c.name, c.party_id, c.last_seq, c.updated_ms, \
     m.read_seq, \
     CASE WHEN m.state = 0 AND NOT EXISTS (SELECT 1 FROM friends f WHERE f.low = c.pair_low \
     AND f.high = c.pair_high) THEN 1 ELSE 0 END AS request, \
     m.muted, m.pinned, c.rev, \
     (SELECT COUNT(*) FROM messages u WHERE u.conversation_id = c.id AND u.seq > m.read_seq \
      AND u.sender <> m.account_id AND u.deleted_ms IS NULL \
      AND NOT EXISTS (SELECT 1 FROM message_hidden h WHERE h.conversation_id = u.conversation_id \
      AND h.seq = u.seq AND h.account_id = m.account_id)) AS unread \
     FROM conversation_members m JOIN conversations c ON c.id = m.conversation_id \
     WHERE m.account_id = $1 AND m.state <> 2";

const LISTED_MEMBERS: &str = "SELECT m.conversation_id, m.account_id, a.handle, m.state, \
     m.read_seq, m.delivered_seq \
     FROM conversation_members m JOIN accounts a ON a.id = m.account_id";

fn assemble(listed: Vec<Listed>, members: Vec<Listed2>, me: u64) -> Vec<Conversation> {
    listed
        .into_iter()
        .map(|c| {
            let of: Vec<&Listed2> = members
                .iter()
                .filter(|m| m.conversation_id == c.id)
                .collect();
            Conversation {
                kind: kind_from(&c.kind),
                name: c.name,
                members: of
                    .iter()
                    .map(|m| Account {
                        id: AccountId(u(m.account_id)),
                        handle: m.handle.clone(),
                    })
                    .collect(),
                last: u(c.last_seq),
                read: u(c.read_seq),
                marks: of
                    .iter()
                    .filter(|m| {
                        m.state == ACCEPTED
                            && u(m.account_id) != me
                            && (m.read_seq > 0 || m.delivered_seq > 0)
                    })
                    .map(|m| ReadMark {
                        account: AccountId(u(m.account_id)),
                        seq: u(m.read_seq),
                        delivered: u(m.delivered_seq),
                    })
                    .collect(),
                updated: u(c.updated_ms / 1000),
                request: c.request == 1,
                party: c.party_id,
                unread: u(c.unread),
                muted: c.muted == 1,
                pinned: c.pinned == 1,
                rev: u(c.rev),
                id: c.id,
            }
        })
        .collect()
}

/// Every conversation of an account that it has not declined, newest first. A request counts
/// once it holds a message.
pub async fn conversations_of(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<Conversation>> {
    let listed: Vec<Listed> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LISTED} ORDER BY m.pinned DESC, c.updated_ms DESC, c.id"
    )))
    .bind(i(account))
    .fetch_all(&mut *conn)
    .await?;
    let members: Vec<Listed2> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LISTED_MEMBERS} WHERE m.conversation_id IN \
         (SELECT conversation_id FROM conversation_members WHERE account_id = $1 AND state <> 2)"
    )))
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(assemble(listed, members, account)
        .into_iter()
        // A request with no message yet is not shown: the other side only opened it.
        .filter(|c| !c.request || c.last > 0)
        .collect())
}

/// One conversation as an account sees it; `None` when it is no member or declined.
pub async fn conversation_of(
    conn: &mut Conn,
    account: u64,
    id: &str,
) -> sqlx::Result<Option<Conversation>> {
    let listed: Vec<Listed> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{LISTED} AND c.id = $2")))
            .bind(i(account))
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    let members: Vec<Listed2> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LISTED_MEMBERS} WHERE m.conversation_id = $1"
    )))
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(assemble(listed, members, account).into_iter().next())
}

// Sweeps, deletion and export

/// The moment before which a held text is no longer readable.
pub fn hold_cut_ms() -> i64 {
    crate::util::now_ms() - (anyplay_api::REPORT_HOLD_SECS * 1000) as i64
}

/// Deletes messages older than `cut_ms`, a batch at a time, and returns how many went.
pub async fn sweep_retention(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<u64> {
    const BATCH: i64 = 1000;
    let mut total = 0;
    // Held texts go after a week (a hold also goes with its message).
    sqlx::query("DELETE FROM message_holds WHERE held_ms < $1")
        .bind(hold_cut_ms())
        .execute(&mut *conn)
        .await?;
    loop {
        let result = sqlx::query(
            "DELETE FROM messages WHERE (conversation_id, seq) IN \
             (SELECT conversation_id, seq FROM messages WHERE created_ms < $1 LIMIT $2)",
        )
        .bind(cut_ms)
        .bind(BATCH)
        .execute(&mut *conn)
        .await?;
        total += result.rows_affected();
        if (result.rows_affected() as i64) < BATCH {
            return Ok(total);
        }
    }
}

/// Deletes direct and group conversations that hold no message and were last touched before
/// `cut_ms`. A party's chat goes with its party.
pub async fn sweep_empty(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "DELETE FROM conversations WHERE kind <> 'party' AND updated_ms < $1 \
         AND NOT EXISTS (SELECT 1 FROM messages WHERE conversation_id = conversations.id)",
    )
    .bind(cut_ms)
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// The conversations an account shares with others, with the others in them.
pub async fn peers_of(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<(String, String, u64)>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT c.id, c.kind, o.account_id FROM conversation_members m \
         JOIN conversations c ON c.id = m.conversation_id \
         JOIN conversation_members o ON o.conversation_id = c.id AND o.account_id <> m.account_id \
         WHERE m.account_id = $1",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(c, k, a)| (c, k, u(a))).collect())
}

/// Deletes the direct conversations of an account, with their messages.
pub async fn delete_directs_of(conn: &mut Conn, account: u64) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM conversations WHERE kind = 'direct' AND (pair_low = $1 OR pair_high = $1)",
    )
    .bind(i(account))
    .execute(conn)
    .await?;
    Ok(())
}

/// What an account wrote, for the export.
pub async fn written_by(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<Written>> {
    let rows: Vec<WrittenRow> = sqlx::query_as(
        "SELECT conversation_id, seq, body, created_ms, edited_ms, deleted_ms, reply_seq \
         FROM messages WHERE sender = $1 ORDER BY created_ms, conversation_id, seq",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Written {
            conversation: r.conversation_id,
            seq: u(r.seq),
            text: r.body,
            created: u(r.created_ms / 1000),
            edited: r.edited_ms.map(|t| u(t / 1000)),
            deleted: r.deleted_ms.is_some(),
            reply: r.reply_seq.map(u),
        })
        .collect())
}

#[derive(Clone, Debug, FromRow)]
struct WrittenRow {
    conversation_id: String,
    seq: i64,
    body: String,
    created_ms: i64,
    edited_ms: Option<i64>,
    deleted_ms: Option<i64>,
    reply_seq: Option<i64>,
}

/// A message an account wrote, for the export. An unsent one has no text.
#[derive(Clone, Debug)]
pub struct Written {
    pub conversation: String,
    pub seq: u64,
    pub text: String,
    pub created: u64,
    pub edited: Option<u64>,
    pub deleted: bool,
    pub reply: Option<u64>,
}

/// The reactions an account set, for the export: conversation, message number, name.
pub async fn reactions_by(
    conn: &mut Conn,
    account: u64,
) -> sqlx::Result<Vec<(String, u64, String)>> {
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT conversation_id, seq, reaction FROM message_reactions WHERE account_id = $1 \
         ORDER BY created_ms, conversation_id, seq",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(c, s, r)| (c, u(s), r)).collect())
}
