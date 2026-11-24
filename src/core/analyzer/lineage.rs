use crate::core::model::action::{Action, CommitInfoAction};
use crate::core::model::lineage::{LineageEntry, OperationType};
use crate::core::reader::CommitEntry;
use crate::core::replay::{canonicalize_path, FileKey};
use std::collections::HashMap;

pub struct LineageTracer;

impl LineageTracer {
    /// Reconstruct lineage from a slice of commit entries.
    /// Emits an entry for every commit (synthesizing one when `commitInfo` is absent)
    /// and resolves missing `Remove.size` from earlier `Add` actions seen in the slice.
    pub fn trace(entries: &[CommitEntry]) -> Vec<LineageEntry> {
        let mut known_sizes: HashMap<FileKey, i64> = HashMap::new();
        entries
            .iter()
            .map(|entry| Self::entry_lineage(entry, &mut known_sizes))
            .collect()
    }

    fn entry_lineage(entry: &CommitEntry, known_sizes: &mut HashMap<FileKey, i64>) -> LineageEntry {
        let ci = entry.actions.iter().find_map(|a| match a {
            Action::CommitInfo(ci) => Some(ci),
            _ => None,
        });

        let mut files_added = 0i64;
        let mut files_removed = 0i64;
        let mut bytes_added = 0i64;
        let mut bytes_removed = 0i64;
        let mut physical_files_added = 0i64;
        let mut physical_files_removed = 0i64;
        let mut physical_bytes_added = 0i64;
        let mut physical_bytes_removed = 0i64;
        let mut stats_rows_sum = 0i64;
        let mut has_stats_rows = false;
        let mut fallback_ts: Option<i64> = None;

        // First tally removes (using known_sizes from earlier commits), then adds.
        for action in &entry.actions {
            match action {
                Action::Remove(r) => {
                    let key = FileKey::from_remove(r);
                    let prior_size = known_sizes.remove(&key);
                    let sz = r.size.or(prior_size).unwrap_or(0);
                    physical_files_removed += 1;
                    physical_bytes_removed += sz;
                    if r.data_change {
                        files_removed += 1;
                        bytes_removed += sz;
                    }
                    if let Some(ts) = r.deletion_timestamp {
                        if ts > 0 {
                            fallback_ts = Some(fallback_ts.map_or(ts, |cur| cur.max(ts)));
                        }
                    }
                }
                Action::MetaData(m) => {
                    if let Some(ts) = m.created_time {
                        if ts > 0 {
                            fallback_ts = Some(fallback_ts.map_or(ts, |cur| cur.max(ts)));
                        }
                    }
                }
                _ => {}
            }
        }

        for action in &entry.actions {
            if let Action::Add(a) = action {
                let key = FileKey {
                    path: canonicalize_path(&a.path),
                    dv_unique_id: a.deletion_vector.as_ref().map(|dv| dv.unique_id()),
                };
                known_sizes.insert(key, a.size);

                physical_files_added += 1;
                physical_bytes_added += a.size;
                if a.data_change {
                    files_added += 1;
                    bytes_added += a.size;
                    if let Some(nr) = a.num_records_from_stats() {
                        stats_rows_sum += nr;
                        has_stats_rows = true;
                    }
                }
                if a.modification_time > 0 {
                    fallback_ts = Some(
                        fallback_ts.map_or(a.modification_time, |cur| cur.max(a.modification_time)),
                    );
                }
            }
        }

        if let Some(ci) = ci {
            let num_output_rows =
                Self::extract_row_count(ci).or(has_stats_rows.then_some(stats_rows_sum));
            let writer = Self::build_writer_string(ci);
            let predicate = Self::extract_predicate(ci);
            let op_raw = if ci.operation.is_empty() {
                Self::infer_operation(entry)
            } else {
                ci.operation.clone()
            };
            let operation = op_raw
                .parse::<OperationType>()
                .unwrap_or_else(|_| OperationType::Other(op_raw.clone()));
            let timestamp = if ci.timestamp > 0 {
                Some(ci.timestamp)
            } else {
                fallback_ts
            };

            LineageEntry {
                version: entry.version,
                timestamp,
                operation,
                operation_raw: op_raw,
                writer,
                num_files_added: files_added,
                num_files_removed: files_removed,
                bytes_added,
                bytes_removed,
                physical_files_added,
                physical_files_removed,
                physical_bytes_added,
                physical_bytes_removed,
                num_output_rows,
                predicate,
                user_name: ci.user_name.clone(),
                cluster_id: ci.cluster_id.clone(),
                engine_info: ci.engine_info.clone(),
                has_commit_info: true,
            }
        } else {
            let op_raw = Self::infer_operation(entry);
            let operation = op_raw
                .parse::<OperationType>()
                .unwrap_or_else(|_| OperationType::Other(op_raw.clone()));
            LineageEntry {
                version: entry.version,
                timestamp: fallback_ts,
                operation,
                operation_raw: op_raw,
                writer: "unknown (no commitInfo)".to_string(),
                num_files_added: files_added,
                num_files_removed: files_removed,
                bytes_added,
                bytes_removed,
                physical_files_added,
                physical_files_removed,
                physical_bytes_added,
                physical_bytes_removed,
                num_output_rows: if has_stats_rows {
                    Some(stats_rows_sum)
                } else {
                    None
                },
                predicate: None,
                user_name: None,
                cluster_id: None,
                engine_info: None,
                has_commit_info: false,
            }
        }
    }

    fn infer_operation(entry: &CommitEntry) -> String {
        let mut has_meta = false;
        let mut has_proto = false;
        let mut has_logical_add = false;
        let mut has_logical_remove = false;
        let mut has_physical_only = false;

        for a in &entry.actions {
            match a {
                Action::MetaData(_) => has_meta = true,
                Action::Protocol(_) => has_proto = true,
                Action::Add(add) => {
                    if add.data_change {
                        has_logical_add = true;
                    } else {
                        has_physical_only = true;
                    }
                }
                Action::Remove(rem) => {
                    if rem.data_change {
                        has_logical_remove = true;
                    } else {
                        has_physical_only = true;
                    }
                }
                _ => {}
            }
        }

        if entry.version == 0 && (has_meta || has_proto) {
            "CREATE TABLE".to_string()
        } else if !has_logical_add && !has_logical_remove && has_physical_only {
            "OPTIMIZE".to_string()
        } else if has_logical_remove && !has_logical_add {
            "DELETE".to_string()
        } else if has_logical_add {
            "WRITE".to_string()
        } else if has_meta || has_proto {
            "CHANGE METADATA".to_string()
        } else {
            "UNKNOWN".to_string()
        }
    }

    fn extract_row_count(ci: &CommitInfoAction) -> Option<i64> {
        let metrics = ci.operation_metrics.as_ref()?;
        let get_i64 = |key: &str| -> Option<i64> {
            metrics.get(key).and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
            })
        };

        // 1. Standard Spark WRITE & delta-rs WRITE
        if let Some(n) = get_i64("numOutputRows").or_else(|| get_i64("num_added_rows")) {
            return Some(n);
        }

        // 2. Spark MERGE (net rows inserted - deleted, or total affected if updated)
        let inserted = get_i64("numTargetRowsInserted");
        let deleted = get_i64("numTargetRowsDeleted");
        let updated = get_i64("numTargetRowsUpdated");
        if inserted.is_some() || deleted.is_some() || updated.is_some() {
            let net = inserted.unwrap_or(0) - deleted.unwrap_or(0);
            if net != 0 || updated.unwrap_or(0) == 0 {
                return Some(net);
            }
            return updated;
        }

        // 3. Spark / delta-rs DELETE (negate to show rows removed)
        if let Some(del) = get_i64("numDeletedRows").or_else(|| get_i64("num_deleted_rows")) {
            return Some(-del.abs());
        }

        // 4. Spark / delta-rs UPDATE
        if let Some(upd) = get_i64("numUpdatedRows").or_else(|| get_i64("num_updated_rows")) {
            return Some(upd);
        }

        None
    }

    fn extract_predicate(ci: &CommitInfoAction) -> Option<String> {
        let val = ci
            .operation_parameters
            .get("predicate")
            .or_else(|| ci.operation_parameters.get("condition"))?;

        match val {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() || trimmed == "[]" {
                    return None;
                }
                // Spark often JSON-encodes predicate arrays as a string: "[\"id > 10\"]"
                if trimmed.starts_with('[') {
                    if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
                        if arr.is_empty() {
                            return None;
                        }
                        return Some(arr.join(" AND "));
                    }
                }
                Some(trimmed.to_string())
            }
            serde_json::Value::Array(arr) => {
                let parts: Vec<String> = arr
                    .iter()
                    .filter_map(|item| match item {
                        serde_json::Value::String(s) if !s.trim().is_empty() => {
                            Some(s.trim().to_string())
                        }
                        serde_json::Value::Null => None,
                        other => Some(other.to_string()),
                    })
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(" AND "))
                }
            }
            other => Some(other.to_string()),
        }
    }

    fn build_writer_string(ci: &CommitInfoAction) -> String {
        // Priority: job > notebook > engineInfo > userName > "unknown"
        if let Some(job) = &ci.job {
            if let Some(name) = job.job_name.as_deref().filter(|s| !s.is_empty()) {
                return format!("job:{}", name);
            }
            if let Some(id) = job.job_id.as_deref().filter(|s| !s.is_empty()) {
                return format!("job_{}", id);
            }
        }

        if let Some(notebook) = &ci.notebook_info {
            if let Some(id) = notebook.notebook_id.as_deref().filter(|s| !s.is_empty()) {
                // UTF-8 safe char truncation (never slices inside a multi-byte char!)
                let short: String = id.chars().take(8).collect();
                return format!("notebook_{}", short);
            }
        }

        if let Some(engine) = &ci.engine_info {
            let parts: Vec<&str> = engine.split_whitespace().collect();
            if let Some(first) = parts.first() {
                let name = first.split('/').next().unwrap_or(first).to_lowercase();
                if let Some(user) = &ci.user_name {
                    return format!("{}/{}", name, user);
                }
                return name;
            }
        }

        if let Some(user) = &ci.user_name {
            return user.clone();
        }

        "unknown".to_string()
    }
}

/// Filter lineage entries by optional criteria.
pub struct LineageFilter {
    pub since_timestamp: Option<i64>,
    pub until_timestamp: Option<i64>,
    pub operations: Option<Vec<String>>,
    pub user: Option<String>,
}

impl LineageFilter {
    pub fn apply<'a>(&self, entries: &'a [LineageEntry]) -> Vec<&'a LineageEntry> {
        entries
            .iter()
            .filter(|e| {
                if let Some(since) = self.since_timestamp {
                    match e.timestamp {
                        Some(ts) if ts >= since => {}
                        _ => return false,
                    }
                }
                if let Some(until) = self.until_timestamp {
                    match e.timestamp {
                        Some(ts) if ts <= until => {}
                        _ => return false,
                    }
                }
                if let Some(ops) = &self.operations {
                    let op_upper = e.operation_raw.to_uppercase();
                    if !ops.iter().any(|o| o.to_uppercase() == op_upper) {
                        return false;
                    }
                }
                if let Some(user) = &self.user {
                    let writer_lower = e.writer.to_lowercase();
                    let user_lower = user.to_lowercase();
                    if !writer_lower.contains(&user_lower)
                        && e.user_name
                            .as_deref()
                            .map(|u| !u.to_lowercase().contains(&user_lower))
                            .unwrap_or(true)
                    {
                        return false;
                    }
                }
                true
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::action::{AddAction, JobInfo, NotebookInfo};

    #[test]
    fn test_lineage_synthetic_entry_and_stats_fallback() {
        let entries = vec![CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![Action::Add(AddAction {
                path: "p0.parquet".to_string(),
                size: 512,
                modification_time: 1700000000000,
                data_change: true,
                partition_values: HashMap::new(),
                stats: Some(r#"{"numRecords":42}"#.to_string()),
                tags: None,
                deletion_vector: None,
            })],
        }];

        let traced = LineageTracer::trace(&entries);
        assert_eq!(traced.len(), 1);
        assert!(!traced[0].has_commit_info);
        assert_eq!(traced[0].num_output_rows, Some(42));
        assert_eq!(traced[0].timestamp, Some(1700000000000));
        assert_eq!(traced[0].operation_raw, "WRITE");
    }

    #[test]
    fn test_lineage_delta_rs_databricks_job_notebook_utf8_and_array_predicate() {
        let mut metrics = HashMap::new();
        metrics.insert("num_added_rows".to_string(), serde_json::json!(250));
        let mut params = HashMap::new();
        params.insert(
            "predicate".to_string(),
            serde_json::json!(["country = 'IN'", "active = true"]),
        );

        let entries = vec![
            CommitEntry {
                version: 1,
                path: "1.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 1700000000000,
                    operation: "WRITE".to_string(),
                    operation_parameters: params,
                    notebook_info: None,
                    job: Some(JobInfo {
                        job_id: Some("99".to_string()),
                        job_name: Some("daily_orders".to_string()),
                        run_id: None,
                    }),
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: Some(metrics),
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
            CommitEntry {
                version: 2,
                path: "2.json".to_string(),
                actions: vec![Action::CommitInfo(CommitInfoAction {
                    timestamp: 1700000005000,
                    operation: "DELETE".to_string(),
                    operation_parameters: HashMap::new(),
                    // Multi-byte UTF-8 notebook ID must never panic on truncation
                    notebook_info: Some(NotebookInfo {
                        notebook_id: Some("🚀αβγδεζηθικλ".to_string()),
                    }),
                    job: None,
                    cluster_id: None,
                    user_id: None,
                    user_name: None,
                    operation_metrics: Some(HashMap::from([(
                        "numDeletedRows".to_string(),
                        serde_json::json!("15"),
                    )])),
                    engine_info: None,
                    is_blind_append: None,
                    txn_id: None,
                })],
            },
        ];

        let traced = LineageTracer::trace(&entries);
        assert_eq!(traced[0].writer, "job:daily_orders");
        assert_eq!(traced[0].num_output_rows, Some(250));
        assert_eq!(
            traced[0].predicate.as_deref(),
            Some("country = 'IN' AND active = true")
        );

        assert_eq!(traced[1].writer, "notebook_🚀αβγδεζη");
        assert_eq!(traced[1].num_output_rows, Some(-15));
    }
}
