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
//! Messages stay for a limited time and are deleted by [`sweep_retention`]
//! (`docs/adr/0011` in the network repository). Written once with `$1` parameters for SQLite and
//! Postgres, like [`crate::live`].

use anyplay_api::{Account, AccountId, Conversation, ConversationKind, Message, ReadMark};
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
}

impl ConversationRow {
    pub fn kind(&self) -> ConversationKind {
        kind_from(&self.kind)
    }

    pub fn last(&self) -> u64 {
        u(self.last_seq)
    }
}

const CONVERSATION_COLUMNS: &str =
    "id, kind, name, party_id, pair_low, pair_high, last_seq, created, updated_ms";

#[derive(Clone, Debug, FromRow)]
pub struct MemberRow {
    pub account_id: i64,
    pub handle: String,
    pub state: i64,
    pub from_seq: i64,
    pub read_seq: i64,
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
        "SELECT m.account_id, a.handle, m.state, m.from_seq, m.read_seq \
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

/// Moves a member's read mark forward. False when it was there already.
pub async fn set_read(
    conn: &mut Conn,
    conversation: &str,
    account: u64,
    seq: u64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE conversation_members SET read_seq = $3 \
         WHERE conversation_id = $1 AND account_id = $2 AND read_seq < $3",
    )
    .bind(conversation)
    .bind(i(account))
    .bind(i(seq))
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
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
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO messages (conversation_id, seq, sender, body, created_ms) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(i(sender))
    .bind(body)
    .bind(created_ms)
    .execute(conn)
    .await?;
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
}

fn message(conversation: &str, row: MessageRow) -> Message {
    Message {
        conversation: conversation.to_owned(),
        seq: u(row.seq),
        sender: Account {
            id: AccountId(u(row.sender)),
            handle: row.handle,
        },
        text: row.body,
        created: u(row.created_ms / 1000),
    }
}

const MESSAGE_SELECT: &str = "SELECT msg.seq, msg.sender, a.handle, msg.body, msg.created_ms \
     FROM messages msg JOIN accounts a ON a.id = msg.sender \
     WHERE msg.conversation_id = $1 \
     AND NOT EXISTS (SELECT 1 FROM message_hidden h WHERE h.conversation_id = msg.conversation_id \
     AND h.seq = msg.seq AND h.account_id = $2)";

/// The messages with a number above `above` that `viewer` may see, oldest first. `from_seq` is
/// where the viewer's view of the conversation starts.
pub async fn messages_after(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    above: u64,
    limit: u32,
) -> sqlx::Result<Vec<Message>> {
    let rows: Vec<MessageRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{MESSAGE_SELECT} AND msg.seq > $3 ORDER BY msg.seq ASC LIMIT $4"
    )))
    .bind(conversation)
    .bind(i(viewer))
    .bind(i(above))
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| message(conversation, r)).collect())
}

/// The newest messages with a number below `below` that `viewer` may see and that come after
/// `above`, oldest first.
pub async fn messages_before(
    conn: &mut Conn,
    conversation: &str,
    viewer: u64,
    below: u64,
    above: u64,
    limit: u32,
) -> sqlx::Result<Vec<Message>> {
    let rows: Vec<MessageRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{MESSAGE_SELECT} AND msg.seq < $3 AND msg.seq > $4 ORDER BY msg.seq DESC LIMIT $5"
    )))
    .bind(conversation)
    .bind(i(viewer))
    .bind(i(below))
    .bind(i(above))
    .bind(i64::from(limit))
    .fetch_all(conn)
    .await?;
    let mut messages: Vec<Message> = rows.into_iter().map(|r| message(conversation, r)).collect();
    messages.reverse();
    Ok(messages)
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
}

#[derive(Clone, Debug, FromRow)]
struct Listed2 {
    conversation_id: String,
    account_id: i64,
    handle: String,
    state: i64,
    read_seq: i64,
}

const LISTED: &str = "SELECT c.id, c.kind, c.name, c.party_id, c.last_seq, c.updated_ms, \
     m.read_seq, \
     CASE WHEN m.state = 0 AND NOT EXISTS (SELECT 1 FROM friends f WHERE f.low = c.pair_low \
     AND f.high = c.pair_high) THEN 1 ELSE 0 END AS request \
     FROM conversation_members m JOIN conversations c ON c.id = m.conversation_id \
     WHERE m.account_id = $1 AND m.state <> 2";

const LISTED_MEMBERS: &str = "SELECT m.conversation_id, m.account_id, a.handle, m.state, m.read_seq \
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
                    .filter(|m| m.state == ACCEPTED && u(m.account_id) != me && m.read_seq > 0)
                    .map(|m| ReadMark {
                        account: AccountId(u(m.account_id)),
                        seq: u(m.read_seq),
                    })
                    .collect(),
                updated: u(c.updated_ms / 1000),
                request: c.request == 1,
                party: c.party_id,
                id: c.id,
            }
        })
        .collect()
}

/// Every conversation of an account that it has not declined, newest first. A request counts
/// once it holds a message.
pub async fn conversations_of(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<Conversation>> {
    let listed: Vec<Listed> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LISTED} ORDER BY c.updated_ms DESC, c.id"
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

/// Deletes messages older than `cut_ms`, a batch at a time, and returns how many went.
pub async fn sweep_retention(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<u64> {
    const BATCH: i64 = 1000;
    let mut total = 0;
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
pub async fn written_by(
    conn: &mut Conn,
    account: u64,
) -> sqlx::Result<Vec<(String, u64, String, u64)>> {
    let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT conversation_id, seq, body, created_ms FROM messages WHERE sender = $1 \
         ORDER BY created_ms, conversation_id, seq",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(c, s, b, t)| (c, u(s), b, u(t / 1000)))
        .collect())
}
