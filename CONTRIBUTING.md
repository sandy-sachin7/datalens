# Contributing to DeltaLens

Thanks for your interest in contributing to **DeltaLens**! We welcome contributions of all kinds — bug fixes, protocol features, analyzers, benchmarks, packaging, and documentation.

## Quick Start

```bash
git clone https://github.com/sandy-sachin7/datalens.git
cd datalens
cargo build --locked
cargo test --locked
```

> **MSRV**: Rust `1.80+`. Run `rustup update stable` if your toolchain is older.

## What We Need Help With

- **Real-world benchmark data**: submit anonymized timing from `deltalens config metrics --json` on large tables
- **New analyzers**: Z-order & Liquid Clustering effectiveness, partition pruning diagnostics, data-skipping stats coverage
- **Packaging & distribution**: Homebrew formula, Scoop manifest, Nix flake, OCI/Docker images
- **Delta protocol edge cases**: additional real-world V2 checkpoint, Variant/Shredding, and Iceberg UniForm fixtures
- **Documentation**: cookbook recipes, CI/CD workflow templates, and troubleshooting guides

## Pre-PR Verification Checklist

Before opening a pull request, run the full local check suite (matching CI):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo test --no-default-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --locked
```

### Pull Request Guidelines

1. Keep PRs focused on a single concern — split large changes into reviewable commits or separate PRs.
2. Add unit and/or integration tests for new parsers, storage behaviors, or analyzers.
3. Update `README.md`, `ARCHITECTURE.md`, or `CHANGELOG.md` if your change alters CLI flags, JSON schemas, or protocol behavior.
4. Never introduce `.unwrap()` or `.expect()` in non-test library/CLI code under `src/`.

## Design Principles

- **Zero runtime dependencies** — a single static/native binary that works with no JVM, no Spark cluster, and no Python environment.
- **Strictly read-only** — DeltaLens never mutates table data or `_delta_log/` metadata.
- **Fail gracefully** — malformed commit lines are skipped with structured warnings (`-v`/`--verbose`), never panicking.
- **Backend-appropriate concurrency** — bounded Tokio async streams for cloud object stores (`s3://`, `gs://`, `az://`, `abfss://`) and Rayon for local disk and CPU-bound JSON/Parquet parsing, with zero nested runtimes.
- **Optional cloud footprint** — cloud SDKs are feature-gated (`aws`, `gcp`, `azure`); `--no-default-features` always builds a pure-local binary.

## Code of Conduct

All contributors must abide by our [Code of Conduct](CODE_OF_CONDUCT.md).
