# Security Policy

## Supported Versions

| Version | Supported | Notes |
| :--- | :--- | :--- |
| `2.x` | Yes | Active maintenance, security patches, and protocol hardening |
| `1.x` | No | End of life — upgrade to `2.0.1` or later |

## Reporting a Vulnerability

If you discover a security vulnerability (such as path traversal, symlink/reparse-point escape, cloud prefix escape, installer integrity bypass, or parser DoS), please **do not open a public GitHub issue**.

Instead, report it privately via **GitHub Private Vulnerability Reporting**:

- https://github.com/sandy-sachin7/datalens/security/advisories/new

### What to Expect

- **Acknowledgment**: Within 48 hours (2 business days).
- **Assessment & timeline**: We will confirm reproducibility, assess severity, and share a remediation timeline.
- **Coordinated disclosure**: We will prepare a patch and publish a security advisory once a fixed release is available. Reporters are credited unless anonymity is requested.

## Threat Model & Security Architecture

DeltaLens is designed to inspect untrusted or shared Delta Lake transaction logs safely:

- **Strictly read-only execution**: DeltaLens never writes to target tables or `_delta_log/` directories. All storage backends expose only read/list/stat operations.
- **Network behavior**:
  - Outbound network requests occur **only** when a cloud object-store URI (`s3://`, `s3a://`, `s3n://`, `gs://`, `az://`, `abfs://`, `abfss://`) is explicitly passed by the user.
  - Local paths (`./table`, `/path/to/table`, `file://...`) perform zero network I/O.
  - For air-gapped or strictly local environments, build with `cargo install deltalens --no-default-features` to compile out all cloud and HTTP dependencies completely.
- **Local filesystem containment (anti-traversal & TOCTOU defense)**:
  - **Linux & macOS**: Pins the table root directory via `O_DIRECTORY | O_CLOEXEC` and traverses every relative path component with `openat(..., O_NOFOLLOW)` so symlinks and mid-flight swaps cannot escape the table root.
  - **Windows**: Uses handle-pinned opens (`open_table_file_windows` / `open_table_directory_windows`) and verifies `GetFinalPathNameByHandleW` against the pinned root handle across `read`, `list`, `exists`, and `is_dir`, rejecting reparse-point escapes and parent traversal.
- **V2 checkpoint sidecar & cloud prefix containment**:
  - V2 checkpoint `sidecar.path` values are validated to disallow absolute paths, `..`, `.`, empty segments, and backslashes so sidecars stay strictly beneath `_delta_log/_sidecars/`.
  - `ObjectStoreBackend::resolve_key` enforces scheme/bucket matching, prefix containment beneath the configured table prefix, and rejects `..`, `.`, empty segments, leading slashes, backslashes, and NUL bytes on both full URIs and relative paths.
- **Release & installer integrity**:
  - Pre-built binaries are published with `.sha256` digests, and both `scripts/install.sh` and `scripts/install.ps1` strictly enforce SHA-256 verification before installation.
- **Local-only opt-in telemetry**:
  - Telemetry is disabled by default (`telemetry = false`). When explicitly opted in, metrics are stored solely in a local JSONL file and are never transmitted over the network.
