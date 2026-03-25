#!/usr/bin/env bash
# Run all benchmarks with both CPython and Molt, reporting times.
#
# Usage:
#   ./benchmarks/run_all.sh
#
# Environment variables:
#   MOLT   — path to molt binary   (default: ./target/release/molt)
#   PYTHON — path to cpython binary (default: python3)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MOLT="${MOLT:-./target/release/molt}"
PYTHON="${PYTHON:-python3}"

BENCHMARKS=(
    bench_sum
    bench_while
    bench_fib
    bench_dict
    bench_calls
    bench_float
    bench_string
    bench_list
)

printf "%-20s %12s %12s %10s\n" "Benchmark" "CPython (s)" "Molt (s)" "Speedup"
printf "%-20s %12s %12s %10s\n" "---" "---" "---" "---"

for bench in "${BENCHMARKS[@]}"; do
    script="$SCRIPT_DIR/${bench}.py"

    if [[ ! -f "$script" ]]; then
        printf "%-20s %12s %12s %10s\n" "$bench" "MISSING" "MISSING" "-"
        continue
    fi

    # CPython
    cpython_time=$( { TIMEFORMAT='%R'; time "$PYTHON" "$script" > /dev/null; } 2>&1 )

    # Molt
    if command -v "$MOLT" &> /dev/null || [[ -x "$MOLT" ]]; then
        molt_time=$( { TIMEFORMAT='%R'; time "$MOLT" run "$script" > /dev/null; } 2>&1 )
    else
        molt_time="N/A"
    fi

    # Speedup
    if [[ "$molt_time" != "N/A" ]] && (( $(echo "$molt_time > 0" | bc -l) )); then
        speedup=$(echo "scale=1; $cpython_time / $molt_time" | bc -l 2>/dev/null || echo "N/A")
        printf "%-20s %12s %12s %9sx\n" "$bench" "$cpython_time" "$molt_time" "$speedup"
    else
        printf "%-20s %12s %12s %10s\n" "$bench" "$cpython_time" "$molt_time" "N/A"
    fi
done
