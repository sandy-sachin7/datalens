use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The operation type performed in a commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationType {
    Write,
    Merge,
    Delete,
    Update,
    Optimize,
    VacuumStart,
    VacuumEnd,
    CreateTable,
    ReplaceTable,
    Restore,
    Other(String),
}

impl std::str::FromStr for OperationType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_uppercase().as_str() {
            "WRITE" => OperationType::Write,
            "MERGE" => OperationType::Merge,
            "DELETE" => OperationType::Delete,
            "UPDATE" => OperationType::Update,
            "OPTIMIZE" => OperationType::Optimize,
            "VACUUM START" => OperationType::VacuumStart,
            "VACUUM END" => OperationType::VacuumEnd,
            "CREATE TABLE" | "CREATE TABLE AS SELECT" => OperationType::CreateTable,
            "REPLACE TABLE" => OperationType::ReplaceTable,
            "RESTORE" => OperationType::Restore,
            other => OperationType::Other(other.to_string()),
        })
    }
}

impl OperationType {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        match self {
            OperationType::Write => "WRITE",
            OperationType::Merge => "MERGE",
            OperationType::Delete => "DELETE",
            OperationType::Update => "UPDATE",
            OperationType::Optimize => "OPTIMIZE",
            OperationType::VacuumStart => "VACUUM START",
            OperationType::VacuumEnd => "VACUUM END",
            OperationType::CreateTable => "CREATE TABLE",
            OperationType::ReplaceTable => "REPLACE TABLE",
            OperationType::Restore => "RESTORE",
            OperationType::Other(s) => s.as_str(),
        }
    }
}

/// A single lineage entry — one commit's attribution and impact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEntry {
    pub version: u64,
    pub timestamp: i64,
    pub operation: OperationType,
    pub operation_raw: String,
    /// Human-readable writer attribution: engine/job/notebook.
    pub writer: String,
    pub num_files_added: i64,
    pub num_files_removed: i64,
    pub bytes_added: i64,
    pub bytes_removed: i64,
    pub num_output_rows: Option<i64>,
    pub predicate: Option<String>,
    pub user_name: Option<String>,
    pub cluster_id: Option<String>,
    pub engine_info: Option<String>,
}

impl LineageEntry {
    #[allow(dead_code)]
    pub fn timestamp_as_datetime(&self) -> Option<DateTime<Utc>> {
        use chrono::TimeZone;
        Utc.timestamp_millis_opt(self.timestamp).single()
    }
}
