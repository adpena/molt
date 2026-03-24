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
        # Dict-keyed format: {"bench_a": {"molt_build_s": 1.0}, ...}
        benchmarks = [
            {**v, "name": k} for k, v in benchmarks.items()
        ]

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


def _extract_benchmark_names(bench_data: dict) -> set[str]:
    """Return the set of benchmark names present in the data (regardless of metrics)."""
    names: set[str] = set()
    benchmarks = bench_data.get("benchmarks", bench_data.get("results", []))
    if isinstance(benchmarks, dict):
        return set(benchmarks.keys())
    for entry in benchmarks:
        name = entry.get("name", entry.get("benchmark", ""))
        if name:
            names.add(name)
    return names


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
    parser.add_argument(
        "--allow-missing-metrics",
        action="store_true",
        help="Warn instead of failing when current is missing metrics for baseline benchmarks.",
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

    # Detect benchmarks present in baseline but missing from current
    missing_keys = sorted(set(baseline_metrics) - set(current_metrics))
    if missing_keys:
        # Check if these benchmarks exist in current data but are missing the metric
        current_bench_names = _extract_benchmark_names(current_data)
        missing_metrics = [k for k in missing_keys if k in current_bench_names]
        missing_benchmarks = [k for k in missing_keys if k not in current_bench_names]

        if missing_metrics:
            msg = f"current missing metric '{args.metric}' for: {', '.join(missing_metrics)}"
            if args.allow_missing_metrics:
                print(f"WARNING: {msg}", file=sys.stderr)
            else:
                print(
                    f"ERROR: {msg}\n"
                    f"Missing metrics are a hard failure by default. "
                    f"Use --allow-missing-metrics to downgrade to a warning.",
                    file=sys.stderr,
                )
                return 1

        if missing_benchmarks:
            msg = f"current missing benchmark keys present in baseline metrics: {', '.join(missing_benchmarks)}"
            if args.allow_missing_metrics:
                print(f"WARNING: {msg}", file=sys.stderr)
            else:
                print(f"ERROR: {msg}", file=sys.stderr)
                return 1

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
