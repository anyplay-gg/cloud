//! What the portal shows of the social side: friends, the player's own profile, privacy and
//! trophies, and the public profile of any player.
//!
//! The rules are those of `/v1`. These handlers call the handlers of [`crate::routes`] for the
//! signed-in player (the browser session stands in for the token), so there is one rule for who
//! may do what and see what ([`crate::visibility`]). Trophies are what the cloud stores: counts
//! by grade, per title. Their names, texts and art never reach the cloud (network ADR 0009).

use anyplay_api::{Count, FriendRequest, Grade, Part, Privacy, Profile, ProfileUpdate};
use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde::Serialize;
use serde_json::json;

use super::{ApiResult, Member, done, limit_public, owner_of};
use crate::db;
use crate::error::AppError;
use crate::http::{ApiJson, Auth, ClientIp};
use crate::routes::{friends, profiles, trophies};
use crate::state::AppState;
use crate::visibility::{Relation, viewing_as};
use crate::web::existing;

/// The token of a request that came with a browser session: the account, no device.
fn auth_of(member: &Member) -> Auth {
    Auth {
        account: member.id(),
        device: 0,
        issued: 0,
    }
}

// Friends

/// `GET /friends`: friends with their presence, incoming and outgoing requests, blocked accounts.
pub async fn friend_lists(State(state): State<AppState>, member: Member) -> ApiResult {
    Ok(friends::list(State(state), auth_of(&member))
        .await?
        .into_response())
}

/// `POST /friends/requests`: asks the account with this handle to be a friend. If they asked
/// first, they are friends now.
pub async fn request_friend(
    State(state): State<AppState>,
    member: Member,
    ApiJson(body): ApiJson<FriendRequest>,
) -> ApiResult {
    Ok(
        friends::request(State(state), auth_of(&member), ApiJson(body))
            .await?
            .into_response(),
    )
}

/// `POST /friends/requests/{id}/accept`
pub async fn accept_request(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    friends::accept(State(state), auth_of(&member), Path(id)).await?;
    done()
}

/// `DELETE /friends/requests/{id}`: declines an incoming request or withdraws an outgoing one.
pub async fn cancel_request(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
) -> ApiResult {
    friends::cancel(State(state), auth_of(&member), Path(id)).await?;
    done()
}

/// `DELETE /friends/{id}`
pub async fn unfriend(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
) -> ApiResult {
    friends::unfriend(State(state), auth_of(&member), Path(id)).await?;
    done()
}

/// `PUT /blocks/{id}`
pub async fn block(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
    ApiJson(_body): ApiJson<serde_json::Value>,
) -> ApiResult {
    friends::block(State(state), auth_of(&member), Path(id)).await?;
    done()
}

/// `DELETE /blocks/{id}`
pub async fn unblock(
    State(state): State<AppState>,
    member: Member,
    Path(id): Path<String>,
) -> ApiResult {
    friends::unblock(State(state), auth_of(&member), Path(id)).await?;
    done()
}

// The player's own profile

/// `GET /me/profile`
pub async fn my_profile(State(state): State<AppState>, member: Member) -> ApiResult {
    let id = member.id();
    let seen = viewing_as(&state.db, Some(id), id).await?;
    Ok(Json(profiles::profile_seen(&state, &seen, id).await?).into_response())
}

/// `PATCH /me/profile`: sets the text and the favourite titles; a part left out stays.
pub async fn update_profile(
    State(state): State<AppState>,
    member: Member,
    ApiJson(update): ApiJson<ProfileUpdate>,
) -> ApiResult {
    Ok(
        profiles::update(State(state), auth_of(&member), ApiJson(update))
            .await?
            .into_response(),
    )
}

/// `GET /me/privacy`: who sees which part of the profile.
pub async fn my_privacy(State(state): State<AppState>, member: Member) -> ApiResult {
    Ok(trophies::get_privacy(State(state), auth_of(&member))
        .await?
        .into_response())
}

/// `PUT /me/privacy`
pub async fn set_privacy(
    State(state): State<AppState>,
    member: Member,
    ApiJson(privacy): ApiJson<Privacy>,
) -> ApiResult {
    Ok(
        trophies::set_privacy(State(state), auth_of(&member), ApiJson(privacy))
            .await?
            .into_response(),
    )
}

/// `PUT /me/avatar`: the body is the picture, a PNG or JPEG file.
pub async fn set_avatar(
    State(state): State<AppState>,
    member: Member,
    request: Request,
) -> ApiResult {
    Ok(
        profiles::set_avatar(State(state), auth_of(&member), request)
            .await?
            .into_response(),
    )
}

/// `DELETE /me/avatar`
pub async fn remove_avatar(State(state): State<AppState>, member: Member) -> ApiResult {
    profiles::remove_avatar(State(state), auth_of(&member)).await?;
    done()
}

// Trophies

/// One title's records, counted.
#[derive(Serialize)]
struct TitleSummary {
    title_id: String,
    counts: Count,
    /// How many trophies the player earned in the title.
    earned: u32,
    /// When the latest one was earned.
    last_earned: u64,
}

#[derive(Serialize)]
struct TrophySummary {
    counts: Count,
    titles: Vec<TitleSummary>,
}

fn add(count: &mut Count, grade: Grade, n: u32) {
    let slot = match grade {
        Grade::Bronze => &mut count.bronze,
        Grade::Silver => &mut count.silver,
        Grade::Gold => &mut count.gold,
        Grade::Platinum => &mut count.platinum,
    };
    *slot = slot.saturating_add(n);
}

/// The trophy records of an account, counted by grade, in total and per title (the title with
/// the latest trophy first). The cloud does not know how many trophies a game has, so there is
/// no percentage.
async fn summary(state: &AppState, owner: u64) -> Result<TrophySummary, AppError> {
    let mut counts = Count::default();
    let mut titles: Vec<TitleSummary> = Vec::new();
    // The store counts: at most one row per title and grade comes back, however many records
    // the player has.
    for (title_id, grade, n, last) in db::trophy_summary(&state.db, owner).await? {
        add(&mut counts, grade, n);
        let index = match titles.iter().position(|t| t.title_id == title_id) {
            Some(index) => index,
            None => {
                titles.push(TitleSummary {
                    title_id: title_id.clone(),
                    counts: Count::default(),
                    earned: 0,
                    last_earned: 0,
                });
                titles.len() - 1
            }
        };
        let title = &mut titles[index];
        add(&mut title.counts, grade, n);
        title.earned = title.earned.saturating_add(n);
        title.last_earned = title.last_earned.max(last);
    }
    titles.sort_by(|a, b| {
        b.last_earned
            .cmp(&a.last_earned)
            .then_with(|| a.title_id.cmp(&b.title_id))
    });
    Ok(TrophySummary { counts, titles })
}

/// `GET /me/trophies`
pub async fn my_trophies(State(state): State<AppState>, member: Member) -> ApiResult {
    Ok(Json(summary(&state, member.id()).await?).into_response())
}

// Other players

/// The account a signed-in, not banned browser session belongs to, if the request has one. The
/// public pages work without.
async fn viewer(state: &AppState, headers: &HeaderMap) -> Option<u64> {
    let session = existing(state, headers).await.ok().flatten()?;
    let account = session.account?;
    state.ban_of(account).is_none().then_some(account)
}

/// How the signed-in viewer stands to the player whose page they look at.
#[derive(Serialize)]
struct ViewerView {
    /// `self`, `friend` or `none`. A player who blocked the viewer shows as `none`.
    relation: &'static str,
    /// `incoming` (they asked the viewer) or `outgoing` (the viewer asked them).
    requested: Option<&'static str>,
    /// The viewer blocked them.
    blocked: bool,
}

#[derive(Serialize)]
struct PublicUser {
    #[serde(flatten)]
    profile: Profile,
    /// The player's trophies by grade, when the viewer may see them.
    trophies: Option<Count>,
    /// Absent for a viewer who is not signed in.
    viewer: Option<ViewerView>,
}

/// `GET /users/{name}`: the profile of a player as the viewer may see it: a signed-out visitor,
/// a signed-in stranger and a friend each see what the player shows to them.
pub async fn user(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult {
    limit_public(&state, ip)?;
    let viewer_id = viewer(&state, &headers).await;
    let owner = owner_of(&state, &name).await?;
    let seen = viewing_as(&state.db, viewer_id, owner.id()).await?;
    let profile = profiles::profile_seen(&state, &seen, owner.id()).await?;
    let trophies = if seen.can(Part::Trophies) {
        Some(db::trophy_counts(&state.db, owner.id()).await?)
    } else {
        None
    };
    let viewer = match viewer_id {
        None => None,
        Some(me) => {
            let relation = match seen.relation() {
                Relation::Own => "self",
                Relation::Friend => "friend",
                Relation::Stranger | Relation::Blocked => "none",
            };
            let requested = if matches!(seen.relation(), Relation::Stranger) {
                if db::request_exists(&state.db, owner.id(), me).await? {
                    Some("incoming")
                } else if db::request_exists(&state.db, me, owner.id()).await? {
                    Some("outgoing")
                } else {
                    None
                }
            } else {
                None
            };
            Some(ViewerView {
                relation,
                requested,
                blocked: me != owner.id() && db::has_blocked(&state.db, me, owner.id()).await?,
            })
        }
    };
    Ok(Json(PublicUser {
        profile,
        trophies,
        viewer,
    })
    .into_response())
}

/// `GET /users/{name}/trophies`: the player's trophies per title, when the viewer may see them.
pub async fn user_trophies(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult {
    limit_public(&state, ip)?;
    let viewer_id = viewer(&state, &headers).await;
    let owner = owner_of(&state, &name).await?;
    let seen = viewing_as(&state.db, viewer_id, owner.id()).await?;
    if !seen.can(Part::Trophies) {
        return Err(AppError::not_found().into());
    }
    Ok(Json(json!(summary(&state, owner.id()).await?)).into_response())
}
