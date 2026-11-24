//! # DeltaLens
//!
//! Strictly read-only Delta Lake transaction log analyzer written in pure Rust — no Spark,
//! no JVM, and no Python required.
//!
//! `deltalens` reads `_delta_log/` metadata directly (NDJSON commits, classic and multi-part
//! Parquet checkpoints, V2 checkpoints with `_sidecars/`, and `.compacted.json` files) across
//! local filesystems and cloud object stores (`s3://`, `gs://`, `az://`, `abfss://`).
//!
//! ## Quick Start
//!
//! ```no_run
//! use deltalens::{storage_for, DeltaLogReader, DiffAnalyzer, HealthAnalyzer, TableState};
//!
//! fn main() -> Result<(), deltalens::DeltaLensError> {
//!     let (storage, root) = storage_for("./transactions")?;
//!     let reader = DeltaLogReader::new(storage, &root)?;
//!
//!     // Checkpoint-accelerated health inspection at the latest version
//!     let snapshot = reader.read_snapshot_log(None)?;
//!     let stats = HealthAnalyzer::analyze_snapshot(&snapshot);
//!     println!(
//!         "v{} — {} active files, skew {:.2}, {} health issues",
//!         stats.current_version,
//!         stats.health.total_files,
//!         stats.health.skew_score,
//!         stats.health_issues().len(),
//!     );
//!
//!     // Diff between two versions (also checkpoint- and offset-list-accelerated)
//!     if stats.current_version > 0 {
//!         let (snap_v0, after, _warnings) = reader.read_diff_logs(0, stats.current_version)?;
//!         let diff = DiffAnalyzer::diff_from_state(0, TableState::from_snapshot(&snap_v0), &after);
//!         println!(
//!             "v0..v{}: +{} / -{} logical files (breaking schema: {})",
//!             diff.v2, diff.files_added, diff.files_removed, diff.has_breaking_schema_changes
//!         );
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Feature Flags
//!
//! Cloud object-store backends are compiled behind Cargo features so you can tailor binary size
//! and network surface area:
//!
//! - `default` — enables `aws`, `gcp`, and `azure`.
//! - `aws` — enables `s3://`, `s3a://`, and `s3n://` via `object_store/aws` (implies `cloud`).
//! - `gcp` — enables `gs://` via `object_store/gcp` (implies `cloud`).
//! - `azure` — enables `az://`, `abfs://`, and `abfss://` via `object_store/azure` (implies `cloud`).
//! - `cloud` — shared `ObjectStoreBackend` and Tokio runtime plumbing.
//! - `--no-default-features` — pure-local binary with zero cloud or HTTP dependencies.
//!
//! ## Top-Level Re-Exports
//!
//! The most common entry points are re-exported at the crate root:
//! [`storage_for`], [`DeltaLogReader`], [`TableState`], [`HealthAnalyzer`],
//! [`DiffAnalyzer`], [`LineageTracer`], [`SchemaTracker`], and [`DeltaLensError`].

pub mod cli;
pub mod core;
pub mod error;
pub mod render;

pub use crate::core::analyzer::diff::DiffAnalyzer;
pub use crate::core::analyzer::health::HealthAnalyzer;
pub use crate::core::analyzer::lineage::LineageTracer;
pub use crate::core::analyzer::schema::SchemaTracker;
pub use crate::core::reader::DeltaLogReader;
pub use crate::core::replay::TableState;
pub use crate::core::storage::storage_for;
pub use crate::error::DeltaLensError;
