//! What the node does around parties and messages beyond one request: telling members, removing
//! a member from a party, forgetting an account's conversations, and the sweeps (invitations
//! that ran out, members gone for too long, messages past their retention).
//!
//! Like sessions, these changes are written to the store first and announced after they commit.
//! News is a pointer or a versioned state: a party carries its revision, a message only its
//! conversation and number, and what a client misses it reads again after `welcome`.

use anyplay_api::{AccountId, CallEnd, Event};

use crate::chat;
use crate::error::AppError;
use crate::party::{self, Gone, PartyState};
use crate::state::Inner;
use crate::util::now;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Missed calls are kept this many days.
const MISSED_CALL_DAYS: i64 = 30;

/// Why party invitations ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// Joined, a block, or the party ended for another reason: nothing more to tell.
    Used,
    /// The callee declined.
    Declined,
    /// The time ran out.
    Expired,
    /// The caller left the party, or the party ended, before the call was answered.
    Cancelled,
}

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

    /// Tells the recipients of party invitations that ended, and settles the calls among them: a
    /// call that rang out or was hung up becomes a missed call for the callee, and the caller
    /// hears that a call rang out or was declined.
    pub async fn end_invitations(&self, gone: &[Gone], why: Ended) -> Result<(), AppError> {
        self.party_invitations_gone(gone);
        for invitation in gone.iter().filter(|g| g.call != 0) {
            let (callee, caller) = (invitation.to(), invitation.from());
            if matches!(why, Ended::Expired | Ended::Cancelled) {
                let at = now() as i64;
                let mut conn = self.db.acquire().await?;
                party::record_missed(&mut conn, callee, caller, &invitation.from_handle, at)
                    .await?;
                drop(conn);
                self.news.push(
                    callee,
                    &Event::MissedCall {
                        call: anyplay_api::MissedCall {
                            from: anyplay_api::Account {
                                id: AccountId(caller),
                                handle: invitation.from_handle.clone(),
                            },
                            at: at as u64,
                        },
                    },
                );
            }
            let reason = match why {
                Ended::Expired => Some(CallEnd::NoAnswer),
                Ended::Declined => Some(CallEnd::Declined),
                Ended::Used | Ended::Cancelled => None,
            };
            if let Some(reason) = reason {
                self.news.push(
                    caller,
                    &Event::CallEnded {
                        account: AccountId(callee),
                        reason,
                    },
                );
            }
        }
        Ok(())
    }

    /// Forgets the missed calls between two accounts that are no longer friends or blocked one
    /// another, and tells the ones who held them.
    pub async fn forget_missed_between(&self, a: u64, b: u64) -> Result<(), AppError> {
        let gone = party::take_missed_between(&mut *self.db.acquire().await?, a, b).await?;
        for (to, from) in gone {
            self.news.push(
                to,
                &Event::MissedCallGone {
                    account: AccountId(from),
                },
            );
        }
        Ok(())
    }

    /// Takes a member out of a party and out of its chat. A leaving leader hands over to the
    /// longest-standing member; the last member ends the party, with its invitations and chat.
    /// Returns whether the account was a member.
    pub async fn remove_party_member(&self, id: &str, account: u64) -> Result<bool, AppError> {
        self.remove_party_member_by(id, account, None).await
    }

    /// [`Self::remove_party_member`] for `by`, the leader who removes another member: the leader
    /// check is made under the party's lock.
    pub async fn remove_party_member_by(
        &self,
        id: &str,
        account: u64,
        by: Option<u64>,
    ) -> Result<bool, AppError> {
        let mut tx = self.begin_write().await?;
        let Some(mut party) = party::lock_party(&mut tx, id).await? else {
            return if by.is_some() {
                Err(AppError::not_found())
            } else {
                Ok(false)
            };
        };
        if let Some(by) = by {
            if !party.is_member(by) {
                return Err(AppError::not_found());
            }
            if party.leader() != by {
                return Err(AppError::forbidden("Only the leader can remove members."));
            }
            if account == by {
                return Err(AppError::bad_request("Leave the party instead."));
            }
        }
        if !party.is_member(account) {
            return if by.is_some() {
                Err(AppError::not_found())
            } else {
                Ok(false)
            };
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
        self.end_invitations(&gone, Ended::Cancelled).await?;
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
        // What the account sent was hung up; what it held it will not answer.
        let (sent, held): (Vec<Gone>, Vec<Gone>) =
            gone.into_iter().partition(|g| g.from() == account);
        self.end_invitations(&sent, Ended::Cancelled).await?;
        self.end_invitations(&held, Ended::Used).await?;
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
        self.end_invitations(&gone, Ended::Expired).await?;
        for account in leaving {
            self.leave_party(account).await?;
        }
        let keep = i64::try_from(self.config.message_retention_days)
            .unwrap_or(i64::MAX / DAY_MS)
            .saturating_mul(DAY_MS);
        let cut = now_ms.saturating_sub(keep);
        let mut conn = crate::db::acquire_maintenance(&self.db).await?;
        party::sweep_missed(
            &mut conn,
            now_ms.saturating_sub(MISSED_CALL_DAYS * DAY_MS) / 1000,
        )
        .await?;
        chat::sweep_retention(&mut conn, cut).await?;
        chat::sweep_empty(&mut conn, cut).await?;
        crate::lfg::sweep(&mut conn, now_ms.saturating_sub(DAY_MS)).await?;
        drop(conn);
        self.sweep_media(now_ms).await?;
        self.sweep_feed().await?;
        Ok(())
    }
}
