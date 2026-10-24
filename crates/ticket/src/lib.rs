//! Signed tokens that the AnyPlay cloud issues and that relays and the cloud itself check.
//!
//! A token is `v1.<payload>.<signature>`. The payload is a small JSON object in unpadded URL-safe
//! base64. The signature is an Ed25519 signature over the ASCII text `v1.` followed by the payload
//! part, in the same encoding. A token is valid while `now < expires`.
//!
//! There are two kinds. An *access* token lets one device of an account call the cloud. A *relay*
//! token lets one endpoint use a relay. The cloud signs both; the relay only needs the public key.

use std::fmt;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "v1.";
/// The longest token a verifier looks at. Real tokens are around 250 bytes.
const MAX_TOKEN_LEN: usize = 1024;

/// What a token allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// One device of an account may call the cloud.
    Access,
    /// One endpoint may use a relay.
    Relay,
}

/// What a valid token says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    pub kind: Kind,
    /// The account the token was issued to.
    pub account: u64,
    /// The device, for access tokens.
    pub device: Option<u64>,
    /// The endpoint ID as lowercase hex, for relay tokens.
    pub endpoint: Option<String>,
    /// Unix seconds. The token is invalid from this second on.
    pub expires: u64,
}

impl Claims {
    /// Claims for an access token.
    pub fn access(account: u64, device: u64, expires: u64) -> Self {
        Self {
            kind: Kind::Access,
            account,
            device: Some(device),
            endpoint: None,
            expires,
        }
    }

    /// Claims for a relay token bound to one endpoint.
    pub fn relay(account: u64, endpoint: impl Into<String>, expires: u64) -> Self {
        Self {
            kind: Kind::Relay,
            account,
            device: None,
            endpoint: Some(endpoint.into()),
            expires,
        }
    }
}

/// The JSON object inside a token.
#[derive(Serialize, Deserialize)]
struct Payload {
    k: Kind,
    a: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    d: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<String>,
    x: u64,
}

/// Why a token was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketError {
    /// The token is not in the `v1.<payload>.<signature>` shape, or the payload is nonsense.
    Malformed,
    /// The signature does not match: forged, altered or signed with another key.
    Signature,
    /// The token is valid but of the other kind.
    Kind,
    /// The token is past its expiry.
    Expired,
}

impl fmt::Display for TicketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TicketError::Malformed => "the token is malformed",
            TicketError::Signature => "the token's signature is wrong",
            TicketError::Kind => "the token is of another kind",
            TicketError::Expired => "the token has expired",
        })
    }
}

impl std::error::Error for TicketError {}

/// Issues tokens. Holds the secret signing key.
pub struct Signer {
    key: SigningKey,
}

impl fmt::Debug for Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Signer(..)")
    }
}

impl Signer {
    /// A signer from a 32-byte secret.
    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(&secret),
        }
    }

    /// A signer with a fresh random secret.
    pub fn generate() -> Self {
        Self::from_secret(rand::random())
    }

    /// The secret as standard base64, the form that [`Signer::load`] reads.
    pub fn secret_base64(&self) -> String {
        STANDARD.encode(self.key.to_bytes())
    }

    /// A signer from base64 text holding the 32-byte secret. Surrounding white space is ignored.
    pub fn from_base64(text: &str) -> Result<Self, KeyError> {
        let bytes = STANDARD.decode(text.trim()).map_err(|_| KeyError)?;
        let secret: [u8; 32] = bytes.try_into().map_err(|_| KeyError)?;
        Ok(Self::from_secret(secret))
    }

    /// Reads a key file written by [`Signer::save`].
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_base64(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Writes the secret to a new file, readable by the owner only where the platform allows it.
    /// Fails when the file exists, so a key is never overwritten by accident.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        use io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(self.secret_base64().as_bytes())?;
        file.write_all(b"\n")
    }

    /// Loads the key at `path`, or creates it when the file does not exist yet. The flag says
    /// whether it was created.
    pub fn load_or_create(path: impl AsRef<Path>) -> io::Result<(Self, bool)> {
        let path = path.as_ref();
        match Self::load(path) {
            Ok(signer) => Ok((signer, false)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let signer = Self::generate();
                signer.save(path)?;
                Ok((signer, true))
            }
            Err(e) => Err(e),
        }
    }

    /// The matching public key, for verifiers.
    pub fn verifier(&self) -> Verifier {
        Verifier {
            key: self.key.verifying_key(),
        }
    }

    /// Signs the claims into a token.
    pub fn issue(&self, claims: &Claims) -> String {
        let payload = Payload {
            k: claims.kind,
            a: claims.account,
            d: claims.device,
            e: claims.endpoint.clone(),
            x: claims.expires,
        };
        // A struct of numbers, an enum and an optional string always serializes.
        let json = serde_json::to_vec(&payload).unwrap_or_default();
        let mut token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json));
        let signature = self.key.sign(token.as_bytes());
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        token
    }
}

/// Checks tokens. Holds the public key only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verifier {
    key: VerifyingKey,
}

impl Verifier {
    /// A verifier from the public key as standard base64 text. Surrounding white space is ignored.
    pub fn from_base64(text: &str) -> Result<Self, KeyError> {
        let bytes = STANDARD.decode(text.trim()).map_err(|_| KeyError)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| KeyError)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| KeyError)?;
        Ok(Self { key })
    }

    /// The public key as standard base64, the form that [`Verifier::from_base64`] reads.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.key.to_bytes())
    }

    /// Checks signature, kind and expiry, in that order, and returns the claims.
    pub fn verify(&self, token: &str, kind: Kind, now: u64) -> Result<Claims, TicketError> {
        if token.len() > MAX_TOKEN_LEN {
            return Err(TicketError::Malformed);
        }
        let rest = token.strip_prefix(PREFIX).ok_or(TicketError::Malformed)?;
        let (payload_part, signature_part) = rest.split_once('.').ok_or(TicketError::Malformed)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_part)
            .map_err(|_| TicketError::Malformed)?;
        let signature: [u8; 64] = signature.try_into().map_err(|_| TicketError::Malformed)?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature);
        let signed = &token[..PREFIX.len() + payload_part.len()];
        self.key
            .verify_strict(signed.as_bytes(), &signature)
            .map_err(|_| TicketError::Signature)?;

        let json = URL_SAFE_NO_PAD
            .decode(payload_part)
            .map_err(|_| TicketError::Malformed)?;
        let payload: Payload = serde_json::from_slice(&json).map_err(|_| TicketError::Malformed)?;
        let well_formed = match payload.k {
            Kind::Access => payload.d.is_some() && payload.e.is_none(),
            Kind::Relay => payload.e.is_some() && payload.d.is_none(),
        };
        if !well_formed {
            return Err(TicketError::Malformed);
        }
        if payload.k != kind {
            return Err(TicketError::Kind);
        }
        if now >= payload.x {
            return Err(TicketError::Expired);
        }
        Ok(Claims {
            kind: payload.k,
            account: payload.a,
            device: payload.d,
            endpoint: payload.e,
            expires: payload.x,
        })
    }
}

/// A key that is not 32 bytes of standard base64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyError;

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a key is 32 bytes in standard base64")
    }
}

impl std::error::Error for KeyError {}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn endpoint() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn an_access_token_round_trips() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW + 900));
        assert!(token.starts_with("v1."));
        assert_eq!(token.matches('.').count(), 2);
        let claims = signer.verifier().verify(&token, Kind::Access, NOW).unwrap();
        assert_eq!(claims, Claims::access(7, 9, NOW + 900));
    }

    #[test]
    fn a_relay_token_round_trips_with_its_endpoint() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::relay(7, endpoint(), NOW + 60));
        let claims = signer.verifier().verify(&token, Kind::Relay, NOW).unwrap();
        assert_eq!(claims.endpoint.as_deref(), Some(endpoint().as_str()));
        assert_eq!(claims.device, None);
    }

    #[test]
    fn the_payload_uses_the_short_field_names() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, 12));
        let payload = token.split('.').nth(1).unwrap();
        let json = URL_SAFE_NO_PAD.decode(payload).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"k": "access", "a": 7, "d": 9, "x": 12})
        );
    }

    #[test]
    fn an_altered_payload_is_refused() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW + 900));
        let forged = signer.issue(&Claims::access(8, 9, NOW + 900));
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[1] = forged.split('.').nth(1).unwrap();
        let tampered = parts.join(".");
        assert_eq!(
            signer.verifier().verify(&tampered, Kind::Access, NOW),
            Err(TicketError::Signature)
        );
    }

    #[test]
    fn an_altered_signature_is_refused() {
        let signer = Signer::generate();
        let mut token = signer.issue(&Claims::access(7, 9, NOW + 900));
        let last = token.pop().unwrap();
        token.push(if last == 'A' { 'B' } else { 'A' });
        let error = signer
            .verifier()
            .verify(&token, Kind::Access, NOW)
            .unwrap_err();
        assert!(matches!(
            error,
            TicketError::Signature | TicketError::Malformed
        ));
    }

    #[test]
    fn the_wrong_kind_is_refused() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW + 900));
        assert_eq!(
            signer.verifier().verify(&token, Kind::Relay, NOW),
            Err(TicketError::Kind)
        );
    }

    #[test]
    fn an_expired_token_is_refused_from_its_expiry_second() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW));
        let verifier = signer.verifier();
        assert!(verifier.verify(&token, Kind::Access, NOW - 1).is_ok());
        assert_eq!(
            verifier.verify(&token, Kind::Access, NOW),
            Err(TicketError::Expired)
        );
        assert_eq!(
            verifier.verify(&token, Kind::Access, NOW + 5),
            Err(TicketError::Expired)
        );
    }

    #[test]
    fn another_key_is_refused() {
        let token = Signer::generate().issue(&Claims::access(7, 9, NOW + 900));
        assert_eq!(
            Signer::generate()
                .verifier()
                .verify(&token, Kind::Access, NOW),
            Err(TicketError::Signature)
        );
    }

    #[test]
    fn garbage_is_malformed() {
        let verifier = Signer::generate().verifier();
        for text in ["", "v1.", "v1..", "v2.a.b", "a.b.c", "v1.abc", "v1.a.b.c"] {
            assert_eq!(
                verifier.verify(text, Kind::Access, NOW),
                Err(TicketError::Malformed),
                "{text:?}"
            );
        }
        let long = format!("v1.{}.{}", "a".repeat(2000), "b".repeat(86));
        assert_eq!(
            verifier.verify(&long, Kind::Access, NOW),
            Err(TicketError::Malformed)
        );
    }

    #[test]
    fn a_token_missing_its_binding_is_malformed_even_when_signed() {
        let signer = Signer::generate();
        let payload = URL_SAFE_NO_PAD.encode(br#"{"k":"relay","a":1,"x":99999999}"#);
        let mut token = format!("v1.{payload}");
        let signature = signer.key.sign(token.as_bytes());
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        assert_eq!(
            signer.verifier().verify(&token, Kind::Relay, NOW),
            Err(TicketError::Malformed)
        );
    }

    #[test]
    fn keys_round_trip_through_text() {
        let signer = Signer::generate();
        let again = Signer::from_base64(&signer.secret_base64()).unwrap();
        let token = again.issue(&Claims::access(1, 2, NOW + 1));
        let verifier = Verifier::from_base64(&signer.verifier().to_base64()).unwrap();
        assert!(verifier.verify(&token, Kind::Access, NOW).is_ok());
        assert!(Signer::from_base64("not a key").is_err());
        assert!(Verifier::from_base64("AAAA").is_err());
    }

    #[test]
    fn a_key_file_is_created_once_and_loaded_after() {
        let dir = std::env::temp_dir().join(format!("anyplay-ticket-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("signing.key");
        let (first, created) = Signer::load_or_create(&path).unwrap();
        assert!(created);
        let (second, created) = Signer::load_or_create(&path).unwrap();
        assert!(!created);
        assert_eq!(first.verifier(), second.verifier());
        assert!(
            first.save(&path).is_err(),
            "an existing key is never overwritten"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
