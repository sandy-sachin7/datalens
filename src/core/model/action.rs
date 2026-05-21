use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Every line in a Delta commit JSON file is one of these.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Add(AddAction),
    Remove(RemoveAction),
    MetaData(MetaDataAction),
    Protocol(ProtocolAction),
    CommitInfo(CommitInfoAction),
    Cdc(CdcAction),
}

/// A file being added to the table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAction {
    pub path: String,
    pub size: i64,
    pub modification_time: i64,
    pub data_change: bool,
    pub partition_values: HashMap<String, Option<String>>,
    /// JSON string with row counts and min/max per column.
    pub stats: Option<String>,
    pub tags: Option<HashMap<String, String>>,
}

/// A file being logically deleted (soft delete).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveAction {
    pub path: String,
    pub deletion_timestamp: Option<i64>,
    pub data_change: bool,
    pub size: Option<i64>,
    pub partition_values: Option<HashMap<String, Option<String>>>,
}

/// Table-level metadata — schema, partitioning, configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaDataAction {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub format: Format,
    /// JSON schema string; needs further parsing via schema module.
    pub schema_string: String,
    pub partition_columns: Vec<String>,
    pub configuration: HashMap<String, String>,
    pub created_time: Option<i64>,
}

/// Protocol version requirements.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAction {
    pub min_reader_version: i32,
    pub min_writer_version: i32,
    pub reader_features: Option<Vec<String>>,
    pub writer_features: Option<Vec<String>>,
}

/// Per-commit metadata — operation, parameters, notebook/job info.
/// This is the primary source for lineage attribution.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfoAction {
    pub timestamp: i64,
    pub operation: String,
    pub operation_parameters: HashMap<String, serde_json::Value>,
    pub notebook_info: Option<NotebookInfo>,
    pub cluster_id: Option<String>,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub operation_metrics: Option<HashMap<String, serde_json::Value>>,
    pub engine_info: Option<String>,
    pub is_blind_append: Option<bool>,
    pub txn_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookInfo {
    pub notebook_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdcAction {
    pub path: String,
    pub size: i64,
    pub partition_values: HashMap<String, Option<String>>,
    pub data_change: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Format {
    pub provider: String,
}
