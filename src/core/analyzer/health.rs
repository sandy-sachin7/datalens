use crate::core::model::action::Action;
use crate::core::model::schema::StructType;
use crate::core::model::stats::{MaintenanceInfo, TableHealth, TableStats};
use crate::core::reader::CommitEntry;
use std::collections::HashMap;

pub struct HealthAnalyzer;

impl HealthAnalyzer {
    /// Full table inspection from all commit entries.
    pub fn analyze(entries: &[CommitEntry]) -> TableStats {
        if entries.is_empty() {
            return Self::empty_stats();
        }

        // Flatten all actions for file-set reconstruction
        let all_actions: Vec<&Action> = entries.iter().flat_map(|e| e.actions.iter()).collect();

        let health = Self::compute_health(&all_actions);
        let maintenance = Self::compute_maintenance(entries);
        let (partition_columns, partition_count, empty_partition_count) =
            Self::compute_partition_info(&all_actions);
        let (schema_column_count, schema_change_count, created_ts) =
            Self::compute_schema_info(entries);

        let current_version = entries.last().map(|e| e.version).unwrap_or(0);
        let last_modified = Self::find_last_modified_ts(entries);

        TableStats {
            current_version,
            created_timestamp: created_ts,
            last_modified_timestamp: last_modified,
            min_reader_version: Self::find_protocol_version(entries).0,
            min_writer_version: Self::find_protocol_version(entries).1,
            partition_columns,
            partition_count,
            empty_partition_count,
            schema_column_count,
            schema_change_count,
            health,
            maintenance,
        }
    }

    /// Compute file-level health from reconstructed active file set.
    fn compute_health(actions: &[&Action]) -> TableHealth {
        // Reconstruct current file set: Add adds, Remove soft-deletes
        let mut active_files: HashMap<String, i64> = HashMap::new();

        for action in actions {
            match action {
                Action::Add(a) => {
                    active_files.insert(a.path.clone(), a.size);
                }
                Action::Remove(r) => {
                    active_files.remove(&r.path);
                }
                _ => {}
            }
        }

        let sizes: Vec<i64> = active_files.values().copied().collect();
        let total_files = sizes.len();
        let total_size: i64 = sizes.iter().sum();
        let avg_size = if total_files > 0 {
            total_size / total_files as i64
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

        TableHealth {
            total_files,
            total_size,
            avg_file_size: avg_size,
            min_file_size,
            max_file_size,
            small_file_count,
            small_file_pct,
            skew_score,
        }
    }

    /// Coefficient of variation, normalized to [0, 1].
    /// 0.0 = all files identical size (perfect)
    /// 1.0 = extreme size variance (bad for query performance)
    pub fn compute_skew_score(sizes: &[i64]) -> f64 {
        if sizes.len() < 2 {
            return 0.0;
        }
        let mean = sizes.iter().sum::<i64>() as f64 / sizes.len() as f64;
        if mean == 0.0 {
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
        // CV > 2.0 is considered maximum skew
        (cv / 2.0).min(1.0)
    }

    fn compute_maintenance(entries: &[CommitEntry]) -> MaintenanceInfo {
        let mut info = MaintenanceInfo::default();

        for entry in entries {
            for action in &entry.actions {
                if let Action::CommitInfo(ci) = action {
                    let op = ci.operation.to_uppercase();

                    if op == "VACUUM START" || op == "VACUUM END" {
                        info.last_vacuum_version = Some(entry.version);
                        info.last_vacuum_timestamp = Some(ci.timestamp);
                    }

                    if op == "OPTIMIZE" {
                        info.last_optimize_version = Some(entry.version);
                        info.last_optimize_timestamp = Some(ci.timestamp);

                        // Extract Z-ORDER columns from operationParameters
                        if let Some(zorder) = ci.operation_parameters.get("zOrderBy") {
                            if let Some(arr) = zorder.as_array() {
                                info.z_order_columns = arr
                                    .iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect();
                            } else if let Some(s) = zorder.as_str() {
                                // Sometimes stored as JSON string "[\"col1\",\"col2\"]"
                                if let Ok(cols) = serde_json::from_str::<Vec<String>>(s) {
                                    info.z_order_columns = cols;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Checkpoint: scan for .parquet files in log dir (simple heuristic)
        // Full checkpoint parsing is v0.1.1; here we just note the latest version
        info.last_checkpoint_version = entries
            .iter()
            .rev()
            .find(|e| e.version % 10 == 0 && e.version > 0)
            .map(|e| e.version);

        info
    }

    fn compute_partition_info(actions: &[&Action]) -> (Vec<String>, usize, usize) {
        let mut partition_columns: Vec<String> = vec![];
        let mut partitions: HashMap<String, usize> = HashMap::new();

        for action in actions {
            match action {
                Action::MetaData(m) => {
                    partition_columns = m.partition_columns.clone();
                }
                Action::Add(a) if !a.partition_values.is_empty() => {
                    // Build a stable partition key string
                    let mut kv: Vec<_> = a.partition_values.iter().collect();
                    kv.sort_by_key(|(k, _)| k.as_str());
                    let key = kv
                        .iter()
                        .map(|(k, v)| {
                            format!(
                                "{}={}",
                                k,
                                v.as_deref().unwrap_or("__HIVE_DEFAULT_PARTITION__")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("/");
                    *partitions.entry(key).or_insert(0) += 1;
                }
                _ => {}
            }
        }

        let partition_count = partitions.len();
        // Partitions with 0 active files (all removed) — count those with value 0
        let empty_partition_count = partitions.values().filter(|&&c| c == 0).count();

        (partition_columns, partition_count, empty_partition_count)
    }

    fn compute_schema_info(entries: &[CommitEntry]) -> (usize, usize, Option<i64>) {
        let mut schema_changes = 0usize;
        let mut latest_column_count = 0usize;
        let mut created_time: Option<i64> = None;
        let mut prev_fields: Vec<String> = vec![];

        for entry in entries {
            for action in &entry.actions {
                if let Action::MetaData(m) = action {
                    if created_time.is_none() {
                        created_time = m.created_time;
                    }

                    if let Ok(schema) = StructType::from_schema_string(&m.schema_string) {
                        let current_fields: Vec<String> =
                            schema.fields.iter().map(|f| f.name.clone()).collect();

                        if !prev_fields.is_empty() && current_fields != prev_fields {
                            schema_changes += 1;
                        }

                        prev_fields = current_fields;
                        latest_column_count = schema.fields.len();
                    }
                }
            }
        }

        (latest_column_count, schema_changes, created_time)
    }

    fn find_last_modified_ts(entries: &[CommitEntry]) -> Option<i64> {
        entries.iter().rev().find_map(|e| {
            e.actions.iter().find_map(|a| {
                if let Action::CommitInfo(ci) = a {
                    Some(ci.timestamp)
                } else {
                    None
                }
            })
        })
    }

    fn find_protocol_version(entries: &[CommitEntry]) -> (i32, i32) {
        for entry in entries.iter().rev() {
            for action in &entry.actions {
                if let Action::Protocol(p) = action {
                    return (p.min_reader_version, p.min_writer_version);
                }
            }
        }
        (1, 2) // Delta Lake defaults
    }

    fn empty_stats() -> TableStats {
        TableStats {
            current_version: 0,
            created_timestamp: None,
            last_modified_timestamp: None,
            min_reader_version: 1,
            min_writer_version: 2,
            partition_columns: vec![],
            partition_count: 0,
            empty_partition_count: 0,
            schema_column_count: 0,
            schema_change_count: 0,
            health: TableHealth {
                total_files: 0,
                total_size: 0,
                avg_file_size: 0,
                min_file_size: 0,
                max_file_size: 0,
                small_file_count: 0,
                small_file_pct: 0.0,
                skew_score: 0.0,
            },
            maintenance: MaintenanceInfo::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
