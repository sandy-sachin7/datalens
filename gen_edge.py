#!/usr/bin/env python3
"""Generate edge case Delta Lake tables for exhaustive DeltaLens testing."""

import os, json, random
from deltalake.writer import write_deltalake
import pyarrow as pa

BASE = "/home/sachin/Desktop/Code/datalens/edge_tables"
os.makedirs(BASE, exist_ok=True)

def t(name):
    return os.path.join(BASE, name)

def make_table(data, **kwargs):
    keys = list(data.keys())
    cols = {k: pa.array(data[k], **kwargs.get(k, {})) for k in keys}
    return pa.table(cols)

def create_normal_table():
    """Happy path: basic table with 3 versions."""
    p = t("normal")
    write_deltalake(p, make_table({
        "id": [1,2,3], "val": ["a","b","c"]
    }), mode="overwrite")
    write_deltalake(p, make_table({
        "id": [4,5], "val": ["d","e"]
    }), mode="append")
    # Schema evolution via append with merge
    write_deltalake(p, make_table({
        "id": [1,2,3,4,5], "val": ["A","b","c","d","e"], "extra": ["x","y","z","w","v"]
    }), mode="append", schema_mode="merge")
    print(f"  normal: 3 versions, schema change v2")

def create_empty_table():
    """Edge: table with 0 records after a delete."""
    p = t("empty_table")
    schema = pa.schema([("id", pa.int64()), ("val", pa.string())])
    table = pa.table({"id": pa.array([], pa.int64()), "val": pa.array([], pa.string())}, schema=schema)
    write_deltalake(p, table, mode="overwrite")
    print(f"  empty_table: 0 records, 1 version")

def create_single_version():
    """Edge: just 1 version, no changes."""
    p = t("single_version")
    write_deltalake(p, make_table({
        "id": [1,2,3,4,5,6,7,8,9,10], "val": list("abcdefghij")
    }), mode="overwrite")
    print(f"  single_version: 1 version, 10 records")

def create_partitioned():
    """Edge: partitioned table."""
    p = t("partitioned")
    write_deltalake(p, make_table({
        "id": [1,2,3,4], "region": ["US","US","EU","EU"], "amt": [10,20,30,40]
    }), partition_by=["region"], mode="overwrite")
    write_deltalake(p, make_table({
        "id": [5,6], "region": ["APAC","APAC"], "amt": [50,60]
    }), mode="append")
    print(f"  partitioned: 2 versions, partitioned by region")

def create_maintenance_history():
    """Edge: table with VACUUM/OPTIMIZE-like commits."""
    p = t("maintenance_history")
    # We manually craft delta log entries with VACUUM/OPTIMIZE operations
    write_deltalake(p, make_table({"id": [1,2,3], "val": ["a","b","c"]}), mode="overwrite")
    write_deltalake(p, make_table({"id": [4,5,6], "val": ["d","e","f"]}), mode="append")
    write_deltalake(p, make_table({"id": [1,2,3,4,5,6], "val": ["A","B","C","d","e","f"]}), mode="overwrite")
    # Manually inject OPTIMIZE and VACUUM commit info entries using delta-rs API
    # delta-rs doesn't expose VACUUM/OPTIMIZE directly, so we simulate
    log_dir = os.path.join(p, "_delta_log")
    v5 = {
        "commitInfo": {
            "timestamp": 1779400000000, "operation": "OPTIMIZE",
            "operationParameters": {"zOrderBy": '["val"]', "predicate": "id > 0"},
            "operationMetrics": {"numAddedFiles": "1", "numRemovedFiles": "2", "numOutputRows": "6"},
            "engineInfo": "delta-rs:py-1.6.0", "clientVersion": "delta-rs.py-1.6.0"
        }
    }
    v6 = {
        "commitInfo": {
            "timestamp": 1779400001000, "operation": "VACUUM END",
            "operationParameters": {"retentionHours": "168"},
            "operationMetrics": {"numDeletedFiles": "3"},
            "engineInfo": "delta-rs:py-1.6.0", "clientVersion": "delta-rs.py-1.6.0"
        }
    }
    with open(os.path.join(log_dir, "00000000000000000005.json"), "w") as f:
        f.write(json.dumps(v5) + "\n")
    with open(os.path.join(log_dir, "00000000000000000006.json"), "w") as f:
        f.write(json.dumps(v6) + "\n")
    print(f"  maintenance_history: 7 versions, has OPTIMIZE+VACUUM history")

def create_100_versions():
    """Edge: 100 versions (triggers maintenance warnings in inspect)."""
    p = t("hundred_versions")
    write_deltalake(p, make_table({"id": [1,2,3], "val": ["a","b","c"]}), mode="overwrite")
    for i in range(4, 102):
        write_deltalake(p, make_table({"id": [i], "val": [f"x{i}"]}), mode="append")
    print(f"  hundred_versions: 101 versions (triggers maintenance warnings)")

def create_corrupted_log():
    """Edge: table with corrupted line in delta log."""
    p = t("corrupted_log")
    write_deltalake(p, make_table({"id": [1], "val": ["ok"]}), mode="overwrite")
    log_dir = os.path.join(p, "_delta_log")
    # Append a corrupted line to the last log file
    last_json = sorted(f for f in os.listdir(log_dir) if f.endswith(".json"))[-1]
    with open(os.path.join(log_dir, last_json), "a") as f:
        f.write("THIS IS NOT JSON {{{{\n")
    print(f"  corrupted_log: has garbled JSON line in log")

def create_custom_actions():
    """Edge: table log with unknown action type (should be silently skipped)."""
    p = t("custom_actions")
    write_deltalake(p, make_table({"id": [1], "val": ["ok"]}), mode="overwrite")
    log_dir = os.path.join(p, "_delta_log")
    v1 = {
        "someFutureExtension": {"version": 2, "config": "testing"}
    }
    with open(os.path.join(log_dir, "00000000000000000001.json"), "w") as f:
        f.write(json.dumps(v1) + "\n")
    print(f"  custom_actions: has unknown action type (silent skip)")

def create_no_delta_log():
    """Edge: directory that looks like a table but has no _delta_log."""
    p = t("no_delta_log")
    os.makedirs(p, exist_ok=True)
    print(f"  no_delta_log: no _delta_log dir (not a real Delta table)")

def create_empty_delta_log():
    """Edge: table with empty _delta_log directory."""
    p = t("empty_delta_log")
    os.makedirs(os.path.join(p, "_delta_log"), exist_ok=True)
    print(f"  empty_delta_log: _delta_log dir but no log files")

def create_zero_byte_log():
    """Edge: table with zero-byte log file."""
    p = t("zero_byte_log")
    write_deltalake(p, make_table({"id": [1], "val": ["ok"]}), mode="overwrite")
    log_dir = os.path.join(p, "_delta_log")
    with open(os.path.join(log_dir, "00000000000000000001.json"), "w") as f:
        pass  # zero bytes
    print(f"  zero_byte_log: one zero-byte log file")

def create_multiple_schema_changes():
    """Edge: table with many schema evolutions."""
    p = t("multi_schema")
    cols = {"id": [1], "a": ["x"]}
    write_deltalake(p, make_table(cols), mode="overwrite")
    for i, letter in enumerate("bcdefghijklmnopqrstuvwxyz", 2):
        cols[letter] = [f"v{i}"]
        tbl = pa.table({k: pa.array(cols[k], pa.string() if k != "id" else pa.int64()) for k in cols})
        write_deltalake(p, tbl, mode="append", schema_mode="merge")
        if i % 5 == 0:
            print(f"    multi_schema: version {i} - added column '{letter}'")
    print(f"  multi_schema: 27 versions, 26 schema changes (all letters)")

def create_extreme_skew():
    """Edge: extreme file size skew."""
    p = t("extreme_skew")
    write_deltalake(p, make_table({"id": [1]*500, "val": ["tiny"]*500}), mode="overwrite")
    # delta-rs always writes one file, so skew is always 0 for small tables
    # We need multiple files - do multiple appends
    for i in range(5):
        write_deltalake(p, make_table({"id": [i], "val": ["a"]}), mode="append")
    print(f"  extreme_skew: 6 versions, multiple small files")

def create_nested_schema():
    """Edge: table with basic nested column types (list)."""
    p = t("nested_schema")
    list_type = pa.list_(pa.int64())
    table = pa.table({
        "id": pa.array([1, 2], pa.int64()),
        "tags": pa.array([[1, 2], [3, 4, 5]], list_type),
    }, schema=pa.schema([("id", pa.int64()), ("tags", list_type)]))
    write_deltalake(p, table, mode="overwrite")
    print(f"  nested_schema: list column type (list<int64>)")

def create_all_null_table():
    """Edge: table with most values NULL."""
    p = t("all_nulls")
    table = pa.table({
        "id": pa.array([1, 2, 3], pa.int64()),
        "val": pa.array([None, None, None], pa.string()),
    })
    write_deltalake(p, table, mode="overwrite")
    print(f"  all_nulls: 3 records, val column all NULLs")

def create_all_types():
    """Edge: table with all supported primitive types."""
    p = t("all_types")
    table = pa.table({
        "i8": pa.array([1], pa.int8()),
        "i16": pa.array([2], pa.int16()),
        "i32": pa.array([3], pa.int32()),
        "i64": pa.array([4], pa.int64()),
        "f32": pa.array([1.5], pa.float32()),
        "f64": pa.array([2.5], pa.float64()),
        "bool": pa.array([True], pa.bool_()),
        "str": pa.array(["hello"], pa.string()),
    })
    write_deltalake(p, table, mode="overwrite")
    print(f"  all_types: 8 column types (int8/16/32/64, float32/64, bool, string)")

def create_decimal_types():
    """Edge: table with decimal types using string-backed decimals."""
    p = t("decimal_types")
    table = pa.table({
        "id": pa.array([1, 2], pa.int64()),
        "price": pa.array(["19.99", "149.99"], pa.string()),
    })
    write_deltalake(p, table, mode="overwrite")
    print(f"  decimal_types: decimal stored as string")


if __name__ == "__main__":
    print("Creating edge case Delta Lake tables...")
    print()
    create_normal_table()
    create_empty_table()
    create_single_version()
    create_partitioned()
    create_maintenance_history()
    create_corrupted_log()
    create_custom_actions()
    create_no_delta_log()
    create_empty_delta_log()
    create_zero_byte_log()
    create_multiple_schema_changes()
    create_extreme_skew()
    create_nested_schema()
    create_all_null_table()
    create_all_types()
    create_decimal_types()
    create_100_versions()
    print(f"\nAll tables created in {BASE}")
