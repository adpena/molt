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
from dataclasses import dataclass
from pathlib import Path


@dataclass
class MetricExtraction:
    values: dict[str, float]
    missing_metric: list[str]


def _extract_metric_value(entry: dict, metric: str) -> float | None:
    if metric in entry:
        val = entry[metric]
        if isinstance(val, (int, float)) and val > 0:
            return float(val)

    for sub_key in ("molt", "metrics", "results"):
        sub = entry.get(sub_key, {})
        if isinstance(sub, dict) and metric in sub:
            val = sub[metric]
            if isinstance(val, (int, float)) and val > 0:
                return float(val)
    return None


def extract_metric(bench_data: dict, metric: str) -> MetricExtraction:
    """Extract a metric from benchmark JSON, preserving benchmark keys."""
    results: dict[str, float] = {}
    missing_metric: list[str] = []

    benchmarks = bench_data.get("benchmarks")
    if benchmarks is None:
        benchmarks = bench_data.get("results", [])

    if isinstance(benchmarks, dict):
        items = benchmarks.items()
        for bench_key, entry in items:
            if not isinstance(entry, dict):
                missing_metric.append(str(bench_key))
                continue
            name = str(entry.get("name") or entry.get("benchmark") or bench_key)
            if not name:
                continue
            metric_value = _extract_metric_value(entry, metric)
            if metric_value is not None:
                results[name] = metric_value
            else:
                missing_metric.append(name)
        return MetricExtraction(results, sorted(set(missing_metric)))

    if not isinstance(benchmarks, list):
        return MetricExtraction(results, missing_metric)

    for idx, entry in enumerate(benchmarks):
        if not isinstance(entry, dict):
            missing_metric.append(f"<entry[{idx}]>")
            continue
        name = str(entry.get("name") or entry.get("benchmark") or "")
        if not name:
            continue
        metric_value = _extract_metric_value(entry, metric)
        if metric_value is not None:
            results[name] = metric_value
        else:
            missing_metric.append(name)

    return MetricExtraction(results, sorted(set(missing_metric)))


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
        help=(
            "Allow missing benchmark metrics/keys in baseline or current and "
            "compare only the overlapping metric set"
        ),
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

    baseline_extract = extract_metric(baseline_data, args.metric)
    current_extract = extract_metric(current_data, args.metric)
    baseline_metrics = baseline_extract.values
    current_metrics = current_extract.values

    missing_in_baseline = sorted(set(current_metrics) - set(baseline_metrics))
    missing_in_current = sorted(set(baseline_metrics) - set(current_metrics))

    missing_errors: list[str] = []
    if baseline_extract.missing_metric:
        missing_errors.append(
            f"baseline missing metric '{args.metric}' for: "
            + ", ".join(baseline_extract.missing_metric)
        )
    if current_extract.missing_metric:
        missing_errors.append(
            f"current missing metric '{args.metric}' for: "
            + ", ".join(current_extract.missing_metric)
        )
    if missing_in_baseline:
        missing_errors.append(
            "baseline missing benchmark keys present in current metrics: "
            + ", ".join(missing_in_baseline)
        )
    if missing_in_current:
        missing_errors.append(
            "current missing benchmark keys present in baseline metrics: "
            + ", ".join(missing_in_current)
        )

    if missing_errors:
        prefix = "WARNING" if args.allow_missing_metrics else "ERROR"
        for line in missing_errors:
            print(f"{prefix}: {line}", file=sys.stderr)
        if not args.allow_missing_metrics:
            print(
                "ERROR: Missing metrics are a hard failure by default. "
                "Use --allow-missing-metrics to opt out.",
                file=sys.stderr,
            )
            return 1

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
