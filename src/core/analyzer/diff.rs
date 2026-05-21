use crate::core::model::action::Action;
use crate::core::model::schema::{SchemaChange, StructType};
use crate::core::reader::CommitEntry;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Summary of differences between two table versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionDiff {
    pub v1: u64,
    pub v2: u64,
    pub schema_changes: Vec<SchemaChange>,
    pub files_added: i64,
    pub files_removed: i64,
    pub bytes_added: i64,
    pub bytes_removed: i64,
    pub new_partitions: Vec<String>,
    pub removed_partitions: Vec<String>,
    pub operations: HashMap<String, usize>,
    pub total_commits: usize,
}

pub struct DiffAnalyzer;

impl DiffAnalyzer {
    pub fn diff(
        v1: u64,
        entries_before: &[CommitEntry],
        entries_after: &[CommitEntry],
    ) -> VersionDiff {
        // File-level delta: files added and removed in the range
        let mut files_added = 0i64;
        let mut files_removed = 0i64;
        let mut bytes_added = 0i64;
        let mut bytes_removed = 0i64;
        let mut operations: HashMap<String, usize> = HashMap::new();

        // Partition sets for before and after
        let partitions_before = Self::active_partitions(entries_before);
        let partitions_after = Self::active_partitions(entries_after);

        let new_partitions: Vec<String> = partitions_after
            .difference(&partitions_before)
            .cloned()
            .collect();
        let removed_partitions: Vec<String> = partitions_before
            .difference(&partitions_after)
            .cloned()
            .collect();

        // Iterate only the range (entries_after represents the commits v1+1..v2)
        for entry in entries_after {
            for action in &entry.actions {
                match action {
                    Action::Add(a) if a.data_change => {
                        files_added += 1;
                        bytes_added += a.size;
                    }
                    Action::Remove(r) if r.data_change => {
                        files_removed += 1;
                        bytes_removed += r.size.unwrap_or(0);
                    }
                    Action::CommitInfo(ci) => {
                        *operations.entry(ci.operation.clone()).or_insert(0) += 1;
                    }
                    _ => {}
                }
            }
        }

        let v2 = entries_after.last().map(|e| e.version).unwrap_or(v1);

        // Schema changes
        let all_entries: Vec<&CommitEntry> =
            entries_before.iter().chain(entries_after.iter()).collect();
        let schema_changes = Self::detect_schema_changes(&all_entries, v1);

        VersionDiff {
            v1,
            v2,
            schema_changes,
            files_added,
            files_removed,
            bytes_added,
            bytes_removed,
            new_partitions,
            removed_partitions,
            operations,
            total_commits: entries_after.len(),
        }
    }

    fn active_partitions(entries: &[CommitEntry]) -> std::collections::HashSet<String> {
        let mut active: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

        for entry in entries {
            for action in &entry.actions {
                match action {
                    Action::Add(a) if !a.partition_values.is_empty() => {
                        let key = Self::partition_key(&a.partition_values);
                        active.insert(key, true);
                    }
                    Action::Remove(r) => {
                        if let Some(pv) = &r.partition_values {
                            if !pv.is_empty() {
                                let key = Self::partition_key(pv);
                                active.remove(&key);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        active.into_keys().collect()
    }

    fn partition_key(pv: &std::collections::HashMap<String, Option<String>>) -> String {
        let mut kv: Vec<_> = pv.iter().collect();
        kv.sort_by_key(|(k, _)| k.as_str());
        kv.iter()
            .map(|(k, v)| format!("{}={}", k, v.as_deref().unwrap_or("__NULL__")))
            .collect::<Vec<_>>()
            .join("/")
    }

    fn detect_schema_changes(entries: &[&CommitEntry], boundary_version: u64) -> Vec<SchemaChange> {
        let mut changes = vec![];
        let mut prev_schema: Option<StructType> = None;

        for entry in entries {
            for action in &entry.actions {
                if let Action::MetaData(m) = action {
                    if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                        if let Some(prev) = &prev_schema {
                            if entry.version > boundary_version {
                                let change = Self::schema_diff(prev, &schema, entry.version, None);
                                if !change.added_columns.is_empty()
                                    || !change.removed_columns.is_empty()
                                    || !change.modified_columns.is_empty()
                                {
                                    changes.push(change);
                                }
                            }
                        }
                        prev_schema = Some(schema);
                    }
                }
            }
        }
        changes
    }

    pub fn schema_diff(
        old: &StructType,
        new: &StructType,
        version: u64,
        timestamp: Option<i64>,
    ) -> SchemaChange {
        let old_names: std::collections::HashSet<_> = old.fields.iter().map(|f| &f.name).collect();
        let new_names: std::collections::HashSet<_> = new.fields.iter().map(|f| &f.name).collect();

        let added = new_names
            .difference(&old_names)
            .map(|n| (*n).clone())
            .collect();
        let removed = old_names
            .difference(&new_names)
            .map(|n| (*n).clone())
            .collect();

        // Type changes for columns in both
        let mut modified = vec![];
        for new_field in &new.fields {
            if let Some(old_field) = old.fields.iter().find(|f| f.name == new_field.name) {
                if old_field.data_type != new_field.data_type {
                    modified.push((
                        new_field.name.clone(),
                        old_field.data_type.to_string(),
                        new_field.data_type.to_string(),
                    ));
                }
            }
        }

        SchemaChange {
            version,
            timestamp,
            added_columns: added,
            removed_columns: removed,
            modified_columns: modified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::schema::{StructField, StructType};

    #[test]
    fn test_diff_analyzer_empty() {
        let v1 = 0u64;
        let entries_before = vec![];
        let entries_after = vec![];
        let diff = DiffAnalyzer::diff(v1, &entries_before, &entries_after);
        assert_eq!(diff.v1, 0);
        assert_eq!(diff.v2, 0);
        assert_eq!(diff.files_added, 0);
        assert_eq!(diff.files_removed, 0);
        assert!(diff.schema_changes.is_empty());
    }

    #[test]
    fn test_schema_diff() {
        let old_schema = StructType {
            type_name: "struct".to_string(),
            fields: vec![
                StructField {
                    name: "id".to_string(),
                    data_type: serde_json::json!("integer"),
                    nullable: false,
                    metadata: serde_json::json!({}),
                },
                StructField {
                    name: "name".to_string(),
                    data_type: serde_json::json!("string"),
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
                    data_type: serde_json::json!("integer"),
                    nullable: false,
                    metadata: serde_json::json!({}),
                },
                StructField {
                    name: "name".to_string(),
                    data_type: serde_json::json!("string"),
                    nullable: true,
                    metadata: serde_json::json!({}),
                },
                StructField {
                    name: "age".to_string(),
                    data_type: serde_json::json!("integer"),
                    nullable: true,
                    metadata: serde_json::json!({}),
                },
            ],
        };

        let change = DiffAnalyzer::schema_diff(&old_schema, &new_schema, 1, None);
        assert_eq!(change.added_columns, vec!["age".to_string()]);
        assert!(change.removed_columns.is_empty());
        assert!(change.modified_columns.is_empty());
    }
}
