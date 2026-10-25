mod common;

use anyplay_api::{RUN_REPORTS_MIN, RunSummary, Verdict};
use common::{Reply, TestCloud, User};
use serde_json::{Value, json};

async fn report(
    cloud: &TestCloud,
    user: &User,
    title: &str,
    version: &str,
    toolkit: &str,
    verdict: &str,
) -> Reply {
    cloud
        .put(
            &format!("/v1/me/run-reports/{title}"),
            user.token(),
            Some(
                json!({"version": version, "toolkit": toolkit, "verdict": verdict, "os": "linux"}),
            ),
        )
        .await
}

async fn summary(cloud: &TestCloud, user: &User, title: &str, version: Option<&str>) -> RunSummary {
    let query = version.map_or_else(String::new, |v| format!("?version={v}"));
    cloud
        .get(&format!("/v1/titles/{title}/runs{query}"), user.token())
        .await
        .ok(200)
        .json()
}

async fn players(cloud: &TestCloud, n: usize) -> Vec<User> {
    let mut users = Vec::new();
    for k in 0..n {
        users.push(cloud.sign_up(&format!("Player{k}")).await);
    }
    users
}

#[tokio::test]
async fn a_summary_needs_three_players_and_names_the_most_reported_verdict() {
    let cloud = TestCloud::start().await;
    let users = players(&cloud, 4).await;
    let title = "GAME00001";

    let empty = summary(&cloud, &users[0], title, None).await;
    assert_eq!((empty.reports, empty.verdict), (0, None));

    report(&cloud, &users[0], title, "01.000", "0.2.0", "playable")
        .await
        .ok(204);
    report(&cloud, &users[1], title, "01.000", "0.2.0", "playable")
        .await
        .ok(204);
    // Two players are too few to say anything, and the answer shows no count at all.
    let two = summary(&cloud, &users[0], title, Some("01.000")).await;
    assert_eq!((two.reports, two.verdict, two.version), (0, None, None));
    assert_eq!(two.counts.total(), 0);

    report(&cloud, &users[2], title, "01.000", "0.2.0", "menus")
        .await
        .ok(204);
    let three = summary(&cloud, &users[0], title, None).await;
    assert_eq!(three.reports, RUN_REPORTS_MIN);
    assert_eq!(three.verdict, Some(Verdict::Playable));
    assert_eq!((three.counts.playable, three.counts.menus), (2, 1));
    assert_eq!(three.version, None);

    // A tie goes to the lower verdict.
    report(&cloud, &users[3], title, "01.000", "0.2.0", "menus")
        .await
        .ok(204);
    assert_eq!(
        summary(&cloud, &users[0], title, None).await.verdict,
        Some(Verdict::Menus)
    );
    // Another title is not affected.
    assert_eq!(
        summary(&cloud, &users[0], "GAME00002", None).await.reports,
        0
    );
}

#[tokio::test]
async fn a_new_answer_replaces_the_old_one_and_an_account_counts_once() {
    let cloud = TestCloud::start().await;
    let users = players(&cloud, 3).await;
    let title = "GAME00001";

    report(&cloud, &users[0], title, "01.000", "0.2.0", "no_start")
        .await
        .ok(204);
    report(&cloud, &users[0], title, "01.000", "0.2.0", "in_game")
        .await
        .ok(204);
    // The same player with another toolkit is still one player; the latest answer counts.
    report(&cloud, &users[0], title, "01.000", "0.3.0", "playable")
        .await
        .ok(204);
    report(&cloud, &users[1], title, "01.000", "0.2.0", "playable")
        .await
        .ok(204);
    let two = summary(&cloud, &users[0], title, None).await;
    assert_eq!(two.reports, 0, "one player twice is still two players");

    report(&cloud, &users[2], title, "01.000", "0.2.0", "menus")
        .await
        .ok(204);
    let three = summary(&cloud, &users[0], title, None).await;
    assert_eq!(three.counts.total(), 3);
    assert_eq!((three.counts.playable, three.counts.menus), (2, 1));

    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM run_reports WHERE account_id = $1")
        .bind(users[0].id as i64)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 2, "the replaced answer did not make a second row");
}

#[tokio::test]
async fn the_asked_version_wins_once_it_has_three_reports() {
    let cloud = TestCloud::start().await;
    let users = players(&cloud, 5).await;
    let title = "GAME00001";
    for user in &users[..3] {
        report(&cloud, user, title, "01.000", "0.2.0", "no_start")
            .await
            .ok(204);
    }
    for user in &users[3..] {
        report(&cloud, user, title, "01.001", "0.2.0", "playable")
            .await
            .ok(204);
    }
    let v1 = summary(&cloud, &users[0], title, Some("01.000")).await;
    assert_eq!(v1.version.as_deref(), Some("01.000"));
    assert_eq!((v1.reports, v1.verdict), (3, Some(Verdict::NoStart)));

    // 01.001 has two reports: the answer falls back to the whole title.
    let v2 = summary(&cloud, &users[0], title, Some("01.001")).await;
    assert_eq!(v2.version, None);
    assert_eq!((v2.reports, v2.verdict), (5, Some(Verdict::NoStart)));

    let all = summary(&cloud, &users[0], title, None).await;
    assert_eq!(all.reports, 5);
}

#[tokio::test]
async fn bad_input_is_refused() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    report(&cloud, &alec, "a.b", "1", "1", "menus")
        .await
        .is_error("bad_request", 400);
    report(&cloud, &alec, "GAME00001", "has space", "1", "menus")
        .await
        .is_error("bad_request", 400);
    report(&cloud, &alec, "GAME00001", "1", &"9".repeat(33), "menus")
        .await
        .is_error("bad_request", 400);
    report(&cloud, &alec, "GAME00001", "1", "1", "great")
        .await
        .is_error("bad_request", 400);
    cloud
        .put(
            "/v1/me/run-reports/GAME00001",
            alec.token(),
            Some(json!({"version": "1", "toolkit": "1", "verdict": "menus", "os": "beos"})),
        )
        .await
        .is_error("bad_request", 400);
    cloud
        .get("/v1/titles/GAME00001/runs?version=a%20b", alec.token())
        .await
        .is_error("bad_request", 400);
    cloud
        .get("/v1/titles/GAME00001/runs", "not-a-token")
        .await
        .is_error("unauthorized", 401);
}

#[tokio::test]
async fn run_reports_are_rate_limited_per_account() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.run_reports = 2).await;
    let alec = cloud.sign_up("Alec").await;
    let bea = cloud.sign_up("Bea").await;
    for n in 0..2 {
        report(&cloud, &alec, "GAME00001", &format!("1.{n}"), "1", "menus")
            .await
            .ok(204);
    }
    let limited = report(&cloud, &alec, "GAME00001", "1.9", "1", "menus").await;
    limited.is_error("rate_limited", 429);
    report(&cloud, &bea, "GAME00001", "1.0", "1", "menus")
        .await
        .ok(204);
}

#[tokio::test]
async fn the_export_lists_the_reports_and_deleting_removes_them() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    report(&cloud, &alec, "GAME00001", "01.000", "0.2.0", "in_game")
        .await
        .ok(204);
    let export: Value = cloud.get("/v1/me/export", alec.token()).await.ok(200).body;
    let rows = export["run_reports"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title_id"], "GAME00001");
    assert_eq!(rows[0]["verdict"], "in_game");
    assert_eq!(rows[0]["os"], "linux");
    assert!(rows[0]["updated"].is_u64());
    cloud.delete_account_now(&alec).await;
    let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM run_reports")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
