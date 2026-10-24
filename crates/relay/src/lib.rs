//! An iroh relay for the AnyPlay network.
//!
//! It is `iroh-relay` with one change: a client is admitted only when it presents a relay ticket
//! that a cloud signed for its own endpoint ([`TicketAccess`]). The relay checks tickets with the
//! cloud's public key and never contacts the cloud.
//!
//! The ticket also names the account, and the relay holds each account to a number of
//! connections ([`limits`]). What the relay did is counted and served as Prometheus text
//! ([`metrics`]).

pub mod config;
pub mod limits;
pub mod metrics;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyplay_ticket::{Claims, Kind, TicketError, Verifier};
use iroh_base::EndpointId;
use iroh_relay::server::{Access, AccessControl, ClientRequest, ConnectionId};

pub use limits::{Admission, AdmissionLimits, Refusal, Stats, TicketResult};

/// Admits an endpoint only with a valid relay ticket that names it, and only while the relay and
/// the ticket's account are within their limits ([`AdmissionLimits`]).
#[derive(Clone)]
pub struct TicketAccess {
    verifier: Arc<Verifier>,
    admission: Arc<Admission>,
    stats: Arc<Stats>,
    /// Unix seconds. A seam for tests.
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for TicketAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TicketAccess")
            .field("admission", &self.admission)
            .finish_non_exhaustive()
    }
}

impl TicketAccess {
    /// With the default limits.
    pub fn new(verifier: Verifier) -> Self {
        Self::with_limits(verifier, AdmissionLimits::default())
    }

    pub fn with_limits(verifier: Verifier, limits: AdmissionLimits) -> Self {
        Self::with_clock(verifier, limits, Arc::new(unix_now))
    }

    /// Reads the time from `clock`, in unix seconds, instead of the system clock.
    pub fn with_clock(
        verifier: Verifier,
        limits: AdmissionLimits,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            verifier: Arc::new(verifier),
            admission: Arc::new(Admission::new(limits)),
            stats: Arc::new(Stats::default()),
            clock,
        }
    }

    pub fn admission(&self) -> &Arc<Admission> {
        &self.admission
    }

    pub fn stats(&self) -> &Arc<Stats> {
        &self.stats
    }

    /// Checks the ticket for one endpoint and returns its claims.
    fn check(&self, endpoint: &str, token: Option<&str>, now: u64) -> Result<Claims, TicketResult> {
        let token = token.ok_or(TicketResult::Missing)?;
        let claims =
            self.verifier
                .verify(token, Kind::Relay, now)
                .map_err(|error| match error {
                    TicketError::Expired => TicketResult::Expired,
                    TicketError::Signature | TicketError::UnknownKey => TicketResult::Signature,
                    TicketError::Kind => TicketResult::Kind,
                    TicketError::Malformed => TicketResult::Malformed,
                })?;
        if claims.endpoint.as_deref() != Some(endpoint) {
            return Err(TicketResult::Endpoint);
        }
        let max = self.admission.limits().max_ticket_lifetime_secs;
        if max != 0 && claims.expires.saturating_sub(now) > max {
            return Err(TicketResult::Lifetime);
        }
        Ok(claims)
    }

    /// Decides for one connection and counts the outcome. A client learns why only when it can do
    /// something about it: that this relay is full, or that its account holds too many
    /// connections. Anything about the ticket is the same answer.
    fn decide(
        &self,
        endpoint: &EndpointId,
        connection: ConnectionId,
        token: Option<&str>,
    ) -> Access {
        let now = (self.clock)();
        let claims = match self.check(&hex(endpoint.as_bytes()), token, now) {
            Ok(claims) => {
                self.stats.check(TicketResult::Ok);
                claims
            }
            Err(result) => {
                self.stats.check(result);
                self.stats.refuse(Refusal::Ticket);
                return deny("not authorized");
            }
        };
        match self.admission.admit(connection, claims.account) {
            Ok(()) => Access::Allow,
            Err(refusal) => {
                self.stats.refuse(refusal);
                deny(match refusal {
                    Refusal::RelayFull => "relay is full",
                    _ => "too many connections for this account",
                })
            }
        }
    }
}

fn deny(reason: &str) -> Access {
    Access::Deny {
        reason: Some(reason.into()),
    }
}

impl AccessControl for TicketAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let token = request.auth_token();
        self.decide(
            &request.endpoint_id(),
            request.connection_id(),
            token.as_deref(),
        )
    }

    fn on_disconnect(&self, _endpoint_id: EndpointId, connection_id: ConnectionId) {
        self.admission.release(connection_id);
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyplay_ticket::Signer;
    use iroh_base::SecretKey;
    use iroh_relay::http::ProtocolVersion;

    use super::*;

    fn endpoint_hex(key: &SecretKey) -> String {
        hex(key.public().as_bytes())
    }

    fn request(key: &SecretKey, token: Option<&str>) -> ClientRequest {
        let mut builder = http::Request::builder().uri("/relay");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let (parts, ()) = builder.body(()).unwrap().into_parts();
        ClientRequest::new(key.public(), ProtocolVersion::default(), parts)
    }

    struct Bench {
        signer: Signer,
        access: TicketAccess,
        now: Arc<AtomicU64>,
    }

    fn bench(limits: AdmissionLimits) -> Bench {
        let signer = Signer::generate();
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock = {
            let now = now.clone();
            Arc::new(move || now.load(Ordering::Relaxed))
        };
        let access = TicketAccess::with_clock(signer.verifier(), limits, clock);
        Bench {
            signer,
            access,
            now,
        }
    }

    impl Bench {
        fn ticket(&self, account: u64, key: &SecretKey, lifetime: u64) -> String {
            let issued = self.now.load(Ordering::Relaxed);
            self.signer.issue(&Claims::relay(
                account,
                endpoint_hex(key),
                issued,
                issued + lifetime,
            ))
        }

        /// A new endpoint of `account` connects.
        async fn connect(&self, account: u64) -> (Access, ClientRequest) {
            let key = SecretKey::generate();
            let ticket = self.ticket(account, &key, 3600);
            let request = request(&key, Some(&ticket));
            (self.access.on_connect(&request).await, request)
        }

        fn leave(&self, request: &ClientRequest) {
            self.access
                .on_disconnect(request.endpoint_id(), request.connection_id());
        }
    }

    fn reason(access: &Access) -> &str {
        match access {
            Access::Deny { reason } => reason.as_deref().unwrap_or(""),
            Access::Allow => "",
        }
    }

    #[tokio::test]
    async fn only_a_ticket_for_this_endpoint_passes() {
        let b = bench(AdmissionLimits::default());
        let key = SecretKey::generate();
        let ticket = b.ticket(1, &key, 100);
        let endpoint = endpoint_hex(&key);
        let now = b.now.load(Ordering::Relaxed);
        assert!(b.access.check(&endpoint, Some(&ticket), now).is_ok());
        assert_eq!(
            b.access.check(&endpoint, Some(&ticket), now + 100).err(),
            Some(TicketResult::Expired)
        );
        assert_eq!(
            b.access.check(&"ef".repeat(32), Some(&ticket), now).err(),
            Some(TicketResult::Endpoint)
        );
        assert_eq!(
            b.access.check(&endpoint, None, now).err(),
            Some(TicketResult::Missing)
        );
        let other = Signer::generate().issue(&Claims::relay(1, endpoint.clone(), now, now + 100));
        assert_eq!(
            b.access.check(&endpoint, Some(&other), now).err(),
            Some(TicketResult::Signature)
        );
        let access_ticket = b.signer.issue(&Claims::access(1, 2, now, now + 100));
        assert_eq!(
            b.access.check(&endpoint, Some(&access_ticket), now).err(),
            Some(TicketResult::Kind)
        );
        assert_eq!(
            b.access.check(&endpoint, Some("v1.x.y"), now).err(),
            Some(TicketResult::Malformed)
        );
    }

    #[tokio::test]
    async fn a_ticket_that_lives_too_long_is_refused_and_one_at_the_limit_is_not() {
        let limits = AdmissionLimits {
            max_ticket_lifetime_secs: 3600,
            ..AdmissionLimits::default()
        };
        let b = bench(limits);
        let key = SecretKey::generate();
        let endpoint = endpoint_hex(&key);
        let now = b.now.load(Ordering::Relaxed);
        let at_limit = b.ticket(1, &key, 3600);
        assert!(b.access.check(&endpoint, Some(&at_limit), now).is_ok());
        let too_long = b.ticket(1, &key, 3601);
        assert_eq!(
            b.access.check(&endpoint, Some(&too_long), now).err(),
            Some(TicketResult::Lifetime)
        );
        // Later the same ticket has less time left and passes: the limit is on what remains.
        assert!(b.access.check(&endpoint, Some(&too_long), now + 1).is_ok());
        // Off: any lifetime.
        let b = bench(AdmissionLimits {
            max_ticket_lifetime_secs: 0,
            ..limits
        });
        let forever = b.ticket(1, &key, 10 * 365 * 86_400);
        assert!(b.access.check(&endpoint, Some(&forever), now).is_ok());
    }

    #[tokio::test]
    async fn an_account_may_hold_only_so_many_connections() {
        let b = bench(AdmissionLimits {
            max_connections_per_account: 2,
            ..AdmissionLimits::default()
        });
        let (first, first_request) = b.connect(7).await;
        let (second, _) = b.connect(7).await;
        assert_eq!((first, second), (Access::Allow, Access::Allow));
        let (third, _) = b.connect(7).await;
        assert!(reason(&third).contains("account"), "{third:?}");
        assert_eq!(b.access.stats().refusals(Refusal::AccountLimit), 1);

        let (other, _) = b.connect(8).await;
        assert_eq!(other, Access::Allow, "another account is not affected");

        // A connection that ends gives its place back.
        b.leave(&first_request);
        let (again, _) = b.connect(7).await;
        assert_eq!(again, Access::Allow);
        assert_eq!(b.access.admission().clients(), 3);
        assert_eq!(b.access.admission().accounts(), 2);
    }

    #[tokio::test]
    async fn a_relay_carries_only_so_many_clients() {
        let b = bench(AdmissionLimits {
            max_clients: 2,
            ..AdmissionLimits::default()
        });
        let (a, a_request) = b.connect(1).await;
        let (c, _) = b.connect(2).await;
        assert_eq!((a, c), (Access::Allow, Access::Allow));
        let (full, _) = b.connect(3).await;
        assert!(reason(&full).contains("full"), "{full:?}");
        assert_eq!(b.access.stats().refusals(Refusal::RelayFull), 1);
        b.leave(&a_request);
        let (room, _) = b.connect(3).await;
        assert_eq!(room, Access::Allow);
        // Leaving twice changes nothing.
        b.leave(&a_request);
        assert_eq!(b.access.admission().clients(), 2);
    }

    #[tokio::test]
    async fn a_zero_limit_is_no_limit() {
        let b = bench(AdmissionLimits {
            max_clients: 0,
            max_connections_per_account: 0,
            max_ticket_lifetime_secs: 0,
        });
        for _ in 0..50 {
            assert_eq!(b.connect(1).await.0, Access::Allow);
        }
    }

    #[tokio::test]
    async fn expiry_follows_the_clock() {
        let b = bench(AdmissionLimits::default());
        let key = SecretKey::generate();
        let ticket = b.ticket(1, &key, 60);
        let first = request(&key, Some(&ticket));
        assert_eq!(b.access.on_connect(&first).await, Access::Allow);
        b.now.fetch_add(60, Ordering::Relaxed);
        let later = request(&key, Some(&ticket));
        assert!(matches!(
            b.access.on_connect(&later).await,
            Access::Deny { .. }
        ));
        assert_eq!(b.access.stats().checks(TicketResult::Expired), 1);
    }

    #[tokio::test]
    async fn every_attempt_is_counted_once_by_result_and_refusal() {
        let b = bench(AdmissionLimits {
            max_connections_per_account: 1,
            ..AdmissionLimits::default()
        });
        let key = SecretKey::generate();
        assert!(matches!(
            b.access.on_connect(&request(&key, None)).await,
            Access::Deny { .. }
        ));
        assert_eq!(b.connect(1).await.0, Access::Allow);
        assert!(matches!(b.connect(1).await.0, Access::Deny { .. }));
        let stats = b.access.stats();
        assert_eq!(stats.checks(TicketResult::Missing), 1);
        // The ticket of the refused account connection was fine.
        assert_eq!(stats.checks(TicketResult::Ok), 2);
        assert_eq!(stats.refusals(Refusal::Ticket), 1);
        assert_eq!(stats.refusals(Refusal::AccountLimit), 1);
        assert_eq!(stats.refusals(Refusal::RelayFull), 0);
    }

    #[test]
    fn endpoint_ids_are_lowercase_hex() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }
}
