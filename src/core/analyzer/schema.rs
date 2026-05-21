use crate::core::analyzer::diff::DiffAnalyzer;
use crate::core::model::action::Action;
use crate::core::model::schema::{SchemaChange, SchemaSnapshot, StructType};
use crate::core::reader::CommitEntry;

pub struct SchemaTracker;

impl SchemaTracker {
    /// Current schema (from latest MetaData action).
    pub fn current_schema(entries: &[CommitEntry]) -> Option<SchemaSnapshot> {
        for entry in entries.iter().rev() {
            for action in entry.actions.iter().rev() {
                if let Action::MetaData(m) = action {
                    if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                        return Some(SchemaSnapshot {
                            version: entry.version,
                            timestamp: m.created_time,
                            schema,
                        });
                    }
                }
            }
        }
        None
    }

    /// Full schema evolution history — one snapshot per MetaData action.
    pub fn history(entries: &[CommitEntry]) -> Vec<SchemaChange> {
        let mut snapshots: Vec<(u64, Option<i64>, StructType)> = vec![];

        for entry in entries {
            for action in &entry.actions {
                if let Action::MetaData(m) = action {
                    // Extract timestamp from CommitInfo in the same commit if possible
                    let ts = entry.actions.iter().find_map(|a| {
                        if let Action::CommitInfo(ci) = a {
                            Some(ci.timestamp)
                        } else {
                            None
                        }
                    });

                    if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                        snapshots.push((entry.version, ts, schema));
                    }
                }
            }
        }

        // Compute diffs between consecutive snapshots
        let mut changes: Vec<SchemaChange> = vec![];

        for window in snapshots.windows(2) {
            let (_, _, prev_schema) = &window[0];
            let (version, ts, curr_schema) = &window[1];

            let change = DiffAnalyzer::schema_diff(prev_schema, curr_schema, *version, *ts);

            if !change.added_columns.is_empty()
                || !change.removed_columns.is_empty()
                || !change.modified_columns.is_empty()
            {
                changes.push(change);
            }
        }

        changes
    }

    /// Schema at a specific version.
    pub fn at_version(entries: &[CommitEntry], version: u64) -> Option<SchemaSnapshot> {
        let relevant: Vec<_> = entries.iter().filter(|e| e.version <= version).collect();
        for entry in relevant.iter().rev() {
            for action in entry.actions.iter().rev() {
                if let Action::MetaData(m) = action {
                    if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                        return Some(SchemaSnapshot {
                            version: entry.version,
                            timestamp: m.created_time,
                            schema,
                        });
                    }
                }
            }
        }
        None
    }
}
