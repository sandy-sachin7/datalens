use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

fn default_true() -> bool {
    true
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

fn deserialize_null_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<bool>::deserialize(deserializer)?;
    Ok(opt.unwrap_or(true))
}

fn deserialize_opt_string_or_int<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match val {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

/// Every line in a Delta commit JSON file or row in a checkpoint is one of these.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Add(AddAction),
    Remove(RemoveAction),
    MetaData(MetaDataAction),
    Protocol(ProtocolAction),
    CommitInfo(CommitInfoAction),
    Cdc(CdcAction),
    Txn(TxnAction),
    DomainMetadata(DomainMetadataAction),
    Sidecar(SidecarAction),
    CheckpointMetadata(CheckpointMetadataAction),
}

/// Deletion vector descriptor (Delta Protocol 2.3+ / 3.x).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletionVectorDescriptor {
    pub storage_type: String,
    pub path_or_inline_dv: String,
    #[serde(default)]
    pub offset: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub size_in_bytes: i32,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub cardinality: i64,
}

impl DeletionVectorDescriptor {
    /// Protocol-defined unique identifier for keying active files with deletion vectors.
    pub fn unique_id(&self) -> String {
        match self.offset {
            Some(off) => format!("{}{}@{}", self.storage_type, self.path_or_inline_dv, off),
            None => format!("{}{}", self.storage_type, self.path_or_inline_dv),
        }
    }
}

/// A file being added to the table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAction {
    pub path: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub size: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub modification_time: i64,
    #[serde(default = "default_true", deserialize_with = "deserialize_null_true")]
    pub data_change: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub partition_values: HashMap<String, Option<String>>,
    /// JSON string with row counts and min/max per column.
    #[serde(default)]
    pub stats: Option<String>,
    #[serde(default)]
    pub tags: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub deletion_vector: Option<DeletionVectorDescriptor>,
}

impl AddAction {
    /// Extract `numRecords` from the embedded JSON `stats` string, if present.
    pub fn num_records_from_stats(&self) -> Option<i64> {
        let stats_str = self.stats.as_deref()?;
        let val: serde_json::Value = serde_json::from_str(stats_str).ok()?;
        val.get("numRecords").and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
    }
}

/// A file being logically deleted (soft delete / tombstone).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveAction {
    pub path: String,
    #[serde(default)]
    pub deletion_timestamp: Option<i64>,
    #[serde(default = "default_true", deserialize_with = "deserialize_null_true")]
    pub data_change: bool,
    #[serde(default)]
    pub extended_file_metadata: Option<bool>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub partition_values: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub tags: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub deletion_vector: Option<DeletionVectorDescriptor>,
}

/// Table-level metadata — schema, partitioning, configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaDataAction {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub format: Format,
    /// JSON schema string; parsed via schema module.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub schema_string: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub partition_columns: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub configuration: HashMap<String, Option<String>>,
    #[serde(default)]
    pub created_time: Option<i64>,
}

/// Protocol version requirements and table features.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAction {
    pub min_reader_version: i32,
    pub min_writer_version: i32,
    #[serde(default)]
    pub reader_features: Option<Vec<String>>,
    #[serde(default)]
    pub writer_features: Option<Vec<String>>,
}

/// Per-commit metadata — operation, parameters, notebook/job info.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfoAction {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub timestamp: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub operation: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub operation_parameters: HashMap<String, serde_json::Value>,
    /// Supports both Databricks `"notebook"` and `"notebookInfo"`.
    #[serde(default, alias = "notebook")]
    pub notebook_info: Option<NotebookInfo>,
    /// Databricks `"job"` metadata (`{"jobId": ..., "jobName": ...}`).
    #[serde(default)]
    pub job: Option<JobInfo>,
    #[serde(default)]
    pub cluster_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub operation_metrics: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub engine_info: Option<String>,
    #[serde(default)]
    pub is_blind_append: Option<bool>,
    #[serde(default)]
    pub txn_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookInfo {
    #[serde(default, deserialize_with = "deserialize_opt_string_or_int")]
    pub notebook_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    #[serde(default, deserialize_with = "deserialize_opt_string_or_int")]
    pub job_id: Option<String>,
    #[serde(default)]
    pub job_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_string_or_int")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdcAction {
    pub path: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub size: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub partition_values: HashMap<String, Option<String>>,
    #[serde(default = "default_true", deserialize_with = "deserialize_null_true")]
    pub data_change: bool,
    #[serde(default)]
    pub tags: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TxnAction {
    pub app_id: String,
    pub version: i64,
    #[serde(default)]
    pub last_updated: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainMetadataAction {
    pub domain: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub configuration: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub removed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarAction {
    pub path: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub size_in_bytes: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub modification_time: i64,
    #[serde(default)]
    pub tags: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointMetadataAction {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub version: i64,
    #[serde(default)]
    pub tags: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Format {
    #[serde(
        default = "default_provider",
        deserialize_with = "deserialize_null_provider"
    )]
    pub provider: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub options: HashMap<String, Option<String>>,
}

fn default_provider() -> String {
    "parquet".to_string()
}

fn deserialize_null_provider<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(default_provider))
}

impl Default for Format {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            options: HashMap::new(),
        }
    }
}
