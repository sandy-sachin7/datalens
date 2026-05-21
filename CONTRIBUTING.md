# Contributing to DeltaLens

Thanks for your interest! We welcome contributions of all kinds — features, bug fixes, docs, benchmarks, and issue reports.

## Quick Start

```bash
git clone https://github.com/sandy-sachin7/datalens.git
cd datalens
cargo build
cargo test
```

## What We Need Help With

- **Real-world benchmark data**: submit anonymized timing from `deltalens config metrics` on your tables
- **New analyzers**: checkpoint health, Z-order efficiency, deletion vector analysis
- **Remote storage**: S3, GCS, Azure Blob backends
- **Packaging**: Homebrew, Scoop, Nix, Docker
- **Documentation**: clearer error messages, more examples, cookbook recipes

## Pull Request Process

1. Run `cargo fmt --check && cargo clippy -- -D warnings && cargo test` before submitting
2. Keep PRs focused on one concern — split large changes into multiple PRs
3. Update the README if your change affects the CLI or user-facing behavior
4. Add tests for new analyzers or parsers

## Design Principles

- **Zero runtime dependencies** — the binary should run on a bare system
- **Fail gracefully** — malformed commits are skipped with a warning, never crash
- **Parallel by default** — use `rayon` for any operation over multiple files
- **No JVM, no Python, no cloud** — the entire value prop is "it just works, locally"

## Code of Conduct

All contributors must abide by our [Code of Conduct](CODE_OF_CONDUCT.md).
