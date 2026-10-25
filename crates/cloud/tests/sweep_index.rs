//! The offline sweep finds its accounts through the `presence_due` index instead of reading the
//! whole `presence` table. Runs on SQLite, and on PostgreSQL when `ANYPLAY_TEST_DATABASE_URL` is
//! set. `ANYPLAY_SWEEP_ROWS` sets the size of the PostgreSQL table (default 20 000).

mod common;

use anyplay_cloud::db::{self, Location, Pool};
use sqlx::Row as _;

const ACCOUNTS: i64 = 20_000;
const ONLINE: i64 = 20;

/// The statement of `live::sweep_offline` with a literal cut, since a plan is read per statement.
const SWEEP: &str = "UPDATE presence SET online = 0, activity = NULL \
    WHERE online = 1 AND last_seen_ms IS NOT NULL AND last_seen_ms <= 1000 \
    AND NOT EXISTS (SELECT 1 FROM connections c WHERE c.account_id = presence.account_id)";

async fn seed_postgres(pool: &Pool, accounts: i64) {
    sqlx::query(
        "INSERT INTO accounts (id, handle, handle_key, handle_skeleton, email, created) \
         SELECT g, 'p' || g, 'p' || g, 'p' || g, g || '@example.com', 1 \
         FROM generate_series(1, $1::bigint) g",
    )
    .bind(accounts)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO presence (account_id, online, activity, last_seen_ms, sent, revision) \
         SELECT g, CASE WHEN g <= $2::bigint THEN 1 ELSE 0 END, NULL, \
                CASE WHEN g <= $2::bigint THEN 500 ELSE 100 END, '{}', 0 \
         FROM generate_series(1, $1::bigint) g",
    )
    .bind(accounts)
    .bind(ONLINE)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE presence").execute(pool).await.unwrap();
}

async fn seed_sqlite(pool: &Pool, accounts: i64) {
    sqlx::query(
        "WITH RECURSIVE n(g) AS (SELECT 1 UNION ALL SELECT g + 1 FROM n WHERE g < $1) \
         INSERT INTO accounts (id, handle, handle_key, handle_skeleton, email, created) \
         SELECT g, 'p' || g, 'p' || g, 'p' || g, g || '@example.com', 1 FROM n",
    )
    .bind(accounts)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "WITH RECURSIVE n(g) AS (SELECT 1 UNION ALL SELECT g + 1 FROM n WHERE g < $1) \
         INSERT INTO presence (account_id, online, activity, last_seen_ms, sent, revision) \
         SELECT g, CASE WHEN g <= $2 THEN 1 ELSE 0 END, NULL, \
                CASE WHEN g <= $2 THEN 500 ELSE 100 END, '{}', 0 FROM n",
    )
    .bind(accounts)
    .bind(ONLINE)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE").execute(pool).await.unwrap();
}

fn rows() -> i64 {
    std::env::var("ANYPLAY_SWEEP_ROWS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(ACCOUNTS)
}

#[tokio::test]
async fn the_sweep_reads_the_partial_index_on_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(Location::File(&dir.path().join("sweep.db")), 2)
        .await
        .unwrap();
    seed_sqlite(&pool, ACCOUNTS).await;
    let plan = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {SWEEP}")))
        .fetch_all(&pool)
        .await
        .unwrap();
    let text = plan
        .iter()
        .map(|r| r.get::<String, _>(3))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("presence_due"), "{text}");
    let swept = sqlx::query(SWEEP).execute(&pool).await.unwrap();
    assert_eq!(swept.rows_affected(), ONLINE as u64);
}

async fn explain(pool: &Pool) -> String {
    // EXPLAIN ANALYZE runs the update, so it is rolled back to leave the rows for the next run.
    let mut tx = pool.begin().await.unwrap();
    let plan: Vec<(String,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, BUFFERS) {SWEEP}"
    )))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    plan.iter()
        .map(|r| r.0.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn the_sweep_reads_the_partial_index_on_postgres() {
    let Some(url) = common::postgres_database().await else {
        return;
    };
    let pool = db::open(Location::Postgres(&url), 2).await.unwrap();
    seed_postgres(&pool, rows()).await;

    // Without the index the sweep reads every row.
    sqlx::query("DROP INDEX presence_due")
        .execute(&pool)
        .await
        .unwrap();
    let before = explain(&pool).await;
    println!("--- without presence_due ---\n{before}");
    assert!(before.contains("Seq Scan on presence"), "{before}");

    // With the migration's index it reads only the accounts that are online.
    let create = include_str!("../migrations/postgres/0022_sweep_indexes.sql");
    sqlx::raw_sql(sqlx::AssertSqlSafe(create.to_owned()))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE presence")
        .execute(&pool)
        .await
        .unwrap();
    let after = explain(&pool).await;
    println!("--- with presence_due ---\n{after}");
    assert!(after.contains("presence_due"), "{after}");
    assert!(!after.contains("Seq Scan on presence"), "{after}");
}
