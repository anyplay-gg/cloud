//! Signed tokens that the AnyPlay cloud issues and that relays and the cloud itself check.
//!
//! A token is `v2.<payload>.<signature>`. The payload is a small JSON object in unpadded URL-safe
//! base64. It names the kind (`k`), the audience (`aud`), the key that signed it (`kid`), the
//! account (`a`), the device (`d`) or relay endpoint (`e`), the time it was issued (`i`) and the
//! time it expires (`x`). The signature is an Ed25519 signature over the domain separator
//! `anyplay-ticket-v2\0`, the text `v2.` and the payload part, so a signature made for anything
//! else (a proof, another version) is never a token. A token is valid while `now < expires`.
//!
//! Tokens of the first version, `v1.<payload>.<signature>`, without `i`, `kid` and `aud`, are still
//! read (with issue time 0) and never made. See `spec/cloud-api.md` in anyplay-gg/network.
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
use sha2::{Digest as _, Sha256};

const PREFIX_V1: &str = "v1.";
const PREFIX_V2: &str = "v2.";
/// Signed in front of the token text of version 2, so that no other protocol that signs with
/// Ed25519 can produce a valid token.
const DOMAIN_V2: &[u8] = b"anyplay-ticket-v2\0";
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
    /// A banned person may read their statements and appeal. Issued with a ban error.
    Appeal,
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
    /// Unix seconds when the token was issued; 0 for a token of version 1. The cloud refuses an
    /// access token issued before the account's "not before" time.
    pub issued: u64,
    /// Unix seconds. The token is invalid from this second on.
    pub expires: u64,
}

impl Claims {
    /// Claims for an access token.
    pub fn access(account: u64, device: u64, issued: u64, expires: u64) -> Self {
        Self {
            kind: Kind::Access,
            account,
            device: Some(device),
            endpoint: None,
            issued,
            expires,
        }
    }

    /// Claims for an appeal token: the account, and nothing else.
    pub fn appeal(account: u64, issued: u64, expires: u64) -> Self {
        Self {
            kind: Kind::Appeal,
            account,
            device: None,
            endpoint: None,
            issued,
            expires,
        }
    }

    /// Claims for a relay token bound to one endpoint.
    pub fn relay(account: u64, endpoint: impl Into<String>, issued: u64, expires: u64) -> Self {
        Self {
            kind: Kind::Relay,
            account,
            device: None,
            endpoint: Some(endpoint.into()),
            issued,
            expires,
        }
    }
}

/// The audience a kind of token is for.
fn audience(kind: Kind) -> &'static str {
    match kind {
        Kind::Access => "cloud",
        Kind::Relay => "relay",
        Kind::Appeal => "appeal",
    }
}

/// The JSON object inside a token of version 1.
#[derive(Deserialize)]
struct PayloadV1 {
    k: Kind,
    a: u64,
    #[serde(default)]
    d: Option<u64>,
    #[serde(default)]
    e: Option<String>,
    x: u64,
}

/// The JSON object inside a token of version 2.
#[derive(Serialize, Deserialize)]
struct Payload {
    k: Kind,
    aud: String,
    kid: String,
    a: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    d: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<String>,
    i: u64,
    x: u64,
}

/// The ID of a public key as it appears in tokens: the first 4 bytes of its SHA-256 digest, in
/// lowercase hex. It names which key to check with, so keys can be rotated without signing
/// everybody out.
pub fn key_id(public: &[u8; 32]) -> String {
    Sha256::digest(public)[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
    /// The token names a key this verifier does not have (version 2).
    UnknownKey,
}

impl fmt::Display for TicketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TicketError::Malformed => "the token is malformed",
            TicketError::Signature => "the token's signature is wrong",
            TicketError::Kind => "the token is of another kind",
            TicketError::Expired => "the token has expired",
            TicketError::UnknownKey => "the token was signed with an unknown key",
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

    /// Signs the claims into a token of version 1, which the cloud no longer makes. For tests and
    /// fuzzing of the code that still reads them; the issue time is not part of the token.
    #[doc(hidden)]
    pub fn issue_legacy(&self, claims: &Claims) -> String {
        let mut json =
            serde_json::json!({"k": claims.kind, "a": claims.account, "x": claims.expires});
        if let Some(device) = claims.device {
            json["d"] = device.into();
        }
        if let Some(endpoint) = &claims.endpoint {
            json["e"] = endpoint.clone().into();
        }
        let mut token = format!("{PREFIX_V1}{}", URL_SAFE_NO_PAD.encode(json.to_string()));
        let signature = self.key.sign(token.as_bytes());
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        token
    }

    /// The matching public key, for verifiers.
    pub fn verifier(&self) -> Verifier {
        Verifier {
            key: self.key.verifying_key(),
        }
    }

    /// Signs the claims into a token of version 2.
    pub fn issue(&self, claims: &Claims) -> String {
        let payload = Payload {
            k: claims.kind,
            aud: audience(claims.kind).to_owned(),
            kid: key_id(&self.key.verifying_key().to_bytes()),
            a: claims.account,
            d: claims.device,
            e: claims.endpoint.clone(),
            i: claims.issued,
            x: claims.expires,
        };
        // A struct of numbers, an enum and strings always serializes.
        let json = serde_json::to_vec(&payload).unwrap_or_default();
        let mut token = format!("{PREFIX_V2}{}", URL_SAFE_NO_PAD.encode(json));
        let mut message = DOMAIN_V2.to_vec();
        message.extend_from_slice(token.as_bytes());
        let signature = self.key.sign(&message);
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

    /// The ID of this verifier's key, as tokens of version 2 name it.
    pub fn kid(&self) -> String {
        key_id(&self.key.to_bytes())
    }

    /// Checks signature, kind and expiry, in that order, and returns the claims. Reads tokens of
    /// version 2 and, for tokens issued before it existed, version 1.
    pub fn verify(&self, token: &str, kind: Kind, now: u64) -> Result<Claims, TicketError> {
        if token.len() > MAX_TOKEN_LEN {
            return Err(TicketError::Malformed);
        }
        let (version, rest) = if let Some(rest) = token.strip_prefix(PREFIX_V2) {
            (2, rest)
        } else if let Some(rest) = token.strip_prefix(PREFIX_V1) {
            (1, rest)
        } else {
            return Err(TicketError::Malformed);
        };
        let (payload_part, signature_part) = rest.split_once('.').ok_or(TicketError::Malformed)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_part)
            .map_err(|_| TicketError::Malformed)?;
        let signature: [u8; 64] = signature.try_into().map_err(|_| TicketError::Malformed)?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature);
        let prefix = if version == 2 { PREFIX_V2 } else { PREFIX_V1 };
        let signed = &token[..prefix.len() + payload_part.len()];
        let mut message = Vec::new();
        if version == 2 {
            message.extend_from_slice(DOMAIN_V2);
        }
        message.extend_from_slice(signed.as_bytes());
        self.key
            .verify_strict(&message, &signature)
            .map_err(|_| TicketError::Signature)?;

        let json = URL_SAFE_NO_PAD
            .decode(payload_part)
            .map_err(|_| TicketError::Malformed)?;
        let (k, account, device, endpoint, issued, expires) = if version == 2 {
            let p: Payload = serde_json::from_slice(&json).map_err(|_| TicketError::Malformed)?;
            if p.kid != self.kid() {
                return Err(TicketError::UnknownKey);
            }
            if p.aud != audience(p.k) || p.i > p.x {
                return Err(TicketError::Malformed);
            }
            (p.k, p.a, p.d, p.e, p.i, p.x)
        } else {
            let p: PayloadV1 = serde_json::from_slice(&json).map_err(|_| TicketError::Malformed)?;
            (p.k, p.a, p.d, p.e, 0, p.x)
        };
        let well_formed = match k {
            Kind::Access => device.is_some() && endpoint.is_none(),
            Kind::Relay => endpoint.is_some() && device.is_none(),
            Kind::Appeal => endpoint.is_none() && device.is_none(),
        };
        if !well_formed {
            return Err(TicketError::Malformed);
        }
        if k != kind {
            return Err(TicketError::Kind);
        }
        if now >= expires {
            return Err(TicketError::Expired);
        }
        Ok(Claims {
            kind: k,
            account,
            device,
            endpoint,
            issued,
            expires,
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

    /// A token of version 1, as the first cloud made it.
    fn v1_token(signer: &Signer, json: &str) -> String {
        let mut token = format!("v1.{}", URL_SAFE_NO_PAD.encode(json));
        let signature = signer.key.sign(token.as_bytes());
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        token
    }

    /// A fixed key and fixed claims give a fixed token. Another implementation of the format
    /// (a relay in another language, say) must read this one and produce this exact string.
    #[test]
    fn test_vector() {
        let signer = Signer::from_secret([7; 32]);
        assert_eq!(signer.verifier().kid(), "fe812c12");
        let access = signer.issue(&Claims::access(41, 3, 1_800_000_000, 1_800_000_900));
        assert_eq!(
            access,
            "v2.eyJrIjoiYWNjZXNzIiwiYXVkIjoiY2xvdWQiLCJraWQiOiJmZTgxMmMxMiIsImEiOjQxLCJkIjozLCJpIjoxODAwMDAwMDAwLCJ4IjoxODAwMDAwOTAwfQ.v5hqHZXmSEZZ7e4zoEjBo16I88VTCue4tXoeyZHnSfpieSu1We51qnpsZ5LQnXDg-6kc5QCT5-GAVjkVRRxCCg"
        );
        let relay = signer.issue(&Claims::relay(41, endpoint(), 1_800_000_000, 1_800_086_400));
        assert_eq!(
            relay,
            "v2.eyJrIjoicmVsYXkiLCJhdWQiOiJyZWxheSIsImtpZCI6ImZlODEyYzEyIiwiYSI6NDEsImUiOiJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiIiwiaSI6MTgwMDAwMDAwMCwieCI6MTgwMDA4NjQwMH0.3mLYH9L8CmrFxXt8P6n2fwcCXLAev3vaP-uVdWvLDOjVjqwqKU_i84TOSNZ0fZF0Vox-JCT9xSfzyI-tumsoDw"
        );
        let claims = signer
            .verifier()
            .verify(&access, Kind::Access, 1_800_000_001)
            .unwrap();
        assert_eq!(claims, Claims::access(41, 3, 1_800_000_000, 1_800_000_900));
    }

    #[test]
    fn an_access_token_round_trips() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW - 5, NOW + 900));
        assert!(token.starts_with("v2."));
        assert_eq!(token.matches('.').count(), 2);
        let claims = signer.verifier().verify(&token, Kind::Access, NOW).unwrap();
        assert_eq!(claims, Claims::access(7, 9, NOW - 5, NOW + 900));
    }

    #[test]
    fn a_relay_token_round_trips_with_its_endpoint() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::relay(7, endpoint(), NOW, NOW + 60));
        let claims = signer.verifier().verify(&token, Kind::Relay, NOW).unwrap();
        assert_eq!(claims.endpoint.as_deref(), Some(endpoint().as_str()));
        assert_eq!(claims.device, None);
    }

    #[test]
    fn the_payload_names_kind_audience_key_and_times() {
        let signer = Signer::from_secret([7; 32]);
        let token = signer.issue(&Claims::access(7, 9, 11, 12));
        let payload = token.split('.').nth(1).unwrap();
        let json = URL_SAFE_NO_PAD.decode(payload).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "k": "access", "aud": "cloud", "kid": signer.verifier().kid(),
                "a": 7, "d": 9, "i": 11, "x": 12
            })
        );
    }

    #[test]
    fn a_token_of_version_1_is_still_read_with_issue_time_0() {
        let signer = Signer::generate();
        let token = v1_token(&signer, r#"{"k":"access","a":7,"d":9,"x":99999999}"#);
        let claims = signer.verifier().verify(&token, Kind::Access, NOW).unwrap();
        assert_eq!(claims.issued, 0);
        assert_eq!(claims.device, Some(9));
        // The signature of version 1 is not valid under the label of version 2, and the other
        // way round.
        let relabelled = token.replacen("v1.", "v2.", 1);
        assert_eq!(
            signer.verifier().verify(&relabelled, Kind::Access, NOW),
            Err(TicketError::Signature)
        );
    }

    #[test]
    fn a_signature_made_without_the_domain_separator_is_refused() {
        let signer = Signer::generate();
        let payload = Payload {
            k: Kind::Access,
            aud: "cloud".into(),
            kid: signer.verifier().kid(),
            a: 1,
            d: Some(2),
            e: None,
            i: 0,
            x: 99_999_999,
        };
        let mut token = format!(
            "v2.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        );
        let signature = signer.key.sign(token.as_bytes());
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        assert_eq!(
            signer.verifier().verify(&token, Kind::Access, NOW),
            Err(TicketError::Signature)
        );
    }

    #[test]
    fn another_audience_or_key_id_is_refused_even_when_signed() {
        let signer = Signer::generate();
        for (aud, kid) in [
            ("relay", signer.verifier().kid()),
            ("cloud", "00000000".into()),
        ] {
            let payload = Payload {
                k: Kind::Access,
                aud: aud.into(),
                kid,
                a: 1,
                d: Some(2),
                e: None,
                i: 0,
                x: 99_999_999,
            };
            let mut token = format!(
                "v2.{}",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
            );
            let mut message = DOMAIN_V2.to_vec();
            message.extend_from_slice(token.as_bytes());
            let signature = signer.key.sign(&message);
            token.push('.');
            token.push_str(&URL_SAFE_NO_PAD.encode(signature.to_bytes()));
            let error = signer
                .verifier()
                .verify(&token, Kind::Access, NOW)
                .unwrap_err();
            assert!(
                matches!(error, TicketError::Malformed | TicketError::UnknownKey),
                "{aud}: {error:?}"
            );
        }
    }

    #[test]
    fn an_altered_payload_is_refused() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW, NOW + 900));
        let forged = signer.issue(&Claims::access(8, 9, NOW, NOW + 900));
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
        let mut token = signer.issue(&Claims::access(7, 9, NOW, NOW + 900));
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
        let token = signer.issue(&Claims::access(7, 9, NOW, NOW + 900));
        assert_eq!(
            signer.verifier().verify(&token, Kind::Relay, NOW),
            Err(TicketError::Kind)
        );
    }

    #[test]
    fn an_expired_token_is_refused_from_its_expiry_second() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::access(7, 9, NOW - 10, NOW));
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
        let token = Signer::generate().issue(&Claims::access(7, 9, NOW, NOW + 900));
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
        for text in [
            "", "v1.", "v1..", "v3.a.b", "a.b.c", "v1.abc", "v2.a.b.c", "v2.",
        ] {
            assert_eq!(
                verifier.verify(text, Kind::Access, NOW),
                Err(TicketError::Malformed),
                "{text:?}"
            );
        }
        let long = format!("v2.{}.{}", "a".repeat(2000), "b".repeat(86));
        assert_eq!(
            verifier.verify(&long, Kind::Access, NOW),
            Err(TicketError::Malformed)
        );
    }

    #[test]
    fn a_token_missing_its_binding_is_malformed_even_when_signed() {
        let signer = Signer::generate();
        let token = v1_token(&signer, r#"{"k":"relay","a":1,"x":99999999}"#);
        assert_eq!(
            signer.verifier().verify(&token, Kind::Relay, NOW),
            Err(TicketError::Malformed)
        );
    }

    #[test]
    fn keys_round_trip_through_text() {
        let signer = Signer::generate();
        let again = Signer::from_base64(&signer.secret_base64()).unwrap();
        let token = again.issue(&Claims::access(1, 2, NOW, NOW + 1));
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

#[cfg(test)]
mod appeal_tests {
    use super::*;

    #[test]
    fn an_appeal_token_is_its_own_kind() {
        let signer = Signer::generate();
        let token = signer.issue(&Claims::appeal(7, 5, 1_000));
        let verifier = signer.verifier();
        let claims = verifier.verify(&token, Kind::Appeal, 10).unwrap();
        assert_eq!(
            (claims.account, claims.device, claims.endpoint),
            (7, None, None)
        );
        assert_eq!(
            verifier.verify(&token, Kind::Access, 10),
            Err(TicketError::Kind)
        );
        let access = signer.issue(&Claims::access(7, 1, 5, 1_000));
        assert_eq!(
            verifier.verify(&access, Kind::Appeal, 10),
            Err(TicketError::Kind)
        );
        assert_eq!(
            verifier.verify(&token, Kind::Appeal, 1_000),
            Err(TicketError::Expired)
        );
    }
}
