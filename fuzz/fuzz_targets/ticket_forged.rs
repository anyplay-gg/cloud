//! Any text a client may present as a token. No key but the cloud's can sign one, so every such
//! text is refused, of either kind, at any time, and never crashes the check.

#![no_main]

use anyplay_ticket::{Kind, Signer};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(token) = std::str::from_utf8(data) else {
        return;
    };
    // A fixed key for the fuzzer only; it signs nothing anywhere else.
    let verifier = Signer::from_secret([7; 32]).verifier();
    for kind in [Kind::Access, Kind::Relay] {
        assert!(verifier.verify(token, kind, 0).is_err(), "{token:?} passed");
    }
});
