//! Parties in the store: up to 8 friends who stay together across games, with one chat.
//!
//! A party belongs to no game. Like a session it is one row that every change touches first
//! ([`lock_party`]), which holds a lock in both databases until the transaction ends and gives
//! the change its revision. The unique `account_id` of `party_members` is what keeps an account
//! in one party at a time on every node. The party's chat is a conversation that goes with the
//! party ([`crate::chat`]). Written once with `$1` parameters for SQLite and Postgres, like
//! [`crate::live`].

use anyplay_api::{Account, AccountId, MAX_PARTY_MEMBERS, Party, PartyInvitation, PartyMember};
use sqlx::FromRow;

use crate::live::Conn;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

#[derive(Clone, Debug, FromRow)]
pub struct PartyRow {
    pub id: String,
    pub leader: i64,
    pub created: i64,
    pub revision: i64,
}

#[derive(Clone, Debug, FromRow)]
pub struct MemberRow {
    pub account_id: i64,
    pub handle: String,
    pub joined: i64,
}

/// A party and its members, longest-standing first.
#[derive(Clone, Debug)]
pub struct PartyState {
    pub row: PartyRow,
    pub members: Vec<MemberRow>,
    /// The party's chat.
    pub conversation: String,
}

impl PartyState {
    pub fn leader(&self) -> u64 {
        u(self.row.leader)
    }

    pub fn member_ids(&self) -> Vec<u64> {
        self.members.iter().map(|m| u(m.account_id)).collect()
    }

    pub fn is_member(&self, account: u64) -> bool {
        self.members.iter().any(|m| u(m.account_id) == account)
    }

    pub fn is_full(&self) -> bool {
        self.members.len() >= usize::from(MAX_PARTY_MEMBERS)
    }

    pub fn view(&self) -> Party {
        Party {
            id: self.row.id.clone(),
            leader: AccountId(self.leader()),
            members: self
                .members
                .iter()
                .map(|m| PartyMember {
                    account: Account {
                        id: AccountId(u(m.account_id)),
                        handle: m.handle.clone(),
                    },
                    joined: u(m.joined),
                })
                .collect(),
            max_members: MAX_PARTY_MEMBERS,
            conversation: self.conversation.clone(),
            created: u(self.row.created),
            revision: u(self.row.revision),
        }
    }
}

const PARTY_COLUMNS: &str = "id, leader, created, revision";

async fn members_of(conn: &mut Conn, id: &str) -> sqlx::Result<Vec<MemberRow>> {
    sqlx::query_as(
        "SELECT account_id, handle, joined FROM party_members WHERE party_id = $1 ORDER BY seq",
    )
    .bind(id)
    .fetch_all(conn)
    .await
}

async fn conversation_of(conn: &mut Conn, id: &str) -> sqlx::Result<String> {
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM conversations WHERE party_id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(|(id,)| id).unwrap_or_default())
}

async fn state_of(conn: &mut Conn, row: PartyRow) -> sqlx::Result<PartyState> {
    let members = members_of(conn, &row.id).await?;
    let conversation = conversation_of(conn, &row.id).await?;
    Ok(PartyState {
        row,
        members,
        conversation,
    })
}

/// The ID of the party an account is in.
pub async fn party_id_of(conn: &mut Conn, account: u64) -> sqlx::Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT party_id FROM party_members WHERE account_id = $1")
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(id,)| id))
}

/// A party as it is now, for reading only.
pub async fn party(conn: &mut Conn, id: &str) -> sqlx::Result<Option<PartyState>> {
    let row: Option<PartyRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PARTY_COLUMNS} FROM parties WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(row) => Ok(Some(state_of(conn, row).await?)),
        None => Ok(None),
    }
}

/// Starts a change to a party inside a transaction: touches its row, which keeps every other
/// change to it waiting until this transaction ends, and reads it. The revision is the new one.
pub async fn lock_party(conn: &mut Conn, id: &str) -> sqlx::Result<Option<PartyState>> {
    let row: Option<PartyRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE parties SET revision = revision + 1 WHERE id = $1 RETURNING {PARTY_COLUMNS}"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(row) => Ok(Some(state_of(conn, row).await?)),
        None => Ok(None),
    }
}

pub async fn insert_party(conn: &mut Conn, row: &PartyRow) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO parties (id, leader, created, revision) VALUES ($1, $2, $3, 0)")
        .bind(&row.id)
        .bind(row.leader)
        .bind(row.created)
        .execute(conn)
        .await?;
    Ok(())
}

/// Adds a member after the longest-standing ones. False when the account is in a party already:
/// the unique account decides, whichever node got there first.
pub async fn add_member(conn: &mut Conn, party: &str, member: &MemberRow) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO party_members (party_id, account_id, seq, handle, joined) \
         VALUES ($1, $2, (SELECT COALESCE(MAX(seq), 0) + 1 FROM party_members WHERE party_id = $1), \
         $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(party)
    .bind(member.account_id)
    .bind(&member.handle)
    .bind(member.joined)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn delete_member(conn: &mut Conn, party: &str, account: u64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM party_members WHERE party_id = $1 AND account_id = $2")
        .bind(party)
        .bind(i(account))
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn set_leader(conn: &mut Conn, party: &str, leader: u64) -> sqlx::Result<()> {
    sqlx::query("UPDATE parties SET leader = $2 WHERE id = $1")
        .bind(party)
        .bind(i(leader))
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes a party with its invitations and its chat. Take the invitations first to tell their
/// recipients.
pub async fn delete_party(conn: &mut Conn, party: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM parties WHERE id = $1")
        .bind(party)
        .execute(conn)
        .await?;
    Ok(())
}

/// True when `account` blocked a member of the party or a member blocked it.
pub async fn blocked_with_member(conn: &mut Conn, party: &str, account: u64) -> sqlx::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM party_members m WHERE m.party_id = $1 AND EXISTS          (SELECT 1 FROM blocks b WHERE (b.blocker = $2 AND b.blocked = m.account_id)          OR (b.blocker = m.account_id AND b.blocked = $2)) LIMIT 1",
    )
    .bind(party)
    .bind(i(account))
    .fetch_optional(conn)
    .await?;
    Ok(row.is_some())
}

/// Accounts in a party whose grace ended before `cut_ms` without a connection.
pub async fn due_to_leave(conn: &mut Conn, cut_ms: i64) -> sqlx::Result<Vec<u64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT m.account_id FROM party_members m \
         JOIN presence p ON p.account_id = m.account_id \
         WHERE p.last_seen_ms IS NOT NULL AND p.last_seen_ms <= $1 \
         AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = m.account_id)",
    )
    .bind(cut_ms)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(a,)| u(a)).collect())
}

// Invitations

#[derive(Clone, Debug, FromRow)]
pub struct InvitationRow {
    pub id: String,
    pub party_id: String,
    pub from_id: i64,
    pub from_handle: String,
    pub to_id: i64,
    pub expires: i64,
    pub deadline_ms: i64,
}

impl InvitationRow {
    pub fn to(&self) -> u64 {
        u(self.to_id)
    }

    pub fn view(&self, members: u16) -> PartyInvitation {
        PartyInvitation {
            id: self.id.clone(),
            from: Account {
                id: AccountId(u(self.from_id)),
                handle: self.from_handle.clone(),
            },
            party: self.party_id.clone(),
            members,
            expires: u(self.expires),
        }
    }
}

/// An invitation that ended: its ID and the account that held it.
#[derive(Clone, Debug, FromRow)]
pub struct Gone {
    pub id: String,
    pub to_id: i64,
}

impl Gone {
    pub fn to(&self) -> u64 {
        u(self.to_id)
    }
}

const INVITATION_COLUMNS: &str = "id, party_id, from_id, from_handle, to_id, expires, deadline_ms";

/// Inserts an invitation. False when the account holds one for this party already.
pub async fn insert_invitation(conn: &mut Conn, row: &InvitationRow) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO party_invitations (id, party_id, from_id, from_handle, to_id, expires, deadline_ms) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
    )
    .bind(&row.id)
    .bind(&row.party_id)
    .bind(row.from_id)
    .bind(&row.from_handle)
    .bind(row.to_id)
    .bind(row.expires)
    .bind(row.deadline_ms)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The invitation `id` if `to` holds it and it has not run out at `now_ms`.
pub async fn invitation(
    conn: &mut Conn,
    id: &str,
    to: u64,
    now_ms: i64,
) -> sqlx::Result<Option<InvitationRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INVITATION_COLUMNS} FROM party_invitations \
         WHERE id = $1 AND to_id = $2 AND deadline_ms > $3"
    )))
    .bind(id)
    .bind(i(to))
    .bind(now_ms)
    .fetch_optional(conn)
    .await
}

/// The invitations an account holds, newest first.
pub async fn invitations_of(
    conn: &mut Conn,
    to: u64,
    now_ms: i64,
) -> sqlx::Result<Vec<InvitationRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INVITATION_COLUMNS} FROM party_invitations \
         WHERE to_id = $1 AND deadline_ms > $2 ORDER BY deadline_ms DESC"
    )))
    .bind(i(to))
    .bind(now_ms)
    .fetch_all(conn)
    .await
}

/// Open invitations of a party, and those an account holds.
pub async fn count_invitations(conn: &mut Conn, party: &str, to: u64) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM party_invitations WHERE party_id = $1), \
         (SELECT COUNT(*) FROM party_invitations WHERE to_id = $2)",
    )
    .bind(party)
    .bind(i(to))
    .fetch_one(conn)
    .await
}

/// True when `to` holds an invitation to the party.
pub async fn is_invited(conn: &mut Conn, to: u64, party: &str) -> sqlx::Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM party_invitations WHERE to_id = $1 AND party_id = $2")
            .bind(i(to))
            .bind(party)
            .fetch_optional(conn)
            .await?;
    Ok(row.is_some())
}

/// Removes every invitation an account holds: it is in a party now.
pub async fn take_held(conn: &mut Conn, to: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM party_invitations WHERE to_id = $1 RETURNING id, to_id")
        .bind(i(to))
        .fetch_all(conn)
        .await
}

/// Removes every invitation of a party.
pub async fn take_of_party(conn: &mut Conn, party: &str) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM party_invitations WHERE party_id = $1 RETURNING id, to_id")
        .bind(party)
        .fetch_all(conn)
        .await
}

/// Removes the invitations between two accounts, in both directions.
pub async fn take_between(conn: &mut Conn, a: u64, b: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as(
        "DELETE FROM party_invitations WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1) \
         RETURNING id, to_id",
    )
    .bind(i(a))
    .bind(i(b))
    .fetch_all(conn)
    .await
}

/// Removes every invitation an account sent or holds.
pub async fn take_of_account(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as(
        "DELETE FROM party_invitations WHERE from_id = $1 OR to_id = $1 RETURNING id, to_id",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await
}

/// Removes an invitation its recipient declines. Empty when it is not theirs.
pub async fn take_declined(conn: &mut Conn, id: &str, to: u64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM party_invitations WHERE id = $1 AND to_id = $2 RETURNING id, to_id")
        .bind(id)
        .bind(i(to))
        .fetch_all(conn)
        .await
}

/// Removes the invitations that ran out.
pub async fn sweep_expired(conn: &mut Conn, now_ms: i64) -> sqlx::Result<Vec<Gone>> {
    sqlx::query_as("DELETE FROM party_invitations WHERE deadline_ms <= $1 RETURNING id, to_id")
        .bind(now_ms)
        .fetch_all(conn)
        .await
}
