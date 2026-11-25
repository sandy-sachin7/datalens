#!/usr/bin/env python3
"""
Spark / Delta Acceptance Testing (DAT) conformance suite for `deltalens`.

Validates `deltalens` against official Apache Spark-generated Delta Lake tables from
`delta-incubator/dat` (v0.0.3, SHA-256 verified) across all 19 reader test tables and
all 29 expected snapshots (`latest` and historical `vN` versions).

Usage:
    cargo build --release --locked
    python3 scripts/dat_test.py [--dat-dir /path/to/extracted/dat]
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
import urllib.request
from pathlib import Path

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


def verify_case(binary: str, case_dir: Path) -> int:
    delta_dir = case_dir / "delta"
    expected_root = case_dir / "expected"
    snapshots = sorted(p for p in expected_root.iterdir() if p.is_dir())
    checked = 0

    for snap_dir in snapshots:
        meta_path = snap_dir / "table_version_metadata.json"
        if not meta_path.exists():
            continue
        expected = json.loads(meta_path.read_text(encoding="utf-8"))
        exp_ver = expected["version"]
        exp_mr = expected["min_reader_version"]
        exp_mw = expected["min_writer_version"]
        exp_props = expected.get("properties", {})

        inspect_args = ["inspect", str(delta_dir)]
        schema_args = ["schema", str(delta_dir)]
        if snap_dir.name != "latest":
            inspect_args += ["--version", str(exp_ver)]
            schema_args += ["--at", str(exp_ver)]

        dl = run_deltalens_json(binary, inspect_args)
        assert dl["current_version"] == exp_ver, (
            f"[{case_dir.name}/{snap_dir.name}] version mismatch: "
            f"deltalens={dl['current_version']} vs Spark DAT={exp_ver}"
        )
        assert dl["min_reader_version"] == exp_mr, (
            f"[{case_dir.name}/{snap_dir.name}] min_reader_version mismatch: "
            f"deltalens={dl['min_reader_version']} vs Spark DAT={exp_mr}"
        )
        assert dl["min_writer_version"] == exp_mw, (
            f"[{case_dir.name}/{snap_dir.name}] min_writer_version mismatch: "
            f"deltalens={dl['min_writer_version']} vs Spark DAT={exp_mw}"
        )
        dl_props = dl.get("table_properties") or dl.get("configuration") or {}
        if dl_props:
            for k, v in exp_props.items():
                assert dl_props.get(k) == v, (
                    f"[{case_dir.name}/{snap_dir.name}] property '{k}' mismatch: "
                    f"deltalens={dl_props.get(k)!r} vs Spark DAT={v!r}"
                )

        # Ensure schema and lineage also succeed cleanly against Spark-written logs.
        dl_schema = run_deltalens_json(binary, schema_args)
        assert isinstance(dl_schema["schema"]["fields"], list) and len(dl_schema["schema"]["fields"]) > 0, (
            f"[{case_dir.name}/{snap_dir.name}] empty schema returned"
        )
        checked += 1

    # Also check lineage on latest.
    dl_lineage = run_deltalens_json(binary, ["lineage", str(delta_dir), "--last", "20"])
    assert isinstance(dl_lineage, list) and len(dl_lineage) >= 1, (
        f"[{case_dir.name}] expected at least 1 lineage entry"
    )
    return checked


def main() -> None:
    parser = argparse.ArgumentParser(description="Run Spark DAT conformance tests for deltalens")
    parser.add_argument(
        "--dat-dir",
        type=Path,
        default=None,
        help="Optional directory holding extracted DAT corpus (downloaded + SHA-256 verified if absent)",
    )
    args = parser.parse_args()

    binary = find_deltalens_bin()
    print(f"Running Apache Spark DAT {DAT_VERSION} conformance suite with {binary}")

    if args.dat_dir is not None:
        generated = ensure_dat_extracted(args.dat_dir)
        run_cases(binary, generated)
    else:
        with tempfile.TemporaryDirectory(prefix="deltalens-dat-") as tmp:
            generated = ensure_dat_extracted(Path(tmp))
            run_cases(binary, generated)


def run_cases(binary: str, generated: Path) -> None:
    cases = sorted(p for p in generated.iterdir() if p.is_dir())
    assert len(cases) >= 19, f"Expected at least 19 Spark DAT tables, found {len(cases)}"
    total_snapshots = 0
    for case in cases:
        n = verify_case(binary, case)
        total_snapshots += n
        print(f"PASS spark-dat/{case.name} ({n} snapshot{'s' if n != 1 else ''})")
    print(f"ALL {len(cases)} SPARK DAT TABLES ({total_snapshots} SNAPSHOTS) PASSED")


if __name__ == "__main__":
    main()
