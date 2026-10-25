//! Run reports: how a game ran for the players, and what they found.

use anyplay_api::{
    RUN_REPORTS_MIN, RunReport, RunSummary, VerdictCounts, is_run_version, is_title_id,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::db;
use crate::error::{AppError, Result};
use crate::http::{ApiJson, Auth};
use crate::state::AppState;
use crate::util::now;

fn title_id(text: &str) -> Result<&str> {
    if is_title_id(text) {
        Ok(text)
    } else {
        Err(AppError::bad_request("That is not a title ID."))
    }
}

fn version(text: &str) -> Result<&str> {
    if is_run_version(text) {
        Ok(text)
    } else {
        Err(AppError::bad_request(
            "A version has 1 to 32 letters, digits, dots, underscores, plus signs and hyphens.",
        ))
    }
}

pub async fn put(
    State(state): State<AppState>,
    auth: Auth,
    Path(title): Path<String>,
    ApiJson(report): ApiJson<RunReport>,
) -> Result<StatusCode> {
    let title = title_id(&title)?;
    version(&report.version)?;
    version(&report.toolkit)?;
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("runs:put:{}", auth.account),
            rates.run_reports,
            rates.run_report_window,
        )
        .map_err(AppError::rate_limited)?;
    let stored = db::runs::put(
        &state.writer,
        &state.db,
        auth.account,
        title,
        &report,
        now(),
    )
    .await?;
    if !stored {
        return Err(AppError::limit(format!(
            "An account keeps at most {} run reports.",
            db::runs::MAX_RUN_REPORT_ROWS
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    version: Option<String>,
}

pub async fn summary(
    State(state): State<AppState>,
    _auth: Auth,
    Path(title): Path<String>,
    query: Result<Query<SummaryQuery>, QueryRejection>,
) -> Result<Json<RunSummary>> {
    let title = title_id(&title)?;
    let Query(query) = query.map_err(|_| AppError::bad_request("Give `version` as text."))?;
    let wanted = query.version.as_deref().map(version).transpose()?;
    // The asked version when enough players reported it, else every version of the title.
    if let Some(wanted) = wanted {
        let counts = db::runs::counts(&state.db, title, Some(wanted)).await?;
        if counts.total() >= RUN_REPORTS_MIN {
            return Ok(Json(answer(title, Some(wanted), counts)));
        }
    }
    let counts = db::runs::counts(&state.db, title, None).await?;
    Ok(Json(answer(title, None, counts)))
}

/// The summary, or an empty one below [`RUN_REPORTS_MIN`] so that it cannot tell what one
/// player said.
fn answer(title: &str, version: Option<&str>, counts: VerdictCounts) -> RunSummary {
    if counts.total() < RUN_REPORTS_MIN {
        return RunSummary {
            title_id: title.to_owned(),
            version: None,
            reports: 0,
            verdict: None,
            counts: VerdictCounts::default(),
        };
    }
    RunSummary {
        title_id: title.to_owned(),
        version: version.map(str::to_owned),
        reports: counts.total(),
        verdict: counts.top(),
        counts,
    }
}
