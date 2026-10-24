//! The command line.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde::Serialize;

/// The scenarios that follow the login wave and the friend graph, which always run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scenario {
    /// Players change their activity; friends receive the presence event.
    Presence,
    /// Players create, join and leave sessions.
    Sessions,
    /// Players fetch the gamer cards of their friends.
    Cards,
    /// Connections drop at once, or a node stops; players come back.
    Storm,
}

/// What the reconnect storm does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StormKind {
    /// Every event connection breaks at once, with no close frame.
    Drop,
    /// Node 1 drains and closes its connections with 1001 (cluster mode).
    StopNode,
    /// Node 1 vanishes without a word (cluster mode).
    KillNode,
}

/// How players wait before they reconnect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reconnect {
    /// As `spec/cloud-api.md` tells clients: 0 to 10 s after 1001/1012, otherwise half the pause
    /// plus a random part of up to the whole pause, from 1 s doubling to 30 s.
    Spec,
    /// At once after a break, the worst case: a client that ignores the spec.
    Immediate,
}

/// Plays many players against the AnyPlay cloud through its real HTTP API and event WebSockets.
#[derive(Parser, Clone, Debug, Serialize)]
#[command(name = "anyplay-load", version)]
pub struct Options {
    /// Test an external cloud at this base URL (`http://host:port`) instead of starting nodes in
    /// this process. Repeat it for several nodes.
    #[arg(long, value_name = "URL")]
    pub url: Vec<String>,
    /// The `/metrics` address of each external node, in the order of `--url`.
    #[arg(long, value_name = "URL")]
    pub metrics_url: Vec<String>,
    /// Where the external cloud logs its sign-in codes (`mail = "log"`), to read them from.
    #[arg(long, value_name = "FILE")]
    pub mail_log: Option<PathBuf>,
    /// The process ID of an external cloud node, to read its memory and CPU. Repeatable.
    #[arg(long, value_name = "PID")]
    pub pid: Vec<u32>,

    /// Nodes to start in this process.
    #[arg(long, default_value_t = 1)]
    pub nodes: usize,
    /// PostgreSQL server for the in-process nodes; a database of its own is created on it and
    /// dropped at the end. Without it the store is a SQLite file.
    #[arg(long, env = "ANYPLAY_LOAD_DATABASE_URL", hide_env_values = true)]
    #[serde(skip)]
    pub database_url: Option<String>,
    /// NATS server for the in-process nodes (needed for more than one node).
    #[arg(long, env = "ANYPLAY_LOAD_NATS_URL", hide_env_values = true)]
    #[serde(skip)]
    pub nats_url: Option<String>,
    /// Worker threads of each in-process node.
    #[arg(long, default_value_t = 4)]
    pub node_threads: usize,
    /// Store connections of each in-process node.
    #[arg(long, default_value_t = 16)]
    pub db_connections: u32,
    /// Seconds a stopping in-process node keeps serving while it reports itself not ready.
    #[arg(long, default_value_t = 5)]
    pub drain_seconds: u64,
    /// Keep the rate limits of the specification. The in-process cloud otherwise runs with limits
    /// that do not get in the way, because every player comes from 127.0.0.1.
    #[arg(long)]
    pub default_limits: bool,

    /// Worker threads of the harness.
    #[arg(long, default_value_t = 6)]
    pub threads: usize,
    /// Players.
    #[arg(long, default_value_t = 1000)]
    pub players: usize,
    /// Logins per second in the login wave; 0 starts them all, `--concurrency` at a time.
    #[arg(long, default_value_t = 100.0)]
    pub rate: f64,
    /// The share of players (0 to 1) that already have an account and sign in instead of up.
    #[arg(long, default_value_t = 0.0)]
    pub returning: f64,
    /// Friends per player (rounded up to an even number).
    #[arg(long, default_value_t = 10)]
    pub friends: usize,
    /// Requests in flight at once during setup steps.
    #[arg(long, default_value_t = 256)]
    pub concurrency: usize,
    /// The most requests in flight in a rate-driven scenario; further ones are counted as skipped.
    #[arg(long, default_value_t = 5000)]
    pub max_in_flight: usize,

    /// The scenarios after the wave and the friend graph.
    #[arg(long, value_delimiter = ',', default_value = "presence,sessions,storm")]
    pub scenarios: Vec<Scenario>,
    /// Activity changes per second, all players together.
    #[arg(long, default_value_t = 50.0)]
    pub presence_rate: f64,
    /// Seconds of presence churn.
    #[arg(long, default_value_t = 30.0)]
    pub presence_seconds: f64,
    /// Gamer card calls per second, all players together.
    #[arg(long, default_value_t = 200.0)]
    pub cards_rate: f64,
    /// Seconds of gamer card calls.
    #[arg(long, default_value_t = 30.0)]
    pub cards_seconds: f64,
    /// Accounts named in one gamer card call (at most 100); the player's friends first, then
    /// other players.
    #[arg(long, default_value_t = 10)]
    pub cards_size: usize,
    /// Sessions created per second.
    #[arg(long, default_value_t = 10.0)]
    pub session_rate: f64,
    /// Members per session, the leader included (2 to 64; limited by `--friends`).
    #[arg(long, default_value_t = 4)]
    pub session_size: usize,
    /// Seconds a session lasts before everyone leaves.
    #[arg(long, default_value_t = 1.0)]
    pub session_hold: f64,
    /// Seconds of session churn.
    #[arg(long, default_value_t = 30.0)]
    pub session_seconds: f64,
    /// What the storm does.
    #[arg(long, value_enum, default_value = "drop")]
    pub storm: StormKind,
    /// How players wait before they reconnect.
    #[arg(long, value_enum, default_value = "spec")]
    pub reconnect: Reconnect,
    /// The share of players that must be back for the headline storm time.
    #[arg(long, default_value_t = 0.95)]
    pub storm_target: f64,
    /// Seconds after which the storm is over, back or not.
    #[arg(long, default_value_t = 120.0)]
    pub storm_timeout: f64,

    /// Write the report as JSON here.
    #[arg(long, value_name = "FILE")]
    pub json: Option<PathBuf>,
    /// A note for the report, such as the run's purpose.
    #[arg(long)]
    pub label: Option<String>,
    /// The commit under test; defaults to `git rev-parse` in the current directory.
    #[arg(long)]
    pub commit: Option<String>,
    /// Print no tables.
    #[arg(long, hide = true)]
    pub quiet: bool,
}

impl Options {
    pub fn external(&self) -> bool {
        !self.url.is_empty()
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::bail;
        if self.players < 2 {
            bail!("--players must be at least 2");
        }
        if !(0.0..=1.0).contains(&self.returning) {
            bail!("--returning is a share between 0 and 1");
        }
        if self.friends >= self.players {
            bail!("--friends must be smaller than --players");
        }
        if !(2..=64).contains(&self.session_size) {
            bail!("--session-size is between 2 and 64");
        }
        let storm = self.scenarios.contains(&Scenario::Storm);
        if self.external() {
            if self.mail_log.is_none() {
                bail!(
                    "--url needs --mail-log: the harness reads sign-in codes from the cloud's log"
                );
            }
            if self.storm != StormKind::Drop && storm {
                bail!("--storm stop-node and kill-node work on in-process nodes only");
            }
        } else {
            if self.nodes == 0 {
                bail!("--nodes must be at least 1");
            }
            if self.nodes > 1 && (self.database_url.is_none() || self.nats_url.is_none()) {
                bail!("more than one node needs --database-url and --nats-url");
            }
            if self.storm != StormKind::Drop && self.nodes < 2 && storm {
                bail!("--storm stop-node and kill-node need --nodes 2 or more");
            }
        }
        Ok(())
    }
}
