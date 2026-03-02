#!/usr/bin/env python3
"""Check compile throughput regression against a baseline.

Reads baseline and current benchmark JSON files, extracts molt_build_s
(build time in seconds) per benchmark, and fails if the median regression
exceeds the threshold.

Usage:
    python tools/check_compile_throughput.py \
        --baseline bench/baseline.json \
        --current bench/results/ci_smoke.json \
        --max-regression-pct 15 \
        --metric molt_build_s

Exit codes:
    0 — no significant regression
    1 — regression exceeds threshold
    2 — usage/data error
"""

import argparse
import json
import statistics
import sys
from pathlib import Path


def extract_metric(bench_data: dict, metric: str) -> dict[str, float]:
    """Extract a metric from benchmark JSON, returning {benchmark_name: value}."""
    results: dict[str, float] = {}

    # Handle various benchmark JSON layouts
    benchmarks = bench_data.get("benchmarks", bench_data.get("results", []))
    if isinstance(benchmarks, dict):
        benchmarks = list(benchmarks.values())

    for entry in benchmarks:
        name = entry.get("name", entry.get("benchmark", ""))
        if not name:
            continue

        # Try direct metric access
        if metric in entry:
            val = entry[metric]
            if isinstance(val, (int, float)) and val > 0:
                results[name] = float(val)
                continue

        # Try nested in "molt" or "metrics" sub-dict
        for sub_key in ("molt", "metrics", "results"):
            sub = entry.get(sub_key, {})
            if isinstance(sub, dict) and metric in sub:
                val = sub[metric]
                if isinstance(val, (int, float)) and val > 0:
                    results[name] = float(val)
                    break

    return results


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--baseline", required=True, help="Path to baseline benchmark JSON"
    )
    parser.add_argument(
        "--current", required=True, help="Path to current benchmark JSON"
    )
    parser.add_argument(
        "--max-regression-pct",
        type=float,
        default=15.0,
        help="Maximum allowed regression percentage (default: 15)",
    )
    parser.add_argument(
        "--metric",
        default="molt_build_s",
        help="Metric to compare (default: molt_build_s)",
    )
    args = parser.parse_args()

    for path_arg, label in [(args.baseline, "baseline"), (args.current, "current")]:
        if not Path(path_arg).exists():
            print(f"ERROR: {label} file not found: {path_arg}", file=sys.stderr)
            return 2

    try:
        with open(args.baseline) as f:
            baseline_data = json.load(f)
        with open(args.current) as f:
            current_data = json.load(f)
    except json.JSONDecodeError as e:
        print(f"ERROR: Invalid JSON: {e}", file=sys.stderr)
        return 2

    baseline_metrics = extract_metric(baseline_data, args.metric)
    current_metrics = extract_metric(current_data, args.metric)

    if not baseline_metrics:
        print(
            f"WARNING: No '{args.metric}' found in baseline. Skipping check.",
            file=sys.stderr,
        )
        return 0

    if not current_metrics:
        print(
            f"WARNING: No '{args.metric}' found in current. Skipping check.",
            file=sys.stderr,
        )
        return 0

    # Compute per-benchmark regression
    regressions: list[float] = []
    print(f"{'Benchmark':<40} {'Baseline':>10} {'Current':>10} {'Change':>10}")
    print("-" * 74)

    common = sorted(set(baseline_metrics) & set(current_metrics))
    for name in common:
        base_val = baseline_metrics[name]
        curr_val = current_metrics[name]
        pct_change = ((curr_val - base_val) / base_val) * 100
        regressions.append(pct_change)

        marker = " ***" if pct_change > args.max_regression_pct else ""
        print(
            f"{name:<40} {base_val:>10.3f} {curr_val:>10.3f} {pct_change:>+9.1f}%{marker}"
        )

    if not regressions:
        print("\nWARNING: No common benchmarks found between baseline and current.")
        return 0

    median_regression = statistics.median(regressions)
    max_regression = max(regressions)
    worst_bench = common[regressions.index(max_regression)]

    print(f"\nMedian regression: {median_regression:+.1f}%")
    print(f"Worst regression: {max_regression:+.1f}% ({worst_bench})")
    print(f"Threshold: {args.max_regression_pct:.1f}%")

    if median_regression > args.max_regression_pct:
        print(
            f"\nFAILED: Median compile throughput regression "
            f"({median_regression:+.1f}%) exceeds threshold "
            f"({args.max_regression_pct:.1f}%)."
        )
        return 1

    print("\nPASSED: Compile throughput within acceptable bounds.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
