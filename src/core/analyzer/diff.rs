use crate::core::analyzer::schema::SchemaTracker;
use crate::core::model::action::Action;
use crate::core::model::schema::{SchemaChange, StructType};
use crate::core::reader::CommitEntry;
use crate::core::replay::TableState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Summary of differences between two table versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionDiff {
    pub v1: u64,
    pub v2: u64,
    pub schema_changes: Vec<SchemaChange>,
    pub has_breaking_schema_changes: bool,
    /// Logical (`dataChange = true`) file additions.
    pub files_added: i64,
    /// Logical (`dataChange = true`) file removals.
    pub files_removed: i64,
    /// Logical (`dataChange = true`) bytes added.
    pub bytes_added: i64,
    /// Logical (`dataChange = true`) bytes removed (enriched from prior state if absent).
    pub bytes_removed: i64,
    /// Physical (all `Add`, including `OPTIMIZE` / `dataChange = false`) file additions.
    pub physical_files_added: i64,
    /// Physical (all `Remove`, including `OPTIMIZE` / `dataChange = false`) file removals.
    pub physical_files_removed: i64,
    /// Physical bytes added.
    pub physical_bytes_added: i64,
    /// Physical bytes removed.
    pub physical_bytes_removed: i64,
    pub new_partitions: Vec<String>,
    pub removed_partitions: Vec<String>,
    pub operations: HashMap<String, usize>,
    pub total_commits: usize,
}

pub struct DiffAnalyzer;

impl DiffAnalyzer {
    /// Diff between `v1` (given its materialized `TableState`) and `v2` by cumulatively
    /// replaying `entries_after` (`v1+1..=v2`) on top of `state_at_v1`.
    pub fn diff_from_state(
        v1: u64,
        mut state: TableState,
        entries_after: &[CommitEntry],
    ) -> VersionDiff {
        let partitions_before = state.active_partitions();
        let initial_schema = state
            .metadata
            .as_ref()
            .and_then(|m| StructType::from_schema_string(&m.schema_string).ok());

        let mut files_added = 0i64;
        let mut files_removed = 0i64;
        let mut bytes_added = 0i64;
        let mut bytes_removed = 0i64;
        let mut physical_files_added = 0i64;
        let mut physical_files_removed = 0i64;
        let mut physical_bytes_added = 0i64;
        let mut physical_bytes_removed = 0i64;
        let mut operations: HashMap<String, usize> = HashMap::new();

        for entry in entries_after {
            let delta = state.apply_commit(entry);
            files_added = files_added.saturating_add(delta.logical_files_added);
            files_removed = files_removed.saturating_add(delta.logical_files_removed);
            bytes_added = bytes_added.saturating_add(delta.logical_bytes_added);
            bytes_removed = bytes_removed.saturating_add(delta.logical_bytes_removed);
            physical_files_added = physical_files_added.saturating_add(delta.physical_files_added);
            physical_files_removed =
                physical_files_removed.saturating_add(delta.physical_files_removed);
            physical_bytes_added = physical_bytes_added.saturating_add(delta.physical_bytes_added);
            physical_bytes_removed =
                physical_bytes_removed.saturating_add(delta.physical_bytes_removed);

            for action in &entry.actions {
                if let Action::CommitInfo(ci) = action {
                    *operations.entry(ci.operation.clone()).or_insert(0) += 1;
                }
            }
        }

        let partitions_after = state.active_partitions();
        let mut new_partitions: Vec<String> = partitions_after
            .difference(&partitions_before)
            .cloned()
            .collect();
        new_partitions.sort();

        let mut removed_partitions: Vec<String> = partitions_before
            .difference(&partitions_after)
            .cloned()
            .collect();
        removed_partitions.sort();

        let v2 = entries_after.last().map(|e| e.version).unwrap_or(v1);
        let schema_changes = SchemaTracker::history_with_base(initial_schema, entries_after);
        let has_breaking_schema_changes = schema_changes.iter().any(|c| c.has_breaking_changes);

        VersionDiff {
            v1,
            v2,
            schema_changes,
            has_breaking_schema_changes,
            files_added,
            files_removed,
            bytes_added,
            bytes_removed,
            physical_files_added,
            physical_files_removed,
            physical_bytes_added,
            physical_bytes_removed,
            new_partitions,
            removed_partitions,
            operations,
            total_commits: entries_after.len(),
        }
    }

    /// Convenience wrapper building `TableState` from `entries_before` (`0..=v1`)
    /// and cumulatively applying `entries_after` (`v1+1..=v2`).
    pub fn diff(
        v1: u64,
        entries_before: &[CommitEntry],
        entries_after: &[CommitEntry],
    ) -> VersionDiff {
        let state_at_v1 = TableState::from_commits(entries_before);
        Self::diff_from_state(v1, state_at_v1, entries_after)
    }

    /// Delegate to recursive typed `schema_diff`.
    pub fn schema_diff(
        old: &StructType,
        new: &StructType,
        version: u64,
        timestamp: Option<i64>,
    ) -> SchemaChange {
        crate::core::analyzer::schema::schema_diff(old, new, version, timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::action::{AddAction, RemoveAction};
    use crate::core::model::schema::{DataType, StructField, StructType};

    fn pv(k: &str, v: &str) -> HashMap<String, Option<String>> {
        let mut m = HashMap::new();
        m.insert(k.to_string(), Some(v.to_string()));
        m
    }

    #[test]
    fn test_diff_analyzer_empty() {
        let diff = DiffAnalyzer::diff(0, &[], &[]);
        assert_eq!(diff.v1, 0);
        assert_eq!(diff.v2, 0);
        assert_eq!(diff.files_added, 0);
        assert_eq!(diff.files_removed, 0);
        assert!(diff.schema_changes.is_empty());
        assert!(!diff.has_breaking_schema_changes);
    }

    #[test]
    fn test_diff_preserves_untouched_and_partially_removed_partitions() {
        // v0 creates 2 partitions: p1 (2 files) and p2 (1 file)
        let before = vec![CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![
                Action::Add(AddAction {
                    path: "dt=2026-01-01/f1.parquet".to_string(),
                    size: 1000,
                    modification_time: 100,
                    data_change: true,
                    partition_values: pv("dt", "2026-01-01"),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "dt=2026-01-01/f2.parquet".to_string(),
                    size: 2000,
                    modification_time: 100,
                    data_change: true,
                    partition_values: pv("dt", "2026-01-01"),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "dt=2026-01-02/f3.parquet".to_string(),
                    size: 3000,
                    modification_time: 100,
                    data_change: true,
                    partition_values: pv("dt", "2026-01-02"),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
            ],
        }];

        // v1 removes 1 of 2 files in 2026-01-01 (still alive!), leaves 2026-01-02 untouched,
        // and adds 2026-01-03. Remove has size: None -> must resolve to 1000 from prior Add!
        let after = vec![CommitEntry {
            version: 1,
            path: "1.json".to_string(),
            actions: vec![
                Action::Remove(RemoveAction {
                    path: "dt=2026-01-01/f1.parquet".to_string(),
                    deletion_timestamp: Some(200),
                    data_change: true,
                    extended_file_metadata: Some(false),
                    size: None,
                    partition_values: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "dt=2026-01-03/f4.parquet".to_string(),
                    size: 4000,
                    modification_time: 200,
                    data_change: true,
                    partition_values: pv("dt", "2026-01-03"),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
            ],
        }];

        let diff = DiffAnalyzer::diff(0, &before, &after);
        assert_eq!(diff.new_partitions, vec!["dt=2026-01-03"]);
        assert!(
            diff.removed_partitions.is_empty(),
            "Neither untouched nor partially-deleted partitions should be marked removed: {:?}",
            diff.removed_partitions
        );
        assert_eq!(diff.bytes_removed, 1000);
        assert_eq!(diff.bytes_added, 4000);
    }

    #[test]
    fn test_diff_tracks_physical_optimize_changes() {
        let before = vec![CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![
                Action::Add(AddAction {
                    path: "s1.parquet".to_string(),
                    size: 500,
                    modification_time: 100,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "s2.parquet".to_string(),
                    size: 500,
                    modification_time: 100,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
            ],
        }];

        let after = vec![CommitEntry {
            version: 1,
            path: "1.json".to_string(),
            actions: vec![
                Action::Remove(RemoveAction {
                    path: "s1.parquet".to_string(),
                    deletion_timestamp: Some(200),
                    data_change: false,
                    extended_file_metadata: None,
                    size: Some(500),
                    partition_values: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Remove(RemoveAction {
                    path: "s2.parquet".to_string(),
                    deletion_timestamp: Some(200),
                    data_change: false,
                    extended_file_metadata: None,
                    size: Some(500),
                    partition_values: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "compacted.parquet".to_string(),
                    size: 950,
                    modification_time: 200,
                    data_change: false,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
            ],
        }];

        let diff = DiffAnalyzer::diff(0, &before, &after);
        assert_eq!(diff.files_added, 0);
        assert_eq!(diff.files_removed, 0);
        assert_eq!(diff.physical_files_added, 1);
        assert_eq!(diff.physical_files_removed, 2);
        assert_eq!(diff.physical_bytes_added, 950);
        assert_eq!(diff.physical_bytes_removed, 1000);
    }

    #[test]
    fn test_schema_diff() {
        let old_schema = StructType {
            type_name: "struct".to_string(),
            fields: vec![
                StructField {
                    name: "id".to_string(),
                    data_type: DataType::Primitive("integer".to_string()),
                    nullable: false,
                    metadata: serde_json::json!({}),
                },
                StructField {
                    name: "name".to_string(),
                    data_type: DataType::Primitive("string".to_string()),
                    nullable: true,
                    metadata: serde_json::json!({}),
                },
            ],
        };

        let new_schema = StructType {
            type_name: "struct".to_string(),
            fields: vec![
                StructField {
                    name: "id".to_string(),
                    data_type: DataType::Primitive("integer".to_string()),
                    nullable: false,
                    metadata: serde_json::json!({}),
                },
                StructField {
                    name: "name".to_string(),
                    data_type: DataType::Primitive("string".to_string()),
                    nullable: true,
                    metadata: serde_json::json!({}),
                },
                StructField {
                    name: "age".to_string(),
                    data_type: DataType::Primitive("integer".to_string()),
                    nullable: true,
                    metadata: serde_json::json!({}),
                },
            ],
        };

        let change = DiffAnalyzer::schema_diff(&old_schema, &new_schema, 1, None);
        assert_eq!(change.added_columns, vec!["age".to_string()]);
        assert!(change.removed_columns.is_empty());
        assert!(change.modified_columns.is_empty());
        assert!(!change.has_breaking_changes);
    }

    #[test]
    fn test_diff_saturating_math_on_huge_file_sizes() {
        let huge = i64::MAX / 2;
        let before = vec![CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![
                Action::Add(AddAction {
                    path: "a.parquet".to_string(),
                    size: huge,
                    modification_time: 100,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "b.parquet".to_string(),
                    size: huge,
                    modification_time: 100,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
                Action::Add(AddAction {
                    path: "c.parquet".to_string(),
                    size: huge,
                    modification_time: 100,
                    data_change: true,
                    partition_values: HashMap::new(),
                    stats: None,
                    tags: None,
                    deletion_vector: None,
                }),
            ],
        }];
        let after = vec![
            CommitEntry {
                version: 1,
                path: "1.json".to_string(),
                actions: vec![
                    Action::Add(AddAction {
                        path: "d.parquet".to_string(),
                        size: huge,
                        modification_time: 200,
                        data_change: true,
                        partition_values: HashMap::new(),
                        stats: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                    Action::Add(AddAction {
                        path: "e.parquet".to_string(),
                        size: huge,
                        modification_time: 200,
                        data_change: true,
                        partition_values: HashMap::new(),
                        stats: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                ],
            },
            CommitEntry {
                version: 2,
                path: "2.json".to_string(),
                actions: vec![
                    Action::Add(AddAction {
                        path: "f.parquet".to_string(),
                        size: huge,
                        modification_time: 300,
                        data_change: true,
                        partition_values: HashMap::new(),
                        stats: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                    Action::Remove(RemoveAction {
                        path: "a.parquet".to_string(),
                        deletion_timestamp: Some(300),
                        data_change: true,
                        extended_file_metadata: None,
                        size: None,
                        partition_values: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                    Action::Remove(RemoveAction {
                        path: "b.parquet".to_string(),
                        deletion_timestamp: Some(300),
                        data_change: true,
                        extended_file_metadata: None,
                        size: None,
                        partition_values: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                    Action::Remove(RemoveAction {
                        path: "c.parquet".to_string(),
                        deletion_timestamp: Some(300),
                        data_change: true,
                        extended_file_metadata: None,
                        size: None,
                        partition_values: None,
                        tags: None,
                        deletion_vector: None,
                    }),
                ],
            },
        ];

        let diff = DiffAnalyzer::diff(0, &before, &after);
        assert_eq!(diff.bytes_added, i64::MAX);
        assert_eq!(diff.bytes_removed, i64::MAX);
        assert_eq!(diff.physical_bytes_added, i64::MAX);
        assert_eq!(diff.physical_bytes_removed, i64::MAX);
    }
}
