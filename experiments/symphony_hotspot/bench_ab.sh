#!/usr/bin/env bash
# A/B benchmark: CPython vs Molt-compiled normalize_issue
# Runs each N times, collects wall-clock timing via hyperfine-style loop
set -euo pipefail

MOLT_BIN="/Volumes/APDataStore/Molt/molt_cache/home/bin/normalize_issue_molt"
HOTSPOT="experiments/symphony_hotspot/normalize_issue.py"
SAMPLES=15

echo "=== Symphony Hotspot A/B Benchmark ==="
echo "Iterations per run: 5000 normalize_issue calls"
echo "Samples: $SAMPLES each"
echo ""

# --- CPython ---
echo "--- CPython baseline ---"
cpython_times=()
for i in $(seq 1 $SAMPLES); do
    t=$( { TIMEFORMAT='%R'; time uv run --python 3.12 python3 "$HOTSPOT" > /dev/null 2>&1; } 2>&1 )
    cpython_times+=("$t")
    printf "  sample %2d: %s s\n" "$i" "$t"
done
echo ""

# --- Molt ---
echo "--- Molt compiled ---"
molt_times=()
for i in $(seq 1 $SAMPLES); do
    t=$( { TIMEFORMAT='%R'; time "$MOLT_BIN" > /dev/null 2>&1; } 2>&1 )
    molt_times+=("$t")
    printf "  sample %2d: %s s\n" "$i" "$t"
done
echo ""

# --- Summary via Python ---
python3 -c "
import sys, statistics
cp = [float(x) for x in sys.argv[1].split(',')]
mt = [float(x) for x in sys.argv[2].split(',')]
cp_mean = statistics.mean(cp) * 1000
mt_mean = statistics.mean(mt) * 1000
cp_med = statistics.median(cp) * 1000
mt_med = statistics.median(mt) * 1000
speedup = cp_mean / mt_mean if mt_mean > 0 else 0
print(f'CPython  mean={cp_mean:.1f}ms  median={cp_med:.1f}ms  min={min(cp)*1000:.1f}ms  max={max(cp)*1000:.1f}ms')
print(f'Molt     mean={mt_mean:.1f}ms  median={mt_med:.1f}ms  min={min(mt)*1000:.1f}ms  max={max(mt)*1000:.1f}ms')
print(f'Speedup: {speedup:.2f}x (mean) / {cp_med/mt_med:.2f}x (median)')
if speedup > 1: print(f'Verdict: MOLT FASTER by {speedup:.1f}x')
elif speedup > 0.9: print('Verdict: PARITY')
else: print(f'Verdict: CPython faster by {1/speedup:.1f}x')
" "$(IFS=,; echo "${cpython_times[*]}")" "$(IFS=,; echo "${molt_times[*]}")"
