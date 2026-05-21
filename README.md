# DeltaLens

**Inspect, diff, trace, and audit Delta Lake tables from your terminal — no Spark, no JVM, no Python.**

```
deltalens inspect ./transactions              # table health report
deltalens diff ./transactions --v1 10 --v2 20 # what changed between versions
deltalens lineage ./transactions --last 10    # who wrote what and when
deltalens audit ./transactions --since 2026-04-01 --op DELETE,MERGE
deltalens schema ./transactions --history     # full schema evolution
```

Native Rust binary, ~1.6 MB, zero runtime dependencies. Parses `_delta_log/*.json` commit files in
parallel across all CPU cores using Rayon's work-stealing scheduler.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![CI](https://github.com/sandy-sachin7/datalens/actions/workflows/ci.yml/badge.svg)](https://github.com/sandy-sachin7/datalens/actions)

---

## Install

**Linux & macOS (one-liner)**

```bash
curl -fsSL https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.sh | bash
```

Auto-detects OS and architecture, downloads from GitHub Releases, verifies SHA256.

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.ps1 | iex
```

**Cargo**

```bash
cargo install deltalens
```

**Build from source**

```bash
git clone https://github.com/sandy-sachin7/datalens.git
cd datalens
cargo build --release
./target/release/deltalens --help
```

---

## Why not just use Spark?

The standard way to inspect a Delta table today requires an active runtime:

| Approach | Cold start | Dependencies | Command |
| :--- | :--- | :--- | :--- |
| Spark SQL | 10–30s (JVM) | Spark cluster + drivers | `DESCRIBE HISTORY` |
| Databricks SQL | 5–15s (warehouse) | Active SQL warehouse | `DESCRIBE HISTORY` |
| delta-rs (Python) | 1–3s | Python + venv + deps | `DeltaTable.history()` |
| **DeltaLens** | **<5ms** | **None** | `deltalens lineage` |

DeltaLens trades query capability for speed and zero setup. It reads only the transaction log — not
Parquet data, not row-level content. That constraint answers 90% of observability questions (file
counts, sizes, schema changes, operations, writers, timestamps, partition layout) in milliseconds
instead of seconds.

**Use DeltaLens when you need:**
- CI/CD pipeline assertions without a Spark cluster
- Local debugging on your laptop without a notebook
- Rapid investigation ("what changed between v10 and v20?" in 46ms)
- Scriptable `--json` output piped into `jq` for automated monitoring

**Use Spark or delta-rs when you need to read actual row data, join across tables, or run SQL.**

---

## Quick start

Point DeltaLens at any directory containing a `_delta_log/` folder:

```bash
deltalens inspect /path/to/delta-table
```

```
┌──────────────────────────────────────────────────────────────┐
│  DeltaLens · Table Health Report                             │
│  /path/to/delta-table                                        │
├──────────────────────────────────────────────────────────────┤
│  Current Version      │  142                                 │
│  Created              │  2024-11-03 09:12:44 UTC             │
│  Last Modified        │  2026-04-13 18:33:01 UTC             │
│  Protocol             │  reader=1, writer=2                  │
│  DATA FILES           │                                      │
│  Total Files          │  8,432                               │
│  Total Size           │  147.3 GB                            │
│  Avg File Size        │  17.4 MB                             │
│  Min File Size        │  12 KB    ← ⚠ HIGH SKEW             │
│  Max File Size        │  891 MB   ← ⚠ HIGH SKEW             │
│  Small Files (<32MB)  │  3,201 (38%) ← ⚠ WARNING            │
│  Skew Score           │  0.73 / 1.0  ← ⚠ HIGH              │
│  PARTITIONS           │                                      │
│  Partition Columns    │  year, month                         │
│  Partition Count      │  84                                  │
│  MAINTENANCE          │                                      │
│  Last VACUUM          │  v128 (12 commits ago)               │
│  Last OPTIMIZE        │  v135 (5 commits ago)                │
│  Last CHECKPOINT      │  v140                                │
│  Z-Order Columns      │  customer_id, event_type             │
│  SCHEMA               │                                      │
│  Column Count         │  23                                  │
│  Schema Changes       │  7 (since inception)                 │
└──────────────────────────────────────────────────────────────┘

⚠  2 issues detected.
```

---

## Commands

| Command | What it does | Key flags |
| :--- | :--- | :--- |
| `inspect <path>` | Table health, file skew, maintenance state, schema stats | `--version <n>` |
| `diff <path>` | File adds/removes and schema changes between two versions | `--v1 <n> --v2 <n>` |
| `lineage <path>` | Writer attribution and operation timeline | `--last <n>`, `--since <date>` |
| `audit <path>` | Security and compliance audit trail, filterable by op and user | `--since <date>`, `--until <date>`, `--op <ops>`, `--user <u>` |
| `schema <path>` | Active column layout and full schema evolution history | `--history`, `--at <v>` |
| `config` | Manage settings and local telemetry | `show`, `path`, `metrics`, `set` |

### Global flags

| Flag | Effect |
| :--- | :--- |
| `--json` | Machine-readable JSON output |
| `--plain` | No ANSI colors — for CI/CD logs and plain text storage |
| `--no-header` | Skip visual headers |
| `--verbose` | Internal debug info |

---

## Usage examples

### CI/CD pipeline assertions

Fail a GitHub Action if small-file ratio exceeds a threshold — without spinning up a warehouse:

```yaml
- name: Assert Delta table health
  run: |
    deltalens inspect ./delta-table --json > health.json
    SKEW=$(jq '.health.small_file_pct' health.json)
    if (( $(echo "$SKEW > 25.0" | bc -l) )); then
      echo "Small file ratio ${SKEW}% exceeds 25% — run OPTIMIZE before merging"
      exit 1
    fi
```

### Local debugging

```bash
# "This table is slow — why?"
deltalens inspect ./transactions            # check file skew and vacuum state
deltalens schema ./transactions --history   # any recent schema changes?

# "Who wrote version 134?"
deltalens lineage ./transactions --last 5

# "What exactly changed between the last two releases?"
deltalens diff ./transactions --v1 130 --v2 142
```

### Databricks notebook cell

```python
# Quick table check without spark.sql() — paste into any notebook cell
import subprocess, json

result = subprocess.run(
    ["deltalens", "inspect", "/dbfs/user/hive/warehouse/transactions", "--json"],
    capture_output=True, text=True
)
health = json.loads(result.stdout)
print(f"Skew score: {health['health']['skew_score']} | Small files: {health['health']['small_file_pct']}%")
```

### JSON output and scripting

```bash
# Check skew score
deltalens inspect ./transactions --json | jq '.health.skew_score'

# List all DELETE operations in Q1 2026
deltalens audit ./transactions --since 2026-01-01 --until 2026-03-31 --op DELETE --json \
  | jq '.[] | {version, timestamp, user_name}'

# Count schema changes
deltalens schema ./transactions --history --json | jq 'length'
```

---

## Performance

DeltaLens parses commit files in parallel using Rayon's work-stealing scheduler. Each commit file
is independent JSON — the workload is embarrassingly parallel.

The JSON parser uses direct-to-struct deserialization, bypassing intermediate `serde_json::Value`
trees. This eliminates allocation pressure on the hot path and results in a 4–8x throughput
improvement over a naive parse-then-map approach.

### Benchmarks

Hardware: x86_64, 4 cores (8 threads). Methodology: median of 10 Criterion runs, cold filesystem
cache between runs (page cache dropped). Fixtures generated by `scripts/gen_fixture.py`.

| Fixture | Commits | Add/Remove actions | Parse latency (P99) | CLI wall-clock |
| :--- | :--- | :--- | :--- | :--- |
| Small | 100 | 1,000 | 444 µs | 5 ms |
| Medium | 1,000 | 50,000 | 7.3 ms | 46 ms |
| Large | 5,000 | 500,000 | 67 ms | 449 ms |

Throughput: ~7.4M actions/second on the large fixture.

### Reproduce

```bash
python3 scripts/gen_fixture.py   # generate small/medium/large fixtures
cargo bench                       # run Criterion suite
```

---

## Telemetry

DeltaLens includes opt-in, fully local telemetry. Nothing is collected or transmitted by default.

```bash
deltalens config set telemetry true   # opt in
deltalens config metrics              # view aggregated latency stats (P50, P95, P99)
```

When enabled, the following is recorded locally to `~/.local/share/deltalens/metrics.jsonl`:
- Command name, version, duration
- File and action counts
- Timestamp (no user identifiers, no table paths, no data)

Nothing is sent to any remote server.

---

## Architecture

DeltaLens reads only `_delta_log/*.json` commit files. It does not read Parquet data files and
cannot execute SQL or evaluate predicates against row data.

The parsing pipeline:

1. Enumerate commit files in the requested version range
2. Dispatch to a Rayon parallel iterator — one task per commit file
3. Each task: open file, stream-deserialize directly into typed action structs
4. Merge results into a single state representation
5. Render to ANSI table or serialize to JSON

There are no intermediate object trees, no heap-allocated `serde_json::Value` nodes on the hot
path, and no synchronization between parse tasks (merge is a final sequential reduce).

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All contributors must follow the [Code of Conduct](CODE_OF_CONDUCT.md).

Good first contributions: additional health metrics in `inspect`, new output fields in `lineage`,
or a cloud storage backend (S3, ADLS, GCS) via the `object_store` crate.

---

## License

MIT. See [LICENSE](LICENSE).
