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
        assert!(text.contains("node=\"n1\""));
        // Every metric is announced before its samples.
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let name = line.split(['{', ' ']).next().unwrap();
            assert!(text.contains(&format!("# TYPE {name} ")), "{name}");
        }
    }
}
