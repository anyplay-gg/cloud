//! What a banned player sees on the web: the statement of reasons (DSA Art. 17) and a form to
//! appeal. The launcher has no sign-in pages of its own, so the browser flow is where a banned
//! player learns of the ban.
//!
//! The appeal token of the API never reaches the page: the browser session remembers which
//! account proved itself (by code or passkey), and the appeal is filed for that account.

use anyplay_api::{AppealStatus, Statement};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;

use super::pages::{self, AppealPart, BanView};
use super::store::{self, Session};
use super::{
    Visit, WebError, WebForm, WebResult, check_csrf, existing, page, redirect, redirect_plain, view,
};
use crate::error::AppError;
use crate::moderation;
use crate::state::AppState;
use crate::util::now;

/// The page of a statement as a view. `until` is the end of the ban, when the statement does not
/// say.
pub fn view_of(statement: Option<&Statement>, until: Option<u64>) -> BanView {
    let Some(s) = statement else {
        return BanView {
            until,
            rule: None,
            reason: None,
            decided: None,
            appeal: AppealPart::None,
        };
    };
    let appeal = match s.appeal.status {
        AppealStatus::Available if now() < s.appeal.deadline && s.lifted.is_none() => {
            AppealPart::Form {
                deadline: s.appeal.deadline,
            }
        }
        AppealStatus::Available | AppealStatus::Expired => AppealPart::Over,
        status => AppealPart::Filed {
            status: status.as_str().to_owned(),
            answer: s.appeal.answer.clone(),
        },
    };
    BanView {
        until: s.until.or(until),
        rule: Some(s.rule.as_str().to_owned()),
        reason: Some(s.reason.clone()),
        decided: Some(s.created),
        appeal,
    }
}

/// A banned account proved who it is: remember it in the session and go to the statement.
pub async fn send_to(
    state: &AppState,
    mut visit: Visit,
    account: u64,
) -> Result<(Visit, &'static str), AppError> {
    visit.session.data.banned = Some(account);
    store::save_data(&state.db, &visit.session).await?;
    Ok((visit, "/web/banned"))
}

/// The page for a ban that is not tied to a signed-in account (an address that was banned and
/// whose account is gone): the statement, no form.
pub fn without_account(ctx: &pages::Ctx<'_>, error: &AppError) -> String {
    let statement = error.body.ban.as_ref().and_then(|b| b.statement.as_ref());
    let mut shown = view_of(statement, error.body.until);
    if let AppealPart::Form { .. } = shown.appeal {
        shown.appeal = AppealPart::None;
    }
    pages::banned(ctx, &shown, None)
}

async fn render(
    state: &AppState,
    mut session: Session,
    problem: Option<&str>,
    status: StatusCode,
) -> WebResult {
    let Some(account) = session.data.banned else {
        return Ok(redirect_plain("/web/sign-in"));
    };
    let Some(ban) = crate::db::ban_of(&state.db, account).await? else {
        // The ban ended since: the player signs in again, now without it.
        session.data.banned = None;
        session.data.notice = Some("The ban has ended. Sign in again.".to_owned());
        store::save_data(&state.db, &session).await?;
        return Ok(redirect_plain("/web/sign-in"));
    };
    let statement = moderation::ban_statement(state, account).await?;
    let shown = view_of(statement.as_ref(), ban.until());
    let mut visit = Visit {
        session,
        token: None,
    };
    let v = view(state, &mut visit.session).await?;
    Ok(page(
        state,
        &visit,
        status,
        pages::banned(&v.ctx(), &shown, problem),
    ))
}

pub async fn show(State(state): State<AppState>, headers: HeaderMap) -> WebResult {
    let session = existing(&state, &headers)
        .await?
        .ok_or_else(WebError::sign_in)?;
    render(&state, session, None, StatusCode::FORBIDDEN).await
}

#[derive(Deserialize)]
pub struct AppealForm {
    csrf: String,
    text: String,
}

pub async fn appeal(
    State(state): State<AppState>,
    headers: HeaderMap,
    WebForm(form): WebForm<AppealForm>,
) -> WebResult {
    let mut session = existing(&state, &headers)
        .await?
        .ok_or_else(WebError::expired)?;
    check_csrf(&session, &form.csrf)?;
    let Some(account) = session.data.banned else {
        return Ok(redirect_plain("/web/sign-in"));
    };
    let Some(statement) = moderation::ban_statement(&state, account).await? else {
        return Err(AppError::not_found().into());
    };
    match moderation::appeal(&state, account, statement.id, &form.text).await {
        Ok(_) => {
            session.data.notice = Some("Your appeal was sent. A moderator will answer it.".into());
            store::save_data(&state.db, &session).await?;
            Ok(redirect(
                &state,
                &Visit {
                    session,
                    token: None,
                },
                "/web/banned",
            ))
        }
        Err(error) if matches!(error.body.error, anyplay_api::ErrorCode::BadRequest) => {
            render(
                &state,
                session,
                Some(&error.body.message),
                StatusCode::BAD_REQUEST,
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}
