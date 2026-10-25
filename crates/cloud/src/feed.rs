//! Recording the feed: what the cloud notices and writes down as items, and the news it sends
//! about them. Every hook here runs after the request it hangs on has done its work and never
//! fails it: an error is logged and dropped.

use std::collections::HashSet;

use anyplay_api::{Audience, Event, Trophy};

use crate::db;
use crate::db::feed::{NewItem, record};
use crate::party::PartyState;
use crate::state::Inner;
use crate::util::now;

/// The most trophy items one upload makes.
pub const MAX_TROPHY_ITEMS: usize = 5;
/// A trophy makes an item only when it was earned this recently (seconds): uploading the history
/// of a game fills no feed.
pub const FRESH: u64 = 24 * 60 * 60;

impl Inner {
    /// Trophies that are new in a stored upload: at most [`MAX_TROPHY_ITEMS`] of those earned in
    /// the last day, in the order given.
    pub async fn record_trophies(&self, account: u64, title_id: &str, new: &[Trophy]) {
        let at = now();
        let fresh: Vec<(u64, NewItem)> = new
            .iter()
            .filter(|t| t.earned >= at.saturating_sub(FRESH))
            .take(MAX_TROPHY_ITEMS)
            .map(|t| {
                (
                    account,
                    NewItem::Trophy {
                        title_id: title_id.to_owned(),
                        number: t.number,
                        grade: t.grade,
                    },
                )
            })
            .collect();
        if fresh.is_empty() {
            return;
        }
        let result = async {
            if self.feed_is_off(account).await? {
                return Ok(());
            }
            self.record_and_tell(&fresh, &[account], &[]).await
        }
        .await;
        log("trophy", result);
    }

    /// Two accounts became friends: an item for each.
    pub async fn record_friends(&self, a: u64, b: u64) {
        let result = async {
            let mut items = Vec::new();
            for (account, with) in [(a, b), (b, a)] {
                if !self.feed_is_off(account).await? {
                    items.push((account, NewItem::Friend { with }));
                }
            }
            self.record_and_tell(&items, &[a, b], &[]).await
        }
        .await;
        log("friend", result);
    }

    /// The account's presence shows `title_id`. The first time ever (for this title) makes an
    /// item; the title is remembered even while the account shows its feed to nobody.
    pub async fn record_first_played(&self, account: u64, title_id: &str) {
        if !anyplay_api::is_title_id(title_id) {
            return;
        }
        let result = async {
            // Nothing is made or marked while the account appears offline: the item comes the
            // first time the title is played in plain sight.
            if db::appear_offline(&self.db, account).await? {
                return Ok(());
            }
            if !db::feed::mark_first(&self.db, account, title_id, now()).await? {
                return Ok(());
            }
            if self.feed_is_off(account).await? {
                return Ok(());
            }
            let item = (
                account,
                NewItem::FirstPlayed {
                    title_id: title_id.to_owned(),
                },
            );
            self.record_and_tell(&[item], &[account], &[]).await
        }
        .await;
        log("first played", result);
    }

    /// A party got its second member. Only the members see the item, so the leader's setting does
    /// not matter.
    pub async fn record_party(&self, party: &PartyState) {
        let leader = party.leader();
        let mut members = vec![leader];
        members.extend(party.member_ids().into_iter().filter(|m| *m != leader));
        let item = (
            leader,
            NewItem::Party {
                members: members.clone(),
            },
        );
        let result = self.record_and_tell(&[item], &[], &members).await;
        log("party", result);
    }

    /// True while the account shows its feed to nobody: nothing of it is recorded.
    async fn feed_is_off(&self, account: u64) -> sqlx::Result<bool> {
        Ok(db::privacy_of(&self.db, account).await?.feed == Audience::Nobody)
    }

    /// Writes the items and, when any was recorded, tells the friends of `owners` and the
    /// `also` accounts that the feed has news.
    async fn record_and_tell(
        &self,
        items: &[(u64, NewItem)],
        owners: &[u64],
        also: &[u64],
    ) -> sqlx::Result<()> {
        let recorded = record(
            &self.writer,
            &self.db,
            now(),
            self.tuning.feed_items_per_day,
            items,
        )
        .await?;
        if recorded == 0 {
            return Ok(());
        }
        let mut tell: HashSet<u64> = also.iter().copied().collect();
        for owner in owners {
            tell.extend(db::friend_ids(&self.db, *owner).await?);
        }
        for account in tell {
            self.news.push(account, &Event::Feed);
        }
        Ok(())
    }

    /// Deletes the items older than `feed_retention_days`; their reactions go with them.
    pub async fn sweep_feed(&self) -> sqlx::Result<()> {
        let keep = self.config.feed_retention_days.saturating_mul(24 * 60 * 60);
        db::feed::sweep(&self.db, now().saturating_sub(keep)).await?;
        Ok(())
    }
}

fn log(what: &str, result: sqlx::Result<()>) {
    if let Err(error) = result {
        tracing::warn!(%error, "cannot record a {what} item in the feed");
    }
}
