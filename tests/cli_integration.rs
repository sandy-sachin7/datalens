use arrow_array::builder::{
    BooleanBuilder, Int64Builder, ListBuilder, StringBuilder, StructBuilder,
};
use arrow_array::RecordBatch;
use arrow_schema::{DataType as ArrowDataType, Field, Fields, Schema};
use assert_cmd::Command;
use parquet::arrow::ArrowWriter;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

fn deltalens_cmd() -> Command {
    Command::cargo_bin("deltalens").unwrap()
}

fn write_commit(log_dir: &Path, version: u64, lines: &[&str]) {
    let filename = format!("{:020}.json", version);
    let content = lines.join("\n") + "\n";
    fs::write(log_dir.join(filename), content).unwrap();
}

fn build_sample_table() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    // v0: CREATE TABLE with 2 partitions, 2 files
    write_commit(
        &log_dir,
        0,
        &[
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["deletionVectors"],"writerFeatures":["deletionVectors"]}}"#,
            r#"{"metaData":{"id":"tbl-test","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"integer\",\"nullable\":false,\"metadata\":{}},{\"name\":\"name\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["dt"],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"dt=2026-01-01/f1.parquet","size":50000000,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{"dt":"2026-01-01"},"stats":"{\"numRecords\":100}"}}"#,
            r#"{"add":{"path":"dt=2026-01-02/f2.parquet","size":50000000,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{"dt":"2026-01-02"},"stats":"{\"numRecords\":200}"}}"#,
            r#"{"commitInfo":{"timestamp":1700000000000,"operation":"CREATE TABLE","operationParameters":{},"engineInfo":"Apache-Spark/3.5.0","userName":"alice"}}"#,
        ],
    );

    // v1: WRITE adding a new column (non-breaking) + new file
    write_commit(
        &log_dir,
        1,
        &[
            r#"{"metaData":{"id":"tbl-test","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"name\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}},{\"name\":\"email\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["dt"],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"dt=2026-01-03/f3.parquet","size":50000000,"modificationTime":1700086400000,"dataChange":true,"partitionValues":{"dt":"2026-01-03"},"stats":"{\"numRecords\":50}"}}"#,
            r#"{"commitInfo":{"timestamp":1700086400000,"operation":"WRITE","operationParameters":{},"operationMetrics":{"num_added_rows":50},"job":{"jobId":"77","jobName":"ingest_job"}}}"#,
        ],
    );

    // v2: DELETE removing 1 file + dropping `email` column (BREAKING) on 2026-01-02 23:59:59.500 UTC
    // 2026-01-02T23:59:59.500Z = 1767398399500 ms
    write_commit(
        &log_dir,
        2,
        &[
            r#"{"metaData":{"id":"tbl-test","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"name\",\"type\":\"string\",\"nullable\":false,\"metadata\":{}}]}","partitionColumns":["dt"],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"remove":{"path":"dt=2026-01-02/f2.parquet","deletionTimestamp":1767398399500,"dataChange":true}}"#,
            r#"{"commitInfo":{"timestamp":1767398399500,"operation":"DELETE","operationParameters":{"predicate":["dt = '2026-01-02'"]},"operationMetrics":{"numDeletedRows":"200"},"userName":"bob"}}"#,
        ],
    );

    // v3: Another WRITE
    write_commit(
        &log_dir,
        3,
        &[
            r#"{"add":{"path":"dt=2026-01-04/f4.parquet","size":1024,"modificationTime":1767484800000,"dataChange":true,"partitionValues":{"dt":"2026-01-04"}}}"#,
            r#"{"commitInfo":{"timestamp":1767484800000,"operation":"WRITE","operationParameters":{},"userName":"alice"}}"#,
        ],
    );

    tmp
}

#[test]
fn test_inspect_json_plain_no_header_verbose_and_version_validation() {
    let tmp = build_sample_table();
    let path = tmp.path().to_str().unwrap();

    // JSON output
    let assert = deltalens_cmd()
        .args(["inspect", path, "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["current_version"], 3);
    assert_eq!(val["health"]["total_files"], 3);
    assert_eq!(val["partition_count"], 3);
    assert_eq!(val["empty_partition_count"], 1);
    assert_eq!(val["tombstones"]["active_tombstones"], 1);
    assert_eq!(val["tombstones"]["reclaimable_bytes"], 50_000_000);
    assert_eq!(
        val["maintenance"]["last_checkpoint_version"],
        serde_json::Value::Null
    );

    // Plain mode must emit ZERO ANSI escape codes
    let plain_out = deltalens_cmd()
        .args(["inspect", path, "--plain"])
        .assert()
        .success();
    let plain_str = String::from_utf8(plain_out.get_output().stdout.clone()).unwrap();
    assert!(!plain_str.contains('\x1b'));
    assert!(plain_str.contains("deltalens · Table Health Report"));

    // --no-header suppresses header
    let no_hdr = deltalens_cmd()
        .args(["inspect", path, "--plain", "--no-header"])
        .assert()
        .success();
    let no_hdr_str = String::from_utf8(no_hdr.get_output().stdout.clone()).unwrap();
    assert!(!no_hdr_str.contains("deltalens · Table Health Report"));

    // -v/--verbose logs diagnostics to stderr
    deltalens_cmd()
        .args(["inspect", path, "--plain", "-v"])
        .assert()
        .success()
        .stderr(predicate::str::contains("[verbose] inspect"));

    // Out-of-range --version fails with exit code 1
    deltalens_cmd()
        .args(["inspect", path, "--version", "999"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Version 999 not found"));
}

#[test]
fn test_fail_on_issues_and_fail_on_breaking_exit_code_2() {
    let tmp = build_sample_table();
    let path = tmp.path().to_str().unwrap();

    // v3 has 1 small file out of 3 (33.3% > 20%) -> triggers health issue -> exit code 2!
    deltalens_cmd()
        .args(["inspect", path, "--plain", "--fail-on-issues"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("issue detected"));

    // v0 has 2 large 50MB files (0% small, 0.0 skew) -> passes health gate with exit code 0!
    deltalens_cmd()
        .args([
            "inspect",
            path,
            "--version",
            "0",
            "--plain",
            "--fail-on-issues",
        ])
        .assert()
        .success();

    // diff v0 -> v1 only widens id (int->long) and adds email -> NOT breaking -> exit 0!
    deltalens_cmd()
        .args(["diff", path, "--v1", "0", "--v2", "1", "--fail-on-breaking"])
        .assert()
        .success();

    // diff v1 -> v2 drops email & tightens name nullable true->false -> BREAKING -> exit 2!
    let diff_out = deltalens_cmd()
        .args([
            "diff",
            path,
            "--v1",
            "1",
            "--v2",
            "2",
            "--json",
            "--fail-on-breaking",
        ])
        .assert()
        .code(2);
    let diff_json: serde_json::Value =
        serde_json::from_slice(&diff_out.get_output().stdout).unwrap();
    assert_eq!(diff_json["has_breaking_schema_changes"], true);
    assert_eq!(
        diff_json["removed_partitions"],
        serde_json::json!(["dt=2026-01-02"])
    );
    assert_eq!(diff_json["bytes_removed"], 50_000_000);
}

#[test]
fn test_lineage_filters_before_last_and_audit_until_end_of_day() {
    let tmp = build_sample_table();
    let path = tmp.path().to_str().unwrap();

    // Commit v2 is DELETE, v3 is WRITE. With `--op DELETE --last 1`, filtering MUST run
    // before `--last 1`, otherwise v3 (the last 1 commit) would hide v2!
    let lin = deltalens_cmd()
        .args(["lineage", path, "--op", "DELETE", "--last", "1", "--json"])
        .assert()
        .success();
    let entries: serde_json::Value = serde_json::from_slice(&lin.get_output().stdout).unwrap();
    let arr = entries.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["version"], 2);
    assert_eq!(arr[0]["num_output_rows"], -200);
    assert_eq!(arr[0]["predicate"], "dt = '2026-01-02'");

    // Audit `--until 2026-01-02` must include commit v2 at 2026-01-02T23:59:59.500Z!
    let aud = deltalens_cmd()
        .args([
            "audit",
            path,
            "--since",
            "2026-01-02",
            "--until",
            "2026-01-02",
            "--json",
        ])
        .assert()
        .success();
    let aud_entries: serde_json::Value = serde_json::from_slice(&aud.get_output().stdout).unwrap();
    assert_eq!(aud_entries.as_array().unwrap().len(), 1);
    assert_eq!(aud_entries[0]["version"], 2);
}

#[test]
fn test_schema_history_and_conflicts() {
    let tmp = build_sample_table();
    let path = tmp.path().to_str().unwrap();

    // Conflicting flags rejected by clap
    deltalens_cmd()
        .args([
            "diff",
            path,
            "--v1",
            "0",
            "--v2",
            "1",
            "--schema-only",
            "--files-only",
        ])
        .assert()
        .failure();
    deltalens_cmd()
        .args(["schema", path, "--history", "--at", "1"])
        .assert()
        .failure();

    // Schema history JSON
    let hist = deltalens_cmd()
        .args(["schema", path, "--history", "--json"])
        .assert()
        .success();
    let hist_json: serde_json::Value = serde_json::from_slice(&hist.get_output().stdout).unwrap();
    assert_eq!(hist_json.as_array().unwrap().len(), 2);
    assert_eq!(hist_json[0]["has_breaking_changes"], false);
    assert_eq!(hist_json[1]["has_breaking_changes"], true);
}

#[test]
fn test_config_set_rejects_invalid_and_records_real_telemetry() {
    let cfg_dir = TempDir::new().unwrap();
    let data_dir = TempDir::new().unwrap();
    let table_dir = build_sample_table();

    // Invalid key exits 1 and does NOT write config file
    deltalens_cmd()
        .env("DELTALENS_CONFIG_DIR", cfg_dir.path())
        .args(["config", "set", "bogus_key", "true"])
        .assert()
        .code(1);
    assert!(!cfg_dir.path().join("config.json").exists());

    // Invalid value exits 1 and does NOT write config file
    deltalens_cmd()
        .env("DELTALENS_CONFIG_DIR", cfg_dir.path())
        .args(["config", "set", "telemetry", "maybe"])
        .assert()
        .code(1);
    assert!(!cfg_dir.path().join("config.json").exists());

    // Valid set + --json
    deltalens_cmd()
        .env("DELTALENS_CONFIG_DIR", cfg_dir.path())
        .args(["config", "set", "telemetry", "true", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"telemetry\""));

    // Run inspect with telemetry enabled -> records REAL non-zero commit/action counts
    deltalens_cmd()
        .env("DELTALENS_CONFIG_DIR", cfg_dir.path())
        .env("DELTALENS_DATA_DIR", data_dir.path())
        .args(["inspect", table_dir.path().to_str().unwrap(), "--json"])
        .assert()
        .success();

    let metrics_raw = fs::read_to_string(data_dir.path().join("metrics.jsonl")).unwrap();
    let metric: serde_json::Value = serde_json::from_str(metrics_raw.trim()).unwrap();
    assert_eq!(metric["command"], "inspect");
    assert_eq!(metric["num_commits"], 4);
    assert!(metric["num_actions"].as_u64().unwrap() > 0);
    assert_eq!(metric["table_version"], "v3");

    // config metrics --json
    let m_out = deltalens_cmd()
        .env("DELTALENS_CONFIG_DIR", cfg_dir.path())
        .env("DELTALENS_DATA_DIR", data_dir.path())
        .args(["config", "metrics", "--json"])
        .assert()
        .success();
    let m_json: serde_json::Value = serde_json::from_slice(&m_out.get_output().stdout).unwrap();
    assert_eq!(m_json["total_commands"], 1);
}

#[test]
fn test_parquet_checkpoint_table_cli_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    // Build a real Parquet checkpoint at v10 (commits 0..=9 are expired/absent!)
    let meta_fields = Fields::from(vec![
        Field::new("id", ArrowDataType::Utf8, false),
        Field::new("schemaString", ArrowDataType::Utf8, false),
        Field::new(
            "partitionColumns",
            ArrowDataType::List(Arc::new(Field::new("item", ArrowDataType::Utf8, true))),
            true,
        ),
    ]);
    let add_fields = Fields::from(vec![
        Field::new("path", ArrowDataType::Utf8, false),
        Field::new("size", ArrowDataType::Int64, false),
        Field::new("modificationTime", ArrowDataType::Int64, false),
        Field::new("dataChange", ArrowDataType::Boolean, false),
    ]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("metaData", ArrowDataType::Struct(meta_fields.clone()), true),
        Field::new("add", ArrowDataType::Struct(add_fields.clone()), true),
    ]));

    let mut mb = StructBuilder::new(
        meta_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(StringBuilder::new()),
            Box::new(ListBuilder::new(StringBuilder::new())),
        ],
    );
    mb.field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("ckpt-tbl");
    mb.field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value(r#"{"type":"struct","fields":[{"name":"k","type":"string","nullable":false,"metadata":{}}]}"#);
    mb.field_builder::<ListBuilder<StringBuilder>>(2)
        .unwrap()
        .append(true);
    mb.append(true);

    let mut ab = StructBuilder::new(
        add_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(Int64Builder::new()),
            Box::new(Int64Builder::new()),
            Box::new(BooleanBuilder::new()),
        ],
    );
    ab.field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("part-ckpt.parquet");
    ab.field_builder::<Int64Builder>(1)
        .unwrap()
        .append_value(40_000_000);
    ab.field_builder::<Int64Builder>(2)
        .unwrap()
        .append_value(1700000000000);
    ab.field_builder::<BooleanBuilder>(3)
        .unwrap()
        .append_value(false);
    ab.append(true);

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(mb.finish()), Arc::new(ab.finish())],
    )
    .unwrap();
    let ckpt_file =
        fs::File::create(log_dir.join("00000000000000000010.checkpoint.parquet")).unwrap();
    let mut writer = ArrowWriter::try_new(ckpt_file, schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    // Commit v11 adds a second file
    write_commit(
        &log_dir,
        11,
        &[
            r#"{"add":{"path":"part-v11.parquet","size":60000000,"modificationTime":1700000005000,"dataChange":true,"partitionValues":{}}}"#,
            r#"{"commitInfo":{"timestamp":1700000005000,"operation":"WRITE"}}"#,
        ],
    );

    let path = tmp.path().to_str().unwrap();

    // `inspect` finds BOTH checkpoint file and v11 file + real checkpoint version 10!
    let ins = deltalens_cmd()
        .args(["inspect", path, "--json"])
        .assert()
        .success();
    let ins_json: serde_json::Value = serde_json::from_slice(&ins.get_output().stdout).unwrap();
    assert_eq!(ins_json["current_version"], 11);
    assert_eq!(ins_json["health"]["total_files"], 2);
    assert_eq!(ins_json["health"]["total_size"], 100_000_000);
    assert_eq!(ins_json["schema_column_count"], 1);
    assert_eq!(ins_json["maintenance"]["last_checkpoint_version"], 10);

    // `schema` recovers schema from checkpoint v10 even though v0..=v9 are expired!
    let sch = deltalens_cmd()
        .args(["schema", path, "--json"])
        .assert()
        .success();
    let sch_json: serde_json::Value = serde_json::from_slice(&sch.get_output().stdout).unwrap();
    assert_eq!(sch_json["version"], 10);
    assert_eq!(sch_json["schema"]["fields"][0]["name"], "k");

    // Commit v12 evolves the schema by adding `extra` — `schema --history` must seed from
    // checkpoint v10 (since commits 0..=9 were pruned) and detect the v12 addition!
    write_commit(
        &log_dir,
        12,
        &[
            r#"{"metaData":{"id":"ckpt-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"k\",\"type\":\"string\",\"nullable\":false,\"metadata\":{}},{\"name\":\"extra\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"commitInfo":{"timestamp":1700000010000,"operation":"WRITE"}}"#,
        ],
    );
    let hist = deltalens_cmd()
        .args(["schema", path, "--history", "--json"])
        .assert()
        .success();
    let hist_json: serde_json::Value = serde_json::from_slice(&hist.get_output().stdout).unwrap();
    let hist_arr = hist_json.as_array().unwrap();
    assert_eq!(hist_arr.len(), 1);
    assert_eq!(hist_arr[0]["version"], 12);
    assert_eq!(hist_arr[0]["added_columns"], serde_json::json!(["extra"]));
}

#[test]
fn test_cli_version_matches_pkg_and_checkpoint_horizon_passes_fail_on_issues() {
    deltalens_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));

    // High-version table (>100) with checkpoint at v150, commit v151, no post-checkpoint VACUUM,
    // and no expired tombstones must pass `inspect --fail-on-issues` (exit 0) and display
    // "Not observed since checkpoint v150" instead of "Never".
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    let ckpt_v150 = concat!(
        r#"{"checkpointMetadata":{"version":2}}"#,
        "\n",
        r#"{"metaData":{"id":"hi-ver","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
        "\n",
        r#"{"add":{"path":"big1.parquet","size":67108864,"modificationTime":1750000000000,"dataChange":false,"partitionValues":{}}}"#,
        "\n",
        r#"{"add":{"path":"big2.parquet","size":67108864,"modificationTime":1750000000000,"dataChange":false,"partitionValues":{}}}"#,
        "\n"
    );
    fs::write(
        log_dir.join("00000000000000000150.checkpoint.uuid-150.json"),
        ckpt_v150,
    )
    .unwrap();
    write_commit(
        &log_dir,
        151,
        &[r#"{"commitInfo":{"timestamp":1750000005000,"operation":"WRITE"}}"#],
    );

    let path = tmp.path().to_str().unwrap();
    let out = deltalens_cmd()
        .args(["inspect", path, "--plain", "--fail-on-issues"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Not observed since checkpoint v150"));
    assert!(!stdout.contains("Never"));
}

#[test]
fn test_error_handling_empty_table_and_unsupported_scheme() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join("_delta_log")).unwrap();
    let path = tmp.path().to_str().unwrap();

    for cmd in [
        vec!["inspect", path],
        vec!["schema", path],
        vec!["lineage", path],
        vec!["audit", path],
        vec!["diff", path, "--v1", "0", "--v2", "0"],
    ] {
        deltalens_cmd()
            .args(&cmd)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("Empty table"));
    }

    deltalens_cmd()
        .args(["inspect", "hdfs://namenode/warehouse/sales"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Unsupported URI scheme"));
}

#[test]
fn test_cli_deletion_vectors_column_mapping_file_uri_and_version_gap() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("_delta_log");
    fs::create_dir(&log_dir).unwrap();

    // v0: add file + column mapping schema
    write_commit(
        &log_dir,
        0,
        &[
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["deletionVectors","columnMapping"],"writerFeatures":["deletionVectors","columnMapping"]}}"#,
            r#"{"metaData":{"id":"dv-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"col_a\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{\"delta.columnMapping.id\":1}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"part-0.parquet","size":60000000,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
            r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE"}}"#,
        ],
    );

    // v1: same-commit Add (with DV) BEFORE Remove (without DV) + rename col_a -> col_b
    write_commit(
        &log_dir,
        1,
        &[
            r#"{"metaData":{"id":"dv-tbl","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"col_b\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{\"delta.columnMapping.id\":1}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            r#"{"add":{"path":"part-0.parquet","size":60000000,"modificationTime":1700000001000,"dataChange":true,"partitionValues":{},"deletionVector":{"storageType":"u","pathOrInlineDv":"abc","offset":1,"sizeInBytes":20,"cardinality":7}}}"#,
            r#"{"remove":{"path":"part-0.parquet","deletionTimestamp":1700000001000,"dataChange":true}}"#,
            r#"{"commitInfo":{"timestamp":1700000001000,"operation":"DELETE"}}"#,
        ],
    );

    let file_uri = format!("file://{}", tmp.path().to_str().unwrap());

    // Inspect via file:// URI verifies DV active file is preserved
    let ins = deltalens_cmd()
        .args(["inspect", &file_uri, "--json"])
        .assert()
        .success();
    let ins_json: serde_json::Value = serde_json::from_slice(&ins.get_output().stdout).unwrap();
    assert_eq!(ins_json["health"]["total_files"], 1);
    assert_eq!(ins_json["health"]["deletion_vector_files"], 1);
    assert_eq!(ins_json["health"]["deleted_rows_in_dvs"], 7);

    // Diff v0 -> v1 reports rename col_a -> col_b rather than drop + add
    let diff = deltalens_cmd()
        .args(["diff", &file_uri, "--v1", "0", "--v2", "1", "--json"])
        .assert()
        .success();
    let diff_json: serde_json::Value = serde_json::from_slice(&diff.get_output().stdout).unwrap();
    assert_eq!(
        diff_json["schema_changes"][0]["added_columns"],
        serde_json::json!([])
    );
    assert_eq!(
        diff_json["schema_changes"][0]["removed_columns"],
        serde_json::json!([])
    );
    assert_eq!(
        diff_json["schema_changes"][0]["renamed_columns"][0]["old_name"],
        "col_a"
    );
    assert_eq!(
        diff_json["schema_changes"][0]["renamed_columns"][0]["new_name"],
        "col_b"
    );

    // Same-version diff v0..v0 succeeds cleanly
    deltalens_cmd()
        .args(["diff", &file_uri, "--v1", "0", "--v2", "0", "--plain"])
        .assert()
        .success();

    // Introduce a version gap (v3 without v2) -> fails with exit 1 and Version gap error
    write_commit(
        &log_dir,
        3,
        &[r#"{"commitInfo":{"timestamp":1700000003000,"operation":"WRITE"}}"#],
    );
    deltalens_cmd()
        .args(["inspect", &file_uri])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Version gap detected"));
}
