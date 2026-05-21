# DeltaLens

**Inspect, diff, trace, and audit Delta Lake tables from your terminal — no Spark, no JVM, no Python.**

```bash
deltalens inspect ./transactions     # table health report
deltalens diff ./transactions --v1 10 --v2 20   # what changed
deltalens lineage ./transactions --last 10      # who wrote what
deltalens audit ./transactions --since 2026-04-01  # audit trail
deltalens schema ./transactions --history       # schema evolution
```

Native binary, ~3 MB, zero runtime dependencies. Parses `_delta_log/*.json` commit files in parallel across all CPU cores.

---

## Why not just use Spark?

The most common way to inspect a Delta table today is:

| Approach | Cold start | Dependencies | Command |
|---|---|---|---|
| **Spark SQL** | 10-30s (JVM) | Spark cluster + drivers | `DESCRIBE HISTORY` |
| **Databricks SQL** | 5-15s (warehouse) | Active SQL warehouse | `DESCRIBE HISTORY` |
| **delta-rs (Python)** | 1-3s | Python + venv + deps | `DeltaTable.history()` |
| **deltalens** | **<5ms** | **None** | `deltalens lineage` |

DeltaLens trades query capability for speed and zero setup. It cannot read Parquet data or run SQL. It reads only the transaction log — which answers 90% of observability questions (file counts, sizes, schema changes, operations, writers, timestamps, partition evolution) in **milliseconds**, not seconds.

Use cases where DeltaLens wins:
- **CI/CD pipelines** — assert table health in a GitHub Action without a Spark cluster
- **Local debugging** — inspect a table on your laptop without spinning up a notebook
- **Rapid investigation** — "what changed between v10 and v20?" in 46ms
- **Scripting** — pipe `--json` output into `jq` for automated monitoring

Use Spark/delta-rs when you need to read actual row data, join across tables, or run SQL.

---

## Install

### One-liner (Linux & macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.sh | bash
```

Auto-detects OS and architecture, downloads from GitHub Releases, verifies SHA256.

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.ps1 | iex
```

### Cargo

```bash
cargo install deltalens
```

### Build from source

```bash
git clone https://github.com/sandy-sachin7/datalens.git
cd datalens
cargo build --release
./target/release/deltalens --help
```

---

## Quick start

Point it at any Delta table (local filesystem path containing `_delta_log/`):

```bash
deltalens inspect /path/to/delta-table
```

Output includes version, schema, file health, partition info, maintenance state, and warnings:

```
┌──────────────────────────────────────────────────────────────┐
│  deltalens · Table Health Report                             │
│  /path/to/delta-table                                        │
├──────────────────────────────────────────────────────────────┤
│  Current Version      │  142                                 │
│  Created              │  2024-11-03 09:12:44 UTC             │
│  Last Modified        │  2026-04-13 18:33:01 UTC             │
│  Protocol             │  reader=1, writer=2                  │
│  DATA FILES           │                                      │
│  Total Files          │  8,432                               │
│  Total Size           │  147.3 GB                             │
│  Avg File Size        │  17.4 MB                              │
│  Min File Size        │  12 KB     ← ⚠ HIGH SKEW            │
│  Max File Size        │  891 MB    ← ⚠ HIGH SKEW            │
│  Small Files (<32MB)  │  3,201 (38%) ← ⚠ WARNING            │
│  Skew Score           │  0.73 / 1.0 ← ⚠ HIGH                │
│  PARTITIONS           │                                      │
│  Partition Columns    │  year, month                          │
│  Partition Count      │  84                                  │
│  MAINTENANCE          │                                      │
│  Last VACUUM          │  v128 (12 commits ago)                │
│  Last OPTIMIZE        │  v135 (5 commits ago)                 │
│  Last CHECKPOINT      │  v140                                │
│  Z-Order Columns      │  customer_id, event_type              │
│  SCHEMA               │                                      │
│  Column Count         │  23                                  │
│  Schema Changes       │  7 (since inception)                  │
└──────────────────────────────────────────────────────────────┘

⚠ 2 issues detected.
```

### Compare two versions

```bash
deltalens diff /path/to/table --v1 100 --v2 142
```

Shows schema changes, file adds/removes, and operation breakdown between versions.

### Trace operation lineage

```bash
deltalens lineage /path/to/table --last 20
```

Attribution table: version, timestamp, operation type, writer engine, data volume, row count.

### Audit trail

```bash
deltalens audit /path/to/table --since 2026-01-01 --op DELETE,MERGE
```

Filtered view of mutations with predicates and user attribution.

### Schema evolution

```bash
deltalens schema /path/to/table --history
```

Complete timeline of every column addition, removal, and type change.

### Machine-readable output

```bash
deltalens inspect /path/to/table --json | jq '.health.skew_score'
```

All commands support `--json` for piping into scripts and `--plain` for CI/CD logs.

---

## Use cases

### CI/CD pipeline assertions

```yaml
# GitHub Action: fail if small-file ratio exceeds 25%
- run: deltalens inspect ./delta-table --json > health.json
- run: |
    SKEW=$(jq '.health.small_file_pct' health.json)
    if (( $(echo "$SKEW > 25.0" | bc -l) )); then
      echo "Small file ratio $SKEW% exceeds 25% — run OPTIMIZE"
      exit 1
    fi
```

### Local debugging

```bash
# "This table is slow — why?"
deltalens inspect ./transactions       # check file skew + vacuum status
deltalens schema ./transactions --history  # recent schema changes?

# "Who wrote version 134?"
deltalens lineage ./transactions --last 5 | grep v134
```

### Notebook investigation

```bash
# Databricks notebook cell — quick table check without spark.sql()
!deltalens inspect /dbfs/user/hive/warehouse/transactions --json
```

---

## Commands

| Command | What it does | Key flags |
|---|---|---|
| `inspect <path>` | Table health, file skew, maintenance, schema stats | `--version <n>` |
| `diff <path>` | Changes between two versions | `--v1 <n> --v2 <n>` |
| `lineage <path>` | Writer attribution + operation timeline | `--last <n> --since <d>` |
| `audit <path>` | Filtered security/compliance audit | `--since <d> --until <d>` |
| `schema <path>` | Column layout + evolution history | `--history --at <v>` |
| `config` | Manage settings + view telemetry | `show`, `path`, `metrics`, `set` |

### Global flags

| Flag | Effect |
|---|---|
| `--json` | Machine-readable JSON output |
| `--plain` | No ANSI colors (CI/CD logs) |
| `--no-header` | Skip visual headers |
| `--verbose` | Internal debug info |

---

## Performance

DeltaLens parses commit files in parallel using all available CPU cores (Rayon). Each commit file is independent JSON — embarrassingly parallel.

### Methodology

Benchmarks run on an x86_64 machine with 4 cores (8 threads). Results are median of 10 runs via Criterion. Fixtures are synthesized with `scripts/gen_fixture.py`. Cold filesystem cache (page cache dropped between runs).

### Results

| Fixture | Commits | Add/Remove actions | Parse latency | CLI E2E |
|---|---|---|---|---|
| Small | 100 | 1,000 | 444 µs | 5 ms |
| Medium | 1,000 | 50,000 | 7.3 ms | 46 ms |
| Large | 5,000 | 500,000 | 67 ms | 449 ms |

> Throughput: **~7.4M actions/second** on the large fixture.

### Reproduce on your machine

```bash
python3 scripts/gen_fixture.py   # generates small/medium/large fixtures
cargo bench                       # runs Criterion benchmark suite
```

---

## Telemetry & config

DeltaLens includes opt-in telemetry for understanding real-world performance. No data is collected by default.

### Enable

```bash
deltalens config set telemetry true
```

### What's recorded

For each command with telemetry enabled:
- Command name, version, duration
- File and action counts (no table paths, no data)
- Timestamp (no user/machine identifiers)

Stored locally at `~/.local/share/deltalens/metrics.jsonl`. Never sent to any remote server.

### View collected metrics

```bash
deltalens config metrics
```

Shows aggregate latency distribution (P50, P95, P99) and command counts. Share your anonymized benchmarks with the community via GitHub Issues.

### Config file

```
~/.config/deltalens/config.json    # Linux/macOS
$DELTALENS_CONFIG_DIR/config.json  # override
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All participants must follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## License

MIT. See [LICENSE](LICENSE).
