//! What a node counts about itself, in the Prometheus text format: event connections, requests
//! per route and status, news between nodes, sweeps and store errors. Served on
//! `metrics_listen`, a port of its own that is never published.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Store errors of the whole process; the error conversion has no state at hand.
pub static STORE_ERRORS: AtomicU64 = AtomicU64::new(0);

/// Queries that gave up waiting for a store connection, a SQLite write turn or a lock, or were
/// cancelled by the statement timeout. They are answered as retryable and are not in
/// [`STORE_ERRORS`].
pub static STORE_POOL_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

/// Requests the request timeout cut off, and requests shed by the in-flight cap.
pub static REQUEST_TIMEOUTS: AtomicU64 = AtomicU64::new(0);
pub static REQUESTS_SHED: AtomicU64 = AtomicU64::new(0);

/// Upper bounds in seconds of the buckets of [`WRITE_WAIT`].
const WAIT_BUCKETS: [f64; 9] = [0.001, 0.005, 0.025, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0];

/// How long [`crate::db::Writer::begin`] waited for its turn and a connection, which is where
/// writers pile up when the pool is too small. Reads that take a connection are not in it.
pub static WRITE_WAIT: WaitHistogram = WaitHistogram::new();

/// A histogram of waits that needs no lock.
pub struct WaitHistogram {
    /// Per bucket, not cumulative; the last slot holds what is above the largest bound.
    buckets: [AtomicU64; WAIT_BUCKETS.len() + 1],
    micros: AtomicU64,
}

impl WaitHistogram {
    const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; WAIT_BUCKETS.len() + 1],
            micros: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, took: Duration) {
        let seconds = took.as_secs_f64();
        let slot = WAIT_BUCKETS
            .iter()
            .position(|bound| seconds <= *bound)
            .unwrap_or(WAIT_BUCKETS.len());
        self.buckets[slot].fetch_add(1, Ordering::Relaxed);
        self.micros
            .fetch_add(took.as_micros() as u64, Ordering::Relaxed);
    }

    fn render(&self, out: &mut String, name: &str, help: &str) {
        let _ = writeln!(
            out,
            "# HELP {name} {help}
# TYPE {name} histogram"
        );
        let mut total = 0;
        for (bound, bucket) in WAIT_BUCKETS.iter().zip(&self.buckets) {
            total += bucket.load(Ordering::Relaxed);
            let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {total}");
        }
        total += self.buckets[WAIT_BUCKETS.len()].load(Ordering::Relaxed);
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {total}");
        let _ = writeln!(
            out,
            "{name}_sum {}
{name}_count {total}",
            self.micros.load(Ordering::Relaxed) as f64 / 1e6
        );
    }
}

#[derive(Default)]
struct Timed {
    count: u64,
    seconds: f64,
}

#[derive(Default)]
pub struct Metrics {
    /// By route template (`/v1/sessions/{id}`, so the labels stay few) and status.
    requests: Mutex<BTreeMap<(String, u16), Timed>>,
    sweeps: AtomicU64,
    sweep_failures: AtomicU64,
    last_sweep_micros: AtomicU64,
}

/// What [`Metrics::render`] needs from the rest of the node.
pub struct Snapshot<'a> {
    pub node: &'a str,
    pub connections: usize,
    pub news_sent: u64,
    pub news_received: u64,
    /// Notifications sent, failed and removed as gone, when the cloud sends them.
    pub push: Option<(u64, u64, u64)>,
    /// Connections the store pool holds, how many of them are idle, and the most it may hold.
    pub pool: (u32, usize, u32),
}

impl Metrics {
    pub fn request(&self, route: &str, status: u16, took: Duration) {
        let mut requests = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        let timed = requests.entry((route.to_owned(), status)).or_default();
        timed.count += 1;
        timed.seconds += took.as_secs_f64();
    }

    pub fn sweep(&self, took: Duration, ok: bool) {
        self.sweeps.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.sweep_failures.fetch_add(1, Ordering::Relaxed);
        }
        self.last_sweep_micros
            .store(took.as_micros() as u64, Ordering::Relaxed);
    }

    pub fn render(&self, now: &Snapshot<'_>) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# HELP anyplay_cloud_info This node, with its version.\n\
             # TYPE anyplay_cloud_info gauge\n\
             anyplay_cloud_info{{version=\"{}\",node=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION"),
            now.node
        );
        gauge(
            &mut out,
            "anyplay_cloud_event_connections",
            "Event connections this node holds.",
            now.connections as f64,
        );
        counter(
            &mut out,
            "anyplay_cloud_news_sent_total",
            "News this node sent to other nodes.",
            now.news_sent,
        );
        counter(
            &mut out,
            "anyplay_cloud_news_received_total",
            "News this node received from other nodes.",
            now.news_received,
        );
        if let Some((sent, failed, pruned)) = now.push {
            counter(
                &mut out,
                "anyplay_cloud_push_sent_total",
                "Web Push notifications a push service accepted.",
                sent,
            );
            counter(
                &mut out,
                "anyplay_cloud_push_failed_total",
                "Web Push notifications that failed.",
                failed,
            );
            counter(
                &mut out,
                "anyplay_cloud_push_pruned_total",
                "Web Push subscriptions removed because they were gone or kept failing.",
                pruned,
            );
        }
        counter(
            &mut out,
            "anyplay_cloud_sweeps_total",
            "Sweeps this node ran.",
            self.sweeps.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "anyplay_cloud_sweep_failures_total",
            "Sweeps that failed.",
            self.sweep_failures.load(Ordering::Relaxed),
        );
        gauge(
            &mut out,
            "anyplay_cloud_last_sweep_seconds",
            "How long the last sweep took.",
            self.last_sweep_micros.load(Ordering::Relaxed) as f64 / 1e6,
        );
        counter(
            &mut out,
            "anyplay_cloud_store_errors_total",
            "Queries the store failed.",
            STORE_ERRORS.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "anyplay_cloud_store_pool_timeouts_total",
            "Queries that timed out waiting for a store connection, a write turn or a lock.",
            STORE_POOL_TIMEOUTS.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "anyplay_cloud_request_timeouts_total",
            "Requests cut off by the request timeout.",
            REQUEST_TIMEOUTS.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "anyplay_cloud_requests_shed_total",
            "Requests turned away because too many were in flight.",
            REQUESTS_SHED.load(Ordering::Relaxed),
        );
        gauge(
            &mut out,
            "anyplay_cloud_store_pool_connections",
            "Connections the store pool holds open.",
            f64::from(now.pool.0),
        );
        gauge(
            &mut out,
            "anyplay_cloud_store_pool_idle_connections",
            "Connections the store pool holds open and idle.",
            now.pool.1 as f64,
        );
        gauge(
            &mut out,
            "anyplay_cloud_store_pool_max_connections",
            "The most connections the store pool may hold.",
            f64::from(now.pool.2),
        );
        WRITE_WAIT.render(
            &mut out,
            "anyplay_cloud_store_write_wait_seconds",
            "How long write transactions waited for their turn and a connection.",
        );

        let requests = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(
            out,
            "# HELP anyplay_cloud_http_requests_total Requests answered, by route and status.\n\
             # TYPE anyplay_cloud_http_requests_total counter"
        );
        for ((route, status), timed) in requests.iter() {
            let _ = writeln!(
                out,
                "anyplay_cloud_http_requests_total{{route=\"{route}\",status=\"{status}\"}} {}",
                timed.count
            );
        }
        let _ = writeln!(
            out,
            "# HELP anyplay_cloud_http_request_seconds_total Time spent answering, by route and status.\n\
             # TYPE anyplay_cloud_http_request_seconds_total counter"
        );
        for ((route, status), timed) in requests.iter() {
            let _ = writeln!(
                out,
                "anyplay_cloud_http_request_seconds_total{{route=\"{route}\",status=\"{status}\"}} {}",
                timed.seconds
            );
        }
        out
    }
}

fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    let _ = writeln!(
        out,
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
    );
}

fn counter(out: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(
        out,
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_counted_by_route_and_status() {
        let metrics = Metrics::default();
        metrics.request("/v1/me", 200, Duration::from_millis(5));
        metrics.request("/v1/me", 200, Duration::from_millis(15));
        metrics.request("/v1/me", 401, Duration::from_millis(1));
        metrics.sweep(Duration::from_millis(3), true);
        let text = metrics.render(&Snapshot {
            node: "n1",
            connections: 2,
            news_sent: 4,
            news_received: 5,
            push: Some((7, 1, 2)),
            pool: (3, 1, 16),
        });
        assert!(
            text.contains("anyplay_cloud_http_requests_total{route=\"/v1/me\",status=\"200\"} 2")
        );
        assert!(
            text.contains("anyplay_cloud_http_requests_total{route=\"/v1/me\",status=\"401\"} 1")
        );
        assert!(text.contains("anyplay_cloud_event_connections 2"));
        assert!(text.contains("anyplay_cloud_news_received_total 5"));
        assert!(text.contains("anyplay_cloud_sweeps_total 1"));
        assert!(text.contains("anyplay_cloud_push_sent_total 7"));
        assert!(text.contains("node=\"n1\""));
        // Every metric is announced before its samples.
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let name = line.split(['{', ' ']).next().unwrap();
            let name = name
                .strip_suffix("_bucket")
                .or_else(|| name.strip_suffix("_sum"))
                .or_else(|| name.strip_suffix("_count"))
                .unwrap_or(name);
            assert!(text.contains(&format!("# TYPE {name} ")), "{name}");
        }
    }
}
