//! Runnable example demonstrating the `deltalens` Rust library API.
//!
//! Creates a temporary local Delta Lake table with two commits (initial write +
//! schema evolution, Deletion Vector update, and soft-deleted tombstone), then
//! runs [`DeltaLogReader`], [`HealthAnalyzer`], and [`DiffAnalyzer`].
//!
//! Run with:
//! ```bash
//! cargo run --example inspect_table --locked
//! ```

use deltalens::{
    storage_for, DeltaLensError, DeltaLogReader, DiffAnalyzer, HealthAnalyzer, TableState,
};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_demo_table(root: &Path) -> Result<(), DeltaLensError> {
    let log_dir = root.join("_delta_log");
    fs::create_dir_all(&log_dir)?;

    let commit_0 = concat!(
        r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["deletionVectors"],"writerFeatures":["deletionVectors"]}}"#,
        "\n",
        r#"{"metaData":{"id":"demo-table-001","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"order_id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}},{\"name\":\"region\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{"delta.deletedFileRetentionDuration":"interval 7 days"},"createdTime":1760000000000}}"#,
        "\n",
        r#"{"commitInfo":{"timestamp":1760000000000,"operation":"WRITE","operationParameters":{"mode":"Overwrite"},"engineInfo":"Apache-Spark/3.5.1 Delta-Lake/3.2.0","operationMetrics":{"numFiles":"2","numOutputRows":"150000"}}}"#,
        "\n",
        r#"{"add":{"path":"region=us-east/part-00000.parquet","size":67108864,"modificationTime":1760000000000,"dataChange":true,"partitionValues":{"region":"us-east"}}}"#,
        "\n",
        r#"{"add":{"path":"region=eu-west/part-00001.parquet","size":1048576,"modificationTime":1760000000000,"dataChange":true,"partitionValues":{"region":"eu-west"}}}"#,
        "\n"
    );
    fs::write(log_dir.join("00000000000000000000.json"), commit_0)?;

    let commit_1 = concat!(
        r#"{"metaData":{"id":"demo-table-001","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"order_id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}},{\"name\":\"region\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}},{\"name\":\"currency\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{"delta.deletedFileRetentionDuration":"interval 7 days"},"createdTime":1760000000000}}"#,
        "\n",
        r#"{"commitInfo":{"timestamp":1760003600000,"operation":"MERGE","engineInfo":"Apache-Spark/3.5.1 Delta-Lake/3.2.0","operationMetrics":{"numTargetRowsDeleted":"42","numOutputRows":"25000"}}}"#,
        "\n",
        r#"{"remove":{"path":"region=us-east/part-00000.parquet","deletionTimestamp":1760003600000,"dataChange":true,"size":67108864,"partitionValues":{"region":"us-east"}}}"#,
        "\n",
        r#"{"add":{"path":"region=us-east/part-00000.parquet","size":67108864,"modificationTime":1760003600000,"dataChange":true,"partitionValues":{"region":"us-east"},"deletionVector":{"storageType":"u","pathOrInlineDv":"vBn[lx{q8@P<9BNH/isA","offset":1,"sizeInBytes":40,"cardinality":42}}}"#,
        "\n",
        r#"{"add":{"path":"region=ap-south/part-00002.parquet","size":33554432,"modificationTime":1760003600000,"dataChange":true,"partitionValues":{"region":"ap-south"}}}"#,
        "\n"
    );
    fs::write(log_dir.join("00000000000000000001.json"), commit_1)?;

    Ok(())
}

fn main() -> Result<(), DeltaLensError> {
    let tmp = TempDir::new()?;
    write_demo_table(tmp.path())?;

    let table_path = tmp.path().to_string_lossy();
    let (storage, root) = storage_for(&table_path)?;
    let reader = DeltaLogReader::new(storage, &root)?;

    // 1. Inspect table health at latest version
    let snapshot = reader.read_snapshot_log(None)?;
    let stats = HealthAnalyzer::analyze_snapshot(&snapshot);
    println!("=== DeltaLens Table Inspection ===");
    println!("Version:          v{}", stats.current_version);
    println!(
        "Protocol:         reader v{} / writer v{}",
        stats.min_reader_version, stats.min_writer_version
    );
    println!("Active files:     {}", stats.health.total_files);
    println!("Total size:       {} bytes", stats.health.total_size);
    println!("Skew score:       {:.2} / 1.0", stats.health.skew_score);
    println!(
        "Deletion vectors: {} files ({} deleted rows)",
        stats.health.deletion_vector_files, stats.health.deleted_rows_in_dvs
    );
    println!(
        "Tombstones:       {} ({} bytes reclaimable)",
        stats.tombstones.active_tombstones, stats.tombstones.reclaimable_bytes
    );
    println!("Health issues:    {}", stats.health_issues().len());

    // 2. Diff v0 -> v1
    let (snap_v0, after_commits, _warnings) = reader.read_diff_logs(0, 1)?;
    let diff =
        DiffAnalyzer::diff_from_state(0, TableState::from_snapshot(&snap_v0), &after_commits);
    println!(
        "\n=== DeltaLens Version Diff (v{} -> v{}) ===",
        diff.v1, diff.v2
    );
    println!(
        "Logical files:    +{} / -{}",
        diff.files_added, diff.files_removed
    );
    println!("New partitions:   {:?}", diff.new_partitions);
    println!("Schema changes:   {}", diff.schema_changes.len());
    println!("Breaking schema:  {}", diff.has_breaking_schema_changes);

    Ok(())
}
