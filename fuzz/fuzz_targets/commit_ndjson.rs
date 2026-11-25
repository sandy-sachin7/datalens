#![no_main]

use deltalens::core::parser::commit::CommitParser;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = CommitParser::parse_bytes_with_warnings(data, "fuzz.json");
});
