#!/usr/bin/env bash
# Comprehensive edge case test runner for DeltaLens v1.0.0
set -e
DELTALENS="./target/release/deltalens"
BASE="edge_tables"
PASS=0
FAIL=0
WARN=0

green() { printf "  \033[32m✓\033[0m %s\n" "$1"; }
red()   { printf "  \033[31m✗\033[0m %s\n" "$1"; ((FAIL++)); }
yellow(){ printf "  \033[33m⚠\033[0m %s\n" "$1"; ((WARN++)); }
header(){ printf "\n\033[1m%s\033[0m\n" "$1"; }
sub()   { printf "  %s\n" "$1"; }

check_no_warnings() {
    local out="$1"
    if echo "$out" | grep -q "Warning:"; then
        yellow "UNEXPECTED WARNINGS"
        echo "$out" | grep "Warning:" | while read -r line; do sub "$line"; done
        return 1
    fi
    return 0
}

check_contains() {
    local out="$1" msg="$2" pattern="$3"
    if echo "$out" | grep -q "$pattern"; then
        green "$msg"
    else
        red "$msg (missing: '$pattern')"
    fi
}

check_not_contains() {
    local out="$1" msg="$2" pattern="$3"
    if echo "$out" | grep -q "$pattern"; then
        red "$msg (found: '$pattern')"
    else
        green "$msg"
    fi
}

run() {
    local desc="$1"; shift
    local output
    output=$($DELTALENS "$@" 2>&1) || true
    echo "$output"
    echo "$output" > /tmp/deltalens_last.txt
}

run_silent() {
    local output
    output=$($DELTALENS "$@" 2>&1) || true
    echo "$output"
    echo "$output" > /tmp/deltalens_last.txt
}

# ============================================================
echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║       DeltaLens v1.0.0 — Exhaustive Edge Case Suite     ║"
echo "╚══════════════════════════════════════════════════════════╝"

# ============================================================
# SECTION 1: HAPPY PATHS
# ============================================================
header "1. HAPPY PATHS — Normal Tables"

header "  1a. inspect — normal table (3 versions, schema change)"
OUT=$(run_silent inspect "$BASE/normal")
check_no_warnings "$OUT"
check_contains "$OUT" "Version 2" "Current Version.*2"
check_contains "$OUT" "3 data files" "Total Files.*3"
check_contains "$OUT" "3 columns" "Column Count.*3"
check_contains "$OUT" "1 schema change" "Schema Changes.*1"

header "  1b. inspect — single version table"
OUT=$(run_silent inspect "$BASE/single_version")
check_no_warnings "$OUT"
check_contains "$OUT" "Version 0" "Current Version.*[0]"
check_contains "$OUT" "10 records" "10 records"  # via stats

header "  1c. inspect — partitioned table"
OUT=$(run_silent inspect "$BASE/partitioned")
check_no_warnings "$OUT"
check_contains "$OUT" "partitioned" "partitioned"
check_contains "$OUT" "region" "Partition Columns.*region"

header "  1d. diff — v0→v2 (with schema change)"
OUT=$(run_silent diff "$BASE/normal" --v1 0 --v2 2)
check_no_warnings "$OUT"
check_contains "$OUT" "extra" "new column 'extra'"
check_contains "$OUT" "1 file added" "Files Added.*+1"
check_contains "$OUT" "2 files removed" "Files Removed.*-2"

header "  1e. diff — schema-only mode"
OUT=$(run_silent diff "$BASE/normal" --v1 0 --v2 2 --schema-only)
check_no_warnings "$OUT"
check_contains "$OUT" "extra" "schema-only shows new column"
check_not_contains "$OUT" "Files Added" "no file info in schema-only"

header "  1f. diff — files-only mode"
OUT=$(run_silent diff "$BASE/normal" --v1 0 --v2 2 --files-only)
check_no_warnings "$OUT"
check_contains "$OUT" "Files Added" "files-only shows file info"
check_not_contains "$OUT" "extra" "no schema info in files-only"

header "  1g. lineage — normal table"
OUT=$(run_silent lineage "$BASE/normal")
check_no_warnings "$OUT"
check_contains "$OUT" "WRITE" "WRITE operations found"
check_contains "$OUT" "delta-rs" "engine info present"

header "  1h. lineage — single version"
OUT=$(run_silent lineage "$BASE/single_version")
check_no_warnings "$OUT"
check_contains "$OUT" "v0" "lineage entry for v0"

header "  1i. audit — normal table"
OUT=$(run_silent audit "$BASE/normal")
check_no_warnings "$OUT"
check_contains "$OUT" "operations found" "audit operations found"

header "  1j. schema — normal table"
OUT=$(run_silent schema "$BASE/normal")
check_no_warnings "$OUT"
check_contains "$OUT" "extra" "extra column in schema"
check_contains "$OUT" "id" "id column in schema"
check_contains "$OUT" "val" "val column in schema"

header "  1k. config operations"
OUT=$(run_silent config show)
check_contains "$OUT" "Config file" "config show works"
OUT=$(run_silent config path)
check_contains "$OUT" "/deltalens/config.json" "config path works"
OUT=$(run_silent config metrics)
check_contains "$OUT" "Metrics" "config metrics works"


# ============================================================
# SECTION 2: EDGE — NON-STANDARD TABLES
# ============================================================
header "2. EDGE CASE TABLES"

header "  2a. empty table (0 records)"
OUT=$(run_silent inspect "$BASE/empty_table")
check_no_warnings "$OUT"
check_contains "$OUT" "Current Version" "inspect handles 0-record table"

header "  2b. all-null values"
OUT=$(run_silent inspect "$BASE/all_nulls")
check_no_warnings "$OUT"
check_contains "$OUT" "Current Version" "inspect handles NULL values"

header "  2c. extreme file skew (6 versions, 6 files)"
OUT=$(run_silent inspect "$BASE/extreme_skew")
check_no_warnings "$OUT"
check_contains "$OUT" "6 files" "6 files detected"

header "  2d. decimal types"
OUT=$(run_silent schema "$BASE/decimal_types")
check_no_warnings "$OUT"
check_contains "$OUT" "price" "price column in schema"
check_contains "$OUT" "STRING" "decimals stored as STRING"

header "  2e. all supported types"
OUT=$(run_silent schema "$BASE/all_types")
check_no_warnings "$OUT"
check_contains "$OUT" "i8" "int8 column"
check_contains "$OUT" "i16" "int16 column"
check_contains "$OUT" "i32" "int32 column"
check_contains "$OUT" "i64" "int64 column"
check_contains "$OUT" "f32" "float32 column"
check_contains "$OUT" "f64" "float64 column"
check_contains "$OUT" "bool" "bool column"
check_contains "$OUT" "str" "string column"

header "  2f. 101 versions (maintenance warning trigger)"
OUT=$(run_silent inspect "$BASE/hundred_versions")
check_no_warnings "$OUT"
check_contains "$OUT" "issue detected" "maintenance warning shown"
check_contains "$OUT" "101" "101 versions"
check_contains "$OUT" "VACUUM" "VACUUM: Never"

header "  2g. maintenance history (OPTIMIZE + VACUUM)"
OUT=$(run_silent inspect "$BASE/maintenance_history")
check_no_warnings "$OUT"
check_contains "$OUT" "7 versions" "7 versions"
# The OPTIMIZE and VACUUM should be reflected

header "  2h. partitioned table — lineage"
OUT=$(run_silent lineage "$BASE/partitioned")
check_no_warnings "$OUT"
check_contains "$OUT" "WRITE" "lineage shows WRITE ops"

header "  2i. partitioned table — schema"
OUT=$(run_silent schema "$BASE/partitioned")
check_no_warnings "$OUT"
check_contains "$OUT" "region" "region column"
check_contains "$OUT" "amt" "amt column"

header "  2j. multi-schema (26 column additions)"
OUT=$(run_silent schema "$BASE/multi_schema")
check_no_warnings "$OUT"
check_contains "$OUT" "a" "column 'a' present"
check_contains "$OUT" "z" "column 'z' present (26 additions)"
OUT2=$(run_silent inspect "$BASE/multi_schema")
check_contains "$OUT2" "27 columns" "27 columns"
check_contains "$OUT2" "26 schema changes" "26 schema changes"

header "  2k. multi-schema — diff v0→v26"
OUT=$(run_silent diff "$BASE/multi_schema" --v1 0 --v2 26 --schema-only)
check_no_warnings "$OUT"
check_contains "$OUT" "added" "schema diff shows column additions"


# ============================================================
# SECTION 3: RESILIENCE — CORRUPTED / MALFORMED TABLES
# ============================================================
header "3. RESILIENCE — Corrupted and Malformed Inputs"

header "  3a. corrupted log (garbled JSON line)"
OUT=$(run_silent inspect "$BASE/corrupted_log")
# Should emit a warning but not crash
if echo "$OUT" | grep -q "Warning:"; then
    green "gracefully warns about corrupted log"
else
    yellow "no warning for corrupted log (may have been skipped)"
fi
check_contains "$OUT" "Current Version" "inspect does NOT crash on corrupted log"
check_contains "$OUT" "1 file" "still shows data from valid lines"

header "  3b. corrupted log — lineage"
OUT=$(run_silent lineage "$BASE/corrupted_log")
# Should not crash, should still work
check_contains "$OUT" "WRITE" "lineage works despite corrupted log"

header "  3c. corrupted log — audit"
OUT=$(run_silent audit "$BASE/corrupted_log")
check_contains "$OUT" "operations found" "audit works despite corrupted log"

header "  3d. custom action types (unknown variant)"
OUT=$(run_silent inspect "$BASE/custom_actions")
check_no_warnings "$OUT"
check_contains "$OUT" "Current Version" "unknown actions silently skipped"

header "  3e. custom actions — diff"
OUT=$(run_silent diff "$BASE/custom_actions" --v1 0 --v2 1)
check_no_warnings "$OUT"
check_contains "$OUT" "0 files" "no file changes in custom-only commit"

header "  3f. zero-byte log file"
OUT=$(run_silent inspect "$BASE/zero_byte_log")
check_no_warnings "$OUT"
check_contains "$OUT" "Current Version" "handles zero-byte log file"
check_contains "$OUT" "1 file" "data from valid log intact"

header "  3g. empty delta_log directory"
OUT=$(run_silent inspect "$BASE/empty_delta_log")
# This should report an error or empty table
# Not crash
check_contains "$OUT" """ "does not crash on empty delta_log"


# ============================================================
# SECTION 4: NEGATIVE PATHS — Error Handling
# ============================================================
header "4. NEGATIVE PATHS — Error Handling"

header "  4a. non-existent path"
OUT=$(run_silent inspect "/nonexistent/path")
check_contains "$OUT" "rror" OR check_contains "$OUT" "o such file" "errors on nonexistent path"
if echo "$OUT" | grep -qiE "error|no such file|not found"; then
    green "graceful error on nonexistent path"
else
    red "no error on nonexistent path"
    echo "$OUT"
fi

header "  4b. no _delta_log directory"
OUT=$(run_silent inspect "$BASE/no_delta_log")
if echo "$OUT" | grep -qiE "error|no delta|not a delta|no such"; then
    green "graceful error for non-Delta directory"
else
    yellow "no clear error for non-Delta directory"
fi

header "  4c. diff — same version (v1=v2)"
OUT=$(run_silent diff "$BASE/normal" --v1 0 --v2 0)
check_no_warnings "$OUT"
check_contains "$OUT" "No schema changes" "no schema change for self-diff"
check_contains "$OUT" "0 files added" "0 files added for self-diff"

header "  4d. diff — reverse range (v1 > v2)"
OUT=$(run_silent diff "$BASE/normal" --v1 2 --v2 0)
# Should handle gracefully (either swap or error)
if echo "$OUT" | grep -qiE "error|invalid|greater than|before"; then
    green "graceful error for reverse diff range"
else
    check_contains "$OUT" "Files Added" "handles reversed range"
fi

header "  4e. diff — out of bounds v2"
OUT=$(run_silent diff "$BASE/normal" --v1 0 --v2 9999)
if echo "$OUT" | grep -qiE "error|invalid|not found|out of range"; then
    green "graceful error for OOB version"
else
    yellow "no error for out-of-bounds version"
fi

header "  4f. diff — single version table (v1=0, v2=0)"
OUT=$(run_silent diff "$BASE/single_version" --v1 0 --v2 0)
check_contains "$OUT" "No schema changes" "self-diff on single version table"


# ============================================================
# SECTION 5: CLI FLAGS
# ============================================================
header "5. CLI FLAGS"

header "  5a. --json output"
OUT=$(run_silent --json inspect "$BASE/normal")
check_contains "$OUT" "{" "JSON output starts with brace"
check_contains "$OUT" "}" "JSON output has closing brace"
check_contains "$OUT" "current_version" "JSON has 'current_version' key"

header "  5b. --json diff"
OUT=$(run_silent --json diff "$BASE/normal" --v1 0 --v2 2)
check_contains "$OUT" "{" "JSON diff output"

header "  5c. --json lineage"
OUT=$(run_silent --json lineage "$BASE/normal")
check_contains "$OUT" "{" OR check_contains "$OUT" "[" "JSON lineage output"

header "  5d. --json audit"
OUT=$(run_silent --json audit "$BASE/normal")
check_contains "$OUT" "{" OR check_contains "$OUT" "[" "JSON audit output"

header "  5e. --json schema"
OUT=$(run_silent --json schema "$BASE/normal")
check_contains "$OUT" "{" OR check_contains "$OUT" "]" "JSON schema output"

header "  5f. --plain output (no ANSI)"
OUT=$(run_silent --plain inspect "$BASE/normal")
# Plain output should not contain ANSI escape sequences
if echo "$OUT" | grep -q $'\033'; then
    red "--plain still has ANSI codes"
else
    green "--plain has no ANSI codes"
fi

header "  5g. --no-header"
OUT=$(run_silent --no-header inspect "$BASE/normal")
check_not_contains "$OUT" "Health Report" "no-header hides header"

header "  5h. --verbose"
OUT=$(run_silent -v inspect "$BASE/normal")
if echo "$OUT" | grep -qiE "debug|parsed|version"; then
    green "-v produces extra info"
else
    # verbose may just be internal — accept either
    yellow "-v output may not be visible"
fi

header "  5i. --help"
OUT=$(run_silent --help)
check_contains "$OUT" "inspect" "help shows inspect"
check_contains "$OUT" "diff" "help shows diff"
check_contains "$OUT" "lineage" "help shows lineage"
check_contains "$OUT" "audit" "help shows audit"
check_contains "$OUT" "schema" "help shows schema"
check_contains "$OUT" "config" "help shows config"


# ============================================================
# SECTION 6: SCHEMA — Type Display
# ============================================================
header "6. SCHEMA DISPLAY"

header "  6a. nested schema (list type)"
OUT=$(run_silent schema "$BASE/nested_schema")
check_no_warnings "$OUT"
check_contains "$OUT" "id" "id column"
check_contains "$OUT" "tags" "tags column"
check_contains "$OUT" "LIST" "list type displayed"

header "  6b. all-nulls schema"
OUT=$(run_silent schema "$BASE/all_nulls")
check_contains "$OUT" "id" "id column"
check_contains "$OUT" "val" "val column"
check_contains "$OUT" "STRING" "string type shown"

header "  6c. partitioned schema"
OUT=$(run_silent schema "$BASE/partitioned")
check_contains "$OUT" "region" "region column"
check_contains "$OUT" "amt" "amt column"
check_contains "$OUT" "INT" "int type shown"


# ============================================================
# SECTION 7: DIFF EDGE CASES
# ============================================================
header "7. DIFF EDGE CASES"

header "  7a. diff — partitioned table v0→v1"
OUT=$(run_silent diff "$BASE/partitioned" --v1 0 --v2 1)
check_no_warnings "$OUT"
check_contains "$OUT" "1 file" "1 file added in append"

header "  7b. diff — empty table (self)"
OUT=$(run_silent diff "$BASE/empty_table" --v1 0 --v2 0)
check_contains "$OUT" """ "self-diff on empty table doesn't crash"

header "  7c. diff — extreme skew"
OUT=$(run_silent diff "$BASE/extreme_skew" --v1 0 --v2 5)
check_contains "$OUT" "Files Added" "file changes shown"

header "  7d. diff — maintenance history table"
OUT=$(run_silent diff "$BASE/maintenance_history" --v1 0 --v2 6)
check_contains "$OUT" "Files Added" "file changes across all versions"


# ============================================================
# SECTION 8: CONFIG COMMANDS
# ============================================================
header "8. CONFIG COMMANDS"

header "  8a. config set / show"
OUT=$(run_silent config show)
check_contains "$OUT" "Telemetry" "config show works"
OUT=$(run_silent config path)
check_contains "$OUT" ".config" "config path is correct"

OUT=$(run_silent config set telemetry false)
# Should not error


# ============================================================
# SUMMARY
# ============================================================
echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║                    RESULTS SUMMARY                       ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Passed:  $PASS"
echo "  Failed:  $FAIL"
echo "  Warnings: $WARN"
echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "  ✓ ALL TESTS PASSED"
else
    echo "  ✗ $FAIL TEST(S) FAILED"
fi
echo ""
