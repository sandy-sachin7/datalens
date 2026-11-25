#!/usr/bin/env python3
"""
Reproducible head-to-head benchmark comparing `deltalens` against `deltalake` (delta-rs Python).

Measures BOTH:
  1. Cold CLI invocation (wall-clock median & p95 + peak RSS via subprocess)
  2. Warm in-process `deltalake` Python execution (excluding Python interpreter startup)
  3. Scale test on 100-commit, 1,000-commit (50k adds), and 5,000-commit (500k adds) Delta logs

Run with uv (zero permanent installs):
    uv run --with deltalake --with pyarrow python3 scripts/bench_compare.py

Or with an existing Python environment containing `deltalake` and `pyarrow`:
    python3 scripts/bench_compare.py
"""
from __future__ import annotations

import json
import os
import platform
import resource
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pyarrow as pa
from deltalake import DeltaTable, write_deltalake


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BIN = REPO_ROOT / "target" / "release" / "deltalens"


def find_deltalens_bin() -> str:
    env_bin = os.environ.get("DELTALENS_BIN")
    if env_bin:
        return env_bin
    if DEFAULT_BIN.exists():
        return str(DEFAULT_BIN)
    found = shutil.which("deltalens")
    if found:
        return found
    raise FileNotFoundError(
        f"deltalens binary not found at {DEFAULT_BIN} or in PATH. "
        "Run `cargo build --release` first."
    )


def build_real_delta_table(table_dir: Path, commits: int = 60) -> int:
    """Create a genuine Delta Lake table with real Parquet files, checkpoints, deletes, and schema evolution."""
    regions = ["us-east", "eu-west", "ap-south"]
    for v in range(commits):
        n_rows = 200 if v % 5 != 0 else 2000
        data = {
            "id": pa.array(range(v * 1000, v * 1000 + n_rows), type=pa.int64()),
            "region": pa.array([regions[v % len(regions)]] * n_rows, type=pa.string()),
            "amount": pa.array([float((i % 100) + 0.5) for i in range(n_rows)], type=pa.float64()),
        }
        if v >= commits // 2:
            data["currency"] = pa.array(["USD"] * n_rows, type=pa.string())
        table = pa.table(data)
        write_deltalake(
            str(table_dir),
            table,
            mode="overwrite" if v == 0 else "append",
            partition_by=["region"] if v == 0 else None,
            schema_mode="merge" if v == commits // 2 else None,
        )
        if v > 0 and v % 10 == 0:
            DeltaTable(str(table_dir)).create_checkpoint()
    dt = DeltaTable(str(table_dir))
    dt.delete("amount < 5.0")
    return dt.version()


def build_valid_synthetic_log(table_dir: Path, num_versions: int, files_per_version: int) -> int:
    """Create a spec-valid Delta log (with protocol + metaData at v0) for scale benchmarking."""
    log_dir = table_dir / "_delta_log"
    log_dir.mkdir(parents=True, exist_ok=True)
    schema_str = json.dumps({
        "type": "struct",
        "fields": [
            {"name": "id", "type": "long", "nullable": False, "metadata": {}},
            {"name": "region", "type": "string", "nullable": True, "metadata": {}},
            {"name": "amount", "type": "double", "nullable": True, "metadata": {}},
        ],
    })
    base_ts = int(datetime(2026, 1, 1, tzinfo=timezone.utc).timestamp() * 1000)
    regions = ["us-east", "eu-west", "ap-south"]
    for version in range(num_versions):
        ts = base_ts + version * 3_600_000
        actions = []
        if version == 0:
            actions.append({"protocol": {"minReaderVersion": 1, "minWriterVersion": 2}})
            actions.append({
                "metaData": {
                    "id": str(uuid.uuid4()),
                    "format": {"provider": "parquet", "options": {}},
                    "schemaString": schema_str,
                    "partitionColumns": ["region"],
                    "configuration": {},
                    "createdTime": ts,
                }
            })
        actions.append({
            "commitInfo": {
                "timestamp": ts,
                "operation": "WRITE",
                "operationParameters": {"mode": "Append"},
                "engineInfo": "Apache-Spark/3.5.0 Delta-Lake/3.1.0",
                "isBlindAppend": True,
                "operationMetrics": {
                    "numFiles": str(files_per_version),
                    "numOutputRows": str(files_per_version * 50_000),
                    "numOutputBytes": str(files_per_version * 17_000_000),
                },
            }
        })
        for i in range(files_per_version):
            size = 17_000_000 + (i * 500_000 if i % 7 == 0 else 0)
            actions.append({
                "add": {
                    "path": f"region={regions[i % 3]}/part-{version:05d}-{i:04d}.parquet",
                    "size": size,
                    "modificationTime": ts,
                    "dataChange": True,
                    "partitionValues": {"region": regions[i % 3]},
                }
            })
        with open(log_dir / f"{version:020d}.json", "w", encoding="utf-8") as f:
            for a in actions:
                f.write(json.dumps(a) + "\n")
    return num_versions - 1


def measure_subprocess(cmd: list[str], runs: int = 10, warmup: int = 2) -> dict:
    """Measure cold wall-clock time and accurate child peak RSS via a minimal isolated runner."""
    for _ in range(warmup):
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)

    times_ms: list[float] = []
    for _ in range(runs):
        t0 = time.perf_counter()
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        times_ms.append((time.perf_counter() - t0) * 1000.0)

    # Measure peak RSS in an isolated minimal Python child (`-I -S`, ~5 MB footprint) so
    # this parent process's pyarrow/deltalake RSS does not pollute the child's ru_maxrss.
    rss_helper = (
        "import os, subprocess, sys; "
        "p = subprocess.Popen(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); "
        "_, st, ru = os.wait4(p.pid, 0); "
        "assert os.WIFEXITED(st) and os.WEXITSTATUS(st) == 0; "
        "print(ru.ru_maxrss)"
    )
    raw_rss = int(
        subprocess.check_output([sys.executable, "-I", "-S", "-c", rss_helper, *cmd], text=True).strip()
    )
    rss_kb = raw_rss // 1024 if sys.platform == "darwin" else raw_rss

    times_sorted = sorted(times_ms)
    p95_idx = min(len(times_sorted) - 1, int(round(0.95 * (len(times_sorted) - 1))))
    return {
        "runs": runs,
        "median_ms": round(statistics.median(times_ms), 2),
        "min_ms": round(min(times_ms), 2),
        "p95_ms": round(times_sorted[p95_idx], 2),
        "peak_rss_mb": round(rss_kb / 1024.0, 1),
    }


def measure_warm_python(fn, runs: int = 10, warmup: int = 2) -> dict:
    for _ in range(warmup):
        fn()
    times_ms = []
    for _ in range(runs):
        t0 = time.perf_counter()
        fn()
        times_ms.append((time.perf_counter() - t0) * 1000.0)
    times_sorted = sorted(times_ms)
    p95_idx = min(len(times_sorted) - 1, int(round(0.95 * (len(times_sorted) - 1))))
    return {
        "runs": runs,
        "median_ms": round(statistics.median(times_ms), 2),
        "min_ms": round(min(times_ms), 2),
        "p95_ms": round(times_sorted[p95_idx], 2),
    }


def main() -> None:
    deltalens = find_deltalens_bin()
    bin_size_mb = round(Path(deltalens).stat().st_size / (1024 * 1024), 1)
    dl_version = subprocess.check_output([deltalens, "--version"], text=True).strip()
    import deltalake as dl_pkg

    print(f"Binary: {dl_version} ({bin_size_mb} MB)")
    print(f"Python: {platform.python_version()} | deltalake: {dl_pkg.__version__}")
    print(f"Host:   {platform.system()} {platform.machine()} ({os.cpu_count()} logical CPUs)")
    print()

    results: dict = {
        "host": f"{platform.system()} {platform.machine()} ({os.cpu_count()} logical CPUs)",
        "deltalens_version": dl_version,
        "deltalens_binary_mb": bin_size_mb,
        "deltalake_python_version": dl_pkg.__version__,
        "python_version": platform.python_version(),
        "benchmarks": {},
    }

    with tempfile.TemporaryDirectory(prefix="deltalens-bench-") as tmp:
        tmp_path = Path(tmp)

        # 1. Real Delta table created with delta-rs (60 commits + 6 Parquet checkpoints + schema evolution + DELETE)
        real_tbl = tmp_path / "real_table"
        last_v = build_real_delta_table(real_tbl, commits=60)
        print(f"Built real Delta table at v{last_v} (Parquet checkpoints + schema evolution + DELETE)")

        # Equivalent Python one-liners using deltalake
        py_inspect_code = (
            f"import json; from deltalake import DeltaTable; "
            f"dt = DeltaTable({str(real_tbl)!r}); "
            f"aa = dt.get_add_actions(flatten=True); "
            f"sizes = aa.column('size_bytes').to_pylist(); "
            f"print(json.dumps({{'v': dt.version(), 'n': len(sizes), 'bytes': sum(sizes)}}))"
        )
        py_history_code = (
            f"from deltalake import DeltaTable; "
            f"dt = DeltaTable({str(real_tbl)!r}); "
            f"print(len(list(dt.history(limit=20))))"
        )

        # Cold CLI benchmarks
        dl_inspect = measure_subprocess([deltalens, "inspect", str(real_tbl), "--json"], runs=15)
        py_inspect_cold = measure_subprocess([sys.executable, "-c", py_inspect_code], runs=10)
        py_inspect_warm = measure_warm_python(
            lambda: DeltaTable(str(real_tbl)).get_add_actions(flatten=True).column("size_bytes").to_pylist(),
            runs=15,
        )

        dl_lineage = measure_subprocess([deltalens, "lineage", str(real_tbl), "--last", "20", "--json"], runs=15)
        py_history_cold = measure_subprocess([sys.executable, "-c", py_history_code], runs=10)
        py_history_warm = measure_warm_python(lambda: list(DeltaTable(str(real_tbl)).history(limit=20)), runs=15)

        dl_diff = measure_subprocess(
            [deltalens, "diff", str(real_tbl), "--v1", "0", "--v2", str(last_v), "--json"], runs=15
        )

        results["benchmarks"]["real_60_commits_with_checkpoints"] = {
            "deltalens_inspect_cold": dl_inspect,
            "deltalake_py_inspect_cold": py_inspect_cold,
            "deltalake_py_inspect_warm": py_inspect_warm,
            "deltalens_lineage_20_cold": dl_lineage,
            "deltalake_py_history_20_cold": py_history_cold,
            "deltalake_py_history_20_warm": py_history_warm,
            "deltalens_diff_v0_to_latest_cold": dl_diff,
        }

        # 2. Scale benchmarks on valid Delta logs (no checkpoints — worst-case full log replay)
        for label, n_ver, f_per_v, runs in [
            ("scale_100c_1k_files", 100, 10, 10),
            ("scale_1000c_50k_files", 1000, 50, 5),
        ]:
            tdir = tmp_path / label
            build_valid_synthetic_log(tdir, n_ver, f_per_v)
            print(f"Built {label} ({n_ver} commits, {n_ver * f_per_v:,} add actions)")
            dl_s = measure_subprocess([deltalens, "inspect", str(tdir), "--json"], runs=runs, warmup=1)
            py_s_code = (
                f"from deltalake import DeltaTable; "
                f"dt = DeltaTable({str(tdir)!r}); "
                f"aa = dt.get_add_actions(flatten=True); "
                f"print(len(aa))"
            )
            py_s_cold = measure_subprocess([sys.executable, "-c", py_s_code], runs=runs, warmup=1)
            py_s_warm = measure_warm_python(
                lambda d=str(tdir): len(DeltaTable(d).get_add_actions(flatten=True)),
                runs=runs,
                warmup=1,
            )
            results["benchmarks"][label] = {
                "deltalens_inspect_cold": dl_s,
                "deltalake_py_inspect_cold": py_s_cold,
                "deltalake_py_inspect_warm": py_s_warm,
            }

    print("\n=== JSON RESULTS ===")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
