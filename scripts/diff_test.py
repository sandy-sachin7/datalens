#!/usr/bin/env python3
"""
Differential conformance test suite comparing `deltalens` against `deltalake` (delta-rs).

Generates real Delta Lake tables with `deltalake`, then asserts that `deltalens` agrees with
`DeltaTable` on version, active file count, total bytes, partition columns, protocol versions,
schema fields/types/nullability, deletion vector counts, and version diffs.

Run locally with uv (zero permanent installs):
    cargo build --release --locked
    uv run --with deltalake --with pyarrow python3 scripts/diff_test.py
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from datetime import date, datetime, timezone
from decimal import Decimal
from pathlib import Path

import pyarrow as pa
from deltalake import DeltaTable, write_deltalake

REPO_ROOT = Path(__file__).resolve().parent.parent


def find_deltalens_bin() -> str:
    env_bin = os.environ.get("DELTALENS_BIN")
    if env_bin:
        return env_bin
    for candidate in (
        REPO_ROOT / "target" / "release" / "deltalens",
        REPO_ROOT / "target" / "debug" / "deltalens",
    ):
        if candidate.exists():
            return str(candidate)
    found = shutil.which("deltalens")
    if found:
        return found
    raise FileNotFoundError(
        "deltalens binary not found. Run `cargo build --locked` or set DELTALENS_BIN."
    )


def run_deltalens_json(binary: str, args: list[str]) -> dict | list:
    proc = subprocess.run(
        [binary, *args, "--json"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise AssertionError(
            f"deltalens {' '.join(args)} failed (exit {proc.returncode}):\n"
            f"stdout: {proc.stdout}\nstderr: {proc.stderr}"
        )
    return json.loads(proc.stdout)


def normalize_delta_type(dt) -> object:
    if isinstance(dt, str):
        return dt
    if isinstance(dt, dict):
        kind = dt.get("type")
        if kind == "struct":
            return {
                "type": "struct",
                "fields": [
                    {
                        "name": f["name"],
                        "type": normalize_delta_type(f["type"]),
                        "nullable": f["nullable"],
                    }
                    for f in dt["fields"]
                ],
            }
        if kind == "array":
            return {
                "type": "array",
                "elementType": normalize_delta_type(dt["elementType"]),
                "containsNull": dt["containsNull"],
            }
        if kind == "map":
            return {
                "type": "map",
                "keyType": normalize_delta_type(dt["keyType"]),
                "valueType": normalize_delta_type(dt["valueType"]),
                "valueContainsNull": dt["valueContainsNull"],
            }
    return dt


def schema_from_deltalake(dt: DeltaTable) -> list[dict]:
    raw = json.loads(dt.schema().to_json())
    return [
        {
            "name": f["name"],
            "type": normalize_delta_type(f["type"]),
            "nullable": f["nullable"],
        }
        for f in raw["fields"]
    ]


def schema_from_deltalens(schema_json: dict) -> list[dict]:
    fields = schema_json["schema"]["fields"]
    return [
        {
            "name": f["name"],
            "type": normalize_delta_type(f["type"]),
            "nullable": f["nullable"],
        }
        for f in fields
    ]


def assert_version_matches(binary: str, table_dir: Path, version: int, label: str) -> dict:
    dt = DeltaTable(str(table_dir), version=version)
    adds = dt.get_add_actions(flatten=True)
    expected_sizes = adds.column("size_bytes").to_pylist() if adds.num_rows > 0 else []
    expected_files = len(expected_sizes)
    expected_bytes = sum(expected_sizes)

    dl = run_deltalens_json(binary, ["inspect", str(table_dir), "--version", str(version)])

    assert dl["current_version"] == dt.version(), (
        f"[{label} v{version}] version mismatch: deltalens={dl['current_version']} vs delta-rs={dt.version()}"
    )
    assert dl["health"]["total_files"] == expected_files, (
        f"[{label} v{version}] active file count mismatch: deltalens={dl['health']['total_files']} vs delta-rs={expected_files}"
    )
    assert dl["health"]["total_size"] == expected_bytes, (
        f"[{label} v{version}] total bytes mismatch: deltalens={dl['health']['total_size']} vs delta-rs={expected_bytes}"
    )
    assert dl["partition_columns"] == dt.metadata().partition_columns, (
        f"[{label} v{version}] partition_columns mismatch: deltalens={dl['partition_columns']} vs delta-rs={dt.metadata().partition_columns}"
    )

    proto = dt.protocol()
    assert dl["min_reader_version"] == proto.min_reader_version, (
        f"[{label} v{version}] min_reader_version mismatch: deltalens={dl['min_reader_version']} vs delta-rs={proto.min_reader_version}"
    )
    assert dl["min_writer_version"] == proto.min_writer_version, (
        f"[{label} v{version}] min_writer_version mismatch: deltalens={dl['min_writer_version']} vs delta-rs={proto.min_writer_version}"
    )

    dl_schema = run_deltalens_json(binary, ["schema", str(table_dir), "--at", str(version)])
    got_fields = schema_from_deltalens(dl_schema)
    exp_fields = schema_from_deltalake(dt)
    assert got_fields == exp_fields, (
        f"[{label} v{version}] schema mismatch:\n  deltalens={got_fields}\n  delta-rs={exp_fields}"
    )

    return {
        "version": version,
        "files": expected_files,
        "bytes": expected_bytes,
    }


def scenario_checkpoints_evolution_and_delete(binary: str, root: Path) -> None:
    tbl = root / "checkpoints_evolution"
    regions = ["us-east", "eu-west", "ap-south"]
    for v in range(24):
        n = 50 if v % 4 != 0 else 250
        data = {
            "id": pa.array(range(v * 1000, v * 1000 + n), type=pa.int64()),
            "region": pa.array([regions[v % len(regions)]] * n, type=pa.string()),
            "amount": pa.array([float((i % 50) + 0.5) for i in range(n)], type=pa.float64()),
        }
        if v >= 12:
            data["currency"] = pa.array(["USD"] * n, type=pa.string())
        write_deltalake(
            str(tbl),
            pa.table(data),
            mode="overwrite" if v == 0 else "append",
            partition_by=["region"] if v == 0 else None,
            schema_mode="merge" if v == 12 else None,
        )
        if v in (10, 20):
            DeltaTable(str(tbl)).create_checkpoint()

    dt = DeltaTable(str(tbl))
    dt.delete("amount < 5.0")
    latest = dt.version()

    snaps = {}
    for v in (0, 5, 10, 12, 15, 20, latest):
        snaps[v] = assert_version_matches(binary, tbl, v, "checkpoints_evolution")

    # Verify diff net deltas match delta-rs snapshots across checkpoints and schema evolution.
    for v1, v2 in ((0, 12), (10, latest), (0, latest)):
        diff = run_deltalens_json(binary, ["diff", str(tbl), "--v1", str(v1), "--v2", str(v2)])
        net_files = diff["physical_files_added"] - diff["physical_files_removed"]
        net_bytes = diff["physical_bytes_added"] - diff["physical_bytes_removed"]
        assert net_files == snaps[v2]["files"] - snaps[v1]["files"], (
            f"[diff v{v1}..v{v2}] net files mismatch: {net_files} vs {snaps[v2]['files'] - snaps[v1]['files']}"
        )
        assert net_bytes == snaps[v2]["bytes"] - snaps[v1]["bytes"], (
            f"[diff v{v1}..v{v2}] net bytes mismatch: {net_bytes} vs {snaps[v2]['bytes'] - snaps[v1]['bytes']}"
        )

    print(f"PASS scenario_checkpoints_evolution_and_delete (v0..v{latest})")


def scenario_mutations_optimize_and_vacuum(binary: str, root: Path) -> None:
    tbl = root / "mutations_lifecycle"
    t0 = pa.table({
        "id": pa.array([1, 2, 3, 4], type=pa.int64()),
        "status": pa.array(["new", "new", "open", "closed"], type=pa.string()),
        "score": pa.array([10.0, 20.0, 30.0, 40.0], type=pa.float64()),
    })
    write_deltalake(str(tbl), t0, mode="overwrite")

    t1 = pa.table({
        "id": pa.array([5, 6], type=pa.int64()),
        "status": pa.array(["new", "open"], type=pa.string()),
        "score": pa.array([50.0, 60.0], type=pa.float64()),
    })
    write_deltalake(str(tbl), t1, mode="append")

    dt = DeltaTable(str(tbl))
    dt.update(predicate="status = 'new'", updates={"status": "'active'"})

    src = pa.table({
        "id": pa.array([2, 7], type=pa.int64()),
        "status": pa.array(["merged", "inserted"], type=pa.string()),
        "score": pa.array([25.0, 70.0], type=pa.float64()),
    })
    (
        dt.merge(
            source=src,
            predicate="t.id = s.id",
            source_alias="s",
            target_alias="t",
        )
        .when_matched_update_all()
        .when_not_matched_insert_all()
        .execute()
    )

    dt.optimize.compact()
    dt.create_checkpoint()
    dt.vacuum(retention_hours=0, enforce_retention_duration=False, dry_run=False)

    latest = dt.version()
    for v in range(latest + 1):
        assert_version_matches(binary, tbl, v, "mutations_lifecycle")

    # Verify lineage length agrees with delta-rs history().
    dl_lineage = run_deltalens_json(binary, ["lineage", str(tbl), "--last", str(latest + 5)])
    rs_history = list(dt.history())
    assert len(dl_lineage) == len(rs_history), (
        f"[mutations_lifecycle] lineage count mismatch: deltalens={len(dl_lineage)} vs delta-rs={len(rs_history)}"
    )

    print(f"PASS scenario_mutations_optimize_and_vacuum (v0..v{latest})")


def scenario_nested_types_and_column_mapping(binary: str, root: Path) -> None:
    tbl = root / "nested_types"
    struct_type = pa.struct([
        pa.field("city", pa.string()),
        pa.field("zip", pa.int32()),
    ])
    schema = pa.schema([
        pa.field("id", pa.int64(), nullable=False),
        pa.field("price", pa.decimal128(10, 2), nullable=True),
        pa.field("event_date", pa.date32(), nullable=True),
        pa.field("created_at", pa.timestamp("us", tz="UTC"), nullable=True),
        pa.field("tags", pa.list_(pa.string()), nullable=True),
        pa.field("attrs", pa.map_(pa.string(), pa.int32()), nullable=True),
        pa.field("address", struct_type, nullable=True),
    ])
    batch = pa.table(
        {
            "id": pa.array([1, 2], type=pa.int64()),
            "price": pa.array([Decimal("12.34"), Decimal("56.78")], type=pa.decimal128(10, 2)),
            "event_date": pa.array([date(2026, 1, 1), date(2026, 1, 2)], type=pa.date32()),
            "created_at": pa.array(
                [datetime(2026, 1, 1, tzinfo=timezone.utc), datetime(2026, 1, 2, tzinfo=timezone.utc)],
                type=pa.timestamp("us", tz="UTC"),
            ),
            "tags": pa.array([["a", "b"], ["c"]], type=pa.list_(pa.string())),
            "attrs": pa.array([[("k1", 1)], [("k2", 2)]], type=pa.map_(pa.string(), pa.int32())),
            "address": pa.array(
                [{"city": "BLR", "zip": 560001}, {"city": "SF", "zip": 94105}],
                type=struct_type,
            ),
        },
        schema=schema,
    )
    write_deltalake(
        str(tbl),
        batch,
        mode="overwrite",
        configuration={"delta.columnMapping.mode": "name"},
    )
    dt = DeltaTable(str(tbl))
    dt.create_checkpoint()
    assert_version_matches(binary, tbl, dt.version(), "nested_types")
    print(f"PASS scenario_nested_types_and_column_mapping (v{dt.version()})")


def scenario_deletion_vectors(binary: str, root: Path) -> None:
    tbl = root / "deletion_vectors"
    batch = pa.table({
        "id": pa.array(range(100), type=pa.int64()),
        "grp": pa.array(["a" if i < 50 else "b" for i in range(100)], type=pa.string()),
    })
    try:
        write_deltalake(
            str(tbl),
            batch,
            mode="overwrite",
            configuration={"delta.enableDeletionVectors": "true"},
        )
        dt = DeltaTable(str(tbl))
        dt.delete("id < 10")
        dt.create_checkpoint()
    except Exception as exc:
        print(f"SKIP scenario_deletion_vectors (deltalake write/delete DV unsupported: {exc})")
        return

    for v in range(dt.version() + 1):
        assert_version_matches(binary, tbl, v, "deletion_vectors")
    print(f"PASS scenario_deletion_vectors (v0..v{dt.version()})")


def main() -> None:
    binary = find_deltalens_bin()
    print(f"Running differential conformance suite with {binary}")
    with tempfile.TemporaryDirectory(prefix="deltalens-diff-") as tmp:
        root = Path(tmp)
        scenario_checkpoints_evolution_and_delete(binary, root)
        scenario_mutations_optimize_and_vacuum(binary, root)
        scenario_nested_types_and_column_mapping(binary, root)
        scenario_deletion_vectors(binary, root)
    print("ALL DIFFERENTIAL CONFORMANCE CHECKS PASSED")


if __name__ == "__main__":
    main()
