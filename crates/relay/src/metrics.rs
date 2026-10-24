//! What the relay counts, in the Prometheus text format, on a port of its own that is never
//! published. No label holds an account or an endpoint ID: those would make a series per player.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use iroh_relay::server::Metrics as IrohMetrics;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use crate::limits::{Admission, Refusal, Stats, TicketResult};

/// Everything [`render`] reads. Cheap to clone.
#[derive(Clone)]
pub struct Sources {
    pub admission: Arc<Admission>,
    pub stats: Arc<Stats>,
    /// `iroh-relay`'s own counters: bytes, packets and rate limiting.
    pub iroh: Arc<IrohMetrics>,
}

/// The text for one scrape.
pub fn render(sources: &Sources) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# HELP anyplay_relay_info This relay, with its version.\n\
         # TYPE anyplay_relay_info gauge\n\
         anyplay_relay_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    gauge(
        &mut out,
        "anyplay_relay_clients",
        "Clients connected now.",
        sources.admission.clients() as u64,
    );
    gauge(
        &mut out,
        "anyplay_relay_accounts",
        "Accounts with a client connected now.",
        sources.admission.accounts() as u64,
    );
    let limits = sources.admission.limits();
    gauge(
        &mut out,
        "anyplay_relay_max_clients",
        "The most clients this relay admits; 0 is no limit.",
        limits.max_clients as u64,
    );
    let iroh = &sources.iroh;
    counter(
        &mut out,
        "anyplay_relay_bytes_in_total",
        "Bytes received from clients, to be relayed.",
        iroh.bytes_recv.get(),
    );
    counter(
        &mut out,
        "anyplay_relay_bytes_out_total",
        "Bytes sent to clients.",
        iroh.bytes_sent.get(),
    );
    counter(
        &mut out,
        "anyplay_relay_packets_dropped_total",
        "Packets dropped because the receiving client was not connected.",
        iroh.send_packets_dropped.get(),
    );
    counter(
        &mut out,
        "anyplay_relay_rate_limited_bytes_total",
        "Bytes from clients that were held back by the per-client rate limit.",
        iroh.bytes_rx_ratelimited_total.get(),
    );
    counter(
        &mut out,
        "anyplay_relay_rate_limited_connections_total",
        "Connections held back by the per-client rate limit at least once.",
        iroh.conns_rx_ratelimited_total.get(),
    );

    let _ = writeln!(
        out,
        "# HELP anyplay_relay_ticket_checks_total Tickets checked, by result.\n\
         # TYPE anyplay_relay_ticket_checks_total counter"
    );
    for result in TicketResult::ALL {
        let _ = writeln!(
            out,
            "anyplay_relay_ticket_checks_total{{result=\"{}\"}} {}",
            result.name(),
            sources.stats.checks(result)
        );
    }
    let _ = writeln!(
        out,
        "# HELP anyplay_relay_refusals_total Connections turned away, by reason.\n\
         # TYPE anyplay_relay_refusals_total counter"
    );
    for refusal in Refusal::ALL {
        let _ = writeln!(
            out,
            "anyplay_relay_refusals_total{{reason=\"{}\"}} {}",
            refusal.name(),
            sources.stats.refusals(refusal)
        );
    }
    out
}

fn gauge(out: &mut String, name: &str, help: &str, value: u64) {
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

pub async fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

/// Serves `GET /metrics` on `listener` until the task is dropped. It speaks just enough HTTP for a
/// scraper and holds no connection open longer than a few seconds.
pub async fn serve(listener: TcpListener, sources: Sources) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            // Out of file descriptors, most likely; do not spin.
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let sources = sources.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), answer(stream, &sources)).await;
        });
    }
}

async fn answer(mut stream: TcpStream, sources: &Sources) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 4096 {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..n]);
    }
    let line = head.split(|b| *b == b'\r').next().unwrap_or_default();
    let (status, body) = if line.starts_with(b"GET /metrics ") || line.starts_with(b"GET /metrics?")
    {
        ("200 OK", render(sources))
    } else {
        ("404 Not Found", String::from("not found\n"))
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::AdmissionLimits;

    fn sources() -> Sources {
        Sources {
            admission: Arc::new(Admission::new(AdmissionLimits::default())),
            stats: Arc::new(Stats::default()),
            iroh: Arc::default(),
        }
    }

    #[test]
    fn the_text_holds_every_number_and_no_ids() {
        let sources = sources();
        sources.stats.check(TicketResult::Expired);
        sources.stats.refuse(Refusal::RelayFull);
        sources.iroh.bytes_recv.inc_by(1234);
        let text = render(&sources);
        for expected in [
            "anyplay_relay_clients 0",
            "anyplay_relay_bytes_in_total 1234",
            "anyplay_relay_bytes_out_total 0",
            "anyplay_relay_ticket_checks_total{result=\"expired\"} 1",
            "anyplay_relay_ticket_checks_total{result=\"ok\"} 0",
            "anyplay_relay_refusals_total{reason=\"relay_full\"} 1",
            "anyplay_relay_refusals_total{reason=\"account_limit\"} 0",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(!text.contains("endpoint=") && !text.contains("account="));
    }

    #[tokio::test]
    async fn the_port_serves_metrics_and_nothing_else() {
        let listener = bind(([127, 0, 0, 1], 0).into()).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(serve(listener, sources()));
        let get = |path: &'static str| async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).await.unwrap();
            text
        };
        let ok = get("/metrics").await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("anyplay_relay_clients 0"));
        assert!(get("/").await.starts_with("HTTP/1.1 404"));
        task.abort();
    }
}
