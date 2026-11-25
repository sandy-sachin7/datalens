#![no_main]

use deltalens::core::parser::checkpoint::LastCheckpoint;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = LastCheckpoint::parse_bytes(data);
});
