//! Profiles, avatars and playtime.

use anyplay_api::{
    AboutProblem, AvatarSet, FavouritesProblem, MAX_ABOUT, MAX_AVATAR_UPLOAD, MAX_DAY_MINUTES,
    MAX_FAVOURITES, MAX_PLAYTIME_UPLOAD, Part, PlaytimeAdded, PlaytimeUpload, Profile,
    ProfileUpdate, check_about, check_favourites, day_number, is_avatar_hash, is_title_id,
};
use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use image::ImageFormat;

use crate::db::profiles as store;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth, parse_id};
use crate::routes::accounts::account_of;
use crate::state::AppState;
use crate::util::now;
use crate::visibility::viewing;
use crate::{avatar, db};

/// A day may be this many days ahead of UTC today, for clocks and time zones.
const DAYS_AHEAD: i64 = 1;

/// The profile of `owner` as `viewer` may see it. A part the viewer may not see, or that is not
/// set, is `None`.
async fn profile_for(state: &AppState, viewer: u64, owner: u64) -> Result<Profile> {
    let row = db::account(&state.db, owner)
        .await?
        .ok_or_else(AppError::not_found)?;
    let seen = viewing(&state.db, viewer, owner).await?;
    let (about, favourites) = store::profile_of(&state.db, owner).await?;
    let about = (seen.can(Part::About) && !about.is_empty()).then_some(about);
    let favourites = (seen.can(Part::Favourites) && !favourites.is_empty()).then_some(favourites);
    let avatar = if seen.can(Part::Avatar) {
        store::avatar_hash_of(&state.db, owner).await?
    } else {
        None
    };
    let playtime = if seen.can(Part::Playtime) {
        Some(store::playtime_totals(&state.db, owner).await?).filter(|t| !t.is_empty())
    } else {
        None
    };
    Ok(Profile {
        account: account_of(&row),
        about,
        favourites,
        avatar,
        playtime,
    })
}

pub async fn of_account(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<Profile>> {
    let id = parse_id(&id)?;
    Ok(Json(profile_for(&state, auth.account, id).await?))
}

pub async fn update(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(update): ApiJson<ProfileUpdate>,
) -> Result<Json<Profile>> {
    let about = update.about.as_deref().map(str::trim);
    if let Some(about) = about {
        check_about(about).map_err(|problem| {
            AppError::bad_request(match problem {
                AboutProblem::TooLong => format!("The text has at most {MAX_ABOUT} characters."),
                AboutProblem::Character => "The text is plain text without line breaks.".into(),
                AboutProblem::Link => "The text cannot hold links.".into(),
            })
        })?;
    }
    if let Some(favourites) = &update.favourites {
        check_favourites(favourites).map_err(|problem| {
            AppError::bad_request(match problem {
                FavouritesProblem::TooMany => {
                    format!("Pick at most {MAX_FAVOURITES} favourite titles.")
                }
                FavouritesProblem::TitleId => "That is not a title ID.".into(),
                FavouritesProblem::Duplicate => "A favourite is listed twice.".into(),
            })
        })?;
    }
    store::set_profile(
        &state.db,
        auth.account,
        about,
        update.favourites.as_deref(),
        now(),
    )
    .await?;
    Ok(Json(profile_for(&state, auth.account, auth.account).await?))
}

/// The format a content type names.
fn declared_format(headers: &HeaderMap) -> Result<ImageFormat> {
    let media = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    match media.as_deref() {
        Some("image/png") => Ok(ImageFormat::Png),
        Some("image/jpeg") => Ok(ImageFormat::Jpeg),
        _ => Err(AppError::bad_request(
            "Send the picture with Content-Type image/png or image/jpeg.",
        )),
    }
}

pub async fn set_avatar(
    State(state): State<AppState>,
    auth: Auth,
    request: Request,
) -> Result<Json<AvatarSet>> {
    let declared = declared_format(request.headers())?;
    let rates = &state.tuning.rates;
    // Every try counts, good or not: decoding is what costs.
    state
        .limiter
        .hit(
            &format!("avatar:{}", auth.account),
            rates.avatar_uploads,
            rates.avatar_window,
        )
        .map_err(AppError::rate_limited)?;
    let too_large = || AppError::too_large("The picture may be at most 1 MB.");
    let announced = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if announced.is_some_and(|n| n > MAX_AVATAR_UPLOAD) {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(request.into_body(), MAX_AVATAR_UPLOAD)
        .await
        .map_err(|_| too_large())?;
    let made = avatar::make(bytes.to_vec(), declared).await?;
    store::put_avatar(&state.db, auth.account, &made.hash, &made.png, now()).await?;
    Ok(Json(AvatarSet { avatar: made.hash }))
}

pub async fn remove_avatar(State(state): State<AppState>, auth: Auth) -> Result<StatusCode> {
    store::remove_avatar(&state.db, auth.account).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The picture with a hash. It needs no token: the hash is 256 bits that only those who may see
/// the avatar are given, and the picture under it never changes.
pub async fn avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Result<Response> {
    if !is_avatar_hash(&hash) {
        return Err(AppError::not_found());
    }
    let etag = format!("\"{hash}\"");
    let cache = (
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    let tag = HeaderValue::from_str(&etag).map_err(|_| AppError::internal())?;
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|v| v.as_bytes() == etag.as_bytes())
        && store::avatar_by_hash(&state.db, &hash).await?.is_some()
    {
        return Ok((StatusCode::NOT_MODIFIED, [cache, (header::ETAG, tag)]).into_response());
    }
    let picture = store::avatar_by_hash(&state.db, &hash)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
            cache,
            (header::ETAG, tag),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        picture,
    )
        .into_response())
}

pub async fn upload_playtime(
    State(state): State<AppState>,
    auth: Auth,
    ApiJson(upload): ApiJson<PlaytimeUpload>,
) -> Result<Json<PlaytimeAdded>> {
    if upload.entries.len() > MAX_PLAYTIME_UPLOAD {
        return Err(AppError::too_large(format!(
            "One upload carries at most {MAX_PLAYTIME_UPLOAD} entries."
        )));
    }
    let latest = i64::try_from(now() / 86_400).unwrap_or(i64::MAX) + DAYS_AHEAD;
    for entry in &upload.entries {
        if !is_title_id(&entry.title_id) {
            return Err(AppError::bad_request("That is not a title ID."));
        }
        match day_number(&entry.day) {
            Some(day) if day <= latest => {}
            Some(_) => return Err(AppError::bad_request("A day cannot be in the future.")),
            None => {
                return Err(AppError::bad_request(
                    "A day is a UTC date written YYYY-MM-DD.",
                ));
            }
        }
        if entry.minutes > MAX_DAY_MINUTES {
            return Err(AppError::bad_request("A day has 1,440 minutes."));
        }
    }
    let rows = store::playtime_rows(&state.db, auth.account).await?;
    if rows + upload.entries.len() as i64 > store::MAX_PLAYTIME_ROWS {
        return Err(AppError::limit(format!(
            "An account keeps at most {} days of titles.",
            store::MAX_PLAYTIME_ROWS
        )));
    }
    let changed = store::add_playtime(&state.db, auth.account, &upload.entries).await?;
    Ok(Json(PlaytimeAdded {
        changed: u32::try_from(changed).unwrap_or(u32::MAX),
    }))
}
