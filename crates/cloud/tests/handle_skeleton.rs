//! Handles that look alike are one handle (network `spec/cloud-api.md`, "handle"): sign-up
//! refuses a handle whose skeleton another account has, and the migration that added the skeleton
//! fills it for the accounts that exist. Like the other tests these run on SQLite, and on
//! PostgreSQL as well when `ANYPLAY_TEST_DATABASE_URL` is set.

mod common;

use std::path::Path;

use anyplay_api::{CodeAnswer, handle_skeleton};
use anyplay_cloud::db::{self, InsertError, Pool};
use common::TestCloud;
use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::migrate::Migrator;

/// A sign-up token for a new e-mail address.
async fn signup_token(cloud: &TestCloud, email: &str) -> String {
    let (challenge, code) = cloud.ask_for_code(email).await;
    let answer: CodeAnswer = cloud
        .enter_code(&challenge.challenge, &code, "pc")
        .await
        .ok(200)
        .json();
    match answer {
        CodeAnswer::NewAccount { signup, .. } => signup,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn sign_up_refuses_a_lookalike_of_a_taken_handle_like_a_taken_handle() {
    let cloud = TestCloud::start().await;
    cloud.sign_up("Flokzy").await;
    let signup = signup_token(&cloud, "other@example.com").await;
    // Capital I, digit one, separators: all pass for Flokzy.
    for lookalike in ["FIokzy", "F1okzy", "Flok_zy", "Flok-zy", "fl_ok-zy"] {
        cloud
            .try_sign_up(&signup, lookalike, true)
            .await
            .is_error("handle_taken", 409);
    }
    // The answer is the one of a taken handle, whatever the reason.
    let exact = cloud.try_sign_up(&signup, "flokzy", true).await;
    let lookalike = cloud.try_sign_up(&signup, "FIokzy", true).await;
    assert_eq!(exact.status, lookalike.status);
    assert_eq!(exact.body, lookalike.body);
    // A handle that only resembles it is free.
    cloud.try_sign_up(&signup, "Flokzx", true).await.ok(200);
}

#[tokio::test]
async fn lookalikes_of_reserved_names_are_reserved() {
    let cloud = TestCloud::start().await;
    let signup = signup_token(&cloud, "someone@example.com").await;
    for handle in ["AnyPIay", "any_play", "Adm1n", "PIaystation_fan", "r00t"] {
        cloud
            .try_sign_up(&signup, handle, true)
            .await
            .is_error("handle_invalid", 400);
    }
}

#[tokio::test]
async fn a_skeleton_is_held_by_one_account_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = db::open(db::Location::File(&dir.path().join("a.db")), 2)
        .await
        .unwrap();
    let mut pools = vec![sqlite];
    // Held until the end of the test: dropping it drops the database.
    let database = common::postgres_database().await;
    if let Some(database) = &database {
        pools.push(
            db::open(db::Location::Postgres(&database.url), 2)
                .await
                .unwrap(),
        );
    }
    for pool in pools {
        db::insert_account(&pool, 1, "Flokzy", "a@example.com", 1)
            .await
            .unwrap();
        for (id, handle) in [(2, "FIokzy"), (3, "F1okzy"), (4, "Flok_zy"), (5, "FLOKZY")] {
            assert_eq!(
                db::insert_account(&pool, id, handle, &format!("{id}@example.com"), 1).await,
                Err(InsertError::Handle),
                "{handle}"
            );
        }
        db::insert_account(&pool, 6, "Flokzx", "6@example.com", 1)
            .await
            .unwrap();
        pool.close().await;
    }
}

/// An account as an older cloud stored it: no skeleton.
async fn old_account(pool: &Pool, id: i64, handle: &str, created: i64) {
    sqlx::query(
        "INSERT INTO accounts (id, handle, handle_key, email, created) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(handle)
    .bind(handle.to_ascii_lowercase())
    .bind(format!("{id}@example.com"))
    .bind(created)
    .execute(pool)
    .await
    .unwrap();
}

async fn skeleton_of(pool: &Pool, id: i64) -> Option<String> {
    let (skeleton,): (Option<String>,) =
        sqlx::query_as("SELECT handle_skeleton FROM accounts WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    skeleton
}

async fn migrate_with_colliding_accounts(flavour: &str, url: &str) {
    let full = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join(flavour);
    // The schema before this change: every migration but 0014.
    let scratch = tempfile::tempdir().unwrap();
    let old = scratch.path().join("old-migrations");
    std::fs::create_dir_all(&old).unwrap();
    for entry in std::fs::read_dir(&full).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("0014") && name.ends_with(".sql") {
            std::fs::copy(&path, old.join(&name)).unwrap();
        }
    }
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .unwrap();
    Migrator::new(old.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();

    // Three that pass for each other (the oldest is 21), two on a tie of time (30 and 31), and
    // some that stand alone and use every mapping.
    old_account(&pool, 20, "Flokzy", 5).await;
    old_account(&pool, 21, "FIokzy", 3).await;
    old_account(&pool, 22, "F1okzy", 4).await;
    old_account(&pool, 31, "Sam_rnith", 7).await;
    old_account(&pool, 30, "Sam-Mith", 7).await;
    let alone = [
        (40, "Alec"),
        (41, "P1ayer_0ne"),
        (42, "Il1O0o-rnvv"),
        (43, "ZED"),
    ];
    for (id, handle) in alone {
        old_account(&pool, id, handle, 9).await;
    }

    Migrator::new(full.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();

    // The oldest keeps the skeleton, the later ones have none, and nothing failed.
    assert_eq!(skeleton_of(&pool, 21).await.as_deref(), Some("flokzy"));
    assert_eq!(skeleton_of(&pool, 22).await, None);
    assert_eq!(skeleton_of(&pool, 20).await, None);
    // On a tie of time the lower ID keeps it.
    assert_eq!(skeleton_of(&pool, 30).await.as_deref(), Some("sammlth"));
    assert_eq!(skeleton_of(&pool, 31).await, None);
    // SQL and Rust agree.
    for (id, handle) in alone {
        assert_eq!(
            skeleton_of(&pool, id).await,
            Some(handle_skeleton(handle)),
            "{handle}"
        );
    }
    // The accounts without a skeleton are still there with their handles.
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 9);
    let (handle,): (String,) = sqlx::query_as("SELECT handle FROM accounts WHERE id = 20")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(handle, "Flokzy");

    // A look-alike of the one that kept the skeleton is a look-alike of all three.
    for (id, handle) in [(50, "Fl0kzy"), (51, "FIOKZY")] {
        assert_eq!(
            db::insert_account(&pool, id, handle, &format!("{id}@example.com"), 1).await,
            Err(InsertError::Handle),
            "{handle}"
        );
    }
    db::insert_account(&pool, 52, "Flokzx", "52@example.com", 1)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn migration_0014_fills_the_skeleton_and_survives_accounts_that_collide() {
    install_default_drivers();
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("old.db").display())
        .replace(std::path::MAIN_SEPARATOR, "/");
    migrate_with_colliding_accounts("sqlite", &url).await;
    if let Some(database) = common::postgres_database().await {
        migrate_with_colliding_accounts("postgres", &database.url).await;
    }
}
