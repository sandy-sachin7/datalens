use serde::{Deserialize, Serialize};

/// Aggregated health metrics for a Delta table's current file set.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Maintenance state extracted from commit metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaintenanceInfo {
    pub last_vacuum_version: Option<u64>,
    pub last_vacuum_timestamp: Option<i64>,
    pub last_optimize_version: Option<u64>,
    pub last_optimize_timestamp: Option<i64>,
    pub last_checkpoint_version: Option<u64>,
    pub z_order_columns: Vec<String>,
}

/// Full table inspection result combining all analyzers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStats {
    pub current_version: u64,
    pub created_timestamp: Option<i64>,
    pub last_modified_timestamp: Option<i64>,
    pub min_reader_version: i32,
    pub min_writer_version: i32,
    pub partition_columns: Vec<String>,
    pub partition_count: usize,
    pub empty_partition_count: usize,
    pub schema_column_count: usize,
    pub schema_change_count: usize,
    pub health: TableHealth,
    pub maintenance: MaintenanceInfo,
}
