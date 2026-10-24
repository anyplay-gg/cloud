//! The keyed hash that stops a banned person from signing up again with the same address.
//!
//! The hash is HMAC-SHA-256 of the normalized address under a key the cloud holds outside the
//! database: the `ban_secret` setting, or a random key in `data_dir/ban.key`. Whoever copies only
//! the database cannot test guessed addresses against it. Hashes made before the key existed are
//! plain SHA-256 (`bans.salted = 0`); they are replaced at start for accounts that still exist
//! and when their address shows up again, and match until then ([`BanKey::legacy`]).

use std::path::Path;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest as _, Sha256};

use super::store;
use crate::live::Conn;
use crate::util::sha256;

/// The file in the data directory that holds the generated key.
pub const KEY_FILE: &str = "ban.key";

const BLOCK: usize = 64;

pub struct BanKey(Vec<u8>);

impl std::fmt::Debug for BanKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BanKey(..)")
    }
}

impl BanKey {
    /// The key from the setting, or from `data_dir/ban.key`, which is made on first use. Nodes of
    /// one cloud must share the setting or the file.
    pub fn load(secret: Option<&str>, data_dir: &Path) -> Result<Self> {
        if let Some(secret) = secret {
            return Ok(Self(secret.as_bytes().to_vec()));
        }
        let path = data_dir.join(KEY_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => return Self::from_text(&text, &path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
        let key: [u8; 32] = rand::random();
        match create_private(&path, &STANDARD.encode(key)) {
            Ok(()) => {
                tracing::info!("created a new key for banned e-mail addresses (ban.key)");
                Ok(Self(key.to_vec()))
            }
            // Another node made it first.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                Self::from_text(&text, &path)
            }
            Err(e) => Err(e).with_context(|| format!("cannot write {}", path.display())),
        }
    }

    fn from_text(text: &str, path: &Path) -> Result<Self> {
        let bytes = STANDARD
            .decode(text.trim())
            .with_context(|| format!("{} is not base64", path.display()))?;
        anyhow::ensure!(bytes.len() >= 16, "{} is too short", path.display());
        Ok(Self(bytes))
    }

    /// HMAC-SHA-256 of an address that is already normalized.
    pub fn hash(&self, email: &str) -> [u8; 32] {
        hmac_sha256(&self.0, email.as_bytes())
    }

    /// The hash an older version stored.
    pub fn legacy(email: &str) -> [u8; 32] {
        sha256(email.as_bytes())
    }

    #[cfg(test)]
    pub fn for_test(secret: &str) -> Self {
        Self(secret.as_bytes().to_vec())
    }
}

#[cfg(unix)]
fn create_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn create_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Gives every ban made before the key existed the keyed hash, as far as its account still
/// exists (the address is then known). Run at start; it changes nothing the second time.
pub async fn resalt_known(conn: &mut Conn, key: &BanKey) -> sqlx::Result<u64> {
    let mut done = 0;
    for (account, email) in store::unsalted_bans(conn).await? {
        store::resalt_ban(conn, store::u(account), &key.hash(&email)).await?;
        done += 1;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_long_key_is_hashed_first() {
        // RFC 4231 case 6: a key of 131 bytes.
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn the_key_changes_the_hash() {
        let a = BanKey::for_test("a-secret-of-some-length");
        let b = BanKey::for_test("another-secret-of-length");
        assert_ne!(a.hash("x@example.com"), b.hash("x@example.com"));
        assert_ne!(a.hash("x@example.com"), BanKey::legacy("x@example.com"));
    }

    #[test]
    fn a_key_file_is_made_once_and_read_after() {
        let dir = tempfile::tempdir().unwrap();
        let first = BanKey::load(None, dir.path()).unwrap();
        let second = BanKey::load(None, dir.path()).unwrap();
        assert_eq!(first.hash("x@example.com"), second.hash("x@example.com"));
    }
}
