#!/usr/bin/env bash
# ============================================================================
# Parity test runner: compares CPython output vs Molt output for each test.
#
# Usage:
#   ./tests/parity/run_parity.sh
#
# Environment variables:
#   PYTHON    — path to CPython binary       (default: python3)
#   MOLT_CLI  — path to molt CLI Python      (default: python3 -m molt.cli)
#   BUILD_PROFILE — build profile            (default: release-fast)
#   CAPABILITIES  — --capabilities flag      (default: fs,env,time,random)
#   TIMEOUT   — per-test timeout in seconds  (default: 30)
#   VERBOSE   — set to 1 for verbose output  (default: 0)
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

PYTHON="${PYTHON:-python3}"
BUILD_PROFILE="${BUILD_PROFILE:-release-fast}"
CAPABILITIES="${CAPABILITIES:-fs,env,time,random}"
TIMEOUT="${TIMEOUT:-30}"
VERBOSE="${VERBOSE:-0}"
MOLT_PYTHON="${MOLT_PYTHON:-$PYTHON}"

# Temp directory for build artifacts
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/molt_parity_XXXXXX")
trap 'rm -rf "$WORK_DIR"' EXIT

# Colors (if terminal supports it)
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    RED='\033[0;31m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    GREEN='' RED='' YELLOW='' BLUE='' BOLD='' RESET=''
fi

pass_count=0
fail_count=0
skip_count=0
error_count=0
failures=()

# Collect test files
test_files=()
for f in "$SCRIPT_DIR"/test_*.py; do
    [ -f "$f" ] && test_files+=("$f")
done

total=${#test_files[@]}
if [ "$total" -eq 0 ]; then
    echo "No test files found in $SCRIPT_DIR"
    exit 1
fi

printf "${BOLD}Molt CPython Parity Test Suite${RESET}\n"
printf "============================================================\n"
printf "CPython:       %s (%s)\n" "$PYTHON" "$($PYTHON --version 2>&1)"
printf "Molt CLI:      %s -m molt.cli\n" "$MOLT_PYTHON"
printf "Build profile: %s\n" "$BUILD_PROFILE"
printf "Capabilities:  %s\n" "$CAPABILITIES"
printf "Timeout:       %s seconds\n" "$TIMEOUT"
printf "Test files:    %d\n" "$total"
printf "Work dir:      %s\n" "$WORK_DIR"
printf "============================================================\n\n"

run_test() {
    local test_file="$1"
    local test_name
    test_name="$(basename "$test_file" .py)"
    local test_work="$WORK_DIR/$test_name"
    mkdir -p "$test_work"

    local cpython_out="$test_work/cpython.out"
    local cpython_err="$test_work/cpython.err"
    local molt_out="$test_work/molt.out"
    local molt_err="$test_work/molt.err"
    local diff_out="$test_work/diff.out"
    local cpython_rc molt_rc

    # --- Run CPython ---
    cpython_rc=0
    timeout "$TIMEOUT" "$PYTHON" "$test_file" > "$cpython_out" 2> "$cpython_err" || cpython_rc=$?

    if [ "$cpython_rc" -ne 0 ]; then
        printf "  ${YELLOW}SKIP${RESET}  %-30s (CPython itself failed, rc=%d)\n" "$test_name" "$cpython_rc"
        if [ "$VERBOSE" = "1" ]; then
            echo "    CPython stderr:"
            sed 's/^/      /' "$cpython_err"
        fi
        skip_count=$((skip_count + 1))
        return
    fi

    # --- Build with Molt ---
    local build_out_dir="$test_work/build"
    local binary_path="$test_work/$test_name"
    mkdir -p "$build_out_dir"

    local build_rc=0
    timeout "$TIMEOUT" "$MOLT_PYTHON" -m molt.cli build "$test_file" \
        --build-profile "$BUILD_PROFILE" \
        --capabilities "$CAPABILITIES" \
        --respect-pythonpath \
        --out-dir "$build_out_dir" \
        --output "$binary_path" \
        > "$test_work/build.out" 2> "$test_work/build.err" || build_rc=$?

    if [ "$build_rc" -ne 0 ]; then
        printf "  ${RED}ERROR${RESET} %-30s (Molt build failed, rc=%d)\n" "$test_name" "$build_rc"
        if [ "$VERBOSE" = "1" ]; then
            echo "    Build stderr:"
            sed 's/^/      /' "$test_work/build.err"
        fi
        error_count=$((error_count + 1))
        failures+=("$test_name [BUILD FAILED]")
        return
    fi

    # --- Run Molt binary ---
    molt_rc=0
    if [ -f "$binary_path" ]; then
        timeout "$TIMEOUT" "$binary_path" > "$molt_out" 2> "$molt_err" || molt_rc=$?
    elif [ -f "$binary_path.wasm" ]; then
        # WASM output — try running via node or wasmtime
        if command -v wasmtime &>/dev/null; then
            timeout "$TIMEOUT" wasmtime "$binary_path.wasm" > "$molt_out" 2> "$molt_err" || molt_rc=$?
        elif command -v node &>/dev/null; then
            timeout "$TIMEOUT" node "$binary_path.wasm" > "$molt_out" 2> "$molt_err" || molt_rc=$?
        else
            printf "  ${YELLOW}SKIP${RESET}  %-30s (no WASM runtime available)\n" "$test_name"
            skip_count=$((skip_count + 1))
            return
        fi
    else
        # Maybe the output is in build dir
        local found_bin
        found_bin=$(find "$build_out_dir" -type f -perm +111 2>/dev/null | head -1)
        if [ -n "$found_bin" ]; then
            timeout "$TIMEOUT" "$found_bin" > "$molt_out" 2> "$molt_err" || molt_rc=$?
        else
            printf "  ${RED}ERROR${RESET} %-30s (no output binary found)\n" "$test_name"
            if [ "$VERBOSE" = "1" ]; then
                echo "    Build dir contents:"
                ls -la "$build_out_dir" 2>&1 | sed 's/^/      /'
                echo "    Build stdout:"
                sed 's/^/      /' "$test_work/build.out"
            fi
            error_count=$((error_count + 1))
            failures+=("$test_name [NO BINARY]")
            return
        fi
    fi

    # --- Compare outputs ---
    if diff -u "$cpython_out" "$molt_out" > "$diff_out" 2>&1; then
        printf "  ${GREEN}PASS${RESET}  %-30s\n" "$test_name"
        pass_count=$((pass_count + 1))
    else
        local diff_lines
        diff_lines=$(wc -l < "$diff_out" | tr -d ' ')
        printf "  ${RED}FAIL${RESET}  %-30s (%s diff lines)\n" "$test_name" "$diff_lines"
        fail_count=$((fail_count + 1))
        failures+=("$test_name")
        if [ "$VERBOSE" = "1" ]; then
            echo "    Diff (first 40 lines):"
            head -40 "$diff_out" | sed 's/^/      /'
            if [ "$molt_rc" -ne 0 ]; then
                echo "    Molt stderr:"
                head -20 "$molt_err" | sed 's/^/      /'
            fi
        fi
    fi
}

# Run all tests
for test_file in "${test_files[@]}"; do
    run_test "$test_file"
done

# Summary
printf "\n============================================================\n"
printf "${BOLD}Summary${RESET}\n"
printf "============================================================\n"
printf "  ${GREEN}PASS:${RESET}  %d / %d\n" "$pass_count" "$total"
printf "  ${RED}FAIL:${RESET}  %d / %d\n" "$fail_count" "$total"
printf "  ${RED}ERROR:${RESET} %d / %d\n" "$error_count" "$total"
printf "  ${YELLOW}SKIP:${RESET}  %d / %d\n" "$skip_count" "$total"

if [ "${#failures[@]}" -gt 0 ]; then
    printf "\nFailed tests:\n"
    for f in "${failures[@]}"; do
        printf "  - %s\n" "$f"
    done
fi

# Pass rate
tested=$((pass_count + fail_count))
if [ "$tested" -gt 0 ]; then
    rate=$(awk "BEGIN { printf \"%.1f\", ($pass_count / $tested) * 100 }")
    printf "\n${BOLD}Pass rate: %s%% (%d/%d tested)${RESET}\n" "$rate" "$pass_count" "$tested"
fi

# Exit code: 0 if all passed, 1 if any failures
if [ "$fail_count" -gt 0 ] || [ "$error_count" -gt 0 ]; then
    exit 1
fi
exit 0
