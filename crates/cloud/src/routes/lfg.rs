//! Looking for group: a player posts a short listing for a game, others who have the game ask to
//! join, and the poster accepts them into their party. The store is [`crate::lfg`]; the
//! rules are in network `spec/cloud-api.md`.
//!
//! The cloud cannot know who owns a game, so "visible to players who have the game" is a
//! filter: the board is read one title at a time, and there is no call that lists every game's
//! listings.

use anyplay_api::{
    Account, AccountId, CreateListing, ErrorCode, Event, LFG_TTL_SECS, LfgAsk, LfgMine, LfgMode,
    Listing, Listings, MAX_LFG_OPEN, MAX_LFG_PAGE, MAX_LFG_REQUESTS, MAX_LFG_SLOTS, check_lfg_note,
    is_language_tag, is_title_id,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::lfg::{self, ListingRow, NewListing};
use crate::moderation::ensure_not_muted;
use crate::party::{self, InvitationRow};
use crate::routes::parties;
use crate::state::AppState;
use crate::util::{now, now_ms, random_uuid};

fn rate(state: &AppState, name: &str, account: u64, limit: u32) -> Result<()> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(&format!("{name}:{account}"), limit, rates.lfg_window)
        .map_err(AppError::rate_limited)
}

#[derive(Deserialize)]
pub struct BoardQuery {
    title: String,
    mode: Option<String>,
    language: Option<String>,
}

/// The open listings of one game.
pub async fn board(
    State(state): State<AppState>,
    auth: Auth,
    query: Result<Query<BoardQuery>, QueryRejection>,
) -> Result<Json<Listings>> {
    let Query(query) = query.map_err(|_| AppError::bad_request("Name the game with title."))?;
    if !is_title_id(&query.title) {
        return Err(AppError::bad_request("That is not a title ID."));
    }
    let mode = match query.mode.as_deref() {
        None | Some("") => "",
        Some(text) => LfgMode::parse(text)
            .map(LfgMode::as_str)
            .ok_or_else(|| AppError::bad_request("The mode is online, lan or play_together."))?,
    };
    let language = match query.language.as_deref() {
        None | Some("") => "",
        Some(text) if is_language_tag(text) => text,
        Some(_) => return Err(AppError::bad_request("A language is such as en or de.")),
    };
    let rates = &state.tuning.rates;
    rate(&state, "lfg_read", auth.account, rates.lfg_reads)?;
    let mut conn = state.db.acquire().await?;
    let rows = lfg::open_for_title(
        &mut conn,
        auth.account,
        &query.title,
        mode,
        language,
        now_ms(),
        MAX_LFG_PAGE as i64,
    )
    .await?;
    let asks = lfg::asks_among(&mut conn, auth.account).await?;
    let listings = rows
        .into_iter()
        // A banned account's listings are gone from the board at once.
        .filter(|row| row.is_open(now_ms()) && state.ban_of(row.poster()).is_none())
        .map(|row| {
            let ask = asks.get(&row.id).copied();
            row.view(ask, Vec::new())
        })
        .collect();
    Ok(Json(Listings { listings }))
}

/// The listing as its poster sees it, with the requests.
async fn poster_view(state: &AppState, row: &ListingRow) -> Result<Listing> {
    let requests = lfg::requests_of(&mut *state.db.acquire().await?, &row.id).await?;
    Ok(row.view(None, requests))
}

pub async fn mine(State(state): State<AppState>, auth: Auth) -> Result<Json<LfgMine>> {
    let mut conn = state.db.acquire().await?;
    let now = now_ms();
    let mut posted = Vec::new();
    for row in lfg::open_of(&mut conn, auth.account, now).await? {
        let requests = lfg::requests_of(&mut conn, &row.id).await?;
        posted.push(row.view(None, requests));
    }
    let asked = lfg::asked_by(&mut conn, auth.account, now)
        .await?
        .into_iter()
        .map(|(row, ask)| row.view(Some(ask), Vec::new()))
        .collect();
    Ok(Json(LfgMine { posted, asked }))
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(request): ApiJson<CreateListing>,
) -> Result<Json<Listing>> {
    ensure_not_muted(&state, auth.account).await?;
    if !is_title_id(&request.title_id) {
        return Err(AppError::bad_request("That is not a title ID."));
    }
    if request.mode == LfgMode::Unknown {
        return Err(AppError::bad_request(
            "The mode is online, lan or play_together.",
        ));
    }
    if let Some(language) = &request.language
        && !is_language_tag(language)
    {
        return Err(AppError::bad_request("A language is such as en or de."));
    }
    if check_lfg_note(&request.note).is_err() {
        return Err(AppError::bad_request(
            "A note has at most 140 characters, no links.",
        ));
    }
    if !(1..=MAX_LFG_SLOTS).contains(&request.slots) {
        return Err(AppError::bad_request("A listing looks for 1 to 7 players."));
    }
    let now = now_ms();
    let rates = &state.tuning.rates;
    rate(&state, "lfg_post", auth.account, rates.lfg_posts)?;
    let id = random_uuid();
    // The open listings are counted and the new one written in one write transaction, so two
    // requests cannot both pass the limit.
    let mut tx = state.begin_write().await?;
    if tx.backend_name() == "PostgreSQL" {
        // SQLite serializes its writers already; PostgreSQL needs the poster's turn.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(auth.account as i64 ^ 0x4c46_4700_0000_0000)
            .execute(&mut *tx)
            .await?;
    }
    let open = lfg::open_of(&mut tx, auth.account, now).await?;
    if open.iter().any(|row| row.title_id == request.title_id) {
        return Err(AppError::limit(
            "You have a listing for this game already. Close it first.",
        ));
    }
    if open.len() >= MAX_LFG_OPEN {
        return Err(AppError::limit("You have three open listings already."));
    }
    lfg::insert(
        &mut tx,
        &NewListing {
            id: &id,
            poster: auth.account,
            title_id: &request.title_id,
            mode: request.mode,
            language: request.language.as_deref(),
            note: request.note.trim(),
            slots: request.slots,
            created_ms: now,
            expires_ms: now + (LFG_TTL_SECS * 1000) as i64,
        },
    )
    .await?;
    let row = lfg::listing(&mut tx, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    tx.commit().await?;
    Ok(Json(poster_view(&state, &row).await?))
}

async fn open_listing(state: &AppState, id: &str) -> Result<ListingRow> {
    lfg::listing(&mut *state.db.acquire().await?, id)
        .await?
        .filter(|row| row.is_open(now_ms()))
        .ok_or_else(AppError::not_found)
}

pub async fn close(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let row = lfg::listing(&mut *state.db.acquire().await?, &id)
        .await?
        .filter(|row| row.poster() == auth.account)
        .ok_or_else(AppError::not_found)?;
    lfg::close(&mut *state.db.acquire().await?, &row.id, now_ms()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Asks to join. The poster hears of it; asking twice changes nothing.
pub async fn ask(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Listing>> {
    ensure_not_muted(&state, auth.account).await?;
    let row = open_listing(&state, &id).await?;
    if row.poster() == auth.account {
        return Err(AppError::bad_request("This is your own listing."));
    }
    if state.ban_of(row.poster()).is_some() {
        return Err(AppError::not_found());
    }
    let rates = &state.tuning.rates;
    rate(&state, "lfg_ask", auth.account, rates.lfg_asks)?;
    let me = crate::db::account(&state.db, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let is_new = {
        let mut conn = state.db.acquire().await?;
        if crate::chat::blocked_either(&mut conn, auth.account, row.poster()).await? {
            return Err(AppError::not_found());
        }
        if lfg::count_requests(&mut conn, &row.id).await? >= MAX_LFG_REQUESTS as i64 {
            return Err(AppError::new(
                ErrorCode::Full,
                "This listing has as many requests as it takes.",
            ));
        }
        lfg::insert_request(&mut conn, &row.id, auth.account, now_ms()).await?
    };
    if is_new {
        state.news.push(
            row.poster(),
            &Event::LfgRequest {
                listing: row.id.clone(),
                title_id: row.title_id.clone(),
                from: Account {
                    id: AccountId(me.id()),
                    handle: me.handle.clone(),
                },
            },
        );
    }
    let ask = lfg::ask_of(&mut *state.db.acquire().await?, &row.id, auth.account).await?;
    let row = lfg::listing(&mut *state.db.acquire().await?, &row.id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok(Json(row.view(ask, Vec::new())))
}

/// Takes the caller's request back.
pub async fn withdraw(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    lfg::delete_request(&mut *state.db.acquire().await?, &id, auth.account).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The poster's decline: the request goes, silently.
pub async fn decline(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, account)): Path<(String, String)>,
) -> Result<Json<Listing>> {
    let account = parse_id(&account)?;
    let row = lfg::listing(&mut *state.db.acquire().await?, &id)
        .await?
        .filter(|row| row.poster() == auth.account)
        .ok_or_else(AppError::not_found)?;
    lfg::delete_request(&mut *state.db.acquire().await?, &row.id, account).await?;
    let row = lfg::listing(&mut *state.db.acquire().await?, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok(Json(poster_view(&state, &row).await?))
}

/// The poster's accept: the asker gets an invitation to the poster's party, which is made when
/// there is none. Strangers cannot be invited otherwise; this is the one way, and it needs a
/// request the asker made themselves.
pub async fn accept(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, account)): Path<(String, String)>,
) -> Result<Json<Listing>> {
    let asker = parse_id(&account)?;
    let row = open_listing(&state, &id).await?;
    if row.poster() != auth.account {
        return Err(AppError::not_found());
    }
    let handle = crate::db::account(&state.db, auth.account)
        .await?
        .ok_or_else(AppError::not_found)?
        .handle;
    {
        let mut conn = state.db.acquire().await?;
        match lfg::ask_of(&mut conn, &row.id, asker).await? {
            None => return Err(AppError::not_found()),
            // Accepted already: the answer is the listing as it stands.
            Some(LfgAsk::Accepted) => {
                drop(conn);
                return Ok(Json(poster_view(&state, &row).await?));
            }
            Some(_) => {}
        }
        if crate::chat::blocked_either(&mut conn, auth.account, asker).await? {
            return Err(AppError::not_found());
        }
    }
    let party_id = match parties::party_of(&state, auth.account).await? {
        Some(party) => party.row.id,
        None => match parties::make_party(&state, auth.account, handle.clone()).await {
            Ok(created) => {
                state.announce_party(&created);
                created.row.id
            }
            // Another request of the same account got there first.
            Err(error) if error.body.error == ErrorCode::Limit => {
                parties::party_of(&state, auth.account)
                    .await?
                    .ok_or(error)?
                    .row
                    .id
            }
            Err(error) => return Err(error),
        },
    };
    let ttl = state.tuning.invitation_ttl;
    let mut tx = state.begin_write().await?;
    let party = party::lock_party(&mut tx, &party_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if party.leader() != auth.account {
        return Err(AppError::forbidden("Only the leader can invite."));
    }
    if party.is_full() && !party.is_member(asker) {
        return Err(AppError::new(ErrorCode::Full, "The party is full."));
    }
    // The listing again, under the lock: the places it has left.
    let current = lfg::listing(&mut tx, &row.id)
        .await?
        .filter(|r| r.is_open(now_ms()))
        .ok_or_else(AppError::not_found)?;
    if current.filled >= current.slots {
        return Err(AppError::new(ErrorCode::Full, "The listing is full."));
    }
    if !lfg::accept_request(&mut tx, &row.id, asker, now_ms()).await? {
        return Err(AppError::not_found());
    }
    let mut sent = None;
    if !party.is_member(asker) && !party::is_invited(&mut tx, asker, &party_id).await? {
        let (open_for_party, held) = party::count_invitations(&mut tx, &party_id, asker).await?;
        if open_for_party >= parties::MAX_INVITATIONS_PER_PARTY
            || held >= parties::MAX_INVITATIONS_PER_ACCOUNT
        {
            return Err(AppError::limit("There are too many open invitations."));
        }
        let invitation = InvitationRow {
            id: random_uuid(),
            party_id: party_id.clone(),
            from_id: auth.account as i64,
            from_handle: handle.clone(),
            to_id: asker as i64,
            expires: (now() + ttl.as_secs()) as i64,
            deadline_ms: now_ms() + ttl.as_millis() as i64,
            call: 0,
        };
        if party::insert_invitation(&mut tx, &invitation).await? {
            sent = Some(invitation);
        }
    }
    // Full now: the listing closes by itself.
    if current.filled + 1 >= current.slots {
        lfg::close(&mut tx, &row.id, now_ms()).await?;
    }
    tx.commit().await?;

    if let Some(invitation) = sent {
        crate::push::notify(
            &state,
            asker,
            crate::push::Note::party_invite(&invitation.from_handle),
        );
        state.news.push(
            asker,
            &Event::PartyInvitation {
                invitation: invitation.view(party.members.len() as u16),
            },
        );
    }
    state.news.push(
        asker,
        &Event::LfgAccepted {
            listing: row.id.clone(),
            title_id: row.title_id.clone(),
            by: Account {
                id: AccountId(auth.account),
                handle,
            },
        },
    );
    let row = lfg::listing(&mut *state.db.acquire().await?, &id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok(Json(poster_view(&state, &row).await?))
}
