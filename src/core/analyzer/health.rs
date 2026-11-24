use crate::core::analyzer::schema::SchemaTracker;
use crate::core::model::action::Action;
use crate::core::model::schema::StructType;
use crate::core::model::stats::{MaintenanceInfo, TableHealth, TableStats};
use crate::core::reader::{CommitEntry, SnapshotLog};
use crate::core::replay::TableState;

pub struct HealthAnalyzer;

impl HealthAnalyzer {
    /// Full table inspection from a checkpoint-aware `SnapshotLog`.
    pub fn analyze_snapshot(snapshot: &SnapshotLog) -> TableStats {
        let state = TableState::from_snapshot(snapshot);
        let health = Self::compute_health(&state);
        let created_timestamp = Self::find_created_ts(&state, &snapshot.commits);
        let last_modified_timestamp = Self::find_last_modified_ts(&state, &snapshot.commits);
        let retention_ms = state.deleted_file_retention_ms();
        let newest_tombstone_ts = state
            .active_tombstones
            .values()
            .filter_map(|r| r.deletion_timestamp)
            .filter(|&t| t > 0)
            .max();
        let reference_ts = last_modified_timestamp
            .map(|ts| ts.max(newest_tombstone_ts.unwrap_or(ts)))
            .or_else(|| Some(chrono::Utc::now().timestamp_millis()));
        let tombstones = state.tombstone_info_with_retention(retention_ms, reference_ts);
        let mut maintenance = Self::compute_maintenance(&snapshot.commits);
        maintenance.last_checkpoint_version = snapshot.checkpoint_version;
        maintenance.clustering_columns = state.clustering_columns();

        let partition_columns = state
            .metadata
            .as_ref()
            .map(|m| m.partition_columns.clone())
            .unwrap_or_default();
        let partition_count = state.active_partitions().len();
        let empty_partition_count = state.empty_partition_count();

        let schema_column_count = state
            .metadata
            .as_ref()
            .and_then(|m| StructType::from_schema_string(&m.schema_string).ok())
            .map(|s| s.fields.len())
            .unwrap_or(0);
        let schema_change_count = SchemaTracker::history_from_snapshot(snapshot).len();

        let (min_reader_version, min_writer_version, reader_features, writer_features) =
            match &state.protocol {
                Some(p) => (
                    p.min_reader_version,
                    p.min_writer_version,
                    p.reader_features.clone().unwrap_or_default(),
                    p.writer_features.clone().unwrap_or_default(),
                ),
                None => (1, 2, Vec::new(), Vec::new()),
            };

        let warnings: Vec<String> = snapshot
            .warnings
            .iter()
            .map(|w| format!("{}:{}: {}", w.path, w.line, w.message))
            .collect();

        TableStats {
            current_version: snapshot.target_version,
            created_timestamp,
            last_modified_timestamp,
            min_reader_version,
            min_writer_version,
            reader_features,
            writer_features,
            partition_columns,
            partition_count,
            empty_partition_count,
            schema_column_count,
            schema_change_count,
            health,
            tombstones,
            maintenance,
            warnings,
        }
    }

    /// Analyze a slice of commits directly (without a checkpoint).
    pub fn analyze(entries: &[CommitEntry]) -> TableStats {
        if entries.is_empty() {
            return Self::empty_stats();
        }
        let snapshot = SnapshotLog {
            target_version: entries.last().map(|e| e.version).unwrap_or(0),
            checkpoint_version: None,
            checkpoint_actions: Vec::new(),
            commits: entries.to_vec(),
            warnings: Vec::new(),
            files_read: entries.len(),
        };
        Self::analyze_snapshot(&snapshot)
    }

    fn compute_health(state: &TableState) -> TableHealth {
        let sizes: Vec<i64> = state.active_files.values().map(|a| a.size).collect();
        let total_files = sizes.len();
        let total_size: i64 = sizes
            .iter()
            .fold(0i64, |acc, &size| acc.saturating_add(size));
        let avg_size = if total_files > 0 {
            let sum_i128: i128 = sizes.iter().map(|&s| i128::from(s)).sum();
            i64::try_from(sum_i128 / total_files as i128).unwrap_or(if sum_i128 < 0 {
                i64::MIN
            } else {
                i64::MAX
            })
        } else {
            0
        };

        let small_file_threshold: i64 = 32 * 1024 * 1024; // 32 MB
        let small_file_count = sizes.iter().filter(|&&s| s < small_file_threshold).count();
        let small_file_pct = if total_files > 0 {
            small_file_count as f64 / total_files as f64 * 100.0
        } else {
            0.0
        };

        let skew_score = Self::compute_skew_score(&sizes);
        let min_file_size = sizes.iter().min().copied().unwrap_or(0);
        let max_file_size = sizes.iter().max().copied().unwrap_or(0);

        let mut deletion_vector_files = 0usize;
        let mut deleted_rows_in_dvs = 0i64;
        for add in state.active_files.values() {
            if let Some(dv) = &add.deletion_vector {
                deletion_vector_files += 1;
                deleted_rows_in_dvs = deleted_rows_in_dvs.saturating_add(dv.cardinality);
            }
        }

        TableHealth {
            total_files,
            total_size,
            avg_file_size: avg_size,
            min_file_size,
            max_file_size,
            small_file_count,
            small_file_pct,
            skew_score,
            deletion_vector_files,
            deleted_rows_in_dvs,
        }
    }

    /// Coefficient of variation, normalized to [0, 1].
    /// 0.0 = all files identical size (perfect), 1.0 = extreme size variance.
    pub fn compute_skew_score(sizes: &[i64]) -> f64 {
        if sizes.len() < 2 {
            return 0.0;
        }
        let sum: i128 = sizes.iter().map(|&s| i128::from(s)).sum();
        let mean = sum as f64 / sizes.len() as f64;
        if mean <= 0.0 {
            return 0.0;
        }
        let variance = sizes
            .iter()
            .map(|&s| {
                let diff = s as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / sizes.len() as f64;

        let std_dev = variance.sqrt();
        let cv = std_dev / mean;
        (cv / 2.0).min(1.0)
    }

    fn compute_maintenance(entries: &[CommitEntry]) -> MaintenanceInfo {
        let mut info = MaintenanceInfo::default();

        for entry in entries {
            for action in &entry.actions {
                if let Action::CommitInfo(ci) = action {
                    let op = ci.operation.to_uppercase();

                    // Only count completed VACUUM END commits whose status is not FAILED.
                    if op == "VACUUM END" {
                        let failed = ci
                            .operation_parameters
                            .get("status")
                            .and_then(|v| v.as_str())
                            .map(|s| s.eq_ignore_ascii_case("FAILED"))
                            .unwrap_or(false);
                        if !failed {
                            info.last_vacuum_version = Some(entry.version);
                            info.last_vacuum_timestamp = Some(ci.timestamp);
                        }
                    }

                    if op == "OPTIMIZE" {
                        info.last_optimize_version = Some(entry.version);
                        info.last_optimize_timestamp = Some(ci.timestamp);

                        // Extract Z-ORDER columns from operationParameters; preserve existing
                        // z_order_columns across non-Z-order OPTIMIZE runs.
                        if let Some(zorder) = ci.operation_parameters.get("zOrderBy") {
                            let parsed: Vec<String> = if let Some(arr) = zorder.as_array() {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            } else if let Some(s) = zorder.as_str() {
                                serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            if !parsed.is_empty() {
                                info.z_order_columns = parsed;
                            }
                        }
                    }
                }
            }
        }

        info
    }

    fn find_created_ts(state: &TableState, entries: &[CommitEntry]) -> Option<i64> {
        for entry in entries {
            for action in &entry.actions {
                if let Action::MetaData(m) = action {
                    if m.created_time.is_some() {
                        return m.created_time;
                    }
                }
            }
        }
        state.metadata.as_ref().and_then(|m| m.created_time)
    }

    fn find_last_modified_ts(state: &TableState, entries: &[CommitEntry]) -> Option<i64> {
        if let Some(ts) = entries.iter().rev().find_map(|e| {
            e.actions.iter().find_map(|a| match a {
                Action::CommitInfo(ci) if ci.timestamp > 0 => Some(ci.timestamp),
                _ => None,
            })
        }) {
            return Some(ts);
        }
        state
            .active_files
            .values()
            .map(|a| a.modification_time)
            .filter(|&t| t > 0)
            .max()
    }

    fn empty_stats() -> TableStats {
        TableStats {
            current_version: 0,
            created_timestamp: None,
            last_modified_timestamp: None,
            min_reader_version: 1,
            min_writer_version: 2,
            reader_features: Vec::new(),
            writer_features: Vec::new(),
            partition_columns: Vec::new(),
            partition_count: 0,
            empty_partition_count: 0,
            schema_column_count: 0,
            schema_change_count: 0,
            health: TableHealth::default(),
            tombstones: Default::default(),
            maintenance: MaintenanceInfo::default(),
            warnings: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::action::{AddAction, CommitInfoAction, RemoveAction};
    use std::collections::HashMap;

    #[test]
    fn test_skew_score_uniform() {
        let sizes = vec![1000i64; 100];
        let score = HealthAnalyzer::compute_skew_score(&sizes);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_skew_score_high_variance() {
        let mut sizes = vec![1_000_000i64; 10];
        sizes.extend(vec![100i64; 90]);
        let score = HealthAnalyzer::compute_skew_score(&sizes);
        assert!(score > 0.5, "High variance should produce high skew score");
    }

    #[test]
    fn test_skew_score_clamped_to_one() {
        let sizes = vec![1i64, 1_000_000_000i64];
        let score = HealthAnalyzer::compute_skew_score(&sizes);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_skew_score_single_element() {
        let score = HealthAnalyzer::compute_skew_score(&[42]);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_vacuum_requires_completed_end_and_preserves_zorder() {
        let mut failed_params = HashMap::new();
        failed_params.insert("status".to_string(), serde_json::json!("FAILED"));
        let mut ok_params = HashMap::new();
        ok_params.insert("status".to_string(), serde_json::json!("COMPLETED"));
        let mut zorder_params = HashMap::new();
        zorder_params.insert("zOrderBy".to_string(), serde_json::json!("[\"user_id\"]"));
        let mut empty_zorder_params = HashMap::new();
        empty_zorder_params.insert("zOrderBy".to_string(), serde_json::json!("[]"));

        let entries = vec![
            CommitEntry {
                version: 1,
                path: "1.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 1000,
                    operation: "VACUUM START".to_string(),
                    operation_parameters: HashMap::new(),
                    notebook_info: None,
                    job: None,
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: None,
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
            CommitEntry {
                version: 2,
                path: "2.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 2000,
                    operation: "VACUUM END".to_string(),
                    operation_parameters: failed_params,
                    notebook_info: None,
                    job: None,
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: None,
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
            CommitEntry {
                version: 3,
                path: "3.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 3000,
                    operation: "OPTIMIZE".to_string(),
                    operation_parameters: zorder_params,
                    notebook_info: None,
                    job: None,
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: None,
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
            CommitEntry {
                version: 4,
                path: "4.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 4000,
                    operation: "OPTIMIZE".to_string(),
                    operation_parameters: empty_zorder_params,
                    notebook_info: None,
                    job: None,
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: None,
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
            CommitEntry {
                version: 10,
                path: "10.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 5000,
                    operation: "VACUUM END".to_string(),
                    operation_parameters: ok_params,
                    notebook_info: None,
                    job: None,
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: None,
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
        ];

        let stats = HealthAnalyzer::analyze(&entries);
        // No fabricated v10 checkpoint when no checkpoint actually exists!
        assert_eq!(stats.maintenance.last_checkpoint_version, None);
        assert_eq!(stats.maintenance.last_vacuum_version, Some(10));
        assert_eq!(stats.maintenance.last_optimize_version, Some(4));
        assert_eq!(stats.maintenance.z_order_columns, vec!["user_id"]);
    }

    #[test]
    fn test_single_small_file_does_not_trigger_small_file_issue_and_empty_partitions_counted() {
        let mut pv1 = HashMap::new();
        pv1.insert("dt".to_string(), Some("2026-01-01".to_string()));
        let mut pv2 = HashMap::new();
        pv2.insert("dt".to_string(), Some("2026-01-02".to_string()));

        let entries = vec![
            CommitEntry {
                version: 0,
                path: "0.json".to_string(),
                actions: vec![
                    Action::Add(AddAction {
                        path: "dt=2026-01-01/a.parquet".to_string(),
                        size: 1024,
                        modification_time: 1000,
                        data_change: true,
                        partition_values: pv1,
                        stats: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                    Action::Add(AddAction {
                        path: "dt=2026-01-02/b.parquet".to_string(),
                        size: 2048,
                        modification_time: 1000,
                        data_change: true,
                        partition_values: pv2,
                        stats: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                ],
            },
            CommitEntry {
                version: 1,
                path: "1.json".to_string(),
                actions: vec![Action::Remove(RemoveAction {
                    path: "dt=2026-01-01/a.parquet".to_string(),
                    deletion_timestamp: Some(2000),
                    data_change: true,
                    extended_file_metadata: None,
                    size: None,
                    partition_values: None,
                    tags: None,
                    deletion_vector: None,
                })],
            },
        ];

        let stats = HealthAnalyzer::analyze(&entries);
        assert_eq!(stats.health.total_files, 1);
        assert_eq!(stats.partition_count, 1);
        assert_eq!(stats.empty_partition_count, 1);
        assert_eq!(stats.tombstones.active_tombstones, 1);
        assert_eq!(stats.tombstones.reclaimable_bytes, 1024);
        assert!(
            stats.health_issues().is_empty(),
            "Single-file table should not trigger small-file issue: {:?}",
            stats.health_issues()
        );
    }

    #[test]
    fn test_health_saturating_math_on_huge_file_sizes_and_dv_cardinality() {
        use crate::core::model::action::DeletionVectorDescriptor;

        let huge = i64::MAX / 2;
        assert_eq!(HealthAnalyzer::compute_skew_score(&[huge, huge, huge]), 0.0);

        let dv = DeletionVectorDescriptor {
            storage_type: "u".to_string(),
            path_or_inline_dv: "abc".to_string(),
            offset: Some(1),
            size_in_bytes: 16,
            cardinality: huge,
        };
        let entries = vec![CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![
                Action::Add(AddAction {
                    path: "a.parquet".to_string(),
                    size: huge,
                    modification_time: 1000,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: Some(dv.clone()),
                }),
                Action::Add(AddAction {
                    path: "b.parquet".to_string(),
                    size: huge,
                    modification_time: 1000,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: Some(dv.clone()),
                }),
                Action::Add(AddAction {
                    path: "c.parquet".to_string(),
                    size: huge,
                    modification_time: 1000,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: Some(dv),
                }),
            ],
        }];

        let stats = HealthAnalyzer::analyze(&entries);
        assert_eq!(stats.health.total_size, i64::MAX);
        assert_eq!(stats.health.avg_file_size, huge);
        assert_eq!(stats.health.skew_score, 0.0);
        assert_eq!(stats.health.deleted_rows_in_dvs, i64::MAX);
    }

    #[test]
    fn test_checkpoint_horizon_avoids_false_positive_and_honors_retention_duration() {
        use crate::core::model::action::{Format, MetaDataAction};

        let day_ms: i64 = 24 * 60 * 60 * 1000;
        let now_ts: i64 = 1_750_000_000_000;

        let make_snapshot = |retention: Option<&str>, tombstone_age_days: Option<i64>| {
            let mut config = HashMap::new();
            if let Some(r) = retention {
                config.insert(
                    "delta.deletedFileRetentionDuration".to_string(),
                    Some(r.to_string()),
                );
            }
            let mut ckpt_actions = vec![
                Action::MetaData(MetaDataAction {
                    id: "tbl".to_string(),
                    name: None,
                    description: None,
                    format: Format::default(),
                    schema_string: r#"{"type":"struct","fields":[{"name":"id","type":"long","nullable":false,"metadata":{}}]}"#.to_string(),
                    partition_columns: vec![],
                    configuration: config,
                    created_time: Some(now_ts - 30 * day_ms),
                }),
                Action::Add(AddAction {
                    path: "big1.parquet".to_string(),
                    size: 64 * 1024 * 1024,
                    modification_time: now_ts - day_ms,
                    data_change: false,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "big2.parquet".to_string(),
                    size: 64 * 1024 * 1024,
                    modification_time: now_ts - day_ms,
                    data_change: false,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
            ];
            if let Some(age_days) = tombstone_age_days {
                ckpt_actions.push(Action::Remove(RemoveAction {
                    path: "old.parquet".to_string(),
                    deletion_timestamp: Some(now_ts - age_days * day_ms),
                    data_change: true,
                    extended_file_metadata: Some(true),
                    size: Some(10_000_000),
                    partition_values: None,
                    tags: None,
                    deletion_vector: None,
                }));
            }
            SnapshotLog {
                target_version: 151,
                checkpoint_version: Some(150),
                checkpoint_actions: ckpt_actions,
                commits: vec![CommitEntry {
                    version: 151,
                    path: "00000000000000000151.json".to_string(),
                    actions: vec![Action::CommitInfo(CommitInfoAction {
                        timestamp: now_ts,
                        operation: "WRITE".to_string(),
                        operation_parameters: HashMap::new(),
                        notebook_info: None,
                        job: None,
                        cluster_id: None,
                        user_id: None,
                        user_name: None,
                        operation_metrics: None,
                        engine_info: None,
                        is_blind_append: None,
                        txn_id: None,
                    })],
                }],
                warnings: Vec::new(),
                files_read: 2,
            }
        };

        // 1. Checkpointed v151 table with NO tombstones and NO post-checkpoint VACUUM/OPTIMIZE:
        //    must NOT falsely trigger `--fail-on-issues`!
        let clean = HealthAnalyzer::analyze_snapshot(&make_snapshot(None, None));
        assert!(
            clean.health_issues().is_empty(),
            "Checkpointed table without expired tombstones should have 0 issues: {:?}",
            clean.health_issues()
        );

        // 2. Tombstone is 3 days old (within default 7-day retention): still 0 issues!
        let within_default = HealthAnalyzer::analyze_snapshot(&make_snapshot(None, Some(3)));
        assert_eq!(within_default.tombstones.expired_tombstones, 0);
        assert!(within_default.health_issues().is_empty());

        // 3. Tombstone is 10 days old (exceeds default 7-day retention): flags VACUUM issue!
        let expired_default = HealthAnalyzer::analyze_snapshot(&make_snapshot(None, Some(10)));
        assert_eq!(expired_default.tombstones.expired_tombstones, 1);
        assert_eq!(expired_default.health_issues().len(), 1);
        assert!(expired_default.health_issues()[0].contains("expired tombstones"));

        // 4. Same 10-day-old tombstone, but custom `interval 30 days` retention: 0 issues!
        let custom_long =
            HealthAnalyzer::analyze_snapshot(&make_snapshot(Some("interval 30 days"), Some(10)));
        assert_eq!(custom_long.tombstones.expired_tombstones, 0);
        assert_eq!(custom_long.tombstones.retention_duration_ms, 30 * day_ms);
        assert!(custom_long.health_issues().is_empty());

        // 5. 2-day-old tombstone with short `interval 24 hours` retention: flags VACUUM issue!
        let custom_short =
            HealthAnalyzer::analyze_snapshot(&make_snapshot(Some("interval 24 hours"), Some(2)));
        assert_eq!(custom_short.tombstones.expired_tombstones, 1);
        assert_eq!(custom_short.tombstones.retention_duration_ms, day_ms);
        assert_eq!(custom_short.health_issues().len(), 1);
    }
}
