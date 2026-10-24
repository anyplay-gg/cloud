//! Small helpers: time, random identifiers, hashes and input checks.

use std::time::{SystemTime, UNIX_EPOCH};

use anyplay_api::MAX_ACCOUNT_ID;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

/// The current time in Unix seconds.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The current time in Unix milliseconds, for deadlines the store compares.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A random number usable as an account or device ID: 1 to 2^53 - 1.
pub fn random_id() -> u64 {
    rand::random_range(1..=MAX_ACCOUNT_ID)
}

/// `bytes` random bytes as unpadded URL-safe base64.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    URL_SAFE_NO_PAD.encode(buf)
}

/// 128 random bits written like a UUID: `8-4-4-4-12` lowercase hex digits.
pub fn random_uuid() -> String {
    let bytes: [u8; 16] = rand::random();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares two byte strings without stopping at the first difference.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// True for 64 lowercase hex digits, the form of an endpoint ID.
pub fn is_endpoint(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// True for an `http` or `https` URL of at most 255 characters without white space or control
/// characters.
pub fn is_http_url(text: &str) -> bool {
    text.len() <= 255
        && (text.starts_with("http://") || text.starts_with("https://"))
        && text.len() > "https://".len()
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// An address with most of its name hidden, for a notice or a hint: `a***@example.com`.
pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((name, domain)) => {
            let first: String = name.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_owned(),
    }
}

/// Trims and lowercases an e-mail address and checks its basic shape.
pub fn normalize_email(text: &str) -> Option<String> {
    let email = text.trim().to_ascii_lowercase();
    if email.len() > 254 || !email.is_ascii() {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~-.".contains(&b));
    let labels: Vec<&str> = domain.split('.').collect();
    let domain_ok = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && labels.last().is_some_and(|tld| tld.len() >= 2);
    (local_ok && domain_ok).then_some(email)
}

/// True for a short, printable text of 1 to `max` characters.
pub fn is_label(text: &str, max: usize) -> bool {
    let n = text.chars().count();
    (1..=max).contains(&n) && !text.chars().any(char::is_control) && text.trim() == text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_have_the_usual_shape() {
        let id = random_uuid();
        let parts: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12]);
        assert!(id.bytes().all(|b| b == b'-' || b.is_ascii_hexdigit()));
        assert_eq!(id, id.to_lowercase());
    }

    #[test]
    fn ids_stay_below_two_to_the_53() {
        for _ in 0..1000 {
            assert!((1..=MAX_ACCOUNT_ID).contains(&random_id()));
        }
    }

    #[test]
    fn e_mail_addresses_are_cleaned_and_checked() {
        assert_eq!(
            normalize_email("  Alec@Example.COM ").as_deref(),
            Some("alec@example.com")
        );
        for bad in [
            "",
            "alec",
            "alec@",
            "@example.com",
            "a b@example.com",
            "a@example",
            "a@exa mple.com",
            "a@@example.com",
            "a\r\nBcc: x@example.com@example.com",
            "ä@example.com",
            "a..b@example.com",
            "a@-example.com",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?}");
        }
        assert_eq!(
            normalize_email(&format!("{}@example.com", "a".repeat(250))),
            None
        );
    }

    #[test]
    fn endpoints_are_lowercase_hex() {
        assert!(is_endpoint(&"ab".repeat(32)));
        assert!(!is_endpoint(&"AB".repeat(32)));
        assert!(!is_endpoint("ab"));
        assert!(!is_endpoint(&"zz".repeat(32)));
    }

    #[test]
    fn relay_urls_are_http_urls() {
        assert!(is_http_url("https://relay.example.com/"));
        assert!(is_http_url("http://127.0.0.1:3340"));
        assert!(!is_http_url("ftp://x"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url("https://a b"));
        assert!(!is_http_url(&format!("https://{}", "a".repeat(250))));
    }
}
