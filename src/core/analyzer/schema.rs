use crate::core::model::action::Action;
use crate::core::model::schema::{
    ColumnModification, ColumnRename, DataType, NullabilityChange, SchemaChange, SchemaSnapshot,
    StructField, StructType,
};
use crate::core::reader::{CommitEntry, SnapshotLog};
use std::collections::{HashMap, HashSet};

pub struct SchemaTracker;

impl SchemaTracker {
    /// Extract the schema snapshot from a checkpoint-aware `SnapshotLog`.
    pub fn from_snapshot(snapshot: &SnapshotLog) -> Option<SchemaSnapshot> {
        // Prefer latest MetaData in post-checkpoint commits, falling back to checkpoint MetaData.
        for entry in snapshot.commits.iter().rev() {
            if let Some(snap) = Self::schema_in_commit(entry) {
                return Some(snap);
            }
        }
        if let Some(ckpt_v) = snapshot.checkpoint_version {
            for action in snapshot.checkpoint_actions.iter().rev() {
                if let Action::MetaData(m) = action {
                    if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                        return Some(SchemaSnapshot {
                            version: ckpt_v,
                            timestamp: m.created_time,
                            schema,
                        });
                    }
                }
            }
        }
        None
    }

    /// Current schema (from latest `MetaData` action in commits).
    pub fn current_schema(entries: &[CommitEntry]) -> Option<SchemaSnapshot> {
        for entry in entries.iter().rev() {
            if let Some(snap) = Self::schema_in_commit(entry) {
                return Some(snap);
            }
        }
        None
    }

    /// Schema at a specific version.
    pub fn at_version(entries: &[CommitEntry], version: u64) -> Option<SchemaSnapshot> {
        for entry in entries.iter().filter(|e| e.version <= version).rev() {
            if let Some(snap) = Self::schema_in_commit(entry) {
                return Some(snap);
            }
        }
        None
    }

    /// Full schema evolution history across commit entries.
    pub fn history(entries: &[CommitEntry]) -> Vec<SchemaChange> {
        Self::history_with_base(None, entries)
    }

    /// Schema evolution history starting from an optional checkpoint baseline.
    pub fn history_from_snapshot(snapshot: &SnapshotLog) -> Vec<SchemaChange> {
        let base_schema = snapshot
            .checkpoint_actions
            .iter()
            .rev()
            .find_map(|a| match a {
                Action::MetaData(m) => StructType::from_schema_string(&m.schema_string).ok(),
                _ => None,
            });
        Self::history_with_base(base_schema, &snapshot.commits)
    }

    pub fn history_with_base(
        initial_schema: Option<StructType>,
        entries: &[CommitEntry],
    ) -> Vec<SchemaChange> {
        let mut prev_schema = initial_schema;
        let mut changes = Vec::new();

        for entry in entries {
            let commit_ts = commit_timestamp(entry);
            for action in &entry.actions {
                if let Action::MetaData(m) = action {
                    if let Ok(curr_schema) = StructType::from_schema_string(&m.schema_string) {
                        let ts = commit_ts.or(m.created_time);
                        if let Some(prev) = &prev_schema {
                            let change = schema_diff(prev, &curr_schema, entry.version, ts);
                            if !change.is_empty() {
                                changes.push(change);
                            }
                        }
                        prev_schema = Some(curr_schema);
                    }
                }
            }
        }

        changes
    }

    fn schema_in_commit(entry: &CommitEntry) -> Option<SchemaSnapshot> {
        let commit_ts = commit_timestamp(entry);
        for action in entry.actions.iter().rev() {
            if let Action::MetaData(m) = action {
                if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                    return Some(SchemaSnapshot {
                        version: entry.version,
                        // Prefer CommitInfo.timestamp over MetaData.created_time.
                        timestamp: commit_ts.or(m.created_time),
                        schema,
                    });
                }
            }
        }
        None
    }
}

fn commit_timestamp(entry: &CommitEntry) -> Option<i64> {
    entry.actions.iter().find_map(|a| match a {
        Action::CommitInfo(ci) if ci.timestamp > 0 => Some(ci.timestamp),
        _ => None,
    })
}

/// Recursively diff two Delta `StructType` schemas with Column Mapping rename awareness,
/// nested struct/array/map comparison, nullability tightening checks, and breaking-change
/// classification.
pub fn schema_diff(
    old: &StructType,
    new: &StructType,
    version: u64,
    timestamp: Option<i64>,
) -> SchemaChange {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut renamed = Vec::new();
    let mut modified = Vec::new();
    let mut nullability = Vec::new();

    diff_struct_fields(
        "",
        &old.fields,
        &new.fields,
        &mut added,
        &mut removed,
        &mut renamed,
        &mut modified,
        &mut nullability,
    );

    added.sort();
    removed.sort();
    renamed.sort_by(|a, b| {
        a.old_name
            .cmp(&b.old_name)
            .then(a.new_name.cmp(&b.new_name))
    });
    modified.sort_by(|a, b| a.name.cmp(&b.name));
    nullability.sort_by(|a, b| a.name.cmp(&b.name));

    let has_breaking_changes = !removed.is_empty()
        || !renamed.is_empty()
        || modified.iter().any(|m| m.breaking)
        || nullability.iter().any(|n| n.breaking);

    SchemaChange {
        version,
        timestamp,
        added_columns: added,
        removed_columns: removed,
        renamed_columns: renamed,
        modified_columns: modified,
        nullability_changes: nullability,
        has_breaking_changes,
    }
}

#[allow(clippy::too_many_arguments)]
fn diff_struct_fields(
    prefix: &str,
    old_fields: &[StructField],
    new_fields: &[StructField],
    added: &mut Vec<String>,
    removed: &mut Vec<String>,
    renamed: &mut Vec<ColumnRename>,
    modified: &mut Vec<ColumnModification>,
    nullability: &mut Vec<NullabilityChange>,
) {
    let old_by_id: HashMap<i64, usize> = old_fields
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.column_mapping_id().map(|id| (id, i)))
        .collect();
    let new_by_id: HashMap<i64, usize> = new_fields
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.column_mapping_id().map(|id| (id, i)))
        .collect();

    let mut matched_old: HashSet<usize> = HashSet::new();
    let mut matched_new: HashSet<usize> = HashSet::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();

    // Pass 1: Match by `delta.columnMapping.id` (accurately detects renames).
    for (id, &old_idx) in &old_by_id {
        if let Some(&new_idx) = new_by_id.get(id) {
            matched_old.insert(old_idx);
            matched_new.insert(new_idx);
            pairs.push((old_idx, new_idx));
            let old_f = &old_fields[old_idx];
            let new_f = &new_fields[new_idx];
            if old_f.name != new_f.name {
                renamed.push(ColumnRename {
                    old_name: qualify(prefix, &old_f.name),
                    new_name: qualify(prefix, &new_f.name),
                    column_id: Some(*id),
                });
            }
        }
    }

    // Pass 2: Match remaining fields by name.
    let new_by_name: HashMap<&str, usize> = new_fields
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched_new.contains(i))
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();

    for (old_idx, old_f) in old_fields.iter().enumerate() {
        if matched_old.contains(&old_idx) {
            continue;
        }
        if let Some(&new_idx) = new_by_name.get(old_f.name.as_str()) {
            matched_old.insert(old_idx);
            matched_new.insert(new_idx);
            pairs.push((old_idx, new_idx));
        }
    }

    // Unmatched old fields -> removed.
    for (old_idx, old_f) in old_fields.iter().enumerate() {
        if !matched_old.contains(&old_idx) {
            removed.push(qualify(prefix, &old_f.name));
        }
    }

    // Unmatched new fields -> added.
    for (new_idx, new_f) in new_fields.iter().enumerate() {
        if !matched_new.contains(&new_idx) {
            added.push(qualify(prefix, &new_f.name));
        }
    }

    // Compare matched pairs (nullability + recursive types).
    for (old_idx, new_idx) in pairs {
        let old_f = &old_fields[old_idx];
        let new_f = &new_fields[new_idx];
        let path = qualify(prefix, &new_f.name);

        if old_f.nullable != new_f.nullable {
            // Tightening nullable: true -> false is breaking.
            let breaking = old_f.nullable && !new_f.nullable;
            nullability.push(NullabilityChange {
                name: path.clone(),
                old_nullable: old_f.nullable,
                new_nullable: new_f.nullable,
                breaking,
            });
        }

        diff_data_types(
            &path,
            &old_f.data_type,
            &new_f.data_type,
            added,
            removed,
            renamed,
            modified,
            nullability,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn diff_data_types(
    path: &str,
    old_ty: &DataType,
    new_ty: &DataType,
    added: &mut Vec<String>,
    removed: &mut Vec<String>,
    renamed: &mut Vec<ColumnRename>,
    modified: &mut Vec<ColumnModification>,
    nullability: &mut Vec<NullabilityChange>,
) {
    if old_ty == new_ty {
        return;
    }

    match (old_ty, new_ty) {
        (DataType::Struct(old_s), DataType::Struct(new_s)) => {
            diff_struct_fields(
                path,
                &old_s.fields,
                &new_s.fields,
                added,
                removed,
                renamed,
                modified,
                nullability,
            );
        }
        (
            DataType::Array {
                element_type: old_e,
                contains_null: old_cn,
            },
            DataType::Array {
                element_type: new_e,
                contains_null: new_cn,
            },
        ) => {
            let elem_path = format!("{}[]", path);
            if old_cn != new_cn {
                nullability.push(NullabilityChange {
                    name: elem_path.clone(),
                    old_nullable: *old_cn,
                    new_nullable: *new_cn,
                    breaking: *old_cn && !*new_cn,
                });
            }
            diff_data_types(
                &elem_path,
                old_e,
                new_e,
                added,
                removed,
                renamed,
                modified,
                nullability,
            );
        }
        (
            DataType::Map {
                key_type: old_k,
                value_type: old_v,
                value_contains_null: old_vcn,
            },
            DataType::Map {
                key_type: new_k,
                value_type: new_v,
                value_contains_null: new_vcn,
            },
        ) => {
            let key_path = format!("{}<key>", path);
            let val_path = format!("{}<value>", path);
            if old_vcn != new_vcn {
                nullability.push(NullabilityChange {
                    name: val_path.clone(),
                    old_nullable: *old_vcn,
                    new_nullable: *new_vcn,
                    breaking: *old_vcn && !*new_vcn,
                });
            }
            diff_data_types(
                &key_path,
                old_k,
                new_k,
                added,
                removed,
                renamed,
                modified,
                nullability,
            );
            diff_data_types(
                &val_path,
                old_v,
                new_v,
                added,
                removed,
                renamed,
                modified,
                nullability,
            );
        }
        (DataType::Primitive(old_p), DataType::Primitive(new_p)) => {
            let safe = is_safe_type_widening(old_p, new_p);
            modified.push(ColumnModification {
                name: path.to_string(),
                old_type: old_p.clone(),
                new_type: new_p.clone(),
                breaking: !safe,
                detail: if safe {
                    "safe type widening".to_string()
                } else {
                    "incompatible or narrowing type change".to_string()
                },
            });
        }
        _ => {
            modified.push(ColumnModification {
                name: path.to_string(),
                old_type: old_ty.display_name(),
                new_type: new_ty.display_name(),
                breaking: true,
                detail: "structural type kind change".to_string(),
            });
        }
    }
}

fn qualify(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{}.{}", prefix, field)
    }
}

/// Delta Lake safe type widening rules (Protocol typeWidening).
fn is_safe_type_widening(old: &str, new: &str) -> bool {
    let o = old.trim().to_ascii_lowercase();
    let n = new.trim().to_ascii_lowercase();
    if o == n {
        return true;
    }
    match (o.as_str(), n.as_str()) {
        ("byte", "short" | "integer" | "long") => true,
        ("short", "integer" | "long") => true,
        ("integer", "long") => true,
        ("float", "double") => true,
        ("date", "timestamp" | "timestamp_ntz") => true,
        _ => {
            if let (Some((p1, s1)), Some((p2, s2))) = (parse_decimal(&o), parse_decimal(&n)) {
                s1 == s2 && p2 >= p1
            } else {
                false
            }
        }
    }
}

fn parse_decimal(s: &str) -> Option<(u32, u32)> {
    let inner = s.strip_prefix("decimal(")?.strip_suffix(')')?;
    let (p, sc) = inner.split_once(',')?;
    Some((p.trim().parse().ok()?, sc.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_schema(json: &str) -> StructType {
        StructType::from_schema_string(json).unwrap()
    }

    #[test]
    fn test_column_mapping_rename_detected_instead_of_drop_and_add() {
        let old = parse_schema(
            r#"{"type":"struct","fields":[
                {"name":"user_id","type":"long","nullable":false,"metadata":{"delta.columnMapping.id":1}},
                {"name":"amt","type":"integer","nullable":true,"metadata":{"delta.columnMapping.id":2}}
            ]}"#,
        );
        let new = parse_schema(
            r#"{"type":"struct","fields":[
                {"name":"user_id","type":"long","nullable":false,"metadata":{"delta.columnMapping.id":1}},
                {"name":"amount","type":"long","nullable":true,"metadata":{"delta.columnMapping.id":2}}
            ]}"#,
        );

        let diff = schema_diff(&old, &new, 2, Some(1000));
        assert!(diff.added_columns.is_empty());
        assert!(diff.removed_columns.is_empty());
        assert_eq!(diff.renamed_columns.len(), 1);
        assert_eq!(diff.renamed_columns[0].old_name, "amt");
        assert_eq!(diff.renamed_columns[0].new_name, "amount");
        assert_eq!(diff.renamed_columns[0].column_id, Some(2));
        // Widening integer -> long is non-breaking, rename itself flags breaking
        assert_eq!(diff.modified_columns.len(), 1);
        assert_eq!(diff.modified_columns[0].old_type, "integer");
        assert_eq!(diff.modified_columns[0].new_type, "long");
        assert!(!diff.modified_columns[0].breaking);
    }

    #[test]
    fn test_nested_struct_and_nullability_breaking_detection() {
        let old = parse_schema(
            r#"{"type":"struct","fields":[
                {"name":"user","type":{"type":"struct","fields":[
                    {"name":"email","type":"string","nullable":true,"metadata":{}},
                    {"name":"age","type":"integer","nullable":true,"metadata":{}}
                ]},"nullable":true,"metadata":{}},
                {"name":"tags","type":{"type":"array","elementType":"string","containsNull":true},"nullable":true,"metadata":{}}
            ]}"#,
        );
        let new = parse_schema(
            r#"{"type":"struct","fields":[
                {"name":"user","type":{"type":"struct","fields":[
                    {"name":"email","type":"string","nullable":false,"metadata":{}},
                    {"name":"age","type":"short","nullable":true,"metadata":{}},
                    {"name":"country","type":"string","nullable":true,"metadata":{}}
                ]},"nullable":true,"metadata":{}},
                {"name":"tags","type":{"type":"array","elementType":"string","containsNull":false},"nullable":true,"metadata":{}}
            ]}"#,
        );

        let diff = schema_diff(&old, &new, 5, None);
        assert_eq!(diff.added_columns, vec!["user.country"]);
        assert_eq!(diff.modified_columns.len(), 1);
        assert_eq!(diff.modified_columns[0].name, "user.age");
        assert!(diff.modified_columns[0].breaking); // narrowing integer -> short
        assert_eq!(diff.nullability_changes.len(), 2); // user.email and tags[]
        assert!(diff.nullability_changes.iter().all(|n| n.breaking));
        assert!(diff.has_breaking_changes);
    }

    #[test]
    fn test_safe_widening_is_not_breaking() {
        let old = parse_schema(
            r#"{"type":"struct","fields":[
                {"name":"a","type":"integer","nullable":false,"metadata":{}},
                {"name":"b","type":"decimal(10,2)","nullable":false,"metadata":{}}
            ]}"#,
        );
        let new = parse_schema(
            r#"{"type":"struct","fields":[
                {"name":"a","type":"long","nullable":true,"metadata":{}},
                {"name":"b","type":"decimal(18,2)","nullable":true,"metadata":{}}
            ]}"#,
        );

        let diff = schema_diff(&old, &new, 1, None);
        assert!(!diff.has_breaking_changes);
        assert_eq!(diff.modified_columns.len(), 2);
        assert!(diff.modified_columns.iter().all(|m| !m.breaking));
        assert_eq!(diff.nullability_changes.len(), 2);
        assert!(diff.nullability_changes.iter().all(|n| !n.breaking));
    }
}
