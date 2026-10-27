#![no_main]

use libfuzzer_sys::fuzz_target;

// Whatever a player uploads as a clip goes through this reader before anything is stored; it must
// return, never panic, whatever the bytes.
fuzz_target!(|data: &[u8]| {
    let _ = anyplay_cloud::media::read_mp4(&mut std::io::Cursor::new(data));
});
