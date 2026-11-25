use assert_cmd::Command;
use deltalens::{DeltaLensError, DeltaLogReader, DiffAnalyzer, HealthAnalyzer, TableState};
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[allow(deprecated)]
fn deltalens_cmd() -> Command {
    Command::cargo_bin("deltalens").unwrap()
}

fn write_commit(log_dir: &Path, version: u64, lines: &[&str]) {
    let path = log_dir.join(format!("{version:020}.json"));
    fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
}

#[test]
fn test_unsupported_reader_version_fails_loudly() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    write_commit(
        &log_dir,
        0,
        &[
            r#"{"protocol":{"minReaderVersion":4,"minWriterVersion":7}}"#,
            r#"{"metaData":{"id":"future-v4","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"part-0.parquet","size":1024,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
        ],
    );

    deltalens_cmd()
        .args(["inspect", tmp.path().to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Unsupported Delta protocol"))
        .stderr(predicate::str::contains("minReaderVersion 4"));

    let (storage, root) = deltalens::storage_for(tmp.path().to_str().unwrap()).unwrap();
    let reader = DeltaLogReader::new(storage, &root).unwrap();
    let err = reader.read_snapshot_log(None).unwrap_err();
    assert!(
        matches!(err, DeltaLensError::UnsupportedProtocol(ref msg) if msg.contains("minReaderVersion 4")),
        "unexpected error: {err:?}"
    );
}

#[test]
fn test_unsupported_reader_feature_fails_loudly_with_feature_name() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    // v0 is a standard v1 reader table.
    write_commit(
        &log_dir,
        0,
        &[
            r#"{"protocol":{"minReaderVersion":1,"minWriterVersion":2}}"#,
            r#"{"metaData":{"id":"cat-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"part-0.parquet","size":1024,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
            r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE"}}"#,
        ],
    );

    // v1 upgrades to an unsupported reader feature (catalogManaged + a future feature).
    write_commit(
        &log_dir,
        1,
        &[
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["deletionVectors","catalogManaged","futureFeature"],"writerFeatures":["deletionVectors","catalogManaged","futureFeature"]}}"#,
            r#"{"commitInfo":{"timestamp":1700000001000,"operation":"SET TBLPROPERTIES"}}"#,
        ],
    );

    let table_path = tmp.path().to_str().unwrap();

    // Time-traveling to v0 still works because v0 only requires minReaderVersion 1.
    deltalens_cmd()
        .args(["inspect", table_path, "--version", "0", "--json"])
        .assert()
        .success();

    // Inspecting latest (v1) must fail loudly and name the unsupported features.
    deltalens_cmd()
        .args(["inspect", table_path])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("catalogManaged"))
        .stderr(predicate::str::contains("futureFeature"));

    // Diffing across the protocol upgrade must also fail loudly.
    deltalens_cmd()
        .args(["diff", table_path, "--v1", "0", "--v2", "1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("catalogManaged"));

    // Schema history and lineage must also reject the unsupported protocol.
    deltalens_cmd()
        .args(["schema", table_path, "--history"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("catalogManaged"));

    deltalens_cmd()
        .args(["lineage", table_path, "--last", "5"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("catalogManaged"));
}

#[test]
fn test_supported_reader_features_succeed() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    write_commit(
        &log_dir,
        0,
        &[
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["columnMapping","deletionVectors","timestampNtz","v2Checkpoint","vacuumProtocolCheck","typeWidening","variantType"],"writerFeatures":["columnMapping","deletionVectors","timestampNtz","v2Checkpoint","vacuumProtocolCheck","typeWidening","variantType"]}}"#,
            r#"{"metaData":{"id":"supported-v3","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"d\",\"type\":\"date\",\"nullable\":true,\"metadata\":{}},{\"name\":\"n\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}},{\"name\":\"v\",\"type\":\"variant\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"part-0.parquet","size":2048,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
        ],
    );

    let (storage, root) = deltalens::storage_for(tmp.path().to_str().unwrap()).unwrap();
    let reader = DeltaLogReader::new(storage, &root).unwrap();
    let snap = reader.read_snapshot_log(None).unwrap();
    let stats = HealthAnalyzer::analyze_snapshot(&snap);
    assert_eq!(stats.min_reader_version, 3);
    assert_eq!(stats.health.total_files, 1);
}

#[test]
fn test_golden_type_widening_diff_semantics() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    // v0: initial types covering integer, long, date, and decimal.
    write_commit(
        &log_dir,
        0,
        &[
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["typeWidening","timestampNtz"],"writerFeatures":["typeWidening","timestampNtz"]}}"#,
            r#"{"metaData":{"id":"widen-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"i_to_dbl\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}},{\"name\":\"i_to_dec\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}},{\"name\":\"l_to_dec\",\"type\":\"long\",\"nullable\":true,\"metadata\":{}},{\"name\":\"d_to_ntz\",\"type\":\"date\",\"nullable\":true,\"metadata\":{}},{\"name\":\"dec_scale\",\"type\":\"decimal(10,2)\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
        ],
    );

    // v1: all Delta-spec-safe widenings -> must NOT trigger breaking-change gate.
    write_commit(
        &log_dir,
        1,
        &[
            r#"{"metaData":{"id":"widen-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"i_to_dbl\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}},{\"name\":\"i_to_dec\",\"type\":\"decimal(12,2)\",\"nullable\":true,\"metadata\":{}},{\"name\":\"l_to_dec\",\"type\":\"decimal(19,0)\",\"nullable\":true,\"metadata\":{}},{\"name\":\"d_to_ntz\",\"type\":\"timestamp_ntz\",\"nullable\":true,\"metadata\":{}},{\"name\":\"dec_scale\",\"type\":\"decimal(12,4)\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
        ],
    );

    // v2: date -> timestamp (timezone-aware) and long -> double -> MUST trigger breaking gate.
    write_commit(
        &log_dir,
        2,
        &[
            r#"{"metaData":{"id":"widen-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"i_to_dbl\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}},{\"name\":\"i_to_dec\",\"type\":\"decimal(12,2)\",\"nullable\":true,\"metadata\":{}},{\"name\":\"l_to_dec\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}},{\"name\":\"d_to_ntz\",\"type\":\"timestamp\",\"nullable\":true,\"metadata\":{}},{\"name\":\"dec_scale\",\"type\":\"decimal(12,4)\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
        ],
    );

    let table_path = tmp.path().to_str().unwrap();

    // v0 -> v1 is completely safe (exit code 0 with --fail-on-breaking).
    deltalens_cmd()
        .args([
            "diff",
            table_path,
            "--v1",
            "0",
            "--v2",
            "1",
            "--fail-on-breaking",
        ])
        .assert()
        .code(0);

    // v1 -> v2 is breaking (exit code 2 with --fail-on-breaking).
    deltalens_cmd()
        .args([
            "diff",
            table_path,
            "--v1",
            "1",
            "--v2",
            "2",
            "--fail-on-breaking",
        ])
        .assert()
        .code(2);

    // Also verify date -> timestamp directly from a fresh date column is breaking.
    let (storage, root) = deltalens::storage_for(table_path).unwrap();
    let reader = DeltaLogReader::new(storage, &root).unwrap();
    let (snap_v0, after, _) = reader.read_diff_logs(0, 2).unwrap();
    let diff = DiffAnalyzer::diff_from_state(0, TableState::from_snapshot(&snap_v0), &after);
    assert!(diff.has_breaking_schema_changes);
}

#[test]
fn test_golden_v2_json_checkpoint_with_sidecars_and_dv_replay() {
    use arrow_array::{ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType as ArrowDataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;

    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    let sidecars_dir = log_dir.join("_sidecars");
    fs::create_dir_all(&sidecars_dir).unwrap();

    // Per Delta V2 checkpoint spec, sidecars are Parquet files under `_delta_log/_sidecars/`.
    let add_fields = vec![
        Field::new("path", ArrowDataType::Utf8, false),
        Field::new("size", ArrowDataType::Int64, false),
        Field::new("modificationTime", ArrowDataType::Int64, false),
        Field::new("dataChange", ArrowDataType::Boolean, false),
    ];
    let add_struct = StructArray::from(vec![
        (
            Arc::new(add_fields[0].clone()),
            Arc::new(StringArray::from(vec!["part-a.parquet"])) as ArrayRef,
        ),
        (
            Arc::new(add_fields[1].clone()),
            Arc::new(Int64Array::from(vec![10_000i64])) as ArrayRef,
        ),
        (
            Arc::new(add_fields[2].clone()),
            Arc::new(Int64Array::from(vec![1_700_000_000_000i64])) as ArrayRef,
        ),
        (
            Arc::new(add_fields[3].clone()),
            Arc::new(BooleanArray::from(vec![true])) as ArrayRef,
        ),
    ]);
    let schema = Arc::new(Schema::new(vec![Field::new(
        "add",
        ArrowDataType::Struct(add_fields.into()),
        true,
    )]));
    let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(add_struct)]).unwrap();
    let sidecar_name = "00000000000000000005.sidecar-a.parquet";
    let mut sidecar_file = fs::File::create(sidecars_dir.join(sidecar_name)).unwrap();
    let mut writer = ArrowWriter::try_new(&mut sidecar_file, schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    // V2 JSON checkpoint at v5 with Parquet sidecar + inline DV add action.
    let ckpt_name = "00000000000000000005.checkpoint.00000000-0000-0000-0000-000000000001.json";
    fs::write(
        log_dir.join(ckpt_name),
        concat!(
            r#"{"checkpointMetadata":{"version":2}}"#,
            "\n",
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["v2Checkpoint","deletionVectors"],"writerFeatures":["v2Checkpoint","deletionVectors"]}}"#,
            "\n",
            r#"{"metaData":{"id":"v2-golden","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"region\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{},"createdTime":1700000000000}}"#,
            "\n",
            r#"{"sidecar":{"path":"00000000000000000005.sidecar-a.parquet","sizeInBytes":200,"modificationTime":1700000000000}}"#,
            "\n",
            r#"{"add":{"path":"part-b.parquet","size":20000,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{"region":"eu"},"deletionVector":{"storageType":"u","pathOrInlineDv":"dv1","offset":1,"sizeInBytes":32,"cardinality":42}}}"#,
            "\n",
        ),
    )
    .unwrap();
    fs::write(
        log_dir.join("_last_checkpoint"),
        r#"{"version":5,"size":5}"#,
    )
    .unwrap();

    // Commit v6 replaces part-b's DV with a larger cardinality DV and adds part-c.
    write_commit(
        &log_dir,
        6,
        &[
            r#"{"remove":{"path":"part-b.parquet","deletionTimestamp":1700000006000,"dataChange":true,"size":20000,"deletionVector":{"storageType":"u","pathOrInlineDv":"dv1","offset":1,"sizeInBytes":32,"cardinality":42}}}"#,
            r#"{"add":{"path":"part-b.parquet","size":20000,"modificationTime":1700000006000,"dataChange":true,"partitionValues":{"region":"eu"},"deletionVector":{"storageType":"u","pathOrInlineDv":"dv2","offset":1,"sizeInBytes":48,"cardinality":99}}}"#,
            r#"{"add":{"path":"part-c.parquet","size":30000,"modificationTime":1700000006000,"dataChange":true,"partitionValues":{"region":"ap"}}}"#,
            r#"{"commitInfo":{"timestamp":1700000006000,"operation":"DELETE"}}"#,
        ],
    );

    let (storage, root) = deltalens::storage_for(tmp.path().to_str().unwrap()).unwrap();
    let reader = DeltaLogReader::new(storage, &root).unwrap();
    let snap = reader.read_snapshot_log(None).unwrap();
    let stats = HealthAnalyzer::analyze_snapshot(&snap);

    assert_eq!(stats.current_version, 6);
    assert_eq!(stats.maintenance.last_checkpoint_version, Some(5));
    assert_eq!(stats.health.total_files, 3);
    assert_eq!(stats.health.total_size, 60_000);
    assert_eq!(stats.health.deletion_vector_files, 1);
    assert_eq!(stats.health.deleted_rows_in_dvs, 99);
    assert_eq!(stats.partition_count, 2);
}
