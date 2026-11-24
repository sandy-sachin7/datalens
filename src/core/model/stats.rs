use serde::{Deserialize, Serialize};

/// Aggregated health metrics for a Delta table's current file set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableHealth {
    pub total_files: usize,
    pub total_size: i64,
    pub avg_file_size: i64,
    pub min_file_size: i64,
    pub max_file_size: i64,
    pub small_file_count: usize,
    pub small_file_pct: f64,
    /// Coefficient of variation, normalized to [0, 1].
    /// 0.0 = perfect uniformity, 1.0 = extreme skew.
    pub skew_score: f64,
    /// Number of active files carrying a Delta Deletion Vector.
    pub deletion_vector_files: usize,
    /// Sum of deleted rows referenced by active Deletion Vectors.
    pub deleted_rows_in_dvs: i64,
}

/// Default Delta Lake deleted file retention duration: 7 days in milliseconds.
pub const DEFAULT_DELETED_FILE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Parse standard Delta Lake / Spark SQL interval syntax into milliseconds.
/// Supports strings such as `"interval 7 days"`, `"interval 168 hours"`,
/// `"interval 1 day 12 hours"`, `"interval 30 minutes"`, `"interval 2 weeks"`, etc.
pub fn parse_delta_interval_ms(input: &str) -> Option<i64> {
    let mut iter = input.split_whitespace().peekable();
    if iter
        .peek()
        .is_some_and(|tok| tok.eq_ignore_ascii_case("interval"))
    {
        iter.next();
    }

    let mut total_ms = 0i64;
    let mut saw_pair = false;
    while let Some(amount_str) = iter.next() {
        let unit = iter.next()?;
        let amount = amount_str.parse::<i64>().ok()?;
        if amount < 0 {
            return None;
        }
        let unit_ms: i64 = match unit.to_ascii_lowercase().as_str() {
            "week" | "weeks" | "w" => 7 * 24 * 60 * 60 * 1000,
            "day" | "days" | "d" => 24 * 60 * 60 * 1000,
            "hour" | "hours" | "hr" | "hrs" | "h" => 60 * 60 * 1000,
            "minute" | "minutes" | "min" | "mins" | "m" => 60 * 1000,
            "second" | "seconds" | "sec" | "secs" | "s" => 1000,
            "millisecond" | "milliseconds" | "ms" => 1,
            _ => return None,
        };
        total_ms = total_ms.saturating_add(amount.saturating_mul(unit_ms));
        saw_pair = true;
    }

    saw_pair.then_some(total_ms)
}

/// Active soft-deleted file tombstones retained in the Delta log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TombstoneInfo {
    pub active_tombstones: usize,
    pub reclaimable_bytes: i64,
    pub oldest_tombstone_timestamp: Option<i64>,
    #[serde(default)]
    pub expired_tombstones: usize,
    #[serde(default)]
    pub expired_reclaimable_bytes: i64,
    #[serde(default)]
    pub retention_duration_ms: i64,
}

/// Maintenance state extracted from commit metadata and log discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaintenanceInfo {
    pub last_vacuum_version: Option<u64>,
    pub last_vacuum_timestamp: Option<i64>,
    pub last_optimize_version: Option<u64>,
    pub last_optimize_timestamp: Option<i64>,
    pub last_checkpoint_version: Option<u64>,
    pub z_order_columns: Vec<String>,
    pub clustering_columns: Vec<String>,
}

/// Full table inspection result combining all analyzers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStats {
    pub current_version: u64,
    pub created_timestamp: Option<i64>,
    pub last_modified_timestamp: Option<i64>,
    pub min_reader_version: i32,
    pub min_writer_version: i32,
    pub reader_features: Vec<String>,
    pub writer_features: Vec<String>,
    pub partition_columns: Vec<String>,
    pub partition_count: usize,
    pub empty_partition_count: usize,
    pub schema_column_count: usize,
    pub schema_change_count: usize,
    pub health: TableHealth,
    pub tombstones: TombstoneInfo,
    pub maintenance: MaintenanceInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl TableStats {
    /// Single source of truth for actionable health issues (shared by renderer & `--fail-on-issues`).
    pub fn health_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.health.skew_score > 0.5 {
            issues.push(format!(
                "High file size skew ({:.2} / 1.0)",
                self.health.skew_score
            ));
        }
        // Suppress small-file warning on single-file (or empty) tables.
        if self.health.total_files > 1 && self.health.small_file_pct > 20.0 {
            issues.push(format!(
                "High small-file ratio: {} of {} files ({:.1}%) are < 32 MB",
                self.health.small_file_count, self.health.total_files, self.health.small_file_pct
            ));
        }
        if self.maintenance.last_vacuum_version.is_none() && self.current_version > 100 {
            match self.maintenance.last_checkpoint_version {
                None => {
                    issues.push("No completed VACUUM found across >100 versions".to_string());
                }
                Some(ckpt_v) => {
                    if self.tombstones.expired_tombstones > 0 {
                        issues.push(format!(
                            "No completed VACUUM observed since checkpoint v{:03} ({} expired tombstones exceed retention)",
                            ckpt_v, self.tombstones.expired_tombstones
                        ));
                    }
                }
            }
        }
        if self.maintenance.last_optimize_version.is_none() && self.current_version > 100 {
            let uninspected_horizon = self
                .maintenance
                .last_checkpoint_version
                .is_some_and(|ckpt_v| self.current_version.saturating_sub(ckpt_v) <= 100);
            if !uninspected_horizon {
                issues.push("No OPTIMIZE found across >100 versions".to_string());
            }
        }
        if self.tombstones.expired_tombstones > 1000
            && self.tombstones.expired_reclaimable_bytes > 1024 * 1024 * 1024
        {
            issues.push(format!(
                "Large expired tombstone backlog ({} tombstones exceeding retention)",
                self.tombstones.expired_tombstones
            ));
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_delta_interval_ms() {
        assert_eq!(
            parse_delta_interval_ms("interval 7 days"),
            Some(DEFAULT_DELETED_FILE_RETENTION_MS)
        );
        assert_eq!(
            parse_delta_interval_ms("INTERVAL 168 HOURS"),
            Some(DEFAULT_DELETED_FILE_RETENTION_MS)
        );
        assert_eq!(
            parse_delta_interval_ms("interval 1 day 12 hours"),
            Some(36 * 60 * 60 * 1000)
        );
        assert_eq!(
            parse_delta_interval_ms("interval 30 minutes"),
            Some(30 * 60 * 1000)
        );
        assert_eq!(
            parse_delta_interval_ms("interval 2 weeks"),
            Some(14 * 24 * 60 * 60 * 1000)
        );
        assert_eq!(parse_delta_interval_ms("interval 0 hours"), Some(0));
        assert_eq!(parse_delta_interval_ms(""), None);
        assert_eq!(parse_delta_interval_ms("interval"), None);
        assert_eq!(parse_delta_interval_ms("interval -5 days"), None);
        assert_eq!(parse_delta_interval_ms("interval 7 fortnights"), None);
    }
}
