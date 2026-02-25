#!/usr/bin/env python3
"""Compare two Molt benchmark JSON artifacts."""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any


_LOWER_IS_BETTER = {
    "cpython_time_s",
    "pypy_time_s",
    "codon_time_s",
    "codon_build_s",
    "codon_size_kb",
    "nuitka_time_s",
    "nuitka_build_s",
    "nuitka_size_kb",
    "pyodide_time_s",
    "pyodide_build_s",
    "pyodide_size_kb",
    "molt_build_s",
    "molt_codon_ratio",
    "molt_cpython_ratio",
    "molt_nuitka_ratio",
    "molt_pyodide_ratio",
    "molt_pypy_ratio",
    "molt_time_s",
    "molt_wasm_build_s",
    "molt_wasm_size_kb",
    "molt_wasm_time_s",
}

_HIGHER_IS_BETTER = {
    "molt_speedup",
}

_SKIP_METRICS = {
    "molt_ok",
    "molt_wasm_ok",
    "molt_wasm_linked",
    "pypy_ok",
    "codon_ok",
    "nuitka_ok",
    "pyodide_ok",
    "run_args",
    "molt_args",
    "super_stats",
    "molt_wasm_stats",
}


@dataclass(frozen=True)
class MetricDiff:
    benchmark: str
    metric: str
    old: float
    new: float
    delta: float
    pct_delta: float | None
    trend: str


@dataclass(frozen=True)
class RegressionViolation:
    benchmark: str
    metric: str
    pct_delta: float | None
    delta: float
    reason: str


def _load_payload(path: Path) -> dict[str, Any]:
    if not path.exists():
        raise SystemExit(f"missing benchmark JSON: {path}")
    return json.loads(path.read_text())


def _benchmark_map(payload: dict[str, Any]) -> dict[str, dict[str, Any]]:
    benches = payload.get("benchmarks")
    if not isinstance(benches, dict):
        raise SystemExit("invalid benchmark JSON: expected top-level 'benchmarks' dict")
    out: dict[str, dict[str, Any]] = {}
    for name, entry in benches.items():
        if isinstance(name, str) and isinstance(entry, dict):
            out[name] = entry
    return out


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _available_metrics(
    old_bench: dict[str, dict[str, Any]],
    new_bench: dict[str, dict[str, Any]],
) -> list[str]:
    metrics: set[str] = set()
    common = sorted(set(old_bench) & set(new_bench))
    for name in common:
        old_entry = old_bench[name]
        new_entry = new_bench[name]
        shared = set(old_entry) & set(new_entry)
        for metric in shared:
            if metric in _SKIP_METRICS:
                continue
            if _is_number(old_entry[metric]) and _is_number(new_entry[metric]):
                metrics.add(metric)
    return sorted(metrics)


def _metric_trend(metric: str, old: float, new: float) -> str:
    if new == old:
        return "flat"
    if metric in _LOWER_IS_BETTER:
        return "improved" if new < old else "regressed"
    if metric in _HIGHER_IS_BETTER:
        return "improved" if new > old else "regressed"
    return "up" if new > old else "down"


def _compute_metric_diffs(
    metric: str,
    old_bench: dict[str, dict[str, Any]],
    new_bench: dict[str, dict[str, Any]],
) -> list[MetricDiff]:
    rows: list[MetricDiff] = []
    for benchmark in sorted(set(old_bench) & set(new_bench)):
        old_raw = old_bench[benchmark].get(metric)
        new_raw = new_bench[benchmark].get(metric)
        if not (_is_number(old_raw) and _is_number(new_raw)):
            continue
        old_val = float(old_raw)
        new_val = float(new_raw)
        delta = new_val - old_val
        pct_delta = None if old_val == 0 else (delta / old_val) * 100.0
        rows.append(
            MetricDiff(
                benchmark=benchmark,
                metric=metric,
                old=old_val,
                new=new_val,
                delta=delta,
                pct_delta=pct_delta,
                trend=_metric_trend(metric, old_val, new_val),
            )
        )
    return rows


def _pct_sort_value(row: MetricDiff) -> float:
    if row.pct_delta is None:
        return abs(row.delta)
    return abs(row.pct_delta)


def _fmt_float(value: float) -> str:
    return f"{value:.6f}"


def _fmt_pct(value: float | None) -> str:
    if value is None:
        return "n/a"
    sign = "+" if value >= 0 else ""
    return f"{sign}{value:.2f}%"


def _print_metric_table(metric: str, rows: list[MetricDiff], top: int) -> None:
    print(f"\n## {metric}")
    if not rows:
        print("No comparable rows.")
        return
    improved = sum(1 for row in rows if row.trend == "improved")
    regressed = sum(1 for row in rows if row.trend == "regressed")
    flat = sum(1 for row in rows if row.trend == "flat")
    print(
        f"Rows: {len(rows)} | improved={improved} | regressed={regressed} | flat={flat}"
    )
    ordered = sorted(rows, key=_pct_sort_value, reverse=True)
    print("| benchmark | old | new | delta | pct_delta | trend |")
    print("| --- | ---: | ---: | ---: | ---: | --- |")
    for row in ordered[:top]:
        print(
            "| "
            f"`{row.benchmark}` | {_fmt_float(row.old)} | {_fmt_float(row.new)} | "
            f"{_fmt_float(row.delta)} | {_fmt_pct(row.pct_delta)} | {row.trend} |"
        )


def _metric_summary_json(
    metric: str, rows: list[MetricDiff], top: int
) -> dict[str, Any]:
    ordered = sorted(rows, key=_pct_sort_value, reverse=True)
    return {
        "metric": metric,
        "rows": len(rows),
        "improved": sum(1 for row in rows if row.trend == "improved"),
        "regressed": sum(1 for row in rows if row.trend == "regressed"),
        "flat": sum(1 for row in rows if row.trend == "flat"),
        "top_changes": [
            {
                "benchmark": row.benchmark,
                "old": row.old,
                "new": row.new,
                "delta": row.delta,
                "pct_delta": row.pct_delta,
                "trend": row.trend,
            }
            for row in ordered[:top]
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Diff two benchmark JSON artifacts.")
    parser.add_argument("old_json", type=Path, help="Older benchmark JSON artifact")
    parser.add_argument("new_json", type=Path, help="Newer benchmark JSON artifact")
    parser.add_argument(
        "--metrics",
        nargs="+",
        default=None,
        help="Explicit metric list to diff (default: all numeric shared metrics).",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        help="Number of top absolute changes to show per metric (default: 10).",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="Optional output path for machine-readable diff summary.",
    )
    parser.add_argument(
        "--include-zero-only-metrics",
        action="store_true",
        help=(
            "Include metrics where every shared row is 0 -> 0 "
            "(default: skip to reduce noise)."
        ),
    )
    parser.add_argument(
        "--fail-regression-count",
        type=int,
        default=None,
        help=(
            "Fail with exit code 2 when total regressed rows exceed this count "
            "(across selected metrics)."
        ),
    )
    parser.add_argument(
        "--fail-regression-pct",
        type=float,
        default=None,
        help=(
            "Fail with exit code 2 when any regressed row has absolute percent "
            "change >= this threshold."
        ),
    )
    parser.add_argument(
        "--fail-regression-abs",
        type=float,
        default=None,
        help=(
            "Absolute fallback threshold for regressions where percent delta is "
            "undefined (for example old value is 0)."
        ),
    )
    args = parser.parse_args()

    old_payload = _load_payload(args.old_json)
    new_payload = _load_payload(args.new_json)
    old_bench = _benchmark_map(old_payload)
    new_bench = _benchmark_map(new_payload)

    old_names = set(old_bench)
    new_names = set(new_bench)
    common_names = sorted(old_names & new_names)
    added = sorted(new_names - old_names)
    removed = sorted(old_names - new_names)

    available = _available_metrics(old_bench, new_bench)
    if args.metrics is None:
        metrics = available
    else:
        wanted = list(dict.fromkeys(args.metrics))
        unknown = [metric for metric in wanted if metric not in available]
        if unknown:
            raise SystemExit(
                "requested metrics are unavailable/non-numeric in shared rows: "
                + ", ".join(unknown)
            )
        metrics = wanted

    print("# Benchmark Diff")
    print(f"Old: `{args.old_json}`")
    print(f"New: `{args.new_json}`")
    print(
        "Benchmarks: "
        f"old={len(old_names)} | new={len(new_names)} | shared={len(common_names)}"
    )
    if added:
        print("Added benchmarks: " + ", ".join(f"`{name}`" for name in added))
    if removed:
        print("Removed benchmarks: " + ", ".join(f"`{name}`" for name in removed))
    if not metrics:
        print("No shared numeric metrics to diff.")
        return 0

    metric_summaries: list[dict[str, Any]] = []
    all_regressions: list[MetricDiff] = []
    threshold_violations: list[RegressionViolation] = []
    fail_regression_pct = (
        None if args.fail_regression_pct is None else abs(args.fail_regression_pct)
    )
    fail_regression_abs = (
        None if args.fail_regression_abs is None else abs(args.fail_regression_abs)
    )
    for metric in metrics:
        rows = _compute_metric_diffs(metric, old_bench, new_bench)
        if (
            not args.include_zero_only_metrics
            and rows
            and all(row.old == 0 and row.new == 0 for row in rows)
        ):
            continue
        _print_metric_table(metric, rows, args.top)
        regressions = [row for row in rows if row.trend == "regressed"]
        all_regressions.extend(regressions)
        if fail_regression_pct is not None or fail_regression_abs is not None:
            for row in regressions:
                if (
                    fail_regression_pct is not None
                    and row.pct_delta is not None
                    and abs(row.pct_delta) >= fail_regression_pct
                ):
                    threshold_violations.append(
                        RegressionViolation(
                            benchmark=row.benchmark,
                            metric=row.metric,
                            pct_delta=row.pct_delta,
                            delta=row.delta,
                            reason=f"abs(pct_delta) >= {fail_regression_pct:.4f}",
                        )
                    )
                    continue
                if (
                    row.pct_delta is None
                    and fail_regression_abs is not None
                    and abs(row.delta) >= fail_regression_abs
                ):
                    threshold_violations.append(
                        RegressionViolation(
                            benchmark=row.benchmark,
                            metric=row.metric,
                            pct_delta=row.pct_delta,
                            delta=row.delta,
                            reason=f"abs(delta) >= {fail_regression_abs:.6f}",
                        )
                    )
        metric_summaries.append(_metric_summary_json(metric, rows, args.top))

    failure_reasons: list[str] = []
    if args.fail_regression_count is not None:
        if args.fail_regression_count < 0:
            raise SystemExit("--fail-regression-count must be >= 0")
        if len(all_regressions) > args.fail_regression_count:
            failure_reasons.append(
                (
                    "regressed-row count exceeded threshold: "
                    f"{len(all_regressions)} > {args.fail_regression_count}"
                )
            )
    if threshold_violations:
        failure_reasons.append(
            f"threshold regressions detected: {len(threshold_violations)}"
        )
        print("\n## Regression Threshold Violations")
        print("| metric | benchmark | pct_delta | delta | reason |")
        print("| --- | --- | ---: | ---: | --- |")
        ordered_violations = sorted(
            threshold_violations,
            key=lambda item: abs(item.pct_delta)
            if item.pct_delta is not None
            else abs(item.delta),
            reverse=True,
        )
        for item in ordered_violations[: args.top]:
            print(
                "| "
                f"`{item.metric}` | `{item.benchmark}` | "
                f"{_fmt_pct(item.pct_delta)} | {_fmt_float(item.delta)} | "
                f"{item.reason} |"
            )

    if args.json_out is not None:
        payload = {
            "schema_version": 1,
            "old_json": str(args.old_json),
            "new_json": str(args.new_json),
            "benchmarks": {
                "old_count": len(old_names),
                "new_count": len(new_names),
                "shared_count": len(common_names),
                "added": added,
                "removed": removed,
            },
            "metrics": metric_summaries,
            "gates": {
                "fail_regression_count": args.fail_regression_count,
                "fail_regression_pct": args.fail_regression_pct,
                "fail_regression_abs": args.fail_regression_abs,
                "regressed_rows": len(all_regressions),
                "threshold_violations": [
                    {
                        "metric": item.metric,
                        "benchmark": item.benchmark,
                        "pct_delta": item.pct_delta,
                        "delta": item.delta,
                        "reason": item.reason,
                    }
                    for item in threshold_violations
                ],
                "failed": bool(failure_reasons),
                "failure_reasons": failure_reasons,
            },
        }
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        print(f"\nWrote diff JSON: {args.json_out}")

    if failure_reasons:
        print("\nBenchmark diff gate failed:")
        for reason in failure_reasons:
            print(f"- {reason}")
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
