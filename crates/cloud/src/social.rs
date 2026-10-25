//! What the node does around parties and messages beyond one request: telling members, removing
//! a member from a party, forgetting an account's conversations, and the sweeps (invitations
//! that ran out, members gone for too long, messages past their retention).
//!
//! Like sessions, these changes are written to the store first and announced after they commit.
//! News is a pointer or a versioned state: a party carries its revision, a message only its
//! conversation and number, and what a client misses it reads again after `welcome`.

use anyplay_api::Event;

use crate::chat;
use crate::error::AppError;
use crate::party::{self, Gone, PartyState};
use crate::state::Inner;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

impl Inner {
    /// Sends a party as it is now to all its members.
    pub fn announce_party(&self, party: &PartyState) {
        let event = Event::Party {
            party: party.view(),
        };
        for member in party.member_ids() {
            self.news.push(member, &event);
        }
    }

    /// Tells the recipients of party invitations that ended.
    pub fn party_invitations_gone(&self, gone: &[Gone]) {
        for invitation in gone {
            self.news.push(
                invitation.to(),
                &Event::PartyInvitationGone {
                    id: invitation.id.clone(),
                },
            );
        }
    }

    /// Takes a member out of a party and out of its chat. A leaving leader hands over to the
    /// longest-standing member; the last member ends the party, with its invitations and chat.
    /// Returns whether the account was a member.
    pub async fn remove_party_member(&self, id: &str, account: u64) -> Result<bool, AppError> {
        let mut tx = self.begin_write().await?;
        let Some(mut party) = party::lock_party(&mut tx, id).await? else {
            return Ok(false);
        };
        if !party.is_member(account) {
            return Ok(false);
        }
        party::delete_member(&mut tx, id, account).await?;
        chat::delete_member(&mut tx, &party.conversation, account).await?;
        party.members.retain(|m| m.account_id as u64 != account);
        let gone = match party.members.first() {
            None => {
                let gone = party::take_of_party(&mut tx, id).await?;
                party::delete_party(&mut tx, id).await?;
                gone
            }
            Some(first) => {
                if party.leader() == account {
                    party.row.leader = first.account_id;
                    party::set_leader(&mut tx, id, first.account_id as u64).await?;
                }
                Vec::new()
            }
        };
        tx.commit().await?;

        self.news
            .push(account, &Event::PartyGone { id: id.to_owned() });
        self.party_invitations_gone(&gone);
        if !party.members.is_empty() {
            self.announce_party(&party);
        }
        Ok(true)
    }

    /// Takes an account out of its party, if it is in one, and drops the party invitations it
    /// sent or holds.
    pub async fn leave_party(&self, account: u64) -> Result<(), AppError> {
        let id = party::party_id_of(&mut *self.db.acquire().await?, account).await?;
        if let Some(id) = id {
            self.remove_party_member(&id, account).await?;
        }
        let gone = party::take_of_account(&mut *self.db.acquire().await?, account).await?;
        self.party_invitations_gone(&gone);
        Ok(())
    }

    /// Deletes what an account wrote and the conversations that cannot go on without it: its
    /// direct conversations go whole, its messages in groups go, and the other members hear of
    /// it. Call it before the account itself is deleted; the account's rows go with it.
    pub async fn forget_conversations(&self, account: u64) -> Result<(), AppError> {
        let peers = chat::peers_of(&mut *self.db.acquire().await?, account).await?;
        chat::delete_directs_of(&mut *self.db.acquire().await?, account).await?;
        sqlx::query("DELETE FROM messages WHERE sender = $1")
            .bind(account as i64)
            .execute(&self.db)
            .await?;
        for (conversation, kind, other) in peers {
            let event = if kind == "direct" {
                Event::ConversationGone { id: conversation }
            } else {
                Event::Conversation { id: conversation }
            };
            self.news.push(other, &event);
        }
        Ok(())
    }

    /// The sweep of parties and messages, run with every other sweep on every node. Each step is
    /// one conditional statement, so it takes effect once however many nodes run it.
    pub async fn sweep_social(&self, now_ms: i64) -> Result<(), AppError> {
        let (gone, leaving) = {
            let mut conn = self.db.acquire().await?;
            let gone = party::sweep_expired(&mut conn, now_ms).await?;
            let leaving = party::due_to_leave(
                &mut conn,
                now_ms - self.tuning.party_grace.as_millis() as i64,
            )
            .await?;
            (gone, leaving)
        };
        self.party_invitations_gone(&gone);
        for account in leaving {
            self.leave_party(account).await?;
        }
        let keep = i64::try_from(self.config.message_retention_days)
            .unwrap_or(i64::MAX / DAY_MS)
            .saturating_mul(DAY_MS);
        let cut = now_ms.saturating_sub(keep);
        let mut conn = self.db.acquire().await?;
        chat::sweep_retention(&mut conn, cut).await?;
        chat::sweep_empty(&mut conn, cut).await?;
        drop(conn);
        self.sweep_feed().await?;
        Ok(())
    }
}
