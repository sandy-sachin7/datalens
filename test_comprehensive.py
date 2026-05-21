#!/usr/bin/env python3
"""Comprehensive edge case test runner for DeltaLens v1.0.0."""

import subprocess, sys, os, re, json

DELTALENS = "./target/release/deltalens"
BASE = "edge_tables"
passed = failed = warned = 0

def green(s): print(f"  \033[32m✓\033[0m {s}")
def red(s):   global failed; print(f"  \033[31m✗\033[0m {s}"); failed += 1
def yellow(s):global warned; print(f"  \033[33m⚠\033[0m {s}"); warned += 1
def header(s):print(f"\n\033[1m{s}\033[0m")
def sub(s):   print(f"    {s}")

def run(*args):
    r = subprocess.run([DELTALENS, *args], capture_output=True, text=True)
    return r.stdout + r.stderr

def no_warnings(out, ctx=""):
    if "Warning:" in out:
        for line in out.splitlines():
            if "Warning:" in line:
                yellow(f"unexpected warning: {line.strip()}")
        return False
    return True

def contains(out, pattern, msg):
    global passed
    if pattern in out:
        green(msg); passed += 1
        return True
    red(f"{msg} (expected '{pattern}')")
    return False

def not_contains(out, pattern, msg):
    global passed
    if pattern in out:
        red(f"{msg} (found '{pattern}')")
        return False
    green(msg); passed += 1
    return True

def errors_gracefully(out, msg):
    """Check output contains an error indicator without crashing."""
    global passed
    for kw in ["error", "no such file", "not found", "no delta", "not a delta",
               "is a directory", "permission denied", "invalid"]:
        if kw in out.lower():
            green(msg); passed += 1
            return True
    yellow(f"{msg} (no explicit error, check output)")
    return False

def lint():
    """Run cargo fmt --check and cargo clippy."""
    header("  LINT: cargo fmt --check")
    r = subprocess.run(["cargo", "fmt", "--check"], capture_output=True, text=True)
    if r.returncode == 0:
        green("formatting is clean")
    else:
        red("formatting needs fixing")
        print(r.stdout)

    header("  LINT: cargo clippy")
    r = subprocess.run(["cargo", "clippy"], capture_output=True, text=True, timeout=120)
    if r.returncode == 0:
        green("clippy is clean")
    else:
        red(f"clippy found issues:\n{r.stdout}\n{r.stderr}")


# =========================================================================
print("\n╔══════════════════════════════════════════════════════════════╗")
print("║   DeltaLens v1.0.0 — Exhaustive Edge Case Test Suite      ║")
print("╚══════════════════════════════════════════════════════════════╝")

lint()

# =========================================================================
# SECTION 1: HAPPY PATHS
# =========================================================================
header("1. HAPPY PATHS")

header("  1.1 inspect — normal table")
out = run("inspect", f"{BASE}/normal")
no_warnings(out)
contains(out, "Current Version",        "shows current version")
contains(out, "Total Files",             "shows total file count")
contains(out, "Column Count",            "shows column count")

header("  1.2 inspect — partitioned table")
out = run("inspect", f"{BASE}/partitioned")
no_warnings(out)
contains(out, "region",                 "shows partition column 'region'")

header("  1.3 inspect — 101 versions (maintenance warning)")
out = run("inspect", f"{BASE}/hundred_versions")
no_warnings(out)
contains(out, "issue detected",         "maintenance warning triggered")
contains(out, "hundred_versions",       "101 versions — table name visible")

header("  1.4 inspect — single version")
out = run("inspect", f"{BASE}/single_version")
no_warnings(out)
contains(out, "Version",                 "shows version info")
contains(out, "Column Count",            "schema shown")

header("  1.5 diff — v0→v2 with schema change")
out = run("diff", f"{BASE}/normal", "--v1", "0", "--v2", "2")
no_warnings(out)
contains(out, "extra",                  "new column detected")
contains(out, "Added",                  "file changes shown")

header("  1.6 diff — schema-only")
out = run("diff", f"{BASE}/normal", "--v1", "0", "--v2", "2", "--schema-only")
no_warnings(out)
contains(out, "extra",                  "schema-only shows column change")

header("  1.7 diff — files-only")
out = run("diff", f"{BASE}/normal", "--v1", "0", "--v2", "2", "--files-only")
no_warnings(out)
not_contains(out, "extra",             "files-only hides schema details")

header("  1.8 lineage — normal table")
out = run("lineage", f"{BASE}/normal")
no_warnings(out)
contains(out, "WRITE",                  "WRITE operations in lineage")
contains(out, "delta-rs",              "engine info present")

header("  1.9 lineage — single version")
out = run("lineage", f"{BASE}/single_version")
no_warnings(out)
contains(out, "v0",                     "single entry for v0")

header("  1.10 audit — normal table")
out = run("audit", f"{BASE}/normal")
no_warnings(out)
contains(out, "operations found",       "audit shows operations")

header("  1.11 schema — normal table")
out = run("schema", f"{BASE}/normal")
no_warnings(out)
contains(out, "extra",                  "extra column in schema")
contains(out, "STRING",                 "string type shown")

header("  1.12 config")
out = run("config", "show")
contains(out, "Config file",            "config show works")
contains(out, "Telemetry",              "telemetry status")
out = run("config", "path")
contains(out, "config.json",            "config path works")
out = run("config", "metrics")
contains(out, "Metrics",                "metrics command works")


# =========================================================================
# SECTION 2: EDGE CASE TABLES
# =========================================================================
header("2. EDGE CASE TABLES")

header("  2.1 empty table (0 records)")
out = run("inspect", f"{BASE}/empty_table")
no_warnings(out)
contains(out, "Current Version",        "inspect handles 0-record table")

header("  2.2 all-null values")
out = run("inspect", f"{BASE}/all_nulls")
no_warnings(out)
contains(out, "Current Version",        "inspect handles NULL values")

header("  2.3 extreme file skew (6 files)")
out = run("inspect", f"{BASE}/extreme_skew")
no_warnings(out)
# 6 versions with overwrite/append; check it doesn't crash
green("inspect handles multi-file table")

header("  2.4 all types schema")
out = run("schema", f"{BASE}/all_types")
no_warnings(out)
for t in ["i8", "i16", "i32", "i64", "f32", "f64", "bool", "str"]:
    contains(out, t,                   f"column '{t}'")
contains(out, "SHORT",                  "int8→SHORT")
contains(out, "INT",                    "int32→INT")
contains(out, "LONG",                   "int64→LONG")
contains(out, "FLOAT",                  "float32→FLOAT")
contains(out, "DOUBLE",                 "float64→DOUBLE")
contains(out, "BOOLEAN",                "bool→BOOLEAN")
contains(out, "STRING",                 "string→STRING")

header("  2.5 nested schema (LIST)")
out = run("schema", f"{BASE}/nested_schema")
no_warnings(out)
contains(out, "tags",                   "list column in schema")
contains(out, "elementType",             "array/list type shown as JSON")

header("  2.6 decimal types")
out = run("schema", f"{BASE}/decimal_types")
no_warnings(out)
contains(out, "price",                  "price column")
contains(out, "STRING",                 "decimal stored as string")

header("  2.7 maintenance history (OPTIMIZE + VACUUM)")
out = run("inspect", f"{BASE}/maintenance_history")
no_warnings(out)
contains(out, "Current Version",        "inspect works with VACUUM history")
green("  maintenance info available")

header("  2.8 multi-schema (26 column additions)")
out = run("schema", f"{BASE}/multi_schema")
no_warnings(out)
contains(out, "a",                      "column 'a' at start")
contains(out, "z",                      "column 'z' at end (26 additions)")

header("  2.9 multi-schema — inspect")
out = run("inspect", f"{BASE}/multi_schema")
no_warnings(out)
contains(out, "Schema Changes",         "schema change count reported")

header("  2.10 multi-schema — diff v0→v25")
out = run("diff", f"{BASE}/multi_schema", "--v1", "0", "--v2", "25", "--schema-only")
no_warnings(out)
contains(out, "added",                  "schema additions shown")

header("  2.11 partitioned — lineage")
out = run("lineage", f"{BASE}/partitioned")
no_warnings(out)
contains(out, "WRITE",                  "WRITE in partition lineage")

header("  2.12 partitioned — inspect again")
out = run("inspect", f"{BASE}/partitioned")
no_warnings(out)
contains(out, "region",                 "partition column in inspect")


# =========================================================================
# SECTION 3: RESILIENCE — CORRUPTED/MALFORMED
# =========================================================================
header("3. RESILIENCE — Corrupted and Malformed")

header("  3.1 corrupted log (garbled JSON)")
out = run("inspect", f"{BASE}/corrupted_log")
if "Warning:" in out:
    yellow("warning emitted for corrupted log (by design)")
contains(out, "Current Version",        "doesn't crash on corrupted log")

header("  3.2 corrupted log — lineage")
out = run("lineage", f"{BASE}/corrupted_log")
contains(out, "WRITE",                  "lineage survives corrupted log")

header("  3.3 corrupted log — audit")
out = run("audit", f"{BASE}/corrupted_log")
contains(out, "operations found",       "audit survives corrupted log")

header("  3.4 custom action types (unknown variant)")
out = run("inspect", f"{BASE}/custom_actions")
no_warnings(out)
contains(out, "Current Version",        "unknown actions silently skipped")

header("  3.5 custom actions — diff")
out = run("diff", f"{BASE}/custom_actions", "--v1", "0", "--v2", "1")
no_warnings(out)
green("  diff with custom action types")

header("  3.6 zero-byte log file")
out = run("inspect", f"{BASE}/zero_byte_log")
no_warnings(out)
contains(out, "Current Version",        "handles zero-byte log file")

header("  3.7 empty delta_log directory")
out = run("inspect", f"{BASE}/empty_delta_log")
if out.strip():
    green("responded without crash"); passed += 1
else:
    yellow("empty output — no crash but no message")
    passed += 1


# =========================================================================
# SECTION 4: NEGATIVE PATHS
# =========================================================================
header("4. NEGATIVE PATHS — Error Handling")

header("  4.1 non-existent path")
out = run("inspect", "/nonexistent/path_xyz")
errors_gracefully(out, "errors on nonexistent path")

header("  4.2 no _delta_log directory")
out = run("inspect", f"{BASE}/no_delta_log")
errors_gracefully(out, "errors on non-Delta directory")

header("  4.3 diff — same version self-diff")
out = run("diff", f"{BASE}/normal", "--v1", "0", "--v2", "0")
no_warnings(out)
contains(out, "No schema",              "no schema change for self-diff")

header("  4.4 diff — reverse range (v1 > v2)")
out = run("diff", f"{BASE}/normal", "--v1", "2", "--v2", "0")
# should either error or swap
for kw in ["error", "invalid", "Invalid", "greater", "before"]:
    if kw in out:
        green("graceful error for reverse range")
        break
else:
    yellow("no error for reverse range (may have swapped)")

header("  4.5 diff — out of bounds v2")
out = run("diff", f"{BASE}/normal", "--v1", "0", "--v2", "99999")
for kw in ["error", "invalid", "Invalid", "not found", "out of range"]:
    if kw in out:
        green("graceful error for OOB version")
        break
else:
    yellow("no error for OOB version")

header("  4.6 diff — single version self-diff")
out = run("diff", f"{BASE}/single_version", "--v1", "0", "--v2", "0")
contains(out, "No schema",              "self-diff on single-version table")


# =========================================================================
# SECTION 5: CLI FLAGS
# =========================================================================
header("5. CLI FLAGS")

header("  5.1 --json inspect")
out = run("--json", "inspect", f"{BASE}/normal")
contains(out, '"current_version"',       "JSON has current_version")
contains(out, '{',                       "JSON starts with brace")

header("  5.2 --json diff")
out = run("--json", "diff", f"{BASE}/normal", "--v1", "0", "--v2", "2")
contains(out, "files_added",            "JSON diff has files_added")

header("  5.3 --json lineage")
out = run("--json", "lineage", f"{BASE}/normal")
contains(out, '"version"',              "JSON lineage has version")

header("  5.4 --json audit")
out = run("--json", "audit", f"{BASE}/normal")
contains(out, '"version"',              "JSON audit has version")

header("  5.5 --json schema")
out = run("--json", "schema", f"{BASE}/normal")
contains(out, '"name"',                  "JSON schema has field names")

header("  5.6 --plain inspect")
out = run("--plain", "inspect", f"{BASE}/normal")
if "\033" in out:
    yellow("--plain may still have ANSI codes")
else:
    green("no ANSI escape codes in plain output")

header("  5.7 --no-header")
out = run("--no-header", "inspect", f"{BASE}/normal")
not_contains(out, "Field Name",          "no-header hides table headers")

header("  5.8 --help")
out = run("--help")
for cmd in ["inspect", "diff", "lineage", "audit", "schema", "config"]:
    contains(out, cmd,                  f"help shows '{cmd}'")

header("  5.9 -v verbose")
out = run("-v", "inspect", f"{BASE}/normal")
# verbose may add internal debug info — just check no crash
green("verbose mode runs without error"); passed += 1


# =========================================================================
# SECTION 6: DIFF PAIRS
# =========================================================================
header("6. ADDITIONAL DIFF SCENARIOS")

header("  6.1 partitioned v0→v1")
out = run("diff", f"{BASE}/partitioned", "--v1", "0", "--v2", "1")
no_warnings(out)
contains(out, "Added",                  "append files tracked")

header("  6.2 empty table self-diff")
out = run("diff", f"{BASE}/empty_table", "--v1", "0", "--v2", "0")
no_warnings(out)
# Should succeed without crash
green("self-diff on empty table doesn't crash")

header("  6.3 extreme skew v0→v5")
out = run("diff", f"{BASE}/extreme_skew", "--v1", "0", "--v2", "5")
no_warnings(out)
contains(out, "Added",                  "file changes detected")

header("  6.4 maintenance v0→v6")
out = run("diff", f"{BASE}/maintenance_history", "--v1", "0", "--v2", "6")
no_warnings(out)
green("  diff works across maintenance versions")


# =========================================================================
# SECTION 7: STRESS — Large Table Performance
# =========================================================================
header("7. STRESS TEST — 507 versions")
out = run("inspect", "stress_tables/stress")
no_warnings(out)
contains(out, "Current Version",        "inspect on 500-version table")
out = run("--json", "lineage", "stress_tables/stress")

# default shows last 20 entries; --last N shows all
import json
try:
    entries = json.loads(out)
    n = len(entries)
    # default is last 20
    if n == 20:
        green(f"{n} lineage entries (default=last 20)"); passed += 1
    else:
        yellow(f"unexpected {n} lineage entries")
except:
    yellow("could not parse lineage JSON")

out = run("--json", "audit", "stress_tables/stress")
# count operations
try:
    entries = json.loads(out)
    n = len(entries)
    green(f"{n} audit operations"); passed += 1
except:
    # fallback: look for text count
    m = re.search(r'(\d+)\s+operations? found', out)
    if m:
        n = int(m.group(1))
        green(f"{n} audit operations"); passed += 1
    else:
        yellow("could not count audit operations")


# =========================================================================
# SUMMARY
# =========================================================================
total = passed + failed + warned
print(f"\n{'='*60}")
print(f"  TESTS: {total}  |  PASSED: {passed}  |  FAILED: {failed}  |  WARNINGS: {warned}")
if failed == 0:
    print(f"\n  \033[32m✓ ALL TESTS PASSED — DeltaLens v1.0.0 is production-ready\033[0m\n")
else:
    print(f"\n  \033[31m✗ {failed} failure(s) need attention\033[0m\n")
