//! A token the cloud signed, altered: the first byte picks an access or a relay token, and each
//! following pair of bytes overwrites one byte of it at a position. A token passes only as it was
//! issued, so an altered one is refused, and its claims can never be changed.

#![no_main]

use anyplay_ticket::{Claims, Kind, Signer};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&pick, edits)) = data.split_first() else {
        return;
    };
    // A fixed key for the fuzzer only; it signs nothing anywhere else.
    let signer = Signer::from_secret([7; 32]);
    let (claims, kind) = if pick % 2 == 0 {
        (Claims::access(41, 3, 2_000_000_000), Kind::Access)
    } else {
        (
            Claims::relay(41, "ab".repeat(32), 2_000_000_000),
            Kind::Relay,
        )
    };
    let issued = signer.issue(&claims);

    let mut bytes = issued.clone().into_bytes();
    let (edits, _) = edits.as_chunks::<2>();
    for &[at, byte] in edits {
        let at = usize::from(at) % bytes.len();
        bytes[at] = byte;
    }
    let Ok(token) = String::from_utf8(bytes) else {
        return;
    };
    match signer.verifier().verify(&token, kind, 0) {
        Ok(read) => {
            assert_eq!(token, issued, "an altered token passed");
            assert_eq!(read, claims);
        }
        Err(_) => assert_ne!(token, issued, "the issued token was refused"),
    }
});
