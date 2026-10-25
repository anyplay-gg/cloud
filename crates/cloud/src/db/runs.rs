//! Run reports (migration 0015).

use anyplay_api::{Os, RunReport, Verdict, VerdictCounts};

use super::{Pool, Writer, i};

/// The most run reports an account keeps; a new answer for a known title, game version and
/// toolkit replaces its row and does not count against this.
pub const MAX_RUN_REPORT_ROWS: i64 = 1000;

pub fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::NoStart => "no_start",
        Verdict::Menus => "menus",
        Verdict::InGame => "in_game",
        Verdict::Playable => "playable",
    }
}

fn verdict_of(name: &str) -> Option<Verdict> {
    Some(match name {
        "no_start" => Verdict::NoStart,
        "menus" => Verdict::Menus,
        "in_game" => Verdict::InGame,
        "playable" => Verdict::Playable,
        _ => return None,
    })
}

pub fn os_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Macos => "macos",
    }
}

/// Stores the report, replacing the account's earlier one for the same title, game version and
/// toolkit. Returns false, and stores nothing, when the account is at [`MAX_RUN_REPORT_ROWS`] and
/// this would be a new row.
pub async fn put(
    writer: &Writer,
    pool: &Pool,
    account: u64,
    title_id: &str,
    report: &RunReport,
    now: u64,
) -> sqlx::Result<bool> {
    let mut tx = writer.begin(pool).await?;
    let (known,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM run_reports \
         WHERE account_id = $1 AND title_id = $2 AND version = $3 AND toolkit = $4",
    )
    .bind(i(account))
    .bind(title_id)
    .bind(&report.version)
    .bind(&report.toolkit)
    .fetch_one(&mut *tx)
    .await?;
    if known == 0 {
        let (rows,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM run_reports WHERE account_id = $1")
                .bind(i(account))
                .fetch_one(&mut *tx)
                .await?;
        if rows >= MAX_RUN_REPORT_ROWS {
            return Ok(false);
        }
    }
    sqlx::query(
        "INSERT INTO run_reports (account_id, title_id, version, toolkit, verdict, os, updated) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (account_id, title_id, version, toolkit) \
         DO UPDATE SET verdict = $5, os = $6, updated = $7",
    )
    .bind(i(account))
    .bind(title_id)
    .bind(&report.version)
    .bind(&report.toolkit)
    .bind(verdict_name(report.verdict))
    .bind(os_name(report.os))
    .bind(i(now))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// How many accounts gave each verdict for the title, or for one game version of it. An account
/// counts once, with its latest report in that scope, so one player cannot make a summary alone.
pub async fn counts(
    pool: &Pool,
    title_id: &str,
    version: Option<&str>,
) -> sqlx::Result<VerdictCounts> {
    let scope = if version.is_some() {
        " AND version = $2"
    } else {
        ""
    };
    let sql = format!(
        "SELECT verdict, COUNT(*) FROM (\
           SELECT verdict, ROW_NUMBER() OVER (\
             PARTITION BY account_id ORDER BY updated DESC, version DESC, toolkit DESC) AS pos \
           FROM run_reports WHERE title_id = $1{scope}\
         ) AS latest WHERE pos = 1 GROUP BY verdict"
    );
    let mut query = sqlx::query_as::<_, (String, i64)>(sqlx::AssertSqlSafe(sql)).bind(title_id);
    if let Some(version) = version {
        query = query.bind(version);
    }
    let mut counts = VerdictCounts::default();
    for (verdict, n) in query.fetch_all(pool).await? {
        if let Some(verdict) = verdict_of(&verdict) {
            counts.add(verdict, u32::try_from(n).unwrap_or(u32::MAX));
        }
    }
    Ok(counts)
}

/// One run report of an account, for the export.
pub struct Row {
    pub title_id: String,
    pub version: String,
    pub toolkit: String,
    pub verdict: String,
    pub os: String,
    pub updated: i64,
}

/// Every run report of an account, for the export.
pub async fn of_account(pool: &Pool, account: u64) -> sqlx::Result<Vec<Row>> {
    let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT title_id, version, toolkit, verdict, os, updated FROM run_reports \
         WHERE account_id = $1 ORDER BY title_id, version, toolkit",
    )
    .bind(i(account))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(title_id, version, toolkit, verdict, os, updated)| Row {
            title_id,
            version,
            toolkit,
            verdict,
            os,
            updated,
        })
        .collect())
}

/// How many accounts gave each verdict, per title and per title and game version, each account
/// with its latest report in that scope: `(title_id, version, verdict, accounts)`. A `None`
/// version is the whole title.
pub async fn all_counts(pool: &Pool) -> sqlx::Result<Vec<(String, Option<String>, Verdict, u32)>> {
    let mut rows = Vec::new();
    for by_version in [false, true] {
        let (partition, select, group) = if by_version {
            (
                "account_id, title_id, version",
                "title_id, version, verdict",
                "title_id, version, verdict",
            )
        } else {
            (
                "account_id, title_id",
                "title_id, '' AS version, verdict",
                "title_id, verdict",
            )
        };
        let sql = format!(
            "SELECT {select}, COUNT(*) FROM (\
               SELECT title_id, version, verdict, ROW_NUMBER() OVER (\
                 PARTITION BY {partition} ORDER BY updated DESC, version DESC, toolkit DESC) AS pos \
               FROM run_reports\
             ) AS latest WHERE pos = 1 GROUP BY {group}"
        );
        let found: Vec<(String, String, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_all(pool)
            .await?;
        for (title, version, verdict, n) in found {
            if let Some(verdict) = verdict_of(&verdict) {
                rows.push((
                    title,
                    by_version.then_some(version),
                    verdict,
                    u32::try_from(n).unwrap_or(u32::MAX),
                ));
            }
        }
    }
    Ok(rows)
}
