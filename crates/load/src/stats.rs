//! What a scenario records while it runs: latencies by operation, HTTP statuses, events.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Event kinds counted on the event connections.
pub const EVENT_KINDS: [&str; 10] = [
    "welcome",
    "friends",
    "presence",
    "invitation",
    "invitation_gone",
    "session",
    "session_gone",
    "signed_out",
    "banned",
    "unknown",
];

#[derive(Default)]
struct OpData {
    micros: Vec<u32>,
    status: BTreeMap<u16, u64>,
}

pub struct Recorder {
    pub started: Instant,
    ops: Mutex<BTreeMap<&'static str, OpData>>,
    counters: Mutex<BTreeMap<String, u64>>,
    events: [AtomicU64; EVENT_KINDS.len()],
    delivery: Mutex<Vec<u32>>,
    pub expected_deliveries: AtomicU64,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            ops: Mutex::default(),
            counters: Mutex::default(),
            events: Default::default(),
            delivery: Mutex::default(),
            expected_deliveries: AtomicU64::new(0),
        }
    }

    /// One finished request or connection attempt. Status 0 means no answer arrived.
    pub fn op(&self, name: &'static str, status: u16, took: Duration) {
        let mut ops = self.ops.lock().unwrap_or_else(|e| e.into_inner());
        let data = ops.entry(name).or_default();
        data.micros
            .push(took.as_micros().min(u32::MAX as u128) as u32);
        *data.status.entry(status).or_default() += 1;
    }

    pub fn count(&self, name: &str, n: u64) {
        let mut counters = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        *counters.entry(name.to_owned()).or_default() += n;
    }

    pub fn event(&self, kind: &str) {
        let i = EVENT_KINDS
            .iter()
            .position(|k| *k == kind)
            .unwrap_or(EVENT_KINDS.len() - 1);
        self.events[i].fetch_add(1, Ordering::Relaxed);
    }

    /// A presence event that arrived `micros` after the change that caused it.
    pub fn delivered(&self, micros: u64) {
        self.delivery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(micros.min(u32::MAX as u64) as u32);
    }

    pub fn delivered_count(&self) -> u64 {
        self.delivery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len() as u64
    }

    pub fn summarize(&self) -> Summary {
        let mut ops = BTreeMap::new();
        let mut status_total: BTreeMap<String, u64> = BTreeMap::new();
        let mut http_calls = 0;
        for (name, data) in self.ops.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            let mut sorted = data.micros.clone();
            sorted.sort_unstable();
            let count = sorted.len() as u64;
            if *name != "ws_connect" && !name.starts_with("e2e_") {
                http_calls += count;
                for (status, n) in &data.status {
                    *status_total.entry(status.to_string()).or_default() += n;
                }
            }
            let ok = data
                .status
                .iter()
                .filter(|(s, _)| (200..300).contains(*s) || **s == 101)
                .map(|(_, n)| n)
                .sum();
            ops.insert(
                (*name).to_owned(),
                OpSummary {
                    count,
                    ok,
                    p50_ms: pick(&sorted, 0.50),
                    p95_ms: pick(&sorted, 0.95),
                    p99_ms: pick(&sorted, 0.99),
                    max_ms: sorted.last().map_or(0.0, |m| *m as f64 / 1000.0),
                    status: data
                        .status
                        .iter()
                        .map(|(s, n)| (s.to_string(), *n))
                        .collect(),
                },
            );
        }
        let mut delivery = self
            .delivery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        delivery.sort_unstable();
        Summary {
            ops,
            http_calls,
            status_total,
            counters: self
                .counters
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            events: EVENT_KINDS
                .iter()
                .zip(&self.events)
                .map(|(k, n)| ((*k).to_owned(), n.load(Ordering::Relaxed)))
                .filter(|(_, n)| *n > 0)
                .collect(),
            delivery_ms: Percentiles::of(&delivery),
        }
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Summary {
    pub ops: BTreeMap<String, OpSummary>,
    pub http_calls: u64,
    pub status_total: BTreeMap<String, u64>,
    pub counters: BTreeMap<String, u64>,
    pub events: BTreeMap<String, u64>,
    pub delivery_ms: Option<Percentiles>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpSummary {
    pub count: u64,
    pub ok: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    /// Status code to count; 0 is a request that got no answer, 101 an accepted upgrade.
    pub status: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Percentiles {
    pub count: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl Percentiles {
    pub fn of(sorted_micros: &[u32]) -> Option<Self> {
        if sorted_micros.is_empty() {
            return None;
        }
        Some(Self {
            count: sorted_micros.len() as u64,
            p50_ms: pick(sorted_micros, 0.50),
            p95_ms: pick(sorted_micros, 0.95),
            p99_ms: pick(sorted_micros, 0.99),
            max_ms: *sorted_micros.last().unwrap() as f64 / 1000.0,
        })
    }
}

/// The value at quantile `q` of sorted microseconds, in milliseconds (nearest rank).
pub fn pick(sorted: &[u32], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1] as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let sorted: Vec<u32> = (1..=100).map(|n| n * 1000).collect();
        assert_eq!(pick(&sorted, 0.50), 50.0);
        assert_eq!(pick(&sorted, 0.95), 95.0);
        assert_eq!(pick(&sorted, 0.99), 99.0);
        assert_eq!(pick(&[], 0.5), 0.0);
        assert_eq!(pick(&[7000], 0.99), 7.0);
    }

    #[test]
    fn operations_are_summed_by_status() {
        let rec = Recorder::new();
        rec.op("a", 200, Duration::from_millis(2));
        rec.op("a", 429, Duration::from_millis(4));
        rec.op("ws_connect", 101, Duration::from_millis(1));
        let s = rec.summarize();
        assert_eq!(s.http_calls, 2);
        assert_eq!(s.ops["a"].ok, 1);
        assert_eq!(s.status_total["429"], 1);
    }
}
