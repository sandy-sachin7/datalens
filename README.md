# DeltaLens

**Standalone CLI to inspect, diff, trace, and audit Delta Lake tables from your terminal — no Spark, no JVM, no Python.**

```bash
deltalens inspect ./transactions                           # table health, checkpoints, tombstones & skew
deltalens diff s3://my-bucket/transactions --v1 10 --v2 20 # schema & file diff on S3
deltalens lineage ./transactions --last 10 --op WRITE      # who wrote what and when
deltalens audit ./transactions --since 2026-04-01 --op DELETE,MERGE
deltalens schema gs://my-bucket/transactions --history     # recursive schema evolution on GCS
```

Native Rust binary that reads Delta Lake transaction logs directly — including JSON commits, classic and multi-part Parquet checkpoints, V2 checkpoints with `_sidecars/`, log compaction files (`.compacted.json`), Deletion Vectors, Column Mapping, and Liquid Clustering.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![CI](https://github.com/sandy-sachin7/datalens/actions/workflows/ci.yml/badge.svg)](https://github.com/sandy-sachin7/datalens/actions)

---

## Install

**Linux & macOS (one-liner)**

```bash
curl -fsSL https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.sh | bash
```

Auto-detects OS and architecture, downloads from GitHub Releases, and strictly verifies SHA-256 checksums before installing.

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.ps1 | iex
```

**Cargo**

```bash
# Full build with S3, GCS, and Azure object storage backends (default)
cargo install deltalens

# Local-only minimal build (no cloud SDKs)
cargo install deltalens --no-default-features
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

| Approach | Cold start | Runtime required | Command |
| :--- | :--- | :--- | :--- |
| Spark SQL | 10–30s | JVM + Spark cluster | `DESCRIBE HISTORY` |
| Databricks SQL | 5–15s | Active SQL warehouse | `DESCRIBE HISTORY` |
| delta-rs (Python) | 1–3s | Python + venv + wheels | `DeltaTable.history()` |
| **DeltaLens** | **<10ms** | **Single binary** | `deltalens lineage` |

DeltaLens reads only `_delta_log/` metadata (checkpoints, sidecars, compacted logs, and JSON commits) — not Parquet data files. That answers observability questions (file counts, size skew, active tombstones, Deletion Vectors, schema evolution, breaking changes, writer attribution, partition layout) in milliseconds.

**Use DeltaLens when you need:**
- CI/CD health gates (`--fail-on-issues`) and breaking-schema gates (`--fail-on-breaking`) without a Spark cluster
- Fast local or cloud (`s3://`, `s3a://`, `gs://`, `az://`, `abfss://`) debugging from your terminal
- Scriptable `--json` output piped into `jq` for automated monitoring

**Use Spark or delta-rs when you need to read row data, join across tables, or run SQL.**

---

## Commands

| Command | What it does | Key flags |
| :--- | :--- | :--- |
| `inspect <path>` | Table health, file skew, tombstones, DVs, maintenance & protocol features | `--version <n>`, `--fail-on-issues` |
| `diff <path>` | File deltas (logical & physical), partition diffs, recursive schema diffs | `--v1 <n> --v2 <n>`, `--schema-only`, `--files-only`, `--fail-on-breaking` |
| `lineage <path>` | Writer attribution, row counts & operation timeline | `--last <n>`, `--since <YYYY-MM-DD>`, `--op <ops>`, `--user <u>` |
| `audit <path>` | Compliance audit trail, filterable by date range, op, and user | `--since <YYYY-MM-DD>`, `--until <YYYY-MM-DD>`, `--op <ops>`, `--user <u>` |
| `schema <path>` | Active column layout or full recursive schema evolution history | `--history`, `--at <v>` |
| `config` | Manage settings and local telemetry | `show`, `path`, `metrics`, `set <key> <value>` |

### Global flags

| Flag | Effect |
| :--- | :--- |
| `--json` | Machine-readable JSON output (supported across all commands including `config`) |
| `--plain` | Strictly ANSI-free plain text (also honors `NO_COLOR` and non-TTY stdout) |
| `--no-header` | Omit table headers and title banners |
| `-v`, `--verbose` | Print checkpoint discovery, file counts, and parse warnings to stderr |

### Exit codes

- `0`: Success
- `1`: Error (invalid arguments, missing table/version, version gap, storage/auth error)
- `2`: CI gate triggered (`inspect --fail-on-issues` found issues, or `diff --fail-on-breaking` detected breaking schema changes)

---

## Usage examples

### CI/CD quality & schema gates

```yaml
- name: Gate on table health and breaking schema changes
  run: |
    deltalens inspect ./delta-table --plain --fail-on-issues
    deltalens diff ./delta-table --v1 10 --v2 20 --fail-on-breaking
```

### Cloud object storage

DeltaLens supports `s3://`, `s3a://`, `s3n://`, `gs://`, `az://`, `abfs://`, `abfss://`, and `file://` URIs via `object_store` (bounded async concurrency on Tokio):

```bash
deltalens inspect s3://my-bucket/warehouse/transactions
deltalens diff gs://my-bucket/warehouse/transactions --v1 100 --v2 150
deltalens schema abfss://container@account.dfs.core.windows.net/transactions --history
```

#### S3 environment variables

| Setting | Environment variable(s) |
| :--- | :--- |
| Access keys | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN` |
| Region | `AWS_REGION` or `AWS_DEFAULT_REGION` |
| Custom endpoint | `AWS_ENDPOINT` (MinIO, Cloudflare R2, LocalStack) |
| Plain HTTP (local dev) | `AWS_ALLOW_HTTP=true` |
| IAM / OIDC roles | Automatic on EC2/ECS/EKS, or `AWS_WEB_IDENTITY_TOKEN_FILE` + `AWS_ROLE_ARN` |

*(Note: `object_store` reads environment variables and IMDS/OIDC directly; if using a named AWS CLI profile, export credentials to the environment first.)*

---

## Telemetry

DeltaLens includes opt-in, strictly local telemetry. Nothing is collected or transmitted by default.

```bash
deltalens config set telemetry true   # opt in
deltalens config metrics              # view P50/P95/P99 latencies and command counts
deltalens config metrics --json       # machine-readable metrics summary
```

Metrics are written locally under `$DELTALENS_DATA_DIR`, `$XDG_DATA_HOME/deltalens`, `~/.local/share/deltalens`, or `%LOCALAPPDATA%\deltalens`. Nothing is ever sent over the network.

---

## Architecture

1. **Single-listing discovery**: Lists `_delta_log/` non-recursively once per command, cataloging strict 20-digit commits, classic/multi-part Parquet checkpoints, V2 JSON/Parquet checkpoints, `_last_checkpoint`, and `.compacted.json` files.
2. **Checkpoint-accelerated replay**: Snapshot commands (`inspect`, `schema`, `diff`) start from the latest complete checkpoint `<= target_version`, resolve V2 `_sidecars/` Parquet files, and replay only subsequent commits.
3. **Concurrent I/O + parallel parse**: Cloud backends batch-read with bounded Tokio `buffer_unordered` concurrency; local disk and JSON/Parquet parsing parallelize via Rayon.
4. **Protocol-compliant state machine**: Applies same-commit `remove` before `add`, percent-decodes paths, keys active files by `(canonical_path, dv_unique_id)`, enriches missing `Remove.size` from prior state, and tracks active tombstones, live partitions, protocol features, and `delta.clustering` domain metadata.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

MIT. See [LICENSE](LICENSE).
