//! The deletion ledger: a deleted account leaves its ID and the time and nothing else, a replay
//! deletes again what a restored backup brought back, and the sweep forgets old rows.

mod common;

use anyplay_cloud::db;
use common::TestCloud;

async fn ledger(cloud: &TestCloud) -> Vec<(i64, i64)> {
    sqlx::query_as("SELECT account_id, deleted_ms FROM deleted_accounts ORDER BY account_id")
        .fetch_all(&cloud.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_deleted_account_goes_into_the_ledger() {
    let cloud = TestCloud::start().await;
    let (alec, bob) = (cloud.sign_up("Alec").await, cloud.sign_up("Bob").await);
    assert!(ledger(&cloud).await.is_empty());
    cloud.delete_account_now(&alec).await;
    let rows = ledger(&cloud).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, alec.id as i64);
    assert!(rows[0].1 > 0);
    assert!(
        db::account(&cloud.state.db, bob.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_replay_deletes_what_the_ledger_lists() {
    let cloud = TestCloud::start().await;
    let (bob, carol) = (cloud.sign_up("Bob").await, cloud.sign_up("Carol").await);
    // As after a restore: the account exists, and the ledger of the replaced database names it.
    let (deleted, failed) = cloud
        .state
        .replay_deletions(&[(bob.id, 1_000), (999_999, 2_000)])
        .await
        .unwrap();
    assert_eq!((deleted, failed), (1, 0));
    assert!(
        db::account(&cloud.state.db, bob.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db::account(&cloud.state.db, carol.id)
            .await
            .unwrap()
            .is_some()
    );
    // The ledger keeps the first time and also knows the ID that did not exist here.
    let rows = ledger(&cloud).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&(bob.id as i64, 1_000)));
    // Running it again changes nothing.
    assert_eq!(cloud.state.replay_deletions(&[]).await.unwrap(), (0, 0));
}

#[tokio::test]
async fn the_sweep_forgets_ledger_rows_after_90_days() {
    let cloud = TestCloud::start().await;
    let day = 24 * 60 * 60 * 1000;
    let now = anyplay_cloud::util::now_ms();
    cloud
        .state
        .replay_deletions(&[(1, now - 91 * day), (2, now - 89 * day)])
        .await
        .unwrap();
    cloud.state.sweep().await.unwrap();
    let rows = ledger(&cloud).await;
    assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [2]);
}
