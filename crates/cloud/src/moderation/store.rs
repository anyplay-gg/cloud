//! The moderation tables: reports, actions, appeals, the audit log and the moderator role. Like
//! [`crate::db`], each query is written once with `$1` parameters for SQLite and PostgreSQL. The
//! functions take a connection so that an action and its audit line can share one transaction.

use sqlx::FromRow;

use crate::live::Conn;

pub fn i(id: u64) -> i64 {
    id as i64
}

pub fn u(id: i64) -> u64 {
    id as u64
}

// Reports

#[derive(Clone, Debug, FromRow)]
pub struct ReportRow {
    pub id: i64,
    pub reporter: Option<i64>,
    pub reporter_handle: String,
    pub reported: i64,
    pub reported_handle: String,
    pub reason: String,
    pub details: String,
    pub content: String,
    pub created: i64,
    pub status: String,
    pub outcome: Option<String>,
    pub decided: Option<i64>,
    pub decided_by: Option<i64>,
    pub decided_by_handle: Option<String>,
    pub action_id: Option<i64>,
    pub reported_exists: i64,
}

macro_rules! report_select {
    ($tail:literal) => {
        concat!(
            "SELECT r.id, r.reporter, r.reporter_handle, r.reported, r.reported_handle, r.reason, \
             r.details, r.content, r.created, r.status, r.outcome, r.decided, r.decided_by, \
             r.decided_by_handle, r.action_id, \
             (SELECT COUNT(*) FROM accounts a WHERE a.id = r.reported) AS reported_exists \
             FROM reports r ",
            $tail
        )
    };
}

pub struct NewReport<'a> {
    pub reporter: u64,
    pub reporter_handle: &'a str,
    pub reported: u64,
    pub reported_handle: &'a str,
    pub reason: &'a str,
    pub details: &'a str,
    pub content: &'a str,
    /// The screenshot or clip the report holds for the moderators.
    pub media_id: Option<&'a str>,
    pub now: u64,
}

pub async fn insert_report(conn: &mut Conn, report: &NewReport<'_>) -> sqlx::Result<u64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO reports (reporter, reporter_handle, reported, reported_handle, reason, \
         details, content, created, status, media_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'open', $9) RETURNING id",
    )
    .bind(i(report.reporter))
    .bind(report.reporter_handle)
    .bind(i(report.reported))
    .bind(report.reported_handle)
    .bind(report.reason)
    .bind(report.details)
    .bind(report.content)
    .bind(i(report.now))
    .bind(report.media_id)
    .fetch_one(conn)
    .await?;
    Ok(u(id))
}

pub async fn count_reports_since(conn: &mut Conn, reporter: u64, since: u64) -> sqlx::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM reports WHERE reporter = $1 AND created >= $2")
            .bind(i(reporter))
            .bind(i(since))
            .fetch_one(conn)
            .await?;
    Ok(n)
}

/// True while `reporter` has a report about `reported` that nobody has decided.
pub async fn open_report_exists(
    conn: &mut Conn,
    reporter: u64,
    reported: u64,
) -> sqlx::Result<bool> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM reports WHERE reporter = $1 AND reported = $2 AND status = 'open'",
    )
    .bind(i(reporter))
    .bind(i(reported))
    .fetch_one(conn)
    .await?;
    Ok(n > 0)
}

pub async fn report(conn: &mut Conn, id: u64) -> sqlx::Result<Option<ReportRow>> {
    sqlx::query_as(report_select!("WHERE r.id = $1"))
        .bind(i(id))
        .fetch_optional(conn)
        .await
}

/// The open reports, oldest first.
pub async fn open_reports(conn: &mut Conn, limit: i64) -> sqlx::Result<Vec<ReportRow>> {
    sqlx::query_as(report_select!(
        "WHERE r.status = 'open' ORDER BY r.created ASC, r.id ASC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(conn)
    .await
}

/// The reports with `status` (or all of them when empty), newest first.
pub async fn reports(conn: &mut Conn, status: &str, limit: i64) -> sqlx::Result<Vec<ReportRow>> {
    sqlx::query_as(report_select!(
        "WHERE ($2 = '' OR r.status = $2) ORDER BY r.created DESC, r.id DESC LIMIT $1"
    ))
    .bind(limit)
    .bind(status)
    .fetch_all(conn)
    .await
}

/// What the reporter filed, for their export.
pub async fn reports_by(conn: &mut Conn, reporter: u64) -> sqlx::Result<Vec<ReportRow>> {
    sqlx::query_as(report_select!(
        "WHERE r.reporter = $1 ORDER BY r.created DESC, r.id DESC"
    ))
    .bind(i(reporter))
    .fetch_all(conn)
    .await
}

pub struct Decision<'a> {
    pub outcome: &'a str,
    pub now: u64,
    pub by: Option<u64>,
    pub by_handle: &'a str,
    pub action: Option<u64>,
    /// When the file the report held is deleted at the latest (seconds): a decided report keeps
    /// it as long as the report itself is kept.
    pub release_media_at: u64,
}

/// Decides an open report. False when it was decided already. A file the report held is released
/// to `release_media_at`, unless another undecided report holds it too.
pub async fn decide_report(conn: &mut Conn, id: u64, d: &Decision<'_>) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE reports SET status = 'decided', outcome = $2, decided = $3, decided_by = $4, \
         decided_by_handle = $5, action_id = $6 WHERE id = $1 AND status = 'open'",
    )
    .bind(i(id))
    .bind(d.outcome)
    .bind(i(d.now))
    .bind(d.by.map(i))
    .bind(d.by_handle)
    .bind(d.action.map(i))
    .execute(&mut *conn)
    .await?;
    if result.rows_affected() != 1 {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE media SET hold_until_ms = $2 WHERE hold_until_ms > $2 \
         AND id = (SELECT media_id FROM reports WHERE id = $1) \
         AND NOT EXISTS (SELECT 1 FROM reports r WHERE r.media_id = media.id AND r.status = 'open')",
    )
    .bind(i(id))
    .bind(i(d.release_media_at) * 1000)
    .execute(conn)
    .await?;
    Ok(true)
}

/// Forgets who filed the reports of an account that is being deleted. The reports stay, as
/// evidence about the accounts they name.
pub async fn forget_reporter(conn: &mut Conn, reporter: u64) -> sqlx::Result<()> {
    sqlx::query("UPDATE reports SET reporter = NULL, reporter_handle = '' WHERE reporter = $1")
        .bind(i(reporter))
        .execute(conn)
        .await?;
    Ok(())
}

// Actions and appeals

#[derive(Clone, Debug, FromRow)]
pub struct ActionRow {
    pub id: i64,
    pub account_id: i64,
    pub account_handle: String,
    pub kind: String,
    pub rule: String,
    pub reason: String,
    pub created: i64,
    pub until: Option<i64>,
    pub actor: Option<i64>,
    pub actor_handle: String,
    pub report_id: Option<i64>,
    pub lifted: Option<i64>,
    pub appeal_id: Option<i64>,
    pub appeal_status: Option<String>,
    pub appeal_text: Option<String>,
    pub appeal_created: Option<i64>,
    pub appeal_decided: Option<i64>,
    pub appeal_decided_by: Option<i64>,
    pub appeal_decided_by_handle: Option<String>,
    pub appeal_answer: Option<String>,
}

macro_rules! action_select {
    ($tail:literal) => {
        concat!(
            "SELECT x.id, x.account_id, x.account_handle, x.kind, x.rule, x.reason, x.created, \
             x.until, x.actor, x.actor_handle, x.report_id, x.lifted, p.id AS appeal_id, \
             p.status AS appeal_status, p.text AS appeal_text, p.created AS appeal_created, \
             p.decided AS appeal_decided, p.decided_by AS appeal_decided_by, \
             p.decided_by_handle AS appeal_decided_by_handle, p.answer AS appeal_answer \
             FROM actions x LEFT JOIN appeals p ON p.action_id = x.id ",
            $tail
        )
    };
}

pub struct NewAction<'a> {
    pub account: u64,
    pub account_handle: &'a str,
    pub kind: &'a str,
    pub rule: &'a str,
    pub reason: &'a str,
    pub now: u64,
    pub until: Option<u64>,
    pub actor: Option<u64>,
    pub actor_handle: &'a str,
    pub report: Option<u64>,
}

pub async fn insert_action(conn: &mut Conn, a: &NewAction<'_>) -> sqlx::Result<u64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO actions (account_id, account_handle, kind, rule, reason, created, until, \
         actor, actor_handle, report_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING id",
    )
    .bind(i(a.account))
    .bind(a.account_handle)
    .bind(a.kind)
    .bind(a.rule)
    .bind(a.reason)
    .bind(i(a.now))
    .bind(a.until.map(i))
    .bind(a.actor.map(i))
    .bind(a.actor_handle)
    .bind(a.report.map(i))
    .fetch_one(conn)
    .await?;
    Ok(u(id))
}

pub async fn action(conn: &mut Conn, id: u64) -> sqlx::Result<Option<ActionRow>> {
    sqlx::query_as(action_select!("WHERE x.id = $1"))
        .bind(i(id))
        .fetch_optional(conn)
        .await
}

/// The actions about an account, newest first.
pub async fn actions_of(conn: &mut Conn, account: u64, limit: i64) -> sqlx::Result<Vec<ActionRow>> {
    sqlx::query_as(action_select!(
        "WHERE x.account_id = $1 ORDER BY x.created DESC, x.id DESC LIMIT $2"
    ))
    .bind(i(account))
    .bind(limit)
    .fetch_all(conn)
    .await
}

/// Takes an action back. False when it was lifted already.
pub async fn mark_lifted(conn: &mut Conn, id: u64, now: u64) -> sqlx::Result<bool> {
    let result = sqlx::query("UPDATE actions SET lifted = $2 WHERE id = $1 AND lifted IS NULL")
        .bind(i(id))
        .bind(i(now))
        .execute(conn)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// The end of the mute that applies to an account at `now`: `Some(None)` for good.
pub async fn active_mute(
    conn: &mut Conn,
    account: u64,
    now: u64,
) -> sqlx::Result<Option<Option<u64>>> {
    let rows: Vec<(Option<i64>,)> = sqlx::query_as(
        "SELECT until FROM actions WHERE account_id = $1 AND kind = 'mute' AND lifted IS NULL \
         AND (until IS NULL OR until > $2)",
    )
    .bind(i(account))
    .bind(i(now))
    .fetch_all(conn)
    .await?;
    // The longest one counts; no end is the longest.
    let mut longest: Option<Option<u64>> = None;
    for (until,) in rows {
        let until = until.map(u);
        longest = match (longest, until) {
            (None, until) => Some(until),
            (Some(None), _) | (_, None) => Some(None),
            (Some(Some(a)), Some(b)) => Some(Some(a.max(b))),
        };
    }
    Ok(longest)
}

pub struct NewAppeal<'a> {
    pub action: u64,
    pub account: u64,
    pub text: &'a str,
    pub now: u64,
}

pub async fn insert_appeal(conn: &mut Conn, a: &NewAppeal<'_>) -> sqlx::Result<u64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO appeals (action_id, account_id, text, created, status) \
         VALUES ($1, $2, $3, $4, 'open') RETURNING id",
    )
    .bind(i(a.action))
    .bind(i(a.account))
    .bind(a.text)
    .bind(i(a.now))
    .fetch_one(conn)
    .await?;
    Ok(u(id))
}

pub async fn appeal(conn: &mut Conn, id: u64) -> sqlx::Result<Option<ActionRow>> {
    sqlx::query_as(action_select!("WHERE p.id = $1"))
        .bind(i(id))
        .fetch_optional(conn)
        .await
}

/// Open appeals, oldest first.
pub async fn open_appeals(conn: &mut Conn, limit: i64) -> sqlx::Result<Vec<ActionRow>> {
    sqlx::query_as(action_select!(
        "WHERE p.status = 'open' ORDER BY p.created ASC, p.id ASC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(conn)
    .await
}

/// Decided appeals (or all when `status` is empty), newest first.
pub async fn decided_appeals(
    conn: &mut Conn,
    status: &str,
    limit: i64,
) -> sqlx::Result<Vec<ActionRow>> {
    sqlx::query_as(action_select!(
        "WHERE p.id IS NOT NULL AND ($2 = '' OR p.status = $2) \
         ORDER BY p.created DESC, p.id DESC LIMIT $1"
    ))
    .bind(limit)
    .bind(status)
    .fetch_all(conn)
    .await
}

pub struct Verdict<'a> {
    pub status: &'a str,
    pub now: u64,
    pub by: Option<u64>,
    pub by_handle: &'a str,
    pub answer: &'a str,
}

/// Decides an open appeal. False when it was decided already.
pub async fn decide_appeal(conn: &mut Conn, id: u64, v: &Verdict<'_>) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE appeals SET status = $2, decided = $3, decided_by = $4, decided_by_handle = $5, \
         answer = $6 WHERE id = $1 AND status = 'open'",
    )
    .bind(i(id))
    .bind(v.status)
    .bind(i(v.now))
    .bind(v.by.map(i))
    .bind(v.by_handle)
    .bind(v.answer)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

// Bans

/// Writes the ban of an account, replacing an older one. `email_hash` is the keyed hash.
pub async fn upsert_ban(
    conn: &mut Conn,
    account: u64,
    email_hash: &[u8],
    until: Option<u64>,
    reason: &str,
    action: u64,
    now: u64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO bans (account_id, email_hash, until, reason, created, action_id, salted) \
         VALUES ($1, $2, $3, $4, $5, $6, 1) \
         ON CONFLICT (account_id) DO UPDATE SET email_hash = excluded.email_hash, \
         until = excluded.until, reason = excluded.reason, created = excluded.created, \
         action_id = excluded.action_id, salted = 1",
    )
    .bind(i(account))
    .bind(email_hash)
    .bind(until.map(i))
    .bind(reason)
    .bind(i(now))
    .bind(i(action))
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes the ban of an account. With `action`, only when that action made it.
pub async fn delete_ban(conn: &mut Conn, account: u64, action: Option<u64>) -> sqlx::Result<bool> {
    let result =
        sqlx::query("DELETE FROM bans WHERE account_id = $1 AND ($2 = 0 OR action_id = $2)")
            .bind(i(account))
            .bind(action.map(i).unwrap_or(0))
            .execute(conn)
            .await?;
    Ok(result.rows_affected() == 1)
}

/// Gives a ban found by its old unsalted hash the keyed hash, now that the address is known.
pub async fn resalt_ban(conn: &mut Conn, account: u64, email_hash: &[u8]) -> sqlx::Result<()> {
    sqlx::query("UPDATE bans SET email_hash = $2, salted = 1 WHERE account_id = $1 AND salted = 0")
        .bind(i(account))
        .bind(email_hash)
        .execute(conn)
        .await?;
    Ok(())
}

/// The bans made before the keyed hash whose account still exists, with its address.
pub async fn unsalted_bans(conn: &mut Conn) -> sqlx::Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT b.account_id, a.email FROM bans b JOIN accounts a ON a.id = b.account_id \
         WHERE b.salted = 0",
    )
    .fetch_all(conn)
    .await
}

// Moderators

pub async fn moderator_since(conn: &mut Conn, account: u64) -> sqlx::Result<Option<u64>> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT moderator_since FROM accounts WHERE id = $1")
            .bind(i(account))
            .fetch_optional(conn)
            .await?;
    Ok(row.and_then(|(since,)| since).map(u))
}

pub async fn set_moderator(conn: &mut Conn, account: u64, since: Option<u64>) -> sqlx::Result<()> {
    sqlx::query("UPDATE accounts SET moderator_since = $2 WHERE id = $1")
        .bind(i(account))
        .bind(since.map(i))
        .execute(conn)
        .await?;
    Ok(())
}

/// The moderators as (account ID, handle, since), by when they were granted the role.
pub async fn moderators(conn: &mut Conn) -> sqlx::Result<Vec<(i64, String, i64)>> {
    sqlx::query_as(
        "SELECT id, handle, moderator_since FROM accounts WHERE moderator_since IS NOT NULL \
         ORDER BY moderator_since ASC, id ASC",
    )
    .fetch_all(conn)
    .await
}

/// How many accounts other than `except` hold the moderator role.
pub async fn other_moderators(conn: &mut Conn, except: Option<u64>) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM accounts WHERE moderator_since IS NOT NULL AND id <> $1",
    )
    .bind(except.map(i).unwrap_or(0))
    .fetch_one(conn)
    .await?;
    Ok(n)
}

// The audit log

pub struct NewEntry<'a> {
    pub now: u64,
    pub actor: Option<u64>,
    pub actor_handle: &'a str,
    pub action: &'a str,
    pub target: Option<u64>,
    pub target_handle: Option<&'a str>,
    pub reason: &'a str,
    pub rule: Option<&'a str>,
    pub reference: Option<(&'a str, u64)>,
}

pub async fn insert_entry(conn: &mut Conn, e: &NewEntry<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO audit_log (created, actor, actor_handle, action, target, target_handle, \
         reason, rule, ref_kind, ref_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(i(e.now))
    .bind(e.actor.map(i))
    .bind(e.actor_handle)
    .bind(e.action)
    .bind(e.target.map(i))
    .bind(e.target_handle)
    .bind(e.reason)
    .bind(e.rule)
    .bind(e.reference.map(|(kind, _)| kind))
    .bind(e.reference.map(|(_, id)| i(id)))
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, FromRow)]
pub struct EntryRow {
    pub id: i64,
    pub created: i64,
    pub actor: Option<i64>,
    pub actor_handle: String,
    pub action: String,
    pub target: Option<i64>,
    pub target_handle: Option<String>,
    pub reason: String,
    pub rule: Option<String>,
    pub ref_kind: Option<String>,
    pub ref_id: Option<i64>,
}

/// Lines older than `before` (an ID; 0 for the newest), newest first.
pub async fn entries(conn: &mut Conn, before: u64, limit: i64) -> sqlx::Result<Vec<EntryRow>> {
    sqlx::query_as(
        "SELECT id, created, actor, actor_handle, action, target, target_handle, reason, rule, \
         ref_kind, ref_id FROM audit_log WHERE ($1 = 0 OR id < $1) ORDER BY id DESC LIMIT $2",
    )
    .bind(i(before))
    .bind(limit)
    .fetch_all(conn)
    .await
}

// New accounts

/// Friend requests a new account sent since `since`.
pub async fn count_new_account_requests(
    conn: &mut Conn,
    account: u64,
    since: u64,
) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM new_account_requests WHERE account_id = $1 AND sent >= $2",
    )
    .bind(i(account))
    .bind(i(since))
    .fetch_one(conn)
    .await?;
    Ok(n)
}

pub async fn insert_new_account_request(
    conn: &mut Conn,
    account: u64,
    now: u64,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO new_account_requests (account_id, sent) VALUES ($1, $2)")
        .bind(i(account))
        .bind(i(now))
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn delete_old_new_account_requests(conn: &mut Conn, cutoff: u64) -> sqlx::Result<u64> {
    let result = sqlx::query("DELETE FROM new_account_requests WHERE sent < $1")
        .bind(i(cutoff))
        .execute(conn)
        .await?;
    Ok(result.rows_affected())
}

// Retention

/// What a retention sweep deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub bans: u64,
    pub actions: u64,
    pub reports: u64,
    pub log_lines: u64,
}

pub async fn delete_old_bans(conn: &mut Conn, cutoff: u64) -> sqlx::Result<u64> {
    let result = sqlx::query("DELETE FROM bans WHERE until IS NOT NULL AND until < $1")
        .bind(i(cutoff))
        .execute(conn)
        .await?;
    Ok(result.rows_affected())
}

/// Deletes actions that ended, were lifted or (warnings) were given before `cutoff`, unless an
/// appeal about them is still open. Their appeals go with them.
pub async fn delete_old_actions(conn: &mut Conn, cutoff: u64) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "DELETE FROM actions WHERE ((kind = 'warning' AND created < $1) \
         OR (kind <> 'warning' AND until IS NOT NULL AND until < $1) \
         OR (lifted IS NOT NULL AND lifted < $1)) \
         AND NOT EXISTS (SELECT 1 FROM appeals p WHERE p.action_id = actions.id AND p.status = 'open')",
    )
    .bind(i(cutoff))
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

pub async fn delete_old_reports(
    conn: &mut Conn,
    decided_before: u64,
    open_before: u64,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "DELETE FROM reports WHERE (status = 'decided' AND decided < $1) \
         OR (status = 'open' AND created < $2)",
    )
    .bind(i(decided_before))
    .bind(i(open_before))
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

pub async fn delete_old_log(conn: &mut Conn, cutoff: u64) -> sqlx::Result<u64> {
    let result = sqlx::query("DELETE FROM audit_log WHERE created < $1")
        .bind(i(cutoff))
        .execute(conn)
        .await?;
    Ok(result.rows_affected())
}
