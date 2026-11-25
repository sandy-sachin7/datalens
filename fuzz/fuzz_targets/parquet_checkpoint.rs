#![no_main]

use deltalens::core::parser::checkpoint::parse_parquet_checkpoint;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = parse_parquet_checkpoint(data, "fuzz.checkpoint.parquet");
});
