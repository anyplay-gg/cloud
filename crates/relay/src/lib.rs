//! An iroh relay for the AnyPlay network.
//!
//! It is `iroh-relay` with one change: a client is admitted only when it presents a relay ticket
//! that a cloud signed for its own endpoint ([`TicketAccess`]). The relay checks tickets with the
//! cloud's public key and never contacts the cloud.

pub mod config;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyplay_ticket::{Kind, Verifier};
use iroh_relay::server::{Access, AccessControl, ClientRequest};

/// Admits an endpoint only with a valid relay ticket that names it.
#[derive(Clone, Debug)]
pub struct TicketAccess {
    verifier: Arc<Verifier>,
}

impl TicketAccess {
    pub fn new(verifier: Verifier) -> Self {
        Self {
            verifier: Arc::new(verifier),
        }
    }

    /// Decides for one endpoint and token. The reason is for the log only; the client is always
    /// told the same thing.
    fn check(&self, endpoint: &str, token: Option<&str>, now: u64) -> Result<(), &'static str> {
        let token = token.ok_or("no ticket")?;
        let claims =
            self.verifier
                .verify(token, Kind::Relay, now)
                .map_err(|error| match error {
                    anyplay_ticket::TicketError::Expired => "ticket expired",
                    anyplay_ticket::TicketError::Signature => "ticket signed by another key",
                    anyplay_ticket::TicketError::Kind => "ticket of the wrong kind",
                    anyplay_ticket::TicketError::Malformed => "ticket malformed",
                })?;
        if claims.endpoint.as_deref() == Some(endpoint) {
            Ok(())
        } else {
            Err("ticket is for another endpoint")
        }
    }
}

impl AccessControl for TicketAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let endpoint = hex(request.endpoint_id().as_bytes());
        let token = request.auth_token();
        match self.check(&endpoint, token.as_deref(), unix_now()) {
            Ok(()) => Access::Allow,
            Err(reason) => {
                tracing::debug!(%endpoint, reason, "relay client refused");
                Access::Deny {
                    reason: Some("not authorized".into()),
                }
            }
        }
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
    use anyplay_ticket::{Claims, Signer};

    use super::*;

    #[test]
    fn only_a_ticket_for_this_endpoint_passes() {
        let signer = Signer::generate();
        let access = TicketAccess::new(signer.verifier());
        let endpoint = "cd".repeat(32);
        let ticket = signer.issue(&Claims::relay(1, endpoint.clone(), 100));
        assert_eq!(access.check(&endpoint, Some(&ticket), 50), Ok(()));
        assert!(access.check(&endpoint, Some(&ticket), 100).is_err());
        assert!(access.check(&"ef".repeat(32), Some(&ticket), 50).is_err());
        assert!(access.check(&endpoint, None, 50).is_err());
        let other = Signer::generate().issue(&Claims::relay(1, endpoint.clone(), 100));
        assert!(access.check(&endpoint, Some(&other), 50).is_err());
        let access_ticket = signer.issue(&Claims::access(1, 2, 100));
        assert!(access.check(&endpoint, Some(&access_ticket), 50).is_err());
    }

    #[test]
    fn endpoint_ids_are_lowercase_hex() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }
}
