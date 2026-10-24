//! The harness against a cloud in this process, with a tiny crowd: enough to know every scenario
//! runs and measures something. The cluster test needs both servers; without
//! `ANYPLAY_TEST_DATABASE_URL` and `ANYPLAY_TEST_NATS_URL` it passes without running.

use anyplay_load::{Options, Report, run};
use clap::Parser as _;

fn options(extra: &[&str]) -> Options {
    let mut args = vec![
        "anyplay-load",
        "--quiet",
        "--players",
        "24",
        "--rate",
        "0",
        "--friends",
        "4",
        "--presence-rate",
        "20",
        "--presence-seconds",
        "1.5",
        "--session-rate",
        "6",
        "--session-seconds",
        "1.5",
        "--session-hold",
        "0.2",
        "--reconnect",
        "immediate",
        "--storm-timeout",
        "20",
        "--drain-seconds",
        "0",
    ];
    args.extend_from_slice(extra);
    Options::try_parse_from(args).unwrap()
}

fn scenario<'a>(report: &'a Report, name: &str) -> &'a anyplay_load::report::ScenarioReport {
    report
        .scenarios
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_small_crowd_on_sqlite_goes_through_every_scenario() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let report = run(options(&[])).await.unwrap();

    let wave = scenario(&report, "login-wave");
    assert_eq!(wave.players, 24);
    assert_eq!(wave.connections_held, 24);
    assert_eq!(wave.ops["auth_signup"].ok, 24);
    assert_eq!(wave.ops["e2e_login"].ok, 24);

    let graph = scenario(&report, "friend-graph");
    assert_eq!(graph.ops["friend_accept"].ok, 48);

    let presence = scenario(&report, "presence-churn");
    let delivery = presence.delivery.as_ref().unwrap();
    assert!(delivery.changes > 5, "{delivery:?}");
    assert!(delivery.delivered_share > 0.99, "{delivery:?}");
    assert!(delivery.latency.as_ref().unwrap().p99_ms < 2000.0);

    let sessions = scenario(&report, "sessions");
    assert!(sessions.ops["session_create"].ok > 3);
    assert!(sessions.ops["session_join"].ok > 3);
    assert_eq!(sessions.server.store_errors, 0);

    let storm = scenario(&report, "reconnect-storm").storm.as_ref().unwrap();
    assert_eq!(storm.hit, 24);
    assert_eq!(storm.back, 24);
    assert!(storm.seconds_to["100%"].unwrap() < 10.0);

    // The JSON a run writes has to read back.
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["scenarios"].as_array().unwrap().len(), 5);
    assert_eq!(json["store"], "sqlite");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_that_stops_sends_its_players_to_the_other() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (Ok(database), Ok(nats)) = (
        std::env::var("ANYPLAY_TEST_DATABASE_URL"),
        std::env::var("ANYPLAY_TEST_NATS_URL"),
    ) else {
        eprintln!("no test database and NATS server; skipped");
        return;
    };
    let report = run(options(&[
        "--nodes",
        "2",
        "--database-url",
        &database,
        "--nats-url",
        &nats,
        "--scenarios",
        "presence,storm",
        "--storm",
        "stop-node",
    ]))
    .await
    .unwrap();
    assert_eq!(report.nodes, 2);
    assert_eq!(report.store, "postgres");
    let presence = scenario(&report, "presence-churn")
        .delivery
        .as_ref()
        .unwrap();
    assert!(presence.delivered_share > 0.99, "{presence:?}");
    let storm = scenario(&report, "reconnect-storm").storm.as_ref().unwrap();
    assert_eq!(storm.hit, 12);
    assert_eq!(storm.back, 12);
}
