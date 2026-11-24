use crate::core::model::action::{
    Action, AddAction, CheckpointMetadataAction, DeletionVectorDescriptor, DomainMetadataAction,
    Format, MetaDataAction, ProtocolAction, RemoveAction, SidecarAction, TxnAction,
};
use crate::core::parser::commit::{ParseResult, ParseWarning};
use crate::error::DeltaLensError;
use arrow_array::{
    Array, ArrayRef, BooleanArray, Int32Array, Int64Array, LargeListArray, LargeStringArray,
    ListArray, MapArray, RecordBatch, StringArray, StructArray,
};
use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;
use std::collections::HashMap;

/// Schema of `_delta_log/_last_checkpoint`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCheckpoint {
    pub version: u64,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub parts: Option<u32>,
    #[serde(default)]
    pub size_in_bytes: Option<i64>,
    #[serde(default)]
    pub num_of_add_files: Option<i64>,
    #[serde(default)]
    pub checksum: Option<String>,
    #[serde(default)]
    pub tags: Option<HashMap<String, Option<String>>>,
}

impl LastCheckpoint {
    pub fn parse_bytes(data: &[u8]) -> Result<Self, DeltaLensError> {
        serde_json::from_slice(data)
            .map_err(|e| DeltaLensError::Checkpoint(format!("Invalid _last_checkpoint JSON: {e}")))
    }
}

/// Parse a Delta Lake Parquet checkpoint or sidecar file into typed `Action` values.
pub fn parse_parquet_checkpoint(
    data: &[u8],
    path_str: &str,
) -> Result<ParseResult, DeltaLensError> {
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(Bytes::copy_from_slice(data)).map_err(|e| {
            DeltaLensError::Checkpoint(format!(
                "Failed to open Parquet checkpoint '{}': {}",
                path_str, e
            ))
        })?;

    let reader = builder.build().map_err(|e| {
        DeltaLensError::Checkpoint(format!(
            "Failed to build Parquet reader for '{}': {}",
            path_str, e
        ))
    })?;

    let mut actions = Vec::new();
    let mut warnings = Vec::new();

    for batch_res in reader {
        let batch = batch_res.map_err(|e| {
            DeltaLensError::Checkpoint(format!(
                "Failed to read record batch from '{}': {}",
                path_str, e
            ))
        })?;
        extract_actions_from_batch(&batch, path_str, &mut actions, &mut warnings);
    }

    Ok(ParseResult { actions, warnings })
}

fn extract_actions_from_batch(
    batch: &RecordBatch,
    path_str: &str,
    actions: &mut Vec<Action>,
    warnings: &mut Vec<ParseWarning>,
) {
    let num_rows = batch.num_rows();
    let add_col = struct_col(batch, "add");
    let remove_col = struct_col(batch, "remove");
    let metadata_col = struct_col(batch, "metaData");
    let protocol_col = struct_col(batch, "protocol");
    let txn_col = struct_col(batch, "txn");
    let domain_col = struct_col(batch, "domainMetadata");
    let sidecar_col = struct_col(batch, "sidecar");
    let ckpt_meta_col = struct_col(batch, "checkpointMetadata");

    for row in 0..num_rows {
        if let Some(s) = add_col {
            if !s.is_null(row) {
                match extract_add(s, row) {
                    Some(a) => actions.push(Action::Add(a)),
                    None => warnings.push(ParseWarning {
                        path: path_str.to_string(),
                        line: row + 1,
                        message: "Skipped checkpoint 'add' row missing required 'path'".to_string(),
                    }),
                }
            }
        }
        if let Some(s) = remove_col {
            if !s.is_null(row) {
                match extract_remove(s, row) {
                    Some(r) => actions.push(Action::Remove(r)),
                    None => warnings.push(ParseWarning {
                        path: path_str.to_string(),
                        line: row + 1,
                        message: "Skipped checkpoint 'remove' row missing required 'path'"
                            .to_string(),
                    }),
                }
            }
        }
        if let Some(s) = metadata_col {
            if !s.is_null(row) {
                actions.push(Action::MetaData(extract_metadata(s, row)));
            }
        }
        if let Some(s) = protocol_col {
            if !s.is_null(row) {
                if let Some(p) = extract_protocol(s, row) {
                    actions.push(Action::Protocol(p));
                }
            }
        }
        if let Some(s) = txn_col {
            if !s.is_null(row) {
                if let Some(t) = extract_txn(s, row) {
                    actions.push(Action::Txn(t));
                }
            }
        }
        if let Some(s) = domain_col {
            if !s.is_null(row) {
                if let Some(d) = extract_domain_metadata(s, row) {
                    actions.push(Action::DomainMetadata(d));
                }
            }
        }
        if let Some(s) = sidecar_col {
            if !s.is_null(row) {
                if let Some(sc) = extract_sidecar(s, row) {
                    actions.push(Action::Sidecar(sc));
                }
            }
        }
        if let Some(s) = ckpt_meta_col {
            if !s.is_null(row) {
                actions.push(Action::CheckpointMetadata(extract_checkpoint_metadata(
                    s, row,
                )));
            }
        }
    }
}

fn struct_col<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StructArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StructArray>())
}

fn get_string(s: &StructArray, field: &str, row: usize) -> Option<String> {
    let col = s.column_by_name(field)?;
    if col.is_null(row) {
        return None;
    }
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        return Some(arr.value(row).to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        return Some(arr.value(row).to_string());
    }
    None
}

fn get_i64(s: &StructArray, field: &str, row: usize) -> Option<i64> {
    let col = s.column_by_name(field)?;
    if col.is_null(row) {
        return None;
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        return Some(arr.value(row));
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
        return Some(i64::from(arr.value(row)));
    }
    None
}

fn get_i32(s: &StructArray, field: &str, row: usize) -> Option<i32> {
    let col = s.column_by_name(field)?;
    if col.is_null(row) {
        return None;
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
        return Some(arr.value(row));
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        return i32::try_from(arr.value(row)).ok();
    }
    None
}

fn get_bool(s: &StructArray, field: &str, row: usize) -> Option<bool> {
    let col = s.column_by_name(field)?;
    if col.is_null(row) {
        return None;
    }
    col.as_any()
        .downcast_ref::<BooleanArray>()
        .map(|arr| arr.value(row))
}

fn get_string_list(s: &StructArray, field: &str, row: usize) -> Option<Vec<String>> {
    let col = s.column_by_name(field)?;
    if col.is_null(row) {
        return None;
    }
    let values: ArrayRef = if let Some(arr) = col.as_any().downcast_ref::<ListArray>() {
        arr.value(row)
    } else {
        let arr = col.as_any().downcast_ref::<LargeListArray>()?;
        arr.value(row)
    };
    let mut out = Vec::with_capacity(values.len());
    if let Some(sa) = values.as_any().downcast_ref::<StringArray>() {
        for i in 0..sa.len() {
            if !sa.is_null(i) {
                out.push(sa.value(i).to_string());
            }
        }
    } else if let Some(sa) = values.as_any().downcast_ref::<LargeStringArray>() {
        for i in 0..sa.len() {
            if !sa.is_null(i) {
                out.push(sa.value(i).to_string());
            }
        }
    }
    Some(out)
}

fn get_string_map(
    s: &StructArray,
    field: &str,
    row: usize,
) -> Option<HashMap<String, Option<String>>> {
    let col = s.column_by_name(field)?;
    if col.is_null(row) {
        return None;
    }
    let map_arr = col.as_any().downcast_ref::<MapArray>()?;
    let entries = map_arr.value(row);
    if entries.num_columns() < 2 {
        return Some(HashMap::new());
    }
    let keys = entries.column(0);
    let vals = entries.column(1);
    let mut out = HashMap::with_capacity(entries.len());
    for i in 0..entries.len() {
        let Some(k) = array_str_at(keys, i) else {
            continue;
        };
        let v = array_str_at(vals, i);
        out.insert(k, v);
    }
    Some(out)
}

fn array_str_at(arr: &ArrayRef, idx: usize) -> Option<String> {
    if arr.is_null(idx) {
        return None;
    }
    if let Some(sa) = arr.as_any().downcast_ref::<StringArray>() {
        return Some(sa.value(idx).to_string());
    }
    if let Some(sa) = arr.as_any().downcast_ref::<LargeStringArray>() {
        return Some(sa.value(idx).to_string());
    }
    None
}

fn extract_dv(s: &StructArray, row: usize) -> Option<DeletionVectorDescriptor> {
    let col = s.column_by_name("deletionVector")?;
    if col.is_null(row) {
        return None;
    }
    let dv = col.as_any().downcast_ref::<StructArray>()?;
    if dv.is_null(row) {
        return None;
    }
    let storage_type = get_string(dv, "storageType", row)?;
    let path_or_inline_dv = get_string(dv, "pathOrInlineDv", row)?;
    Some(DeletionVectorDescriptor {
        storage_type,
        path_or_inline_dv,
        offset: get_i32(dv, "offset", row),
        size_in_bytes: get_i32(dv, "sizeInBytes", row).unwrap_or(0),
        cardinality: get_i64(dv, "cardinality", row).unwrap_or(0),
    })
}

fn extract_add(s: &StructArray, row: usize) -> Option<AddAction> {
    let path = get_string(s, "path", row)?;
    Some(AddAction {
        path,
        size: get_i64(s, "size", row).unwrap_or(0),
        modification_time: get_i64(s, "modificationTime", row).unwrap_or(0),
        data_change: get_bool(s, "dataChange", row).unwrap_or(false),
        partition_values: get_string_map(s, "partitionValues", row).unwrap_or_default(),
        stats: get_string(s, "stats", row),
        tags: get_string_map(s, "tags", row),
        deletion_vector: extract_dv(s, row),
    })
}

fn extract_remove(s: &StructArray, row: usize) -> Option<RemoveAction> {
    let path = get_string(s, "path", row)?;
    Some(RemoveAction {
        path,
        deletion_timestamp: get_i64(s, "deletionTimestamp", row),
        data_change: get_bool(s, "dataChange", row).unwrap_or(false),
        extended_file_metadata: get_bool(s, "extendedFileMetadata", row),
        size: get_i64(s, "size", row),
        partition_values: get_string_map(s, "partitionValues", row),
        tags: get_string_map(s, "tags", row),
        deletion_vector: extract_dv(s, row),
    })
}

fn extract_metadata(s: &StructArray, row: usize) -> MetaDataAction {
    let format = s
        .column_by_name("format")
        .and_then(|c| {
            if c.is_null(row) {
                return None;
            }
            c.as_any().downcast_ref::<StructArray>()
        })
        .map(|f| Format {
            provider: get_string(f, "provider", row).unwrap_or_else(|| "parquet".to_string()),
            options: get_string_map(f, "options", row).unwrap_or_default(),
        })
        .unwrap_or_default();

    MetaDataAction {
        id: get_string(s, "id", row).unwrap_or_default(),
        name: get_string(s, "name", row),
        description: get_string(s, "description", row),
        format,
        schema_string: get_string(s, "schemaString", row).unwrap_or_default(),
        partition_columns: get_string_list(s, "partitionColumns", row).unwrap_or_default(),
        configuration: get_string_map(s, "configuration", row).unwrap_or_default(),
        created_time: get_i64(s, "createdTime", row),
    }
}

fn extract_protocol(s: &StructArray, row: usize) -> Option<ProtocolAction> {
    let min_reader_version = get_i32(s, "minReaderVersion", row)?;
    let min_writer_version = get_i32(s, "minWriterVersion", row)?;
    Some(ProtocolAction {
        min_reader_version,
        min_writer_version,
        reader_features: get_string_list(s, "readerFeatures", row),
        writer_features: get_string_list(s, "writerFeatures", row),
    })
}

fn extract_txn(s: &StructArray, row: usize) -> Option<TxnAction> {
    let app_id = get_string(s, "appId", row)?;
    let version = get_i64(s, "version", row)?;
    Some(TxnAction {
        app_id,
        version,
        last_updated: get_i64(s, "lastUpdated", row),
    })
}

fn extract_domain_metadata(s: &StructArray, row: usize) -> Option<DomainMetadataAction> {
    let domain = get_string(s, "domain", row)?;
    Some(DomainMetadataAction {
        domain,
        configuration: get_string(s, "configuration", row).unwrap_or_default(),
        removed: get_bool(s, "removed", row).unwrap_or(false),
    })
}

fn extract_sidecar(s: &StructArray, row: usize) -> Option<SidecarAction> {
    let path = get_string(s, "path", row)?;
    Some(SidecarAction {
        path,
        size_in_bytes: get_i64(s, "sizeInBytes", row).unwrap_or(0),
        modification_time: get_i64(s, "modificationTime", row).unwrap_or(0),
        tags: get_string_map(s, "tags", row),
    })
}

fn extract_checkpoint_metadata(s: &StructArray, row: usize) -> CheckpointMetadataAction {
    CheckpointMetadataAction {
        version: get_i64(s, "version", row).unwrap_or(2),
        tags: get_string_map(s, "tags", row),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::builder::{
        BooleanBuilder, Int32Builder, Int64Builder, ListBuilder, MapBuilder, StringBuilder,
        StructBuilder,
    };
    use arrow_schema::{DataType as ArrowDataType, Field, Fields, Schema};
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;

    #[test]
    fn test_parse_parquet_checkpoint_all_action_types() {
        let protocol_fields = Fields::from(vec![
            Field::new("minReaderVersion", ArrowDataType::Int32, false),
            Field::new("minWriterVersion", ArrowDataType::Int32, false),
            Field::new(
                "readerFeatures",
                ArrowDataType::List(Arc::new(Field::new("item", ArrowDataType::Utf8, true))),
                true,
            ),
            Field::new(
                "writerFeatures",
                ArrowDataType::List(Arc::new(Field::new("item", ArrowDataType::Utf8, true))),
                true,
            ),
        ]);

        let add_fields = Fields::from(vec![
            Field::new("path", ArrowDataType::Utf8, false),
            Field::new("size", ArrowDataType::Int64, false),
            Field::new("modificationTime", ArrowDataType::Int64, false),
            Field::new("dataChange", ArrowDataType::Boolean, false),
            Field::new(
                "partitionValues",
                ArrowDataType::Map(
                    Arc::new(Field::new(
                        "entries",
                        ArrowDataType::Struct(Fields::from(vec![
                            Field::new("keys", ArrowDataType::Utf8, false),
                            Field::new("values", ArrowDataType::Utf8, true),
                        ])),
                        false,
                    )),
                    false,
                ),
                true,
            ),
        ]);

        let metadata_fields = Fields::from(vec![
            Field::new("id", ArrowDataType::Utf8, false),
            Field::new("schemaString", ArrowDataType::Utf8, false),
            Field::new(
                "partitionColumns",
                ArrowDataType::List(Arc::new(Field::new("item", ArrowDataType::Utf8, true))),
                true,
            ),
            Field::new("createdTime", ArrowDataType::Int64, true),
        ]);

        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "protocol",
                ArrowDataType::Struct(protocol_fields.clone()),
                true,
            ),
            Field::new(
                "metaData",
                ArrowDataType::Struct(metadata_fields.clone()),
                true,
            ),
            Field::new("add", ArrowDataType::Struct(add_fields.clone()), true),
        ]));

        // Row 0: protocol, Row 1: metaData, Row 2: add
        let mut proto_b = StructBuilder::new(
            protocol_fields,
            vec![
                Box::new(Int32Builder::new()),
                Box::new(Int32Builder::new()),
                Box::new(ListBuilder::new(StringBuilder::new())),
                Box::new(ListBuilder::new(StringBuilder::new())),
            ],
        );
        // Row 0: protocol valid
        proto_b
            .field_builder::<Int32Builder>(0)
            .unwrap()
            .append_value(3);
        proto_b
            .field_builder::<Int32Builder>(1)
            .unwrap()
            .append_value(7);
        let rf = proto_b
            .field_builder::<ListBuilder<StringBuilder>>(2)
            .unwrap();
        rf.values().append_value("deletionVectors");
        rf.append(true);
        let wf = proto_b
            .field_builder::<ListBuilder<StringBuilder>>(3)
            .unwrap();
        wf.values().append_value("deletionVectors");
        wf.values().append_value("clustering");
        wf.append(true);
        proto_b.append(true);
        // Row 1 & 2: protocol null
        for _ in 0..2 {
            proto_b
                .field_builder::<Int32Builder>(0)
                .unwrap()
                .append_value(0);
            proto_b
                .field_builder::<Int32Builder>(1)
                .unwrap()
                .append_value(0);
            proto_b
                .field_builder::<ListBuilder<StringBuilder>>(2)
                .unwrap()
                .append(false);
            proto_b
                .field_builder::<ListBuilder<StringBuilder>>(3)
                .unwrap()
                .append(false);
            proto_b.append(false);
        }

        let mut meta_b = StructBuilder::new(
            metadata_fields,
            vec![
                Box::new(StringBuilder::new()),
                Box::new(StringBuilder::new()),
                Box::new(ListBuilder::new(StringBuilder::new())),
                Box::new(Int64Builder::new()),
            ],
        );
        // Row 0: null
        meta_b
            .field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value("");
        meta_b
            .field_builder::<StringBuilder>(1)
            .unwrap()
            .append_value("");
        meta_b
            .field_builder::<ListBuilder<StringBuilder>>(2)
            .unwrap()
            .append(false);
        meta_b
            .field_builder::<Int64Builder>(3)
            .unwrap()
            .append_null();
        meta_b.append(false);
        // Row 1: valid metaData
        meta_b
            .field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value("tbl-1");
        meta_b
            .field_builder::<StringBuilder>(1)
            .unwrap()
            .append_value(r#"{"type":"struct","fields":[{"name":"id","type":"long","nullable":false,"metadata":{}}]}"#);
        let pc = meta_b
            .field_builder::<ListBuilder<StringBuilder>>(2)
            .unwrap();
        pc.values().append_value("dt");
        pc.append(true);
        meta_b
            .field_builder::<Int64Builder>(3)
            .unwrap()
            .append_value(1700000000000);
        meta_b.append(true);
        // Row 2: null
        meta_b
            .field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value("");
        meta_b
            .field_builder::<StringBuilder>(1)
            .unwrap()
            .append_value("");
        meta_b
            .field_builder::<ListBuilder<StringBuilder>>(2)
            .unwrap()
            .append(false);
        meta_b
            .field_builder::<Int64Builder>(3)
            .unwrap()
            .append_null();
        meta_b.append(false);

        let mut add_b = StructBuilder::new(
            add_fields,
            vec![
                Box::new(StringBuilder::new()),
                Box::new(Int64Builder::new()),
                Box::new(Int64Builder::new()),
                Box::new(BooleanBuilder::new()),
                Box::new(MapBuilder::new(
                    None,
                    StringBuilder::new(),
                    StringBuilder::new(),
                )),
            ],
        );
        // Row 0 & 1: null
        for _ in 0..2 {
            add_b
                .field_builder::<StringBuilder>(0)
                .unwrap()
                .append_value("");
            add_b
                .field_builder::<Int64Builder>(1)
                .unwrap()
                .append_value(0);
            add_b
                .field_builder::<Int64Builder>(2)
                .unwrap()
                .append_value(0);
            add_b
                .field_builder::<BooleanBuilder>(3)
                .unwrap()
                .append_value(false);
            add_b
                .field_builder::<MapBuilder<StringBuilder, StringBuilder>>(4)
                .unwrap()
                .append(false)
                .unwrap();
            add_b.append(false);
        }
        // Row 2: valid add
        add_b
            .field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value("dt=2026-01-01/part-0.parquet");
        add_b
            .field_builder::<Int64Builder>(1)
            .unwrap()
            .append_value(50_000_000);
        add_b
            .field_builder::<Int64Builder>(2)
            .unwrap()
            .append_value(1700000001000);
        add_b
            .field_builder::<BooleanBuilder>(3)
            .unwrap()
            .append_value(false);
        let pv = add_b
            .field_builder::<MapBuilder<StringBuilder, StringBuilder>>(4)
            .unwrap();
        pv.keys().append_value("dt");
        pv.values().append_value("2026-01-01");
        pv.append(true).unwrap();
        add_b.append(true);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(proto_b.finish()),
                Arc::new(meta_b.finish()),
                Arc::new(add_b.finish()),
            ],
        )
        .unwrap();

        let mut buf = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buf, schema, None).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }

        let parsed =
            parse_parquet_checkpoint(&buf, "00000000000000000010.checkpoint.parquet").unwrap();
        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.actions.len(), 3);
        match &parsed.actions[0] {
            Action::Protocol(p) => {
                assert_eq!(p.min_reader_version, 3);
                assert_eq!(p.min_writer_version, 7);
                assert_eq!(
                    p.reader_features.as_deref(),
                    Some(&["deletionVectors".to_string()][..])
                );
            }
            other => panic!("Expected Protocol, got {:?}", other),
        }
        match &parsed.actions[1] {
            Action::MetaData(m) => {
                assert_eq!(m.id, "tbl-1");
                assert_eq!(m.partition_columns, vec!["dt"]);
            }
            other => panic!("Expected MetaData, got {:?}", other),
        }
        match &parsed.actions[2] {
            Action::Add(a) => {
                assert_eq!(a.path, "dt=2026-01-01/part-0.parquet");
                assert_eq!(a.size, 50_000_000);
                assert_eq!(
                    a.partition_values.get("dt").and_then(|v| v.as_deref()),
                    Some("2026-01-01")
                );
            }
            other => panic!("Expected Add, got {:?}", other),
        }
    }
}
