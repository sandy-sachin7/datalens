use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

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

/// Real Delta Lake data type representation supporting primitives and nested types.
#[derive(Debug, Clone, PartialEq)]
pub enum DataType {
    Primitive(String),
    Struct(StructType),
    Array {
        element_type: Box<DataType>,
        contains_null: bool,
    },
    Map {
        key_type: Box<DataType>,
        value_type: Box<DataType>,
        value_contains_null: bool,
    },
}

impl DataType {
    /// Parse a Delta JSON `type` value (either a primitive string or complex object).
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::String(s) => Ok(DataType::Primitive(s.clone())),
            serde_json::Value::Object(map) => {
                let kind = map
                    .get("type")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Complex DataType object missing string 'type'".to_string())?;
                match kind {
                    "struct" => {
                        let st: StructType = serde_json::from_value(value.clone())
                            .map_err(|e| format!("Invalid struct type: {}", e))?;
                        Ok(DataType::Struct(st))
                    }
                    "array" => {
                        let elem_val = map
                            .get("elementType")
                            .ok_or_else(|| "Array type missing 'elementType'".to_string())?;
                        let contains_null = map
                            .get("containsNull")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        Ok(DataType::Array {
                            element_type: Box::new(Self::from_json_value(elem_val)?),
                            contains_null,
                        })
                    }
                    "map" => {
                        let key_val = map
                            .get("keyType")
                            .ok_or_else(|| "Map type missing 'keyType'".to_string())?;
                        let val_val = map
                            .get("valueType")
                            .ok_or_else(|| "Map type missing 'valueType'".to_string())?;
                        let value_contains_null = map
                            .get("valueContainsNull")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        Ok(DataType::Map {
                            key_type: Box::new(Self::from_json_value(key_val)?),
                            value_type: Box::new(Self::from_json_value(val_val)?),
                            value_contains_null,
                        })
                    }
                    other => Ok(DataType::Primitive(other.to_string())),
                }
            }
            other => Ok(DataType::Primitive(other.to_string())),
        }
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            DataType::Primitive(s) => serde_json::Value::String(s.clone()),
            DataType::Struct(st) => serde_json::to_value(st).unwrap_or(serde_json::Value::Null),
            DataType::Array {
                element_type,
                contains_null,
            } => serde_json::json!({
                "type": "array",
                "elementType": element_type.to_json_value(),
                "containsNull": contains_null,
            }),
            DataType::Map {
                key_type,
                value_type,
                value_contains_null,
            } => serde_json::json!({
                "type": "map",
                "keyType": key_type.to_json_value(),
                "valueType": value_type.to_json_value(),
                "valueContainsNull": value_contains_null,
            }),
        }
    }

    /// Clean, human-readable type name without surrounding JSON quotes.
    pub fn display_name(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Primitive(s) => write!(f, "{}", s),
            DataType::Struct(st) => {
                write!(f, "struct<")?;
                for (i, field) in st.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", field.name, field.data_type)?;
                }
                write!(f, ">")
            }
            DataType::Array { element_type, .. } => write!(f, "array<{}>", element_type),
            DataType::Map {
                key_type,
                value_type,
                ..
            } => write!(f, "map<{}, {}>", key_type, value_type),
        }
    }
}

impl Serialize for DataType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_json_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DataType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        DataType::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

/// A single column in a Delta schema.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StructField {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: DataType,
    #[serde(default = "default_true", deserialize_with = "deserialize_null_true")]
    pub nullable: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub metadata: serde_json::Value,
}

impl StructField {
    /// Extract `delta.columnMapping.id` from field metadata if Column Mapping is enabled.
    pub fn column_mapping_id(&self) -> Option<i64> {
        self.metadata
            .as_object()?
            .get("delta.columnMapping.id")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
            })
    }

    /// Extract `delta.columnMapping.physicalName` from field metadata if present.
    pub fn physical_name(&self) -> Option<&str> {
        self.metadata
            .as_object()?
            .get("delta.columnMapping.physicalName")
            .and_then(|v| v.as_str())
    }
}

/// The top-level schema struct.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StructType {
    #[serde(rename = "type", default = "default_struct_type")]
    pub type_name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub fields: Vec<StructField>,
}

fn default_struct_type() -> String {
    "struct".to_string()
}

impl StructType {
    /// Parse from the raw schemaString JSON stored in MetaDataAction.
    pub fn from_schema_string(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn column_names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_str()).collect()
    }

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnRename {
    pub old_name: String,
    pub new_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnModification {
    pub name: String,
    pub old_type: String,
    pub new_type: String,
    pub breaking: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NullabilityChange {
    pub name: String,
    pub old_nullable: bool,
    pub new_nullable: bool,
    pub breaking: bool,
}

/// A schema change event between two versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaChange {
    pub version: u64,
    pub timestamp: Option<i64>,
    pub added_columns: Vec<String>,
    pub removed_columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamed_columns: Vec<ColumnRename>,
    pub modified_columns: Vec<ColumnModification>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nullability_changes: Vec<NullabilityChange>,
    pub has_breaking_changes: bool,
}

impl SchemaChange {
    pub fn is_empty(&self) -> bool {
        self.added_columns.is_empty()
            && self.removed_columns.is_empty()
            && self.renamed_columns.is_empty()
            && self.modified_columns.is_empty()
            && self.nullability_changes.is_empty()
    }
}
