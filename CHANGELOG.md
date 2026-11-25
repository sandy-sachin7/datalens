# Changelog

All notable changes to **DeltaLens** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.1.0] - 2026-11-25

### Added
- **Unsupported Reader Protocol Enforcement**: Added `DeltaLensError::UnsupportedProtocol`, `MAX_SUPPORTED_READER_VERSION`, `SUPPORTED_READER_FEATURES`, `ProtocolAction::validate_reader_support()`, and `TableState::validate_protocol()`. All log readers fail loudly (exit code `1`) naming the unsupported `minReaderVersion` or `readerFeatures` (such as `catalogManaged`) instead of emitting an inaccurate report.
- **Differential Conformance Suite (`scripts/diff_test.py`)**: CI-enforced differential testing comparing `deltalens` against `deltalake` (`delta-rs`) across checkpoints, schema evolution, `DELETE`, `UPDATE`, `MERGE`, `OPTIMIZE`, `VACUUM`, nested types, column mapping, deletion vectors, and time-travel diffs.
- **Golden Protocol & Parser Resilience Tests**: Added `tests/protocol_conformance.rs` and `tests/parser_resilience.rs` covering unsupported reader protocols, full Delta `typeWidening` CI gates, V2 JSON checkpoints with Parquet sidecars and Deletion Vectors, and adversarial parser inputs.
- **Parser Fuzz Targets (`fuzz/`)**: Added four `cargo-fuzz` targets for commit NDJSON, Parquet checkpoints, `_last_checkpoint`, and schema JSON parsing.
- **Cloud S3 Integration Tests (`tests/cloud_s3.rs`)**: Added live S3/MinIO end-to-end tests exercising `_last_checkpoint` offset listing, pre-checkpoint fallback, V2 checkpoints, Deletion Vectors, CLI commands, and error paths in CI.

### Fixed
- **Delta Lake Type-Widening Rules**: Corrected `is_safe_type_widening` in `src/core/analyzer/schema.rs` to match the official Delta Protocol `typeWidening` specification (`date -> timestamp` is now breaking; `byte`/`short`/`integer` to `double`, integer types to `decimal(p, s)` with sufficient integral digits, and `decimal` scale-preserving widenings are now safe).
- **Documentation Accuracy**: Replaced the `<10ms` summary claim in `README.md` with measured latency numbers across 60-commit and 50,000-file tables.

## [2.0.1] - 2026-11-25

### Security & Compatibility
- Upgraded `arrow-array`/`arrow-schema` and `parquet` to 60.2.0, clearing the active `arrow-data`, `thrift`, and `zstd` advisories.
- Upgraded `object_store` to 0.14.2, clearing the active `quick-xml` advisory.
- Adapted checkpoint and cloud-storage code to the new APIs and raised MSRV to Rust 1.88.

## [2.0.0] - 2026-11-24

### Added
- **Full Delta Lake Checkpoint Support**: Classic single-file Parquet checkpoints, multi-part Parquet checkpoints (`part-00001-of-00010`), and V2 JSON/Parquet checkpoints with `_delta_log/_sidecars/` resolution.
- **Log Compaction Support**: Reads `.compacted.json` files during contiguous range and snapshot replay, with individual `.json` commits taking precedence in `read_last`.
- **`_last_checkpoint` Offset Listing**: Cloud object stores use `_last_checkpoint` + `list_with_offset` across `inspect`, `schema`, and `diff` to skip listing historical pre-checkpoint commits, with automatic fallback to full listing on stale or pre-checkpoint targets.
- **Multi-Cloud Object Storage**: Native `object_store` backends for AWS S3 (`s3://`, `s3a://`, `s3n://`), Google Cloud Storage (`gs://`), and Azure Data Lake / Blob Storage (`az://`, `abfs://`, `abfss://`), feature-gated via `aws`, `gcp`, `azure`, and `cloud` (with `--no-default-features` for a pure-local build).
- **Modern Delta Protocol Features**:
  - Deletion Vector (DV) keying by `(canonical_path, dv_unique_id)`, DV row/file metrics, and same-commit `remove`-before-`add` replay semantics.
  - Column Mapping (`id` / `name`) rename detection and recursive nested `struct` / `array` / `map` schema diffing with type-widening awareness.
  - Liquid Clustering (`delta.clustering` domain metadata) column discovery and reporting.
  - Tombstone retention tracking (`delta.deletedFileRetentionDuration`) with expired-tombstone reclaimable byte calculations.
- **CI/CD Quality & Schema Gates**: Dedicated exit code `2` for `inspect --fail-on-issues` and `diff --fail-on-breaking`.
- **Rust Library API & Docs**: Top-level crate re-exports in `deltalens`, full rustdoc, runnable `examples/inspect_table.rs`, and checkpoint-accelerated Criterion benchmarks.

### Changed
- **Unified `TableState` Replay Engine**: Consolidated `inspect`, `diff`, `schema`, and `lineage` onto a single protocol-compliant state machine with percent-decoded path canonicalization, missing `Remove.size` enrichment, and saturating arithmetic.
- **Concurrency Architecture**: Bounded Tokio async `buffer_unordered` streams for cloud I/O paired with Rayon for local disk reads and CPU-bound JSON/Parquet parsing — zero nested Tokio runtimes.
- **Single-Listing Discovery**: Caches `_delta_log/` inventory per reader instance to minimize storage round-trips.

### Security
- **TOCTOU-Free Local Storage**: Unix `openat` + `O_NOFOLLOW` + `O_DIRECTORY` fd-relative traversal on Linux/macOS and handle-pinned `GetFinalPathNameByHandleW` verification on Windows across `read`, `list`, `exists`, and `is_dir`.
- **Sidecar & Cloud Prefix Containment**: Strict validation rejecting parent traversal (`..`), dot/empty segments, backslashes, NUL bytes, and bucket/prefix escapes on both full URIs and relative paths.
- **Strict Installer Verification**: `scripts/install.sh` and `scripts/install.ps1` strictly enforce SHA-256 checksum verification before extracting binaries.
- **Panic-Free Production Code**: Zero non-test `.unwrap()` or `.expect()` calls across `src/`.

## [1.1.0] - 2026-04-01

### Added
- **Initial S3 Support**: Basic `s3://` reading support via `object_store` with environment-based AWS credentials.
- **Audit Command**: `deltalens audit` for compliance reporting with `--since`, `--until`, `--op`, and `--user` filters.
- **Local Opt-In Telemetry**: `deltalens config` subcommands (`show`, `path`, `metrics`, `set`) with local P50/P95/P99 latency tracking.
- **Cross-Platform Installers**: Automated release builds and install scripts for Linux, macOS, and Windows.

## [1.0.0] - 2026-03-15

### Added
- Initial release of **DeltaLens** standalone CLI (`inspect`, `diff`, `lineage`, `schema`).
- Parallel JSON commit log parsing via Rayon with zero JVM/Spark/Python dependencies.
- Terminal table rendering (`--plain`, `--no-header`) and `--json` output mode.
