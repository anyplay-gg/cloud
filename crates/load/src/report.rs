//! The report: types for the JSON and the tables for people.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;

use crate::stats::{OpSummary, Percentiles};
use crate::sys::{Machine, PoolUse, ProcessUse};

#[derive(Debug, Serialize)]
pub struct Report {
    pub tool: String,
    pub version: String,
    /// UTC, `YYYY-MM-DD`.
    pub date: String,
    pub commit: Option<String>,
    pub label: Option<String>,
    pub machine: Machine,
    /// `in-process` or `external`.
    pub mode: String,
    /// `sqlite`, `postgres` or `external`.
    pub store: String,
    pub nodes: usize,
    pub settings: Value,
    pub scenarios: Vec<ScenarioReport>,
}

#[derive(Debug, Serialize)]
pub struct ScenarioReport {
    pub name: String,
    pub seconds: f64,
    /// Players signed in when the scenario began.
    pub players: usize,
    /// Event connections open when it ended.
    pub connections_held: usize,
    pub http_calls: u64,
    pub http_per_s: f64,
    pub ops: BTreeMap<String, OpSummary>,
    pub http_status: BTreeMap<String, u64>,
    pub events: BTreeMap<String, u64>,
    pub counters: BTreeMap<String, u64>,
    pub delivery: Option<DeliveryReport>,
    pub storm: Option<StormReport>,
    pub server: ServerReport,
    pub process: Vec<ProcessUse>,
    pub pool: Option<PoolUse>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeliveryReport {
    /// Activity changes the cloud accepted.
    pub changes: u64,
    /// Presence events that should arrive: each change times the friends online at that moment.
    pub expected: u64,
    pub delivered: u64,
    pub delivered_share: f64,
    pub latency: Option<Percentiles>,
}

#[derive(Debug, Serialize)]
pub struct StormReport {
    pub kind: String,
    pub reconnect: String,
    /// Players whose connection the storm broke.
    pub hit: usize,
    pub back: usize,
    pub back_share: f64,
    /// Seconds from the storm's start until this share of `hit` was back; null if never.
    pub seconds_to: BTreeMap<String, Option<f64>>,
    pub target_share: f64,
    pub seconds_to_target: Option<f64>,
    pub min_connections: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct RouteStat {
    pub route: String,
    pub status: String,
    pub count: u64,
    pub mean_ms: f64,
    pub total_s: f64,
}

/// What the nodes' own `/metrics` say changed during the scenario.
#[derive(Debug, Default, Serialize)]
pub struct ServerReport {
    pub store_errors: u64,
    pub sweep_failures: u64,
    pub news_sent: u64,
    pub news_received: u64,
    pub event_connections_end: u64,
    /// The routes that took the most time in total.
    pub top_routes: Vec<RouteStat>,
}

pub fn civil_date(unix_secs: u64) -> String {
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn ms(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn statuses(map: &BTreeMap<String, u64>) -> String {
    if map.is_empty() {
        return "none".into();
    }
    map.iter()
        .map(|(s, n)| {
            if s == "0" {
                format!("no answer: {n}")
            } else {
                format!("{s}: {n}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl ScenarioReport {
    /// The table for people.
    pub fn table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "== {} ({:.1} s)", self.name, self.seconds);
        let _ = writeln!(
            out,
            "players {}   connections held {}   HTTP calls {} ({:.1}/s)",
            self.players, self.connections_held, self.http_calls, self.http_per_s
        );
        if !self.ops.is_empty() {
            let _ = writeln!(
                out,
                "  {:<16} {:>8} {:>8} {:>9} {:>9} {:>9} {:>9}",
                "operation", "count", "ok", "p50 ms", "p95 ms", "p99 ms", "max ms"
            );
            for (name, op) in &self.ops {
                let _ = writeln!(
                    out,
                    "  {:<16} {:>8} {:>8} {:>9} {:>9} {:>9} {:>9}",
                    name,
                    op.count,
                    op.ok,
                    ms(op.p50_ms),
                    ms(op.p95_ms),
                    ms(op.p99_ms),
                    ms(op.max_ms)
                );
            }
        }
        let _ = writeln!(out, "HTTP status: {}", statuses(&self.http_status));
        if let Some(d) = &self.delivery {
            let _ = writeln!(
                out,
                "presence delivery: {} changes, {} of {} events arrived ({:.2}%)",
                d.changes,
                d.delivered,
                d.expected,
                d.delivered_share * 100.0
            );
            if let Some(l) = &d.latency {
                let _ = writeln!(
                    out,
                    "  latency ms: p50 {}  p95 {}  p99 {}  max {}",
                    ms(l.p50_ms),
                    ms(l.p95_ms),
                    ms(l.p99_ms),
                    ms(l.max_ms)
                );
            }
        }
        if let Some(s) = &self.storm {
            let _ = writeln!(
                out,
                "storm {} ({} reconnect): {} of {} back ({:.1}%), fewest connections held {}",
                s.kind,
                s.reconnect,
                s.back,
                s.hit,
                s.back_share * 100.0,
                s.min_connections
            );
            let times: Vec<String> = s
                .seconds_to
                .iter()
                .map(|(q, t)| match t {
                    Some(t) => format!("{q} {t:.2} s"),
                    None => format!("{q} never"),
                })
                .collect();
            let _ = writeln!(out, "  time until: {}", times.join("   "));
        }
        if !self.events.is_empty() {
            let events: Vec<String> = self
                .events
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            let _ = writeln!(out, "events received: {}", events.join(", "));
        }
        if !self.counters.is_empty() {
            let counters: Vec<String> = self
                .counters
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            let _ = writeln!(out, "counters: {}", counters.join(", "));
        }
        let _ = writeln!(
            out,
            "store errors {}   sweep failures {}   news sent/received {}/{}",
            self.server.store_errors,
            self.server.sweep_failures,
            self.server.news_sent,
            self.server.news_received
        );
        for p in &self.process {
            let _ = writeln!(
                out,
                "{}: CPU avg {:.2} cores, peak {:.2}; memory peak {:.0} MB",
                p.process, p.cpu_avg_cores, p.cpu_peak_cores, p.rss_peak_mb
            );
        }
        if let Some(pool) = &self.pool {
            let _ = writeln!(
                out,
                "store connections busy: avg {:.1}, peak {} of {}; all busy in {:.0}% of samples",
                pool.busy_avg,
                pool.busy_peak,
                pool.pool_size_max,
                pool.saturated_share * 100.0
            );
        }
        if !self.server.top_routes.is_empty() {
            let _ = writeln!(out, "server side, by time spent in the handler:");
            for r in &self.server.top_routes {
                let _ = writeln!(
                    out,
                    "  {:<34} {:>3} {:>9} calls {:>8} ms mean {:>8.1} s total",
                    r.route,
                    r.status,
                    r.count,
                    ms(r.mean_ms),
                    r.total_s
                );
            }
        }
        for note in &self.notes {
            let _ = writeln!(out, "note: {note}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_calendar_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(1_000_000_000), "2001-09-09");
        assert_eq!(civil_date(1_792_800_000), "2026-10-24");
    }
}
