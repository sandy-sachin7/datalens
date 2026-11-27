#!/usr/bin/env python3
"""
Deep Spark / Delta Acceptance Testing (DAT) conformance suite for `deltalens`.

Validates `deltalens` against all 19 official Apache Spark-generated Delta Lake
tables from `delta-incubator/dat` (v0.0.3, SHA-256 verified) across all 29
expected snapshots (`latest` and historical `vN` versions):
  - protocol versions (`min_reader_version`, `min_writer_version`)
  - exact table properties (`table_properties` vs `table_version_metadata.json`)
  - active file count, total active bytes, and partition columns
  - exact Spark data-row parity (`expected/*/table_content/*.parquet` vs active
    Parquet file rows minus `deleted_rows_in_dvs`)
  - full schema conformance (field names, nested types, nullability, and column
    presence against Spark `table_content` Parquet files and `delta-rs`)
  - multi-version `deltalens diff` checks across historical snapshots (including
    schema evolution on `with_schema_change`) and `deltalens lineage` checks.

Usage:
    cargo build --release --locked
    uv run --with deltalake --with pyarrow python3 scripts/dat_test.py [--dat-dir .dat-cache]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

import pyarrow.parquet as pq
from deltalake import DeltaTable

REPO_ROOT = Path(__file__).resolve().parent.parent
DAT_VERSION = "v0.0.3"
DAT_URL = f"https://github.com/delta-incubator/dat/releases/download/{DAT_VERSION}/deltalake-dat-{DAT_VERSION}.tar.gz"
DAT_SHA256 = "19c045bc6f4e8531d1985d0f7bb156d788e65078b435d613c8e4a9c753b4a982"


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
        "deltalens binary not found. Run `cargo build --release --locked` or set DELTALENS_BIN."
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


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def ensure_dat_extracted(cache_root: Path) -> Path:
    generated = cache_root / "out" / "reader_tests" / "generated"
    if generated.is_dir():
        return generated

    tar_path = cache_root / f"deltalake-dat-{DAT_VERSION}.tar.gz"
    cache_root.mkdir(parents=True, exist_ok=True)
    if not tar_path.exists():
        print(f"Downloading Spark DAT {DAT_VERSION} corpus from {DAT_URL}...")
        req = urllib.request.Request(DAT_URL, headers={"User-Agent": "deltalens-dat-conformance"})
        with urllib.request.urlopen(req, timeout=60) as resp, tar_path.open("wb") as out:
            shutil.copyfileobj(resp, out)

    actual_sha = sha256_file(tar_path)
    if actual_sha != DAT_SHA256:
        tar_path.unlink(missing_ok=True)
        raise AssertionError(
            f"DAT tarball SHA-256 mismatch: expected {DAT_SHA256}, got {actual_sha}"
        )

    with tarfile.open(tar_path, "r:gz") as tf:
        tf.extractall(cache_root, filter="data")

    if not generated.is_dir():
        raise FileNotFoundError(f"Expected DAT directory not found at {generated}")
    return generated


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
    return [
        {
            "name": f["name"],
            "type": normalize_delta_type(f["type"]),
            "nullable": f["nullable"],
        }
        for f in schema_json["schema"]["fields"]
    ]


def resolve_local_uri(uri: str) -> Path:
    if uri.startswith("file://"):
        parsed = urllib.parse.urlparse(uri)
        return Path(urllib.parse.unquote(parsed.path))
    return Path(uri)


def verify_case(binary: str, case_dir: Path) -> tuple[int, int]:
    delta_dir = case_dir / "delta"
    expected_root = case_dir / "expected"
    snapshots = sorted(p for p in expected_root.iterdir() if p.is_dir())
    checked_snapshots = 0
    checked_diffs = 0
    seen_versions: dict[int, dict] = {}

    for snap_dir in snapshots:
        meta_path = snap_dir / "table_version_metadata.json"
        if not meta_path.exists():
            continue
        expected = json.loads(meta_path.read_text(encoding="utf-8"))
        exp_ver = expected["version"]
        exp_mr = expected["min_reader_version"]
        exp_mw = expected["min_writer_version"]
        exp_props = expected.get("properties", {})
        tag = f"{case_dir.name}/{snap_dir.name}"

        inspect_args = ["inspect", str(delta_dir)]
        schema_args = ["schema", str(delta_dir)]
        if snap_dir.name != "latest":
            inspect_args += ["--version", str(exp_ver)]
            schema_args += ["--at", str(exp_ver)]

        dl = run_deltalens_json(binary, inspect_args)
        dl_schema = run_deltalens_json(binary, schema_args)
        seen_versions[exp_ver] = dl

        # 1. Exact version and protocol assertions against Spark DAT metadata.
        assert dl["current_version"] == exp_ver, (
            f"[{tag}] version mismatch: deltalens={dl['current_version']} vs Spark DAT={exp_ver}"
        )
        assert dl["min_reader_version"] == exp_mr, (
            f"[{tag}] min_reader_version mismatch: deltalens={dl['min_reader_version']} vs Spark DAT={exp_mr}"
        )
        assert dl["min_writer_version"] == exp_mw, (
            f"[{tag}] min_writer_version mismatch: deltalens={dl['min_writer_version']} vs Spark DAT={exp_mw}"
        )

        # 2. Strict table properties assertion (never skipped).
        assert "table_properties" in dl, f"[{tag}] inspect --json missing table_properties"
        dl_props = dl["table_properties"]
        assert dl_props == exp_props, (
            f"[{tag}] table_properties mismatch: deltalens={dl_props!r} vs Spark DAT={exp_props!r}"
        )

        # 3. Cross-check active files, total bytes, partition columns, and full schema
        #    against delta-rs on the exact same Spark-written table version.
        dt = DeltaTable(str(delta_dir), version=exp_ver)
        dt_files = [resolve_local_uri(u) for u in dt.file_uris()]
        dt_total_bytes = sum(p.stat().st_size for p in dt_files)
        dt_part_cols = list(dt.metadata().partition_columns)

        assert dl["health"]["total_files"] == len(dt_files), (
            f"[{tag}] active file count mismatch: deltalens={dl['health']['total_files']} vs delta-rs={len(dt_files)}"
        )
        assert dl["health"]["total_size"] == dt_total_bytes, (
            f"[{tag}] total active bytes mismatch: deltalens={dl['health']['total_size']} vs disk={dt_total_bytes}"
        )
        assert dl["partition_columns"] == dt_part_cols, (
            f"[{tag}] partition_columns mismatch: deltalens={dl['partition_columns']} vs delta-rs={dt_part_cols}"
        )

        dl_fields = schema_from_deltalens(dl_schema)
        dt_fields = schema_from_deltalake(dt)
        assert dl_fields == dt_fields, (
            f"[{tag}] schema mismatch:\ndeltalens={dl_fields}\ndelta-rs={dt_fields}"
        )
        assert dl["schema_column_count"] == len(dl_fields), (
            f"[{tag}] schema_column_count={dl['schema_column_count']} != len(fields)={len(dl_fields)}"
        )

        # 4. Exact Spark data-row and column-name parity against expected/*/table_content/*.parquet.
        tc_dir = snap_dir / "table_content"
        expected_pqs = sorted(tc_dir.glob("*.parquet"))
        assert expected_pqs, f"[{tag}] expected table_content parquet files missing"
        expected_rows = sum(pq.read_metadata(p).num_rows for p in expected_pqs)
        expected_col_names = [f.name for f in pq.read_schema(expected_pqs[0])]
        dl_col_names = [f["name"] for f in dl_fields]
        assert set(dl_col_names) == set(expected_col_names), (
            f"[{tag}] logical columns mismatch: deltalens={dl_col_names} vs Spark table_content={expected_col_names}"
        )

        active_raw_rows = sum(pq.read_metadata(p).num_rows for p in dt_files)
        net_active_rows = active_raw_rows - dl["health"]["deleted_rows_in_dvs"]
        assert net_active_rows == expected_rows, (
            f"[{tag}] net active row count mismatch: "
            f"raw_parquet_rows({active_raw_rows}) - deleted_rows_in_dvs({dl['health']['deleted_rows_in_dvs']}) "
            f"= {net_active_rows} vs Spark DAT expected_rows={expected_rows}"
        )

        if case_dir.name == "deletion_vectors":
            assert dl["health"]["deletion_vector_files"] >= 1, (
                f"[{tag}] expected deletion_vector_files >= 1"
            )
            assert dl["health"]["deleted_rows_in_dvs"] > 0, (
                f"[{tag}] expected deleted_rows_in_dvs > 0"
            )

        checked_snapshots += 1

    # 5. Multi-version diff checks across historical Spark snapshots.
    sorted_vers = sorted(seen_versions.keys())
    if len(sorted_vers) >= 2:
        v_old, v_new = sorted_vers[0], sorted_vers[-1]
        dl_diff = run_deltalens_json(
            binary, ["diff", str(delta_dir), "--v1", str(v_old), "--v2", str(v_new)]
        )
        assert dl_diff["v1"] == v_old and dl_diff["v2"] == v_new, (
            f"[{case_dir.name}] diff version range mismatch: {dl_diff}"
        )
        net_delta = dl_diff["files_added"] - dl_diff["files_removed"]
        expected_delta = (
            seen_versions[v_new]["health"]["total_files"]
            - seen_versions[v_old]["health"]["total_files"]
        )
        assert net_delta == expected_delta, (
            f"[{case_dir.name}] diff net file delta ({net_delta}) != snapshot delta ({expected_delta})"
        )
        checked_diffs += 1

    if case_dir.name == "with_schema_change":
        dl_diff = run_deltalens_json(
            binary, ["diff", str(delta_dir), "--v1", "0", "--v2", str(max(sorted_vers))]
        )
        assert len(dl_diff["schema_changes"]) >= 1, (
            f"[{case_dir.name}] expected schema_changes >= 1 between v0 and latest"
        )
        checked_diffs += 1

    # 6. Lineage check on latest.
    dl_lineage = run_deltalens_json(binary, ["lineage", str(delta_dir), "--last", "20"])
    assert isinstance(dl_lineage, list) and len(dl_lineage) >= 1, (
        f"[{case_dir.name}] expected at least 1 lineage entry"
    )
    return checked_snapshots, checked_diffs


def run_cases(binary: str, generated: Path) -> None:
    cases = sorted(p for p in generated.iterdir() if p.is_dir())
    assert len(cases) >= 19, f"Expected at least 19 Spark DAT tables, found {len(cases)}"
    total_snapshots = 0
    total_diffs = 0
    for case in cases:
        n_snaps, n_diffs = verify_case(binary, case)
        total_snapshots += n_snaps
        total_diffs += n_diffs
        print(
            f"PASS spark-dat/{case.name} ({n_snaps} snapshot{'s' if n_snaps != 1 else ''}"
            f"{f', {n_diffs} diff' if n_diffs else ''})"
        )
    print(
        f"ALL {len(cases)} SPARK DAT TABLES ({total_snapshots} SNAPSHOTS, {total_diffs} DIFFS) PASSED"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Run deep Spark DAT conformance tests for deltalens")
    parser.add_argument(
        "--dat-dir",
        type=Path,
        default=None,
        help="Optional directory holding extracted DAT corpus (downloaded + SHA-256 verified if absent)",
    )
    args = parser.parse_args()

    binary = find_deltalens_bin()
    print(f"Running deep Apache Spark DAT {DAT_VERSION} conformance suite with {binary}")

    if args.dat_dir is not None:
        generated = ensure_dat_extracted(args.dat_dir)
        run_cases(binary, generated)
    else:
        with tempfile.TemporaryDirectory(prefix="deltalens-dat-") as tmp:
            generated = ensure_dat_extracted(Path(tmp))
            run_cases(binary, generated)


if __name__ == "__main__":
    main()
