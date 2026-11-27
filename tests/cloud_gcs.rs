#![cfg(feature = "gcp")]

use assert_cmd::Command;
use deltalens::{storage_for, DeltaLogReader, DiffAnalyzer, HealthAnalyzer, TableState};
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStoreExt, PutPayload};

#[allow(deprecated)]
fn deltalens_cmd() -> Command {
    Command::cargo_bin("deltalens").unwrap()
}

fn gcs_bucket() -> String {
    std::env::var("DELTALENS_TEST_GCS_BUCKET").unwrap_or_else(|_| "deltalenstests".to_string())
}

#[test]
#[ignore = "requires live GCS / fake-gcs-server endpoint (GOOGLE_SERVICE_ACCOUNT_KEY with gcs_base_url & disable_oauth)"]
fn test_gcs_inspect_diff_lineage_and_offset_listing() {
    let bucket = gcs_bucket();
    let prefix = format!(
        "ci-gcs-table-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    let store = GoogleCloudStorageBuilder::from_env()
        .with_bucket_name(&bucket)
        .build()
        .expect("failed to build GCS client from env");

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

    for v in 0..=10u64 {
        let ts = 1_700_000_000_000i64 + (v as i64) * 60_000;
        let mut lines = Vec::new();
        if v == 0 {
            lines.push(r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["v2Checkpoint","deletionVectors"],"writerFeatures":["v2Checkpoint","deletionVectors"]}}"#.to_string());
            lines.push(r#"{"metaData":{"id":"gcs-ci","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{"delta.enableDeletionVectors":"true"},"createdTime":1700000000000}}"#.to_string());
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

    let mut ckpt_lines = vec![
        r#"{"checkpointMetadata":{"version":2}}"#.to_string(),
        r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["v2Checkpoint","deletionVectors"],"writerFeatures":["v2Checkpoint","deletionVectors"]}}"#.to_string(),
        r#"{"metaData":{"id":"gcs-ci","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"integer\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{"delta.enableDeletionVectors":"true"},"createdTime":1700000000000}}"#.to_string(),
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

    put(
        "_delta_log/00000000000000000011.json",
        concat!(
            r#"{"metaData":{"id":"gcs-ci","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"amount\",\"type\":\"double\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":["region"],"configuration":{"delta.enableDeletionVectors":"true"},"createdTime":1700000000000}}"#, "\n",
            r#"{"add":{"path":"region=eu/part-0011.parquet","size":25000,"modificationTime":1700000660000,"dataChange":true,"partitionValues":{"region":"eu"}}}"#, "\n",
            r#"{"commitInfo":{"timestamp":1700000660000,"operation":"WRITE"}}"#, "\n",
        )
        .to_string(),
    );

    put(
        "_delta_log/00000000000000000012.json",
        concat!(
            r#"{"remove":{"path":"region=eu/part-0011.parquet","deletionTimestamp":1700000720000,"dataChange":true,"size":25000}}"#, "\n",
            r#"{"add":{"path":"region=eu/part-0011.parquet","size":25000,"modificationTime":1700000720000,"dataChange":true,"partitionValues":{"region":"eu"},"deletionVector":{"storageType":"u","pathOrInlineDv":"dv-gcs","offset":1,"sizeInBytes":24,"cardinality":13}}}"#, "\n",
            r#"{"commitInfo":{"timestamp":1700000720000,"operation":"DELETE"}}"#, "\n",
        )
        .to_string(),
    );

    let uri = format!("gs://{bucket}/{prefix}");
    let (storage, root) = storage_for(&uri).expect("storage_for gs failed");
    let reader = DeltaLogReader::new(storage, &root).expect("DeltaLogReader::new on GCS failed");

    let snap = reader
        .read_snapshot_log(None)
        .expect("read_snapshot_log on GCS failed");
    assert_eq!(snap.target_version, 12);
    assert_eq!(snap.checkpoint_version, Some(10));
    let stats = HealthAnalyzer::analyze_snapshot(&snap);
    assert_eq!(stats.health.total_files, 12);
    assert_eq!(stats.health.deletion_vector_files, 1);
    assert_eq!(stats.health.deleted_rows_in_dvs, 13);
    assert_eq!(
        stats
            .table_properties
            .get("delta.enableDeletionVectors")
            .map(String::as_str),
        Some("true")
    );

    let snap_v5 = reader
        .read_snapshot_log(Some(5))
        .expect("time-travel v5 on GCS failed");
    assert_eq!(
        HealthAnalyzer::analyze_snapshot(&snap_v5)
            .health
            .total_files,
        6
    );

    let (snap_v10, after, _) = reader
        .read_diff_logs(10, 12)
        .expect("read_diff_logs on GCS failed");
    let diff = DiffAnalyzer::diff_from_state(10, TableState::from_snapshot(&snap_v10), &after);
    assert!(!diff.has_breaking_schema_changes);
    assert_eq!(diff.schema_changes.len(), 1);

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
        .success();
}
