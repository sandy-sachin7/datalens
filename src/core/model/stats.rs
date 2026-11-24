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

/// Active soft-deleted file tombstones retained in the Delta log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TombstoneInfo {
    pub active_tombstones: usize,
    pub reclaimable_bytes: i64,
    pub oldest_tombstone_timestamp: Option<i64>,
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
            issues.push("No completed VACUUM found across >100 versions".to_string());
        }
        if self.maintenance.last_optimize_version.is_none() && self.current_version > 100 {
            issues.push("No OPTIMIZE found across >100 versions".to_string());
        }
        if self.tombstones.active_tombstones > 1000
            && self.tombstones.reclaimable_bytes > 1024 * 1024 * 1024
        {
            issues.push(format!(
                "Large unvacuumed tombstone backlog ({} tombstones)",
                self.tombstones.active_tombstones
            ));
        }
        issues
    }
}
