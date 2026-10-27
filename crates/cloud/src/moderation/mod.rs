//! Moderation: reports and their queue, actions with a reason the player reads, appeals, the
//! audit log, the moderator role, the rules for new and muted accounts, and the retention of all
//! of it. `docs/moderation.md` describes it for operators and the Moderation section of
//! `spec/cloud-api.md` in anyplaynet/network for clients.
//!
//! The interface is a handful of functions on [`AppState`]. Each checks who may call it, writes
//! its change and its audit line in one transaction, and only then tells the player and the other
//! nodes, so a failed write sends no news. The HTTP routes in [`crate::routes::moderation`] and
//! [`crate::routes::admin`] only translate.

use anyplay_api::{
    self as api, ActionKind, ActionRequest, ActionView, AppealDecision, AppealInfo, AppealOutcome,
    AppealStatus, AppealView, AuditAction, AuditEntry, BanNotice, Event, ReferenceKind,
    ReportOutcome, ReportReason, ReportStatus, ReportView, Restriction, Rule, Statement,
};
use anyplay_ticket::Claims;

use crate::db;
use crate::error::{AppError, Result};
use crate::state::AppState;
use crate::util::now;

pub mod access;
pub mod banhash;
pub mod store;

use store::{ActionRow, ReportRow};

/// How long an appeal token lives.
const APPEAL_TOKEN_SECS: u64 = 7 * 24 * 60 * 60;
const DAY: u64 = 24 * 60 * 60;

/// Who does something: a moderator or player by account, or the operator by the admin token.
#[derive(Clone, Debug)]
pub struct Actor {
    id: Option<u64>,
    handle: String,
}

impl Actor {
    pub fn operator() -> Self {
        Self {
            id: None,
            handle: "operator".into(),
        }
    }

    pub fn account(id: u64, handle: impl Into<String>) -> Self {
        Self {
            id: Some(id),
            handle: handle.into(),
        }
    }

    pub fn id(&self) -> Option<u64> {
        self.id
    }

    /// The name the actor acts under: a moderator's handle, or `operator`.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    pub fn is_operator(&self) -> bool {
        self.id.is_none()
    }

    fn require_operator(&self) -> Result<()> {
        if self.is_operator() {
            Ok(())
        } else {
            Err(AppError::forbidden("Only the operator can do this."))
        }
    }
}

fn valid_reason(text: &str) -> Result<String> {
    let text = text.trim();
    let length = text.chars().count();
    if length == 0 || length > api::MAX_REASON {
        return Err(AppError::bad_request("A reason has 1 to 500 characters."));
    }
    Ok(text.to_owned())
}

// Reports

/// Files a report about another account.
pub async fn file_report(state: &AppState, reporter: u64, request: api::Report) -> Result<()> {
    let target = request.account.0;
    if target == reporter {
        return Err(AppError::bad_request("You cannot report yourself."));
    }
    if request.details.chars().count() > api::MAX_REPORT_DETAILS {
        return Err(AppError::bad_request(
            "The details have at most 1,000 characters.",
        ));
    }
    let Some(reported) = db::account(&state.db, target).await? else {
        return Err(AppError::not_found());
    };
    let Some(me) = db::account(&state.db, reporter).await? else {
        return Err(AppError::not_found());
    };
    let mut conn = state.db.acquire().await?;
    let day_ago = now().saturating_sub(DAY);
    if store::count_reports_since(&mut conn, reporter, day_ago).await?
        >= i64::from(state.config.report_limit_per_day)
    {
        return Err(AppError::limit(
            "You have filed the most reports allowed today.",
        ));
    }
    if store::open_report_exists(&mut conn, reporter, target).await? {
        return Err(AppError::limit(
            "You have an open report about this account already.",
        ));
    }
    // Last, so that a refused report does not use up the allowance.
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("report:{reporter}"),
            rates.report_per_account,
            rates.report_window,
        )
        .map_err(AppError::rate_limited)?;
    let reason = serde_json::to_value(request.reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".into());
    // What a moderator would otherwise have to look up: the reported account as it is now, and
    // the message when one is named.
    let mut content = format!(
        "handle: {}\naccount created: {}",
        reported.handle, reported.created
    );
    if let Some((message, held)) = reported_message(&mut conn, reporter, target, &request).await? {
        // An unsent message is reported by the text it had last; an edited one also with every
        // text it had before.
        let mut held = held;
        let text = if message.deleted {
            held.pop().unwrap_or_default()
        } else {
            message.text.clone()
        };
        content.push_str(&format!(
            "\nmessage {} in conversation {} (sent {}):\n{}",
            message.seq, message.conversation, message.created, text
        ));
        if message.deleted {
            content.push_str("\n(the writer unsent this message)");
        }
        for (k, before) in held.iter().enumerate() {
            content.push_str(&format!("\n(earlier text {}:)\n{before}", k + 1));
        }
    }
    if let Some(listing) = &request.listing {
        let row = crate::lfg::listing(&mut conn, listing)
            .await?
            .filter(|row| row.poster() == target)
            .ok_or_else(AppError::not_found)?;
        content.push_str(&format!(
            "\nlooking-for-group listing {} (game {}, mode {}, posted {}):\n{}",
            row.id,
            row.title_id,
            row.mode,
            row.created_ms / 1000,
            row.note
        ));
    }
    let mut held_media = None;
    if let (Some(conversation), Some(seq)) = (&request.conversation, request.seq) {
        // A report that names a screenshot or clip keeps the file for the moderators for as long
        // as an undecided report lives.
        let until = (now() + state.config.retention_open_reports_days * DAY) as i64 * 1000;
        if let Some(media) = crate::media::hold(&mut conn, conversation, seq, until).await? {
            held_media = Some(media.id.clone());
            content.push_str(&format!(
                "\nmedia {} ({}, {} bytes) is held for moderators{}",
                media.id,
                media.kind,
                media.bytes,
                if media.removed_ms.is_some() {
                    "; the sender unsent it"
                } else {
                    ""
                }
            ));
        }
    }
    store::insert_report(
        &mut conn,
        &store::NewReport {
            reporter,
            reporter_handle: &me.handle,
            reported: target,
            reported_handle: &reported.handle,
            reason: &reason,
            details: &request.details,
            content: &content,
            media_id: held_media.as_deref(),
            now: now(),
        },
    )
    .await?;
    Ok(())
}

/// The message a report names, as the reporter sees it. `None` when the report names none.
/// The reporter must be in the conversation and able to read the message, and `account` must have
/// written it.
async fn reported_message(
    conn: &mut crate::live::Conn,
    reporter: u64,
    account: u64,
    request: &api::Report,
) -> Result<Option<(api::Message, Vec<String>)>> {
    let (conversation, seq) = match (&request.conversation, request.seq) {
        (None, None) => return Ok(None),
        (Some(conversation), Some(seq)) if seq > 0 => (conversation, seq),
        _ => {
            return Err(AppError::bad_request(
                "Name a message with both conversation and seq.",
            ));
        }
    };
    let members = crate::chat::members(conn, conversation).await?;
    let me = members
        .iter()
        .find(|m| m.id() == reporter && m.state != crate::chat::DECLINED)
        .ok_or_else(AppError::not_found)?;
    let floor = me.from_seq.max(0) as u64;
    let message = crate::chat::messages_after(conn, conversation, reporter, floor, seq - 1, 1)
        .await?
        .into_iter()
        .next()
        .filter(|m| m.seq == seq)
        .ok_or_else(AppError::not_found)?;
    let held = crate::chat::held_texts(conn, conversation, seq, crate::chat::hold_cut_ms()).await?;
    if message.deleted && held.is_empty() {
        return Err(AppError::not_found());
    }
    if message.sender.id.0 != account {
        return Err(AppError::bad_request(
            "The message was written by another account.",
        ));
    }
    Ok(Some((message, held)))
}

/// The reports a moderator reads. `status` is `open` (oldest first), `decided` or `all`.
pub async fn queue(state: &AppState, status: &str, limit: i64) -> Result<Vec<ReportView>> {
    let limit = limit.clamp(1, 1000);
    let mut conn = state.db.acquire().await?;
    let rows = match status {
        "open" => store::open_reports(&mut conn, limit).await?,
        "decided" => store::reports(&mut conn, "decided", limit).await?,
        "all" => store::reports(&mut conn, "", limit).await?,
        _ => {
            return Err(AppError::bad_request("status is open, decided or all."));
        }
    };
    Ok(rows.iter().map(report_view).collect())
}

pub async fn report(state: &AppState, id: u64) -> Result<ReportView> {
    let mut conn = state.db.acquire().await?;
    let row = store::report(&mut conn, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(report_view(&row))
}

/// A moderator never works on a case about themselves, and only the operator works on one about
/// another moderator.
async fn guard_target(conn: &mut crate::live::Conn, actor: &Actor, target: u64) -> Result<()> {
    if actor.id == Some(target) {
        return Err(AppError::forbidden(
            "You cannot decide a case about yourself.",
        ));
    }
    if !actor.is_operator() && store::moderator_since(conn, target).await?.is_some() {
        return Err(AppError::forbidden(
            "Only the operator can act on a moderator.",
        ));
    }
    Ok(())
}

/// Decides a report without an action.
pub async fn dismiss(state: &AppState, actor: &Actor, id: u64, reason: &str) -> Result<ReportView> {
    let reason = valid_reason(reason)?;
    let mut tx = begin(state).await?;
    let row = store::report(&mut tx, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if actor.id == Some(row.reported as u64) {
        return Err(AppError::forbidden(
            "You cannot decide a report about yourself.",
        ));
    }
    let decision = store::Decision {
        outcome: ReportOutcome::Dismissed.as_str(),
        now: now(),
        by: actor.id,
        by_handle: &actor.handle,
        action: None,
        release_media_at: now() + state.config.retention_reports_days * DAY,
    };
    if !store::decide_report(&mut tx, id, &decision).await? {
        return Err(AppError::limit("This report is decided already."));
    }
    audit(
        &mut tx,
        actor,
        AuditAction::ReportDismissed,
        Some((u(row.reported), &row.reported_handle)),
        &reason,
        None,
        Some((ReferenceKind::Report, id)),
    )
    .await?;
    tx.commit().await?;
    report(state, id).await
}

// Actions

/// Takes an action against an account and tells it.
pub async fn act(state: &AppState, actor: &Actor, request: ActionRequest) -> Result<ActionView> {
    let reason = valid_reason(&request.reason)?;
    let days = match (request.kind, request.days) {
        (ActionKind::Warning, None) => None,
        (ActionKind::Warning, Some(_)) => {
            return Err(AppError::bad_request("A warning has no length."));
        }
        (ActionKind::Mute, None) => {
            return Err(AppError::bad_request("A mute needs days."));
        }
        (_, Some(0)) => return Err(AppError::bad_request("days must be at least 1.")),
        (_, Some(days)) if days > api::MAX_ACTION_DAYS => {
            return Err(AppError::bad_request("days is at most 3650."));
        }
        (_, days) => days,
    };
    let target = request.account.0;
    if actor.id == Some(target) {
        return Err(AppError::forbidden("You cannot act on yourself."));
    }
    let account = db::account(&state.db, target)
        .await?
        .ok_or_else(AppError::not_found)?;
    let created = now();
    let until = days.map(|d| created + d * DAY);

    let mut tx = begin(state).await?;
    if !actor.is_operator() && store::moderator_since(&mut tx, target).await?.is_some() {
        return Err(AppError::forbidden(
            "Only the operator can act on a moderator.",
        ));
    }
    if let Some(report_id) = request.report {
        let report = store::report(&mut tx, report_id)
            .await?
            .ok_or_else(AppError::not_found)?;
        if report.reported as u64 != target {
            return Err(AppError::bad_request(
                "The report is about another account.",
            ));
        }
        if report.status != "open" {
            return Err(AppError::limit("This report is decided already."));
        }
    }
    let id = store::insert_action(
        &mut tx,
        &store::NewAction {
            account: target,
            account_handle: &account.handle,
            kind: request.kind.as_str(),
            rule: request.rule.as_str(),
            reason: &reason,
            now: created,
            until,
            actor: actor.id,
            actor_handle: &actor.handle,
            report: request.report,
        },
    )
    .await?;
    if let Some(report_id) = request.report {
        let decision = store::Decision {
            outcome: ReportOutcome::Actioned.as_str(),
            now: created,
            by: actor.id,
            by_handle: &actor.handle,
            action: Some(id),
            release_media_at: created + state.config.retention_reports_days * DAY,
        };
        if !store::decide_report(&mut tx, report_id, &decision).await? {
            // Another moderator decided it between the check and now.
            return Err(AppError::limit("This report is decided already."));
        }
    }
    if request.kind == ActionKind::Ban {
        let hash = state.ban_key.hash(&account.email);
        store::upsert_ban(&mut tx, target, &hash, until, &reason, id, created).await?;
        crate::lfg::forget_account(&mut tx, target, crate::util::now_ms()).await?;
    }
    let audit_action = match request.kind {
        ActionKind::Warning => AuditAction::Warned,
        ActionKind::Mute => AuditAction::Muted,
        ActionKind::Ban => AuditAction::Banned,
    };
    audit(
        &mut tx,
        actor,
        audit_action,
        Some((target, &account.handle)),
        &reason,
        Some(request.rule),
        Some((ReferenceKind::Action, id)),
    )
    .await?;
    let row = store::action(&mut tx, id)
        .await?
        .ok_or_else(AppError::internal)?;
    tx.commit().await?;

    let view = action_view(state, &row);
    match request.kind {
        ActionKind::Ban => {
            state
                .apply_ban(target, until, Some(view.statement.clone()))
                .await?;
        }
        _ => state.news.push(
            target,
            &Event::Action {
                statement: Box::new(view.statement.clone()),
            },
        ),
    }
    tracing::warn!(
        account = target,
        kind = request.kind.as_str(),
        "moderation action"
    );
    Ok(view)
}

pub async fn actions_about(state: &AppState, account: u64, limit: i64) -> Result<Vec<ActionView>> {
    let mut conn = state.db.acquire().await?;
    let rows = store::actions_of(&mut conn, account, limit.clamp(1, 1000)).await?;
    Ok(rows.iter().map(|r| action_view(state, r)).collect())
}

/// Takes an action back: a mute ends, a ban is lifted, a warning is withdrawn.
pub async fn lift(state: &AppState, actor: &Actor, id: u64, reason: &str) -> Result<ActionView> {
    let reason = valid_reason(reason)?;
    let mut tx = begin(state).await?;
    let row = store::action(&mut tx, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    guard_target(&mut tx, actor, row.account_id as u64).await?;
    let ban_lifted = lift_in(&mut tx, actor, &row, &reason)
        .await?
        .ok_or_else(|| AppError::limit("This action was lifted already."))?;
    let row = store::action(&mut tx, id)
        .await?
        .ok_or_else(AppError::internal)?;
    tx.commit().await?;
    if ban_lifted {
        state.lift_ban(row.account_id as u64);
    }
    let view = action_view(state, &row);
    state.news.push(
        row.account_id as u64,
        &Event::Action {
            statement: Box::new(view.statement.clone()),
        },
    );
    Ok(view)
}

/// The legacy `unban`: lifts the ban on an account whatever action made it.
pub async fn unban(state: &AppState, actor: &Actor, account: u64) -> Result<()> {
    let ban = db::ban_of(&state.db, account)
        .await?
        .ok_or_else(AppError::not_found)?;
    match ban.action_id {
        Some(action) => {
            lift(state, actor, action as u64, "The ban was lifted.").await?;
        }
        None => {
            let mut tx = begin(state).await?;
            store::delete_ban(&mut tx, account, None).await?;
            audit(
                &mut tx,
                actor,
                AuditAction::Lifted,
                Some((account, "")),
                "The ban was lifted.",
                None,
                None,
            )
            .await?;
            tx.commit().await?;
            state.lift_ban(account);
        }
    }
    Ok(())
}

/// Marks an action lifted, removes its ban and writes the audit line. `None` when it was lifted
/// already; otherwise whether a ban was removed.
async fn lift_in(
    conn: &mut crate::live::Conn,
    actor: &Actor,
    row: &ActionRow,
    reason: &str,
) -> sqlx::Result<Option<bool>> {
    let id = row.id as u64;
    if !store::mark_lifted(conn, id, now()).await? {
        return Ok(None);
    }
    let ban_removed = row.kind == ActionKind::Ban.as_str()
        && store::delete_ban(conn, row.account_id as u64, Some(id)).await?;
    audit(
        conn,
        actor,
        AuditAction::Lifted,
        Some((row.account_id as u64, &row.account_handle)),
        reason,
        Rule::from_name(&row.rule),
        Some((ReferenceKind::Action, id)),
    )
    .await?;
    Ok(Some(ban_removed))
}

// What the player sees

/// The actions against an account, newest first.
pub async fn statements(state: &AppState, account: u64) -> Result<Vec<Statement>> {
    let mut conn = state.db.acquire().await?;
    let rows = store::actions_of(&mut conn, account, 500).await?;
    Ok(rows.iter().map(|r| statement_of(state, r)).collect())
}

/// Files the one appeal of an action. `account` owns the action.
pub async fn appeal(state: &AppState, account: u64, action: u64, text: &str) -> Result<Statement> {
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("appeal:{account}"),
            rates.appeal_per_account,
            rates.appeal_window,
        )
        .map_err(AppError::rate_limited)?;
    let text = text.trim();
    let length = text.chars().count();
    if length == 0 || length > api::MAX_APPEAL_TEXT {
        return Err(AppError::bad_request(
            "An appeal has 1 to 1,000 characters.",
        ));
    }
    let mut tx = begin(state).await?;
    let row = store::action(&mut tx, action)
        .await?
        .filter(|r| r.account_id as u64 == account)
        .ok_or_else(AppError::not_found)?;
    if row.lifted.is_some() {
        return Err(AppError::bad_request("This action was lifted already."));
    }
    if row.appeal_id.is_some() {
        return Err(AppError::limit("This action has an appeal already."));
    }
    let created = now();
    if created >= deadline(state, &row) {
        return Err(AppError::forbidden("The time to appeal has ended."));
    }
    let id = match store::insert_appeal(
        &mut tx,
        &store::NewAppeal {
            action,
            account,
            text,
            now: created,
        },
    )
    .await
    {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::limit("This action has an appeal already."));
        }
        Err(e) => return Err(e.into()),
    };
    audit(
        &mut tx,
        &Actor::account(account, row.account_handle.clone()),
        AuditAction::AppealFiled,
        Some((account, &row.account_handle)),
        text,
        Rule::from_name(&row.rule),
        Some((ReferenceKind::Appeal, id)),
    )
    .await?;
    let row = store::action(&mut tx, action)
        .await?
        .ok_or_else(AppError::internal)?;
    tx.commit().await?;
    Ok(statement_of(state, &row))
}

/// The ban an account is under, as its statement of reasons, and a token to appeal with.
pub async fn banned_error(state: &AppState, account: u64, until: Option<u64>) -> AppError {
    let statement = match ban_statement(state, account).await {
        Ok(statement) => statement,
        Err(error) => {
            tracing::error!(error = %error.body.message, "cannot read the statement of a ban");
            None
        }
    };
    let issued = now();
    let token = state
        .signer
        .issue(&Claims::appeal(account, issued, issued + APPEAL_TOKEN_SECS));
    AppError::banned(until).with_ban(BanNotice {
        statement,
        appeal_token: token,
    })
}

/// The statement behind the ban of an account, if the ban has one.
pub async fn ban_statement(state: &AppState, account: u64) -> Result<Option<Statement>> {
    let Some(ban) = db::ban_of(&state.db, account).await? else {
        return Ok(None);
    };
    let Some(action) = ban.action_id else {
        return Ok(None);
    };
    let mut conn = state.db.acquire().await?;
    Ok(store::action(&mut conn, action as u64)
        .await?
        .map(|row| statement_of(state, &row)))
}

// Appeals

/// The open appeals (oldest first), or the decided ones.
pub async fn appeals(state: &AppState, status: &str, limit: i64) -> Result<Vec<AppealView>> {
    let limit = limit.clamp(1, 1000);
    let mut conn = state.db.acquire().await?;
    let rows = match status {
        "open" => store::open_appeals(&mut conn, limit).await?,
        "upheld" | "reversed" => store::decided_appeals(&mut conn, status, limit).await?,
        "all" => store::decided_appeals(&mut conn, "", limit).await?,
        _ => {
            return Err(AppError::bad_request(
                "status is open, upheld, reversed or all.",
            ));
        }
    };
    Ok(rows.iter().filter_map(|r| appeal_view(state, r)).collect())
}

/// Decides an appeal. A different moderator than the one who acted decides it when there is one.
pub async fn decide_appeal(
    state: &AppState,
    actor: &Actor,
    id: u64,
    decision: AppealDecision,
) -> Result<AppealView> {
    let reason = valid_reason(&decision.reason)?;
    let mut tx = begin(state).await?;
    let row = store::appeal(&mut tx, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if row.appeal_status.as_deref() != Some("open") {
        return Err(AppError::limit("This appeal is decided already."));
    }
    guard_target(&mut tx, actor, row.account_id as u64).await?;
    if actor.id == row.actor.map(|a| a as u64)
        && store::other_moderators(&mut tx, actor.id).await? > 0
    {
        return Err(AppError::forbidden(
            "A different moderator has to decide this appeal.",
        ));
    }
    let (status, audit_action) = match decision.outcome {
        AppealOutcome::Uphold => (AppealStatus::Upheld, AuditAction::AppealUpheld),
        AppealOutcome::Reverse => (AppealStatus::Reversed, AuditAction::AppealReversed),
    };
    let verdict = store::Verdict {
        status: status.as_str(),
        now: now(),
        by: actor.id,
        by_handle: &actor.handle,
        answer: &reason,
    };
    if !store::decide_appeal(&mut tx, id, &verdict).await? {
        return Err(AppError::limit("This appeal is decided already."));
    }
    audit(
        &mut tx,
        actor,
        audit_action,
        Some((row.account_id as u64, &row.account_handle)),
        &reason,
        Rule::from_name(&row.rule),
        Some((ReferenceKind::Appeal, id)),
    )
    .await?;
    let mut ban_lifted = false;
    if decision.outcome == AppealOutcome::Reverse {
        ban_lifted = lift_in(&mut tx, actor, &row, &reason)
            .await?
            .unwrap_or(false);
    }
    let row = store::appeal(&mut tx, id)
        .await?
        .ok_or_else(AppError::internal)?;
    tx.commit().await?;
    if ban_lifted {
        state.lift_ban(row.account_id as u64);
    }
    state.news.push(
        row.account_id as u64,
        &Event::Action {
            statement: Box::new(statement_of(state, &row)),
        },
    );
    appeal_view(state, &row).ok_or_else(AppError::internal)
}

// The audit log

/// Lines of the audit log older than the line `before` (0 for the newest), newest first.
pub async fn log(state: &AppState, before: u64, limit: i64) -> Result<Vec<AuditEntry>> {
    let mut conn = state.db.acquire().await?;
    let rows = store::entries(&mut conn, before, limit.clamp(1, 1000)).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(AuditEntry {
                id: u(r.id),
                time: u(r.created),
                actor: api::Actor {
                    id: r.actor.map(|a| api::AccountId(a as u64)),
                    handle: r.actor_handle,
                },
                action: AuditAction::from_name(&r.action)?,
                target: r.target.map(|t| api::Account {
                    id: api::AccountId(t as u64),
                    handle: r.target_handle.clone().unwrap_or_default(),
                }),
                reason: r.reason,
                rule: r.rule.as_deref().and_then(Rule::from_name),
                reference: r
                    .ref_kind
                    .as_deref()
                    .and_then(ReferenceKind::from_name)
                    .zip(r.ref_id)
                    .map(|(kind, id)| api::Reference { kind, id: u(id) }),
            })
        })
        .collect())
}

async fn audit(
    conn: &mut crate::live::Conn,
    actor: &Actor,
    action: AuditAction,
    target: Option<(u64, &str)>,
    reason: &str,
    rule: Option<Rule>,
    reference: Option<(ReferenceKind, u64)>,
) -> sqlx::Result<()> {
    store::insert_entry(
        conn,
        &store::NewEntry {
            now: now(),
            actor: actor.id,
            actor_handle: &actor.handle,
            action: action.as_str(),
            target: target.map(|(id, _)| id),
            target_handle: target.map(|(_, handle)| handle),
            reason,
            rule: rule.map(Rule::as_str),
            reference: reference.map(|(kind, id)| (kind.as_str(), id)),
        },
    )
    .await
}

// Moderators

pub async fn moderators(state: &AppState) -> Result<Vec<api::Moderator>> {
    let mut conn = state.db.acquire().await?;
    Ok(store::moderators(&mut conn)
        .await?
        .into_iter()
        .map(|(id, handle, since)| api::Moderator {
            account: api::Account {
                id: api::AccountId(id as u64),
                handle,
            },
            since: since as u64,
        })
        .collect())
}

/// Gives an account the moderator role. Only the operator does.
pub async fn grant(state: &AppState, actor: &Actor, handle: &str) -> Result<api::Moderator> {
    actor.require_operator()?;
    let account = db::account_by_handle(&state.db, &handle.to_ascii_lowercase())
        .await?
        .ok_or_else(AppError::not_found)?;
    let mut tx = begin(state).await?;
    let since = match store::moderator_since(&mut tx, account.id()).await? {
        Some(since) => since,
        None => {
            let since = now();
            store::set_moderator(&mut tx, account.id(), Some(since)).await?;
            audit(
                &mut tx,
                actor,
                AuditAction::ModeratorGranted,
                Some((account.id(), &account.handle)),
                "The moderator role was granted.",
                None,
                None,
            )
            .await?;
            since
        }
    };
    tx.commit().await?;
    Ok(api::Moderator {
        account: api::Account {
            id: api::AccountId(account.id()),
            handle: account.handle,
        },
        since,
    })
}

/// Takes the moderator role back. Only the operator does.
pub async fn revoke(state: &AppState, actor: &Actor, account: u64) -> Result<()> {
    actor.require_operator()?;
    let row = db::account(&state.db, account)
        .await?
        .ok_or_else(AppError::not_found)?;
    let mut tx = begin(state).await?;
    if store::moderator_since(&mut tx, account).await?.is_none() {
        return Err(AppError::not_found());
    }
    store::set_moderator(&mut tx, account, None).await?;
    audit(
        &mut tx,
        actor,
        AuditAction::ModeratorRevoked,
        Some((account, &row.handle)),
        "The moderator role was taken back.",
        None,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// What an account may do

/// The end of the time an account counts as new, or `None` when it is old enough (or the rule is
/// off).
async fn new_until(state: &crate::state::Inner, account: u64) -> Result<Option<u64>> {
    let hours = state.config.new_account_hours;
    if hours == 0 {
        return Ok(None);
    }
    let Some(row) = db::account(&state.db, account).await? else {
        return Ok(None);
    };
    let until = u(row.created) + hours * 60 * 60;
    Ok((now() < until).then_some(until))
}

/// Fails with `restricted` while a moderator's mute on the account runs.
pub async fn ensure_not_muted(state: &crate::state::Inner, account: u64) -> Result<()> {
    let mut conn = state.db.acquire().await?;
    match store::active_mute(&mut conn, account, now()).await? {
        Some(until) => Err(AppError::restricted(
            Restriction::Muted,
            until,
            "A moderator muted this account.",
        )),
        None => Ok(()),
    }
}

/// The one rule for writing to someone who is not a friend, which is how a conversation with a
/// stranger starts: the account must be older than `new_account_hours` and not muted.
pub async fn may_write_to_strangers(state: &crate::state::Inner, account: u64) -> Result<()> {
    if let Some(until) = new_until(state, account).await? {
        return Err(AppError::restricted(
            Restriction::NewAccount,
            Some(until),
            "A new account cannot write to people who are not its friends yet. Try again later.",
        ));
    }
    ensure_not_muted(state, account).await
}

/// The rules for one more friend request by an account, applied by [`FriendRequestRules::check`]
/// in the request's transaction. A muted account sends none; a new account sends
/// `new_account_friend_requests_per_day` while it is new. Whether the account is new is read
/// here, before the transaction, which must not wait for a second store connection while it
/// holds one; it changes only with time. The mute and the count are read in the transaction.
pub async fn friend_request_rules(
    state: &crate::state::Inner,
    account: u64,
) -> Result<FriendRequestRules> {
    let new = new_until(state, account).await?.map(|until| NewAccount {
        until,
        since: until.saturating_sub(state.config.new_account_hours * 60 * 60),
        per_day: state.config.new_account_friend_requests_per_day,
    });
    Ok(FriendRequestRules { account, new })
}

/// What [`friend_request_rules`] read.
pub struct FriendRequestRules {
    account: u64,
    new: Option<NewAccount>,
}

struct NewAccount {
    until: u64,
    since: u64,
    per_day: u32,
}

impl FriendRequestRules {
    /// Fails when a mute runs or a new account used up its allowance. Call it in the transaction
    /// that holds the account, so that two requests at once cannot both take the last one.
    pub async fn check(&self, conn: &mut sqlx::AnyConnection) -> Result<()> {
        if let Some(until) = store::active_mute(conn, self.account, now()).await? {
            return Err(AppError::restricted(
                Restriction::Muted,
                until,
                "A moderator muted this account.",
            ));
        }
        let Some(new) = &self.new else {
            return Ok(());
        };
        let sent = store::count_new_account_requests(conn, self.account, new.since).await?;
        if sent >= i64::from(new.per_day) {
            return Err(AppError::restricted(
                Restriction::NewAccount,
                Some(new.until),
                format!(
                    "A new account may send {} friend requests on its first day. Try again later.",
                    new.per_day
                ),
            ));
        }
        Ok(())
    }

    /// Counts a stored request against a new account's allowance, in the same transaction.
    pub async fn record(&self, conn: &mut sqlx::AnyConnection) -> Result<()> {
        if self.new.is_some() {
            store::insert_new_account_request(conn, self.account, now()).await?;
        }
        Ok(())
    }
}

// Retention

/// Deletes what has outlived its retention period. Safe to run on several nodes at once.
pub async fn sweep_retention(state: &crate::state::Inner) -> Result<store::Swept> {
    let config = &state.config;
    let now = now();
    let ago = |days: u64| now.saturating_sub(days.saturating_mul(DAY));
    // The deletes are not capped, so they run without the statement timeout of requests.
    let mut conn = crate::db::acquire_maintenance(&state.db).await?;
    let bans = store::delete_old_bans(&mut conn, ago(config.retention_bans_days)).await?;
    let actions = store::delete_old_actions(&mut conn, ago(config.retention_bans_days)).await?;
    let reports = store::delete_old_reports(
        &mut conn,
        ago(config.retention_reports_days),
        ago(config.retention_open_reports_days),
    )
    .await?;
    let log_lines = store::delete_old_log(&mut conn, ago(config.retention_audit_days)).await?;
    // The allowance of a new account only matters for its first day.
    store::delete_old_new_account_requests(&mut conn, ago(2)).await?;
    Ok(store::Swept {
        bans,
        actions,
        reports,
        log_lines,
    })
}

// Views

fn u(id: i64) -> u64 {
    id as u64
}

fn deadline(state: &AppState, row: &ActionRow) -> u64 {
    u(row.created) + state.config.appeal_days * DAY
}

fn statement_of(state: &AppState, row: &ActionRow) -> Statement {
    let deadline = deadline(state, row);
    let status = match row.appeal_status.as_deref() {
        Some("open") => AppealStatus::Open,
        Some("upheld") => AppealStatus::Upheld,
        Some("reversed") => AppealStatus::Reversed,
        _ if now() >= deadline => AppealStatus::Expired,
        _ => AppealStatus::Available,
    };
    Statement {
        id: u(row.id),
        kind: ActionKind::from_name(&row.kind).unwrap_or(ActionKind::Warning),
        rule: Rule::from_name(&row.rule).unwrap_or(Rule::Other),
        reason: row.reason.clone(),
        created: u(row.created),
        until: row.until.map(u),
        lifted: row.lifted.map(u),
        from_report: row.report_id.is_some(),
        automated: false,
        appeal: AppealInfo {
            status,
            deadline,
            text: row.appeal_text.clone(),
            answer: row.appeal_answer.clone(),
            decided: row.appeal_decided.map(u),
        },
    }
}

fn action_view(state: &AppState, row: &ActionRow) -> ActionView {
    ActionView {
        account: api::Account {
            id: api::AccountId(u(row.account_id)),
            handle: row.account_handle.clone(),
        },
        actor: api::Actor {
            id: row.actor.map(|a| api::AccountId(u(a))),
            handle: row.actor_handle.clone(),
        },
        statement: statement_of(state, row),
    }
}

fn appeal_view(state: &AppState, row: &ActionRow) -> Option<AppealView> {
    Some(AppealView {
        id: u(row.appeal_id?),
        action: action_view(state, row),
        created: u(row.appeal_created?),
        decided_by: row.appeal_decided.map(|_| api::Actor {
            id: row.appeal_decided_by.map(|a| api::AccountId(u(a))),
            handle: row.appeal_decided_by_handle.clone().unwrap_or_default(),
        }),
    })
}

fn report_view(row: &ReportRow) -> ReportView {
    ReportView {
        id: u(row.id),
        reporter: row.reporter.map(|id| api::Account {
            id: api::AccountId(u(id)),
            handle: row.reporter_handle.clone(),
        }),
        reported: api::Account {
            id: api::AccountId(u(row.reported)),
            handle: row.reported_handle.clone(),
        },
        reported_deleted: row.reported_exists == 0,
        reason: serde_json::from_value(serde_json::Value::String(row.reason.clone()))
            .unwrap_or(ReportReason::Other),
        details: row.details.clone(),
        content: row.content.clone(),
        created: u(row.created),
        status: ReportStatus::from_name(&row.status).unwrap_or(ReportStatus::Open),
        outcome: row.outcome.as_deref().and_then(ReportOutcome::from_name),
        decided: row.decided.map(u),
        decided_by: row.decided.map(|_| api::Actor {
            id: row.decided_by.map(|a| api::AccountId(u(a))),
            handle: row.decided_by_handle.clone().unwrap_or_default(),
        }),
        action: row.action_id.map(u),
    }
}

/// Starts a transaction that will write; see [`crate::state::Inner::begin_write`].
async fn begin(state: &crate::state::Inner) -> sqlx::Result<crate::state::WriteTx> {
    state.begin_write().await
}
