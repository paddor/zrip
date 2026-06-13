#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must not panic on arbitrary input, only return Err
    let _ = zrip::decompress(data, 4 * 1024 * 1024);
});
