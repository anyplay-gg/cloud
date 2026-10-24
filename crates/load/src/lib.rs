//! A load harness for the AnyPlay cloud. It plays many players through the real HTTP API and the
//! event WebSockets: a login wave, a friend graph, presence churn, sessions and a reconnect storm.
//! `docs/LOAD.md` says how to run it and what its numbers mean.

use std::sync::Arc;

use anyhow::Result;

pub mod client;
pub mod opts;
pub mod report;
pub mod scenarios;
pub mod stats;
pub mod sys;
pub mod target;

pub use opts::Options;
pub use report::Report;

use opts::Scenario;

fn commit_of(opts: &Options) -> Option<String> {
    opts.commit.clone().or_else(|| {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "--short=10", "HEAD"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|c| !c.is_empty())
    })
}

/// Runs the scenarios in order and returns the report. Tables are printed as each one ends,
/// unless `opts.quiet`.
pub async fn run(opts: Options) -> Result<Report> {
    opts.validate()?;
    let target = Arc::new(target::Target::start(&opts).await?);
    let ctx = client::Ctx::new(opts.clone(), target.clone())?;
    let sampler = sys::Sampler::start(&opts.pid, !opts.external());
    sampler.read();
    sampler.run(target.clone());

    let mut scenarios = Vec::new();
    let mut emit = |report: report::ScenarioReport| {
        if !opts.quiet {
            println!("{}", report.table());
        }
        scenarios.push(report);
    };
    let result: Result<()> = async {
        emit(scenarios::login_wave(&ctx, &sampler).await);
        if ctx.players.read().unwrap_or_else(|e| e.into_inner()).len() < 2 {
            anyhow::bail!("fewer than two players signed in; nothing to measure");
        }
        if opts.friends > 0 {
            emit(scenarios::friend_graph(&ctx, &sampler).await);
        }
        // The storm goes last: it can take a node down.
        if opts.scenarios.contains(&Scenario::Presence) {
            emit(scenarios::presence_churn(&ctx, &sampler).await);
        }
        if opts.scenarios.contains(&Scenario::Sessions) {
            emit(scenarios::session_churn(&ctx, &sampler).await);
        }
        if opts.scenarios.contains(&Scenario::Storm) {
            emit(scenarios::reconnect_storm(&ctx, &sampler).await);
        }
        Ok(())
    }
    .await;

    ctx.stop_all();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (store, nodes) = (target.store.to_owned(), target.nodes.len());
    target.shutdown().await;
    result?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let report = Report {
        tool: "anyplay-load".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        date: report::civil_date(now),
        commit: commit_of(&opts),
        label: opts.label.clone(),
        machine: sys::machine(),
        mode: if opts.external() {
            "external".into()
        } else {
            "in-process".into()
        },
        store,
        nodes,
        settings: serde_json::to_value(&opts)?,
        scenarios,
    };
    if let Some(path) = &opts.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(report)
}
