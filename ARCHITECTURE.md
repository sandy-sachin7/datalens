# DeltaLens Architecture

DeltaLens is a strictly read-only Delta Lake transaction log analyzer written in Rust. It operates exclusively on `_delta_log/` metadata — never reading Parquet data files — to deliver millisecond-scale observability locally and across cloud object stores.

## Module Map

```text
src/
├── lib.rs                      # Crate documentation & public API re-exports
├── main.rs                     # CLI bootstrap, exit codes (0 / 1 / 2), opt-in telemetry hook
├── error.rs                    # Structured DeltaLensError (thiserror)
├── cli/
│   ├── args.rs                 # Clap command definitions & global flags
│   ├── output_opts.rs          # OutputOpts & CommandOutcome (exit code propagation)
│   └── commands/               # inspect, diff, lineage, audit, schema, config
├── core/
│   ├── storage.rs              # StorageBackend trait, LocalStorage (Unix/Windows hardened), ObjectStoreBackend
│   ├── reader.rs               # DeltaLogReader, single-listing LogInventory, _last_checkpoint offset seek
│   ├── replay.rs               # Unified Delta TableState machine, FileKey, percent-decoding, tombstones
│   ├── model/                  # Action, Schema (StructType/DataType), Stats, Lineage structs
│   ├── parser/
│   │   ├── commit.rs           # Streaming NDJSON commit parser with structured ParseWarnings
│   │   └── checkpoint.rs       # Arrow/Parquet checkpoint & sidecar parser, _last_checkpoint parser
│   ├── analyzer/               # HealthAnalyzer, DiffAnalyzer, LineageTracer, SchemaTracker
│   ├── config.rs               # User config persistence (~/.config/deltalens)
│   └── metrics.rs              # Strictly local opt-in telemetry (JSONL)
└── render/
    ├── table.rs                # comfy-table + colored terminal renderer (ANSI-free in --plain / NO_COLOR)
    └── json.rs                 # Deterministic serde_json renderer
```

## Log Discovery & `_last_checkpoint` Offset Listing

1. **Snapshot & Diff Fast Path (`try_snapshot_inventory`)**:
   - Before performing a full directory listing, `read_snapshot_log` and `read_diff_logs` read `_delta_log/_last_checkpoint`.
   - If the hint points to version `V > 0`, the target version (or `v1` for `diff`) is `>= V`, and the backend supports offset listing (`ObjectStoreBackend::list_from_offset` via `object_store::ObjectStore::list_with_offset`), DeltaLens lists only immediate entries lexicographically greater than `"{V:020}"`.
   - On tables with tens of thousands of historical JSON commits, this reduces cloud `ListObjectsV2` calls from dozens of pages to a single page.
   - If `_last_checkpoint` is missing, unreadable, stale, points to an incomplete multi-part checkpoint, or the requested version precedes `V`, DeltaLens seamlessly falls back to a full single listing.
2. **Single-Listing Cached Inventory (`inventory()`)**:
   - Calls `storage.list("_delta_log")` exactly once per `DeltaLogReader` instance and memoizes the result in a `OnceLock<LogInventory>`.
   - Classifies strict 20-digit commits (`00000000000000000000.json`), classic Parquet checkpoints, complete multi-part Parquet checkpoints (`part-00001-of-N`), V2 JSON/Parquet checkpoints (preferring V2 when co-located with classic checkpoints), and `.compacted.json` range files.
   - Contiguous range replay prefers the widest `.compacted.json` starting at the current cursor that fits within the target bound, and fails fast with `VersionGap` if a required commit is missing.

## Concurrency Model (Tokio vs. Rayon)

DeltaLens separates I/O concurrency from CPU concurrency to guarantee **zero nested Tokio runtimes**:

| Workload | Execution Engine | Notes |
| :--- | :--- | :--- |
| Cloud object reads (`s3://`, `gs://`, `az://`, `abfss://`) | Shared process-wide `OnceLock<tokio::runtime::Runtime>` | `read_many` streams object fetches with `futures::stream::buffer_unordered(32)` |
| Local filesystem reads | Rayon (`par_iter`) | Parallel descriptor-pinned file reads |
| NDJSON commit & Arrow/Parquet checkpoint parsing | Rayon (`par_iter`) | Pure CPU-bound parsing after all bytes are fetched into memory |

`DeltaLogReader::load_and_parse_commits` and `read_checkpoint` always complete `storage.read_many(&paths)` **first**, then hand the owned byte buffers to Rayon for parallel parsing. This guarantees Rayon worker threads never enter `rt.block_on(...)` on cloud backends.

## Unified `TableState` Replay State Machine

All snapshot-based analyzers (`HealthAnalyzer`, `DiffAnalyzer`, `SchemaTracker`) materialize state via `src/core/replay.rs`:

1. **Checkpoint Seeding (`apply_checkpoint`)**: Applies metadata, protocol, transaction IDs, domain metadata (`delta.clustering`), and retained `remove` tombstones first, then applies active `add` actions so live files always win.
2. **Same-Commit `Remove`-Before-`Add` (`apply_commit`)**: Within every commit, `Remove` actions are processed in Phase 1 and `Add` actions in Phase 2. Operations that emit `Add` and `Remove` for the same physical file in one commit (e.g., `RESTORE`, Deletion Vector updates, re-adds) therefore never drop a live file.
3. **Canonical File Keying (`FileKey`)**: Active files and tombstones are keyed by `(percent_decoded_path, deletion_vector_unique_id)`:
   - RFC 3986 percent escapes (e.g., `region=us%20east/part%2D0.parquet`) are decoded and leading `./` stripped before matching.
   - Including `dv_unique_id` ensures a file updated with a new Deletion Vector is tracked distinctly from its prior un-DV'd or older-DV entry.
4. **Metadata Enrichment & Saturating Math**: When a `Remove` omits `size` or `partitionValues`, `TableState` enriches them from the prior active `Add`. All byte and file counters use saturating arithmetic (`saturating_add` / `saturating_sub`) to prevent overflow on malformed logs.
5. **Tombstone Retention**: Parses `delta.deletedFileRetentionDuration` (defaulting to 7 days) and computes both total reclaimable bytes and expired tombstones eligible for `VACUUM`.
6. **Recursive Schema Evolution**: `SchemaTracker` diffs `StructType`, `ArrayType`, and `MapType` recursively, identifies column renames via `delta.columnMapping.id`, distinguishes safe type widenings from breaking changes, and propagates breaking-change flags to `diff --fail-on-breaking` (exit code `2`).

## Security Boundaries

- **Read-Only Contract**: `StorageBackend` provides no write, delete, or rename methods.
- **Local Anti-Traversal (Linux & macOS)**: `LocalStorage` opens the table root with `O_DIRECTORY | O_CLOEXEC` and walks each relative component with `openat(..., O_NOFOLLOW)`, rejecting symlinks, parent traversal (`..`), absolute paths, and NUL bytes with zero TOCTOU window across `read`, `list`, `exists`, and `is_dir`.
- **Local Anti-Traversal (Windows)**: `LocalStorage` opens root and target handles (`open_table_file_windows` / `open_table_directory_windows`), validates `GetFinalPathNameByHandleW` containment against the pinned root handle across `read`, `list`, `exists`, and `is_dir`, and enumerates directories via `GetFileInformationByHandleEx` on the pinned directory handle.
- **V2 Sidecar & Cloud Prefix Containment**:
  - `sidecar_storage_path` restricts V2 sidecar references strictly to normalized child names under `_delta_log/_sidecars/`.
  - `ObjectStoreBackend::resolve_key` enforces scheme/bucket equality, table-prefix containment, and rejects `..`, `.`, empty segments, leading slashes, backslashes, and NUL bytes on both full URIs and relative subpaths.
