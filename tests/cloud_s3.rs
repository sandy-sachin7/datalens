#![cfg(feature = "aws")]

use assert_cmd::Command;
use deltalens::{
    storage_for, DeltaLensError, DeltaLogReader, DiffAnalyzer, HealthAnalyzer, TableState,
};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStoreExt, PutPayload};

#[allow(deprecated)]
fn deltalens_cmd() -> Command {
    Command::cargo_bin("deltalens").unwrap()
}

fn s3_bucket() -> String {
    std::env::var("DELTALENS_TEST_S3_BUCKET").unwrap_or_else(|_| "deltalens-test".to_string())
}

#[test]
#[ignore = "requires live S3/MinIO endpoint (AWS_ENDPOINT, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_REGION, AWS_ALLOW_HTTP=true)"]
fn test_s3_minio_inspect_diff_lineage_and_offset_listing() {
    let bucket = s3_bucket();
    let prefix = format!(
        "ci-table-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    let store = AmazonS3Builder::from_env()
        .with_bucket_name(&bucket)
        .build()
        .expect("failed to build S3 client from env");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let put = |rel: &str, body: String| {
        let key = ObjectPath::from(format!("{prefix}/{rel}"));
        rt.block_on(async {
            store
                .put(&key, PutPayload::from(body.into_bytes()))
                .await
                .unwrap_or_else(|e| panic!("failed to upload {key}: {e}"));
        });
    };

    // Commits v0..=v10
    for v in 0..=10u64 {
        let ts = 1_700_000_000_000i64 + (v as i64) * 60_000;
        let mut lines = Vec::new();
        if v == 0 {
            lines.push(r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["v2Checkpoint","deletionVectors"],"writerFeatures":["v2Checkpoint","deletionVectors"]}}"#.to_string());
            lines.push(r#"{"metaData":{"id":"s3-ci","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{},"createdTime":1700000000000}}"#.to_string());
        }
        lines.push(format!(
            r#"{{"add":{{"path":"region=us/part-{v:04}.parquet","size":{size},"modificationTime":{ts},"dataChange":true,"partitionValues":{{"region":"us"}}}}}}"#,
            size = 10_000 + v * 1_000
        ));
        lines.push(format!(
            r#"{{"commitInfo":{{"timestamp":{ts},"operation":"WRITE"}}}}"#
        ));
        put(
            &format!("_delta_log/{v:020}.json"),
            format!("{}\n", lines.join("\n")),
        );
    }

    // V2 JSON checkpoint at v10 + _last_checkpoint to exercise S3 offset listing.
    let mut ckpt_lines = vec![
        r#"{"checkpointMetadata":{"version":2}}"#.to_string(),
        r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["v2Checkpoint","deletionVectors"],"writerFeatures":["v2Checkpoint","deletionVectors"]}}"#.to_string(),
        r#"{"metaData":{"id":"s3-ci","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{},"createdTime":1700000000000}}"#.to_string(),
    ];
    for v in 0..=10u64 {
        let ts = 1_700_000_000_000i64 + (v as i64) * 60_000;
        ckpt_lines.push(format!(
            r#"{{"add":{{"path":"region=us/part-{v:04}.parquet","size":{size},"modificationTime":{ts},"dataChange":true,"partitionValues":{{"region":"us"}}}}}}"#,
            size = 10_000 + v * 1_000
        ));
    }
    put(
        "_delta_log/00000000000000000010.checkpoint.00000000-0000-0000-0000-000000000010.json",
        format!("{}\n", ckpt_lines.join("\n")),
    );
    put(
        "_delta_log/_last_checkpoint",
        r#"{"version":10,"size":14}"#.to_string(),
    );

    // v11: widen amount integer -> double (safe) and add one file.
    put(
        "_delta_log/00000000000000000011.json",
        concat!(
            r#"{"metaData":{"id":"s3-ci","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{},"createdTime":1700000000000}}"#, "\n",
            r#"{"add":{"path":"region=eu/part-0011.parquet","size":25000,"modificationTime":1700000660000,"dataChange":true,"partitionValues":{"region":"eu"}}}"#, "\n",
            r#"{"commitInfo":{"timestamp":1700000660000,"operation":"WRITE"}}"#, "\n",
        )
        .to_string(),
    );

    // v12: attach a deletion vector to part-0011.
    put(
        "_delta_log/00000000000000000012.json",
        concat!(
            r#"{"remove":{"path":"region=eu/part-0011.parquet","deletionTimestamp":1700000720000,"dataChange":true,"size":25000}}"#, "\n",
            r#"{"add":{"path":"region=eu/part-0011.parquet","size":25000,"modificationTime":1700000720000,"dataChange":true,"partitionValues":{"region":"eu"},"deletionVector":{"storageType":"u","pathOrInlineDv":"dv-s3","offset":1,"sizeInBytes":24,"cardinality":13}}}"#, "\n",
            r#"{"commitInfo":{"timestamp":1700000720000,"operation":"DELETE"}}"#, "\n",
        )
        .to_string(),
    );

    let uri = format!("s3://{bucket}/{prefix}");
    let (storage, root) = storage_for(&uri).expect("storage_for s3 failed");
    let reader = DeltaLogReader::new(storage, &root).expect("DeltaLogReader::new on S3 failed");

    // Latest snapshot uses v10 checkpoint + offset listing.
    let snap = reader
        .read_snapshot_log(None)
        .expect("read_snapshot_log on S3 failed");
    assert_eq!(snap.target_version, 12);
    assert_eq!(snap.checkpoint_version, Some(10));
    let stats = HealthAnalyzer::analyze_snapshot(&snap);
    assert_eq!(stats.health.total_files, 12);
    assert_eq!(stats.health.deletion_vector_files, 1);
    assert_eq!(stats.health.deleted_rows_in_dvs, 13);
    assert_eq!(stats.partition_count, 2);

    // Pre-checkpoint time travel falls back to full listing.
    let snap_v5 = reader
        .read_snapshot_log(Some(5))
        .expect("time-travel v5 on S3 failed");
    let stats_v5 = HealthAnalyzer::analyze_snapshot(&snap_v5);
    assert_eq!(stats_v5.current_version, 5);
    assert_eq!(stats_v5.health.total_files, 6);

    // Diff v10 -> v12 sees safe integer -> double widening.
    let (snap_v10, after, _) = reader
        .read_diff_logs(10, 12)
        .expect("read_diff_logs on S3 failed");
    let diff = DiffAnalyzer::diff_from_state(10, TableState::from_snapshot(&snap_v10), &after);
    assert!(!diff.has_breaking_schema_changes);
    assert_eq!(diff.schema_changes.len(), 1);

    // CLI inspect & diff end-to-end against S3.
    let out = deltalens_cmd()
        .args(["inspect", &uri, "--json"])
        .assert()
        .success();
    let json: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["current_version"], 12);
    assert_eq!(json["health"]["total_files"], 12);
    assert_eq!(json["health"]["deleted_rows_in_dvs"], 13);

    deltalens_cmd()
        .args([
            "diff",
            &uri,
            "--v1",
            "10",
            "--v2",
            "12",
            "--fail-on-breaking",
        ])
        .assert()
        .code(0);

    // Missing table prefix returns NotADeltaTable.
    let missing_uri = format!("s3://{bucket}/nonexistent-{prefix}");
    let (missing_storage, missing_root) = storage_for(&missing_uri).unwrap();
    let missing_err = DeltaLogReader::new(missing_storage, &missing_root).err();
    assert!(matches!(
        missing_err,
        Some(DeltaLensError::NotADeltaTable(_))
    ));
}
