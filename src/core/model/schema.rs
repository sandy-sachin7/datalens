use serde::{Deserialize, Serialize};

/// Root of a Delta schema — always a struct type at the top level.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DataType {
    #[serde(rename = "string")]
    String,
    #[serde(rename = "long")]
    Long,
    #[serde(rename = "integer")]
    Integer,
    #[serde(rename = "short")]
    Short,
    #[serde(rename = "byte")]
    Byte,
    #[serde(rename = "float")]
    Float,
    #[serde(rename = "double")]
    Double,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "binary")]
    Binary,
    #[serde(rename = "date")]
    Date,
    #[serde(rename = "timestamp")]
    Timestamp,
    #[serde(other)]
    Other,
}

/// A single column in a Delta schema.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StructField {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: serde_json::Value, // flexible — can be primitive string or nested object
    pub nullable: bool,
    pub metadata: serde_json::Value,
}

/// The top-level schema struct.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StructType {
    #[serde(rename = "type")]
    pub type_name: String, // always "struct"
    pub fields: Vec<StructField>,
}

impl StructType {
    /// Parse from the raw schemaString JSON stored in MetaDataAction.
    pub fn from_schema_string(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    #[allow(dead_code)]
    pub fn column_names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_str()).collect()
    }

    #[allow(dead_code)]
    pub fn nullable_count(&self) -> usize {
        self.fields.iter().filter(|f| f.nullable).count()
    }
}

/// A point-in-time schema snapshot tied to a table version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    pub version: u64,
    pub timestamp: Option<i64>,
    pub schema: StructType,
}

/// A schema change event between two consecutive versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaChange {
    pub version: u64,
    pub timestamp: Option<i64>,
    pub added_columns: Vec<String>,
    pub removed_columns: Vec<String>,
    pub modified_columns: Vec<(String, String, String)>, // (name, old_type, new_type)
}
