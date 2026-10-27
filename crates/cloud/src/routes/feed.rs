//! The feed: a page of items and the reactions to them (network spec/cloud-api.md, "Feed").

use std::collections::HashSet;

use anyplay_api::{
    Account, DEFAULT_FEED_PAGE, Event, Feed, FeedEvent, FeedItem, MAX_FEED_PAGE, Part,
    RARITY_MIN_PLAYERS, ReactionRequest, Reactions,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::db;
use crate::db::feed::{ItemRow, Rarities};
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::routes::accounts::account_of;
use crate::state::AppState;
use crate::util::now;
use crate::visibility::{Relation, viewing_many};

#[derive(Deserialize)]
pub struct FeedQuery {
    before: Option<u64>,
    limit: Option<u32>,
}

/// `GET /v1/feed`: the caller's own items and their friends', newest first, and the party items
/// that name the caller.
pub async fn page(
    State(state): State<AppState>,
    auth: Auth,
    query: Result<Query<FeedQuery>, QueryRejection>,
) -> Result<Json<Feed>> {
    let Query(query) =
        query.map_err(|_| AppError::bad_request("`before` and `limit` are whole numbers."))?;
    let limit = match query.limit {
        None => DEFAULT_FEED_PAGE,
        Some(0) => return Err(AppError::bad_request("`limit` is at least 1.")),
        Some(n) => n.min(MAX_FEED_PAGE),
    } as usize;
    let before = query.before.unwrap_or(i64::MAX as u64);
    let me = auth.account;
    let ask = limit as i64 + 1;

    let mut conn = state.db.acquire().await?;
    let friends: Vec<u64> = db::friends_of(&mut *conn, me)
        .await?
        .iter()
        .map(db::Related::id)
        .collect();
    let seen = viewing_many(&mut conn, me, &friends).await?;
    let mut owners = vec![me];
    owners.extend(friends.iter().copied().filter(|id| {
        let seen = &seen[id];
        seen.relation() == Relation::Friend && seen.can(Part::Feed)
    }));
    let mut rows = db::feed::items_of_owners(&mut conn, &owners, before, ask).await?;
    rows.extend(db::feed::party_items_of(&mut conn, me, before, ask).await?);
    rows.sort_by_key(|row| std::cmp::Reverse(row.id));
    rows.truncate(limit + 1);
    let next = (rows.len() > limit).then(|| rows[limit - 1].id());
    rows.truncate(limit);

    // A party item of an account that blocked the caller, or that they blocked, is not shown.
    let party_owners: Vec<u64> = rows
        .iter()
        .filter(|row| row.is_party() && row.owner() != me)
        .map(ItemRow::owner)
        .collect();
    let blocked = db::blocked_among(&mut conn, me, &party_owners).await?;
    rows.retain(|row| !(row.is_party() && blocked.contains(&row.owner())));
    keep_visible(&mut conn, me, &mut rows).await?;

    let items = build(&mut conn, me, rows).await?;
    Ok(Json(Feed { items, next }))
}

/// Drops what the viewer may not see of the items: a trophy item needs the owner's trophies to be
/// visible to them and the trophy not to be secret, a first-played item the owner's playtime, and
/// a friend item must not name an account that blocked the viewer or that they blocked. The
/// owner's own items are always theirs; party items are decided elsewhere.
async fn keep_visible(
    conn: &mut sqlx::AnyConnection,
    me: u64,
    rows: &mut Vec<ItemRow>,
) -> Result<()> {
    let owners: Vec<u64> = rows
        .iter()
        .filter(|row| !row.is_party())
        .map(ItemRow::owner)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let seen = viewing_many(conn, me, &owners).await?;
    let others: Vec<u64> = rows
        .iter()
        .filter_map(|row| row.other_account.map(|a| a as u64))
        .collect();
    let blocked = db::blocked_among(conn, me, &others).await?;
    let mut secrets: std::collections::HashMap<(u64, String), Vec<i64>> = Default::default();
    let mut keep = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        let owner = row.owner();
        let ok = if row.is_party() || owner == me && row.kind != "friend" {
            true
        } else {
            let seen = &seen[&owner];
            match row.kind.as_str() {
                "trophy" => {
                    let (Some(title), Some(number)) = (&row.title_id, row.trophy_number) else {
                        keep.push(false);
                        continue;
                    };
                    if !seen.can(Part::Trophies) {
                        false
                    } else if owner == me {
                        true
                    } else {
                        let key = (owner, title.clone());
                        if !secrets.contains_key(&key) {
                            let hidden = db::feed::hidden_numbers(conn, owner, title).await?;
                            secrets.insert(key.clone(), hidden);
                        }
                        !secrets[&key].contains(&number)
                    }
                }
                "first_played" => seen.can(Part::Playtime),
                "friend" => !row
                    .other_account
                    .is_some_and(|a| blocked.contains(&(a as u64))),
                _ => true,
            }
        };
        keep.push(ok);
    }
    let mut keep = keep.into_iter();
    rows.retain(|_| keep.next().unwrap_or(false));
    Ok(())
}

/// The items as the API shows them, with the accounts, reactions and rarities read for all of
/// them at once.
async fn build(
    conn: &mut sqlx::AnyConnection,
    me: u64,
    rows: Vec<ItemRow>,
) -> Result<Vec<FeedItem>> {
    let mut ids = HashSet::new();
    let mut titles = HashSet::new();
    for row in &rows {
        ids.insert(row.owner());
        ids.extend(row.other_account.map(|a| a as u64));
        ids.extend(row.member_ids());
        if row.kind == "trophy" {
            titles.extend(row.title_id.clone());
        }
    }
    let ids: Vec<u64> = ids.into_iter().collect();
    let titles: Vec<String> = titles.into_iter().collect();
    let accounts = db::accounts_by_ids(conn, &ids).await?;
    let item_ids: Vec<u64> = rows.iter().map(ItemRow::id).collect();
    let (counts, mine) = db::feed::reactions_of(conn, &item_ids, me).await?;
    let rarities = db::feed::rarities_of(conn, &titles).await?;
    let account = |id: u64| -> Option<Account> { accounts.get(&id).map(account_of) };

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(owner) = account(row.owner()) else {
            continue;
        };
        let event = match row.kind.as_str() {
            "trophy" => {
                let (Some(title_id), Some(number), Some(grade)) = (
                    row.title_id.clone(),
                    row.trophy_number.and_then(|n| u16::try_from(n).ok()),
                    row.grade.as_deref().and_then(db::grade_of),
                ) else {
                    continue;
                };
                FeedEvent::Trophy {
                    rarity: rarity(&rarities, &title_id, number),
                    title_id,
                    number,
                    grade,
                }
            }
            "friend" => match row.other_account.and_then(|a| account(a as u64)) {
                Some(with) => FeedEvent::Friend { with },
                None => continue,
            },
            "first_played" => match row.title_id.clone() {
                Some(title_id) => FeedEvent::FirstPlayed { title_id },
                None => continue,
            },
            "party" => FeedEvent::Party {
                // A member whose account is gone is left out.
                members: row.member_ids().into_iter().filter_map(account).collect(),
            },
            _ => continue,
        };
        items.push(FeedItem {
            id: row.id(),
            account: owner,
            at: row.created as u64,
            event,
            reactions: counts.get(&row.id()).copied().unwrap_or_default(),
            mine: mine.get(&row.id()).copied(),
        });
    }
    Ok(items)
}

/// The share of the title's players who earned the trophy, in percent with one decimal; `None`
/// below [`RARITY_MIN_PLAYERS`] players, as for the rarity request.
fn rarity(rarities: &Rarities, title_id: &str, number: u16) -> Option<f32> {
    let players = *rarities.players.get(title_id)?;
    if players < i64::from(RARITY_MIN_PLAYERS) {
        return None;
    }
    let earned = rarities
        .earned
        .get(&(title_id.to_owned(), i64::from(number)))
        .copied()
        .unwrap_or(0);
    Some(((earned as f64 / players as f64 * 1000.0).round() / 10.0) as f32)
}

/// The item, when `me` may see it: the same rule as the page. An item that does not exist and one
/// that may not be seen are the same answer.
async fn visible_item(state: &AppState, me: u64, id: u64) -> Result<ItemRow> {
    let mut conn = state.db.acquire().await?;
    let row = db::feed::item(&mut conn, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let owner = row.owner();
    let seen = viewing_many(&mut conn, me, &[owner]).await?[&owner];
    let allowed = match seen.relation() {
        Relation::Blocked => false,
        Relation::Own => true,
        // A party item belongs to its members, whatever the leader chose.
        _ if row.is_party() => db::feed::is_member(&mut conn, id, me).await?,
        Relation::Friend => seen.can(Part::Feed),
        Relation::Stranger => false,
    };
    let mut rows = vec![row];
    if allowed {
        keep_visible(&mut conn, me, &mut rows).await?;
    } else {
        rows.clear();
    }
    rows.pop().ok_or_else(AppError::not_found)
}

fn limit_reactions(state: &AppState, me: u64) -> Result<()> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("feed-reaction:{me}"),
            rates.feed_reactions,
            rates.feed_reaction_window,
        )
        .map_err(AppError::rate_limited)
}

async fn reactions(state: &AppState, me: u64, id: u64) -> Result<Json<Reactions>> {
    let mut conn = state.db.acquire().await?;
    let (mut counts, mut mine) = db::feed::reactions_of(&mut conn, &[id], me).await?;
    Ok(Json(Reactions {
        reactions: counts.remove(&id).unwrap_or_default(),
        mine: mine.remove(&id),
    }))
}

/// `PUT /v1/feed/{id}/reaction`
pub async fn react(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<ReactionRequest>,
) -> Result<Json<Reactions>> {
    let id = parse_id(&id)?;
    limit_reactions(&state, auth.account)?;
    let item = visible_item(&state, auth.account, id).await?;
    let changed = db::feed::set_reaction(
        &state.writer,
        &state.db,
        id,
        auth.account,
        request.reaction,
        now(),
    )
    .await?;
    if changed && item.owner() != auth.account {
        state.news.push(item.owner(), &Event::Feed);
    }
    reactions(&state, auth.account, id).await
}

/// `DELETE /v1/feed/{id}/reaction`
pub async fn unreact(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Reactions>> {
    let id = parse_id(&id)?;
    limit_reactions(&state, auth.account)?;
    let item = visible_item(&state, auth.account, id).await?;
    let changed = db::feed::clear_reaction(&state.db, id, auth.account).await?;
    if changed && item.owner() != auth.account {
        state.news.push(item.owner(), &Event::Feed);
    }
    reactions(&state, auth.account, id).await
}
