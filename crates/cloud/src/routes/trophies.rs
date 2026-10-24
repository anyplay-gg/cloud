//! Trophy records, their rarity, gamer cards and what a player shows.

use std::collections::BTreeSet;

use anyplay_api::{
    AccountTrophies, Card, Cards, MAX_CARDS, MAX_TROPHY_NUMBER, MAX_TROPHY_UPLOAD, Part, Privacy,
    RARITY_MIN_PLAYERS, Rarity, TitleTrophies, TrophiesAdded, TrophyRarity, TrophyUpload,
    is_title_id,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::routes::accounts::account_of;
use crate::state::AppState;
use crate::util::now;
use crate::visibility::{may_see, viewing_many};

/// A trophy may not be earned later than this many seconds from now.
const FUTURE_SLACK: u64 = 24 * 60 * 60;

fn title_id(text: &str) -> Result<&str> {
    if is_title_id(text) {
        Ok(text)
    } else {
        Err(AppError::bad_request("That is not a title ID."))
    }
}

pub async fn upload(
    State(state): State<AppState>,
    auth: Auth,
    Path(title): Path<String>,
    ApiJson(upload): ApiJson<TrophyUpload>,
) -> Result<Json<TrophiesAdded>> {
    let title = title_id(&title)?;
    if upload.trophies.len() > MAX_TROPHY_UPLOAD {
        return Err(AppError::too_large(format!(
            "One upload carries at most {MAX_TROPHY_UPLOAD} trophies."
        )));
    }
    let latest = now() + FUTURE_SLACK;
    for trophy in &upload.trophies {
        if trophy.number >= MAX_TROPHY_NUMBER {
            return Err(AppError::bad_request(format!(
                "Trophy numbers are below {MAX_TROPHY_NUMBER}."
            )));
        }
        if trophy.earned > latest {
            return Err(AppError::bad_request(
                "A trophy cannot be earned in the future.",
            ));
        }
    }
    let added = db::add_trophies(
        &state.writer,
        &state.db,
        auth.account,
        title,
        &upload.trophies,
    )
    .await?;
    Ok(Json(TrophiesAdded {
        added: u32::try_from(added).unwrap_or(u32::MAX),
    }))
}

pub async fn of_account(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<AccountTrophies>> {
    let id = parse_id(&id)?;
    if db::account(&state.db, id).await?.is_none()
        || !may_see(&state.db, auth.account, id, Part::Trophies).await?
    {
        return Err(AppError::not_found());
    }
    let mut titles: Vec<TitleTrophies> = Vec::new();
    for row in db::trophies_of(&state.db, id).await? {
        let Some(trophy) = row.trophy() else { continue };
        match titles.last_mut() {
            Some(last) if last.title_id == row.title_id => last.trophies.push(trophy),
            _ => titles.push(TitleTrophies {
                title_id: row.title_id,
                trophies: vec![trophy],
            }),
        }
    }
    Ok(Json(AccountTrophies { titles }))
}

pub async fn rarity(
    State(state): State<AppState>,
    _auth: Auth,
    Path(title): Path<String>,
) -> Result<Json<Rarity>> {
    let title = title_id(&title)?;
    let players = db::title_players(&state.db, title).await?;
    // Few players would let a count point at one person.
    if players < i64::from(RARITY_MIN_PLAYERS) {
        return Ok(Json(Rarity {
            title_id: title.to_owned(),
            players: 0,
            trophies: Vec::new(),
        }));
    }
    let trophies = db::title_rarity(&state.db, title)
        .await?
        .into_iter()
        .filter_map(|(number, earned_by)| {
            Some(TrophyRarity {
                number: u16::try_from(number).ok()?,
                earned_by: u32::try_from(earned_by).unwrap_or(u32::MAX),
            })
        })
        .collect();
    Ok(Json(Rarity {
        title_id: title.to_owned(),
        players: u32::try_from(players).unwrap_or(u32::MAX),
        trophies,
    }))
}

#[derive(Deserialize)]
pub struct CardsQuery {
    accounts: String,
}

pub async fn cards(
    State(state): State<AppState>,
    auth: Auth,
    query: Result<Query<CardsQuery>, QueryRejection>,
) -> Result<Json<Cards>> {
    let Query(query) =
        query.map_err(|_| AppError::bad_request("Give the account IDs as `accounts=1,2,3`."))?;
    let mut ids = BTreeSet::new();
    for part in query.accounts.split(',').filter(|p| !p.is_empty()) {
        let id = parse_id(part.trim())
            .map_err(|_| AppError::bad_request("The account IDs are numbers."))?;
        ids.insert(id);
    }
    if ids.len() > MAX_CARDS {
        return Err(AppError::bad_request(format!(
            "Ask for at most {MAX_CARDS} accounts."
        )));
    }
    // A fixed number of queries on one connection, whatever the number of accounts.
    let ids: Vec<u64> = ids.into_iter().collect();
    let mut conn = state.db.acquire().await?;
    let mut accounts = db::accounts_by_ids(&mut conn, &ids).await?;
    // An account that does not exist is left out.
    let ids: Vec<u64> = ids
        .into_iter()
        .filter(|id| accounts.contains_key(id))
        .collect();
    let seen = viewing_many(&mut conn, auth.account, &ids).await?;
    let of = |part| -> Vec<u64> {
        ids.iter()
            .copied()
            .filter(|id| seen[id].can(part))
            .collect()
    };
    let mut counts = db::trophy_counts_of(&mut conn, &of(Part::Trophies)).await?;
    let mut avatars = db::profiles::avatar_hashes_of(&mut conn, &of(Part::Avatar)).await?;
    let mut cards = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(row) = accounts.remove(&id) else {
            continue;
        };
        cards.push(Card {
            account: account_of(&row),
            trophies: counts.remove(&id),
            avatar: avatars.remove(&id),
        });
    }
    Ok(Json(Cards { cards }))
}

pub async fn get_privacy(State(state): State<AppState>, auth: Auth) -> Result<Json<Privacy>> {
    Ok(Json(db::privacy_of(&state.db, auth.account).await?))
}

pub async fn set_privacy(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(privacy): ApiJson<Privacy>,
) -> Result<Json<Privacy>> {
    db::set_privacy(&state.db, auth.account, privacy).await?;
    Ok(Json(privacy))
}
