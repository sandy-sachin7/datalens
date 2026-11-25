use deltalens::core::model::schema::StructType;
use deltalens::core::parser::checkpoint::{parse_parquet_checkpoint, LastCheckpoint};
use deltalens::core::parser::commit::CommitParser;

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

#[test]
fn test_commit_ndjson_parser_never_panics_on_malformed_inputs() {
    let corpus: &[&[u8]] = &[
        b"",
        b"\n\r\n   \t  \n",
        b"{",
        b"{{{}}}",
        b"null\n[]\n42\n\"str\"\n",
        b"\xff\xfe\x00\x80\n{\"add\":null}\n",
        br#"{"add":{"path":123,"size":"nan"}}"#,
        br#"{"protocol":{"minReaderVersion":"bad","minWriterVersion":null}}"#,
        br#"{"metaData":{"schemaString":123,"partitionColumns":null,"configuration":{"a":null}}}"#,
        br#"{"commitInfo":{"timestamp":-9223372036854775808,"notebook":{"notebookId":999999999999999999999}}}"#,
        br#"{"unknownExtensionAction":{"nested":[1,2,3]}}"#,
    ];

    for input in corpus {
        let _ = CommitParser::parse_bytes_with_warnings(input, "adversarial.json");
    }

    // Deterministic pseudo-random byte fuzzing (500 mutated inputs).
    let mut seed = 0xDEAD_BEEF_CAFE_BABEu64;
    let base = br#"{"add":{"path":"a.parquet","size":1,"modificationTime":0,"dataChange":true,"partitionValues":{"k":"v"}}}"#;
    for i in 0..500 {
        let len = (xorshift64(&mut seed) % 512) as usize;
        let mut buf = Vec::with_capacity(len);
        if i % 2 == 0 {
            buf.extend_from_slice(&base[..base.len().min(len)]);
            for byte in &mut buf {
                if (xorshift64(&mut seed) & 3) == 0 {
                    *byte = (xorshift64(&mut seed) & 0xFF) as u8;
                }
            }
        } else {
            for _ in 0..len {
                buf.push((xorshift64(&mut seed) & 0xFF) as u8);
            }
        }
        let _ = CommitParser::parse_bytes_with_warnings(&buf, "fuzz.json");
    }
}

#[test]
fn test_parquet_checkpoint_parser_never_panics_on_malformed_inputs() {
    let corpus: &[&[u8]] = &[
        b"",
        b"PAR1",
        b"PAR1PAR1",
        b"PAR1\x00\x00\x00\x00PAR1",
        b"PAR1\xff\xff\xff\x7fPAR1",
        b"not a parquet file at all",
    ];

    for input in corpus {
        assert!(parse_parquet_checkpoint(input, "bad.checkpoint.parquet").is_err());
    }

    let mut seed = 0x1234_5678_9ABC_DEF0u64;
    for _ in 0..300 {
        let len = (xorshift64(&mut seed) % 1024) as usize;
        let mut buf = Vec::with_capacity(len + 8);
        buf.extend_from_slice(b"PAR1");
        for _ in 0..len {
            buf.push((xorshift64(&mut seed) & 0xFF) as u8);
        }
        buf.extend_from_slice(b"PAR1");
        let _ = parse_parquet_checkpoint(&buf, "fuzz.checkpoint.parquet");
    }
}

#[test]
fn test_last_checkpoint_parser_never_panics_on_malformed_inputs() {
    let corpus: &[&[u8]] = &[
        b"",
        b"{}",
        b"{\"version\":-1}",
        b"{\"version\":18446744073709551616}",
        b"{\"version\":10,\"parts\":-5,\"tags\":null}",
        b"\x00\xff\xfe",
    ];

    for input in corpus {
        let _ = LastCheckpoint::parse_bytes(input);
    }

    let mut seed = 0xFEED_FACE_1234_5678u64;
    for _ in 0..300 {
        let len = (xorshift64(&mut seed) % 256) as usize;
        let buf: Vec<u8> = (0..len)
            .map(|_| (xorshift64(&mut seed) & 0xFF) as u8)
            .collect();
        let _ = LastCheckpoint::parse_bytes(&buf);
    }
}

#[test]
fn test_schema_json_parser_never_panics_on_malformed_or_deep_inputs() {
    let deep_struct = format!(
        "{}{{\"type\":\"struct\",\"fields\":[]}}{}",
        "{\"type\":\"struct\",\"fields\":[{\"name\":\"n\",\"type\":".repeat(128),
        ",\"nullable\":true,\"metadata\":{}}]}".repeat(128),
    );
    let corpus: &[&str] = &[
        "",
        "{}",
        "null",
        "[]",
        r#"{"type":"struct","fields":null}"#,
        r#"{"type":"struct","fields":[{"name":"x","type":{"type":"array"}}]}"#,
        r#"{"type":"struct","fields":[{"name":"x","type":{"type":"map","keyType":123}}]}"#,
        r#"{"type":"struct","fields":[{"name":"x","type":"decimal(999999999999,2)"}]}"#,
        &deep_struct,
    ];

    for input in corpus {
        let _ = StructType::from_schema_string(input);
    }
}
