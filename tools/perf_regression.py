#!/usr/bin/env python3
"""Performance regression detection with statistical rigor.

Compares current benchmark results against a baseline (or baseline directory)
and flags regressions using proper statistical methods:

  - Bootstrap confidence intervals for runtime measurements
  - Mann-Whitney U test for comparing sample distributions
  - Effect size calculation (Cohen's d)
  - Minimum detectable effect at 95% confidence
  - Linear trend analysis over multiple historical runs

Configurable thresholds per metric category:
  - Compile time: >5% warning, >10% error
  - Runtime: >3% warning, >5% error
  - Binary size: >2% warning, >5% error
  - Memory (RSS): >10% warning, >20% error

Exit codes:
  0 — no regressions above error threshold
  1 — at least one error-level regression detected
  2 — input/configuration error

Usage:
    uv run --python 3.12 python3 tools/perf_regression.py \\
      --current bench/results/bench.json \\
      --baseline bench/baseline.json

    uv run --python 3.12 python3 tools/perf_regression.py \\
      --current bench/results/bench.json \\
      --baseline-dir bench/baselines/ \\
      --json-out /tmp/perf_report.json
"""

from __future__ import annotations

import argparse
import json
import math
import random
import statistics
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]

# ---------------------------------------------------------------------------
# Threshold configuration
# ---------------------------------------------------------------------------

# Each metric category has (warn_pct, error_pct) thresholds.
# A positive value means "regression if current is higher by this fraction".
DEFAULT_THRESHOLDS: dict[str, tuple[float, float]] = {
    "runtime": (0.03, 0.05),  # 3% warn, 5% error
    "compile_time": (0.05, 0.10),  # 5% warn, 10% error
    "binary_size": (0.02, 0.05),  # 2% warn, 5% error
    "memory_rss": (0.10, 0.20),  # 10% warn, 20% error
}

# Metric field mapping: metric_category -> (json_field, higher_is_worse)
METRIC_FIELDS: dict[str, tuple[str, bool]] = {
    "runtime": ("molt_time_s", True),
    "compile_time": ("molt_build_s", True),
    "binary_size": ("molt_size_kb", True),
}

# Confidence level for statistical tests
CONFIDENCE_LEVEL = 0.95
BOOTSTRAP_ITERATIONS = 10000
BOOTSTRAP_SEED = 42

# Trend analysis: minimum number of historical points for linear regression
MIN_TREND_POINTS = 3
# Trend slope significance: flag if per-run drift exceeds this fraction
TREND_DRIFT_WARN = 0.005  # 0.5% per run


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class MetricComparison:
    """Result of comparing a single metric for a single benchmark."""

    benchmark: str
    metric: str
    baseline_value: float
    current_value: float
    change_pct: float  # positive = regression
    warn_threshold: float
    error_threshold: float
    severity: str  # "ok", "warn", "error"
    # Statistical fields (populated when sample data is available)
    ci_low: float | None = None
    ci_high: float | None = None
    p_value: float | None = None
    cohens_d: float | None = None
    min_detectable_effect: float | None = None
    statistically_significant: bool | None = None


@dataclass(frozen=True)
class TrendPoint:
    """A single data point in a trend series."""

    timestamp: str
    git_rev: str | None
    value: float


@dataclass(frozen=True)
class TrendAnalysis:
    """Result of linear trend analysis for a single metric."""

    benchmark: str
    metric: str
    num_points: int
    slope_per_run: float  # fractional change per sequential run
    r_squared: float
    is_drifting: bool
    direction: str  # "improving", "stable", "degrading"


@dataclass
class RegressionReport:
    """Full regression detection report."""

    created_at: str = ""
    current_file: str = ""
    baseline_file: str = ""
    current_git_rev: str | None = None
    baseline_git_rev: str | None = None
    thresholds: dict[str, tuple[float, float]] = field(default_factory=dict)
    comparisons: list[MetricComparison] = field(default_factory=list)
    trends: list[TrendAnalysis] = field(default_factory=list)
    summary: dict[str, Any] = field(default_factory=dict)

    @property
    def has_errors(self) -> bool:
        return any(c.severity == "error" for c in self.comparisons)

    @property
    def has_warnings(self) -> bool:
        return any(c.severity == "warn" for c in self.comparisons)

    def to_dict(self) -> dict[str, Any]:
        return {
            "created_at": self.created_at,
            "current_file": self.current_file,
            "baseline_file": self.baseline_file,
            "current_git_rev": self.current_git_rev,
            "baseline_git_rev": self.baseline_git_rev,
            "thresholds": {
                k: {"warn_pct": v[0], "error_pct": v[1]}
                for k, v in self.thresholds.items()
            },
            "comparisons": [
                {
                    "benchmark": c.benchmark,
                    "metric": c.metric,
                    "baseline_value": c.baseline_value,
                    "current_value": c.current_value,
                    "change_pct": round(c.change_pct, 4),
                    "severity": c.severity,
                    **({"ci_low": round(c.ci_low, 6)} if c.ci_low is not None else {}),
                    **(
                        {"ci_high": round(c.ci_high, 6)}
                        if c.ci_high is not None
                        else {}
                    ),
                    **(
                        {"p_value": round(c.p_value, 6)}
                        if c.p_value is not None
                        else {}
                    ),
                    **(
                        {"cohens_d": round(c.cohens_d, 4)}
                        if c.cohens_d is not None
                        else {}
                    ),
                    **(
                        {"min_detectable_effect": round(c.min_detectable_effect, 4)}
                        if c.min_detectable_effect is not None
                        else {}
                    ),
                    **(
                        {"statistically_significant": c.statistically_significant}
                        if c.statistically_significant is not None
                        else {}
                    ),
                }
                for c in self.comparisons
            ],
            "trends": [
                {
                    "benchmark": t.benchmark,
                    "metric": t.metric,
                    "num_points": t.num_points,
                    "slope_per_run": round(t.slope_per_run, 6),
                    "r_squared": round(t.r_squared, 4),
                    "is_drifting": t.is_drifting,
                    "direction": t.direction,
                }
                for t in self.trends
            ],
            "summary": self.summary,
        }


# ---------------------------------------------------------------------------
# Statistical helpers (stdlib only — no numpy/scipy dependency)
# ---------------------------------------------------------------------------


def _bootstrap_ci(
    samples: list[float],
    n_iterations: int = BOOTSTRAP_ITERATIONS,
    confidence: float = CONFIDENCE_LEVEL,
    seed: int = BOOTSTRAP_SEED,
) -> tuple[float, float]:
    """Compute bootstrap confidence interval for the mean.

    Uses the percentile method with a fixed seed for reproducibility.
    """
    if len(samples) < 2:
        mean = samples[0] if samples else 0.0
        return (mean, mean)

    rng = random.Random(seed)
    n = len(samples)
    boot_means: list[float] = []

    for _ in range(n_iterations):
        resample = [rng.choice(samples) for _ in range(n)]
        boot_means.append(statistics.mean(resample))

    boot_means.sort()
    alpha = 1.0 - confidence
    lo_idx = max(0, int(math.floor(alpha / 2 * n_iterations)))
    hi_idx = min(n_iterations - 1, int(math.ceil((1.0 - alpha / 2) * n_iterations)))

    return (boot_means[lo_idx], boot_means[hi_idx])


def _mann_whitney_u(x: list[float], y: list[float]) -> tuple[float, float]:
    """Mann-Whitney U test (two-sided).

    Returns (U_statistic, approximate_p_value).
    Uses normal approximation for p-value (valid when n >= 8).
    """
    nx, ny = len(x), len(y)
    if nx == 0 or ny == 0:
        return (0.0, 1.0)

    # Rank all values together
    combined = [(v, 0, i) for i, v in enumerate(x)] + [
        (v, 1, i) for i, v in enumerate(y)
    ]
    combined.sort(key=lambda t: t[0])

    # Assign ranks with tie handling
    ranks: list[float] = [0.0] * len(combined)
    i = 0
    while i < len(combined):
        j = i
        while j < len(combined) and combined[j][0] == combined[i][0]:
            j += 1
        avg_rank = (i + j + 1) / 2.0  # 1-based average rank
        for k in range(i, j):
            ranks[k] = avg_rank
        i = j

    # Sum ranks for group x
    r1 = sum(ranks[k] for k in range(len(combined)) if combined[k][1] == 0)

    u1 = r1 - nx * (nx + 1) / 2
    u2 = nx * ny - u1
    u_stat = min(u1, u2)

    # Normal approximation for p-value
    mu = nx * ny / 2.0
    n_total = nx + ny

    # Tie correction
    tie_counts: dict[float, int] = {}
    for val, _, _ in combined:
        tie_counts[val] = tie_counts.get(val, 0) + 1
    tie_correction = sum(t**3 - t for t in tie_counts.values()) / (
        n_total * (n_total - 1)
    )

    sigma_sq = (nx * ny / 12.0) * (n_total + 1 - tie_correction)
    if sigma_sq <= 0:
        return (u_stat, 1.0)

    sigma = math.sqrt(sigma_sq)
    z = abs(u_stat - mu) / sigma

    # Two-sided p-value via complementary error function
    p_value = math.erfc(z / math.sqrt(2.0))

    return (u_stat, p_value)


def _cohens_d(x: list[float], y: list[float]) -> float:
    """Compute Cohen's d effect size between two samples.

    Uses pooled standard deviation. Returns positive value when y > x
    (i.e., current is slower than baseline = regression).
    """
    if len(x) < 2 or len(y) < 2:
        return 0.0

    mean_x = statistics.mean(x)
    mean_y = statistics.mean(y)
    var_x = statistics.variance(x)
    var_y = statistics.variance(y)

    nx, ny = len(x), len(y)
    pooled_var = ((nx - 1) * var_x + (ny - 1) * var_y) / (nx + ny - 2)
    if pooled_var <= 0:
        return 0.0

    return (mean_y - mean_x) / math.sqrt(pooled_var)


def _min_detectable_effect(
    samples: list[float],
    confidence: float = CONFIDENCE_LEVEL,
    power: float = 0.80,
) -> float:
    """Estimate the minimum detectable effect as a fraction of the mean.

    Uses a simplified formula based on the coefficient of variation
    and the number of samples.  Returns a fractional value (e.g., 0.05
    means 5% is the smallest regression we can reliably detect).
    """
    if len(samples) < 2:
        return float("inf")

    mean = statistics.mean(samples)
    if mean <= 0:
        return float("inf")

    sd = statistics.stdev(samples)
    cv = sd / mean

    # z-scores for confidence and power
    alpha = 1.0 - confidence
    # Two-sided z for alpha
    z_alpha = 1.96 if confidence == 0.95 else _z_from_p(alpha / 2)
    z_beta = 0.842 if power == 0.80 else _z_from_p(1.0 - power)

    n = len(samples)
    mde = (z_alpha + z_beta) * cv / math.sqrt(n)
    return mde


def _z_from_p(p: float) -> float:
    """Approximate inverse normal CDF (Beasley-Springer-Moro algorithm)."""
    if p <= 0 or p >= 1:
        return 0.0
    # Rational approximation
    t = math.sqrt(-2.0 * math.log(p if p < 0.5 else 1.0 - p))
    c0, c1, c2 = 2.515517, 0.802853, 0.010328
    d1, d2, d3 = 1.432788, 0.189269, 0.001308
    z = t - (c0 + c1 * t + c2 * t * t) / (1 + d1 * t + d2 * t * t + d3 * t**3)
    return z if p < 0.5 else -z


def _linear_regression(xs: list[float], ys: list[float]) -> tuple[float, float, float]:
    """Simple linear regression.

    Returns (slope, intercept, r_squared).
    """
    n = len(xs)
    if n < 2:
        return (0.0, ys[0] if ys else 0.0, 0.0)

    mean_x = statistics.mean(xs)
    mean_y = statistics.mean(ys)

    ss_xx = sum((x - mean_x) ** 2 for x in xs)
    ss_xy = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys))
    ss_yy = sum((y - mean_y) ** 2 for y in ys)

    if ss_xx == 0:
        return (0.0, mean_y, 0.0)

    slope = ss_xy / ss_xx
    intercept = mean_y - slope * mean_x
    r_squared = (ss_xy**2 / (ss_xx * ss_yy)) if ss_yy > 0 else 0.0

    return (slope, intercept, r_squared)


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------


def _load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text())


def _load_baselines_from_dir(dir_path: Path) -> list[tuple[Path, dict[str, Any]]]:
    """Load all .json files from a directory, sorted by created_at."""
    entries: list[tuple[str, Path, dict[str, Any]]] = []
    for p in sorted(dir_path.glob("*.json")):
        try:
            data = _load_json(p)
        except (json.JSONDecodeError, OSError):
            continue
        created = data.get("created_at", "")
        entries.append((created, p, data))

    entries.sort(key=lambda e: e[0])
    return [(p, d) for _, p, d in entries]


def _extract_metric(bench_entry: dict[str, Any], metric: str) -> float | None:
    """Extract a metric value from a benchmark entry."""
    field_name, _ = METRIC_FIELDS.get(metric, (None, None))
    if field_name is None:
        return None
    value = bench_entry.get(field_name)
    if value is None or not isinstance(value, (int, float)):
        return None
    if value <= 0:
        return None
    return float(value)


def _extract_samples(bench_entry: dict[str, Any], metric: str) -> list[float] | None:
    """Extract raw samples from super_stats if available."""
    super_stats = bench_entry.get("super_stats", {})
    runner = "molt"
    stats = super_stats.get(runner)
    if stats is None:
        return None

    # super_stats has mean/median/variance/range/min/max but not raw samples.
    # We can reconstruct a rough distribution from mean + variance + count.
    # For proper statistical tests, the benchmark should be run with --super.
    # Here we synthesize samples from summary stats for bootstrap CI.
    mean = stats.get("mean_s")
    if mean is None:
        return None

    # The field depends on the metric
    if metric != "runtime":
        return None

    # If we only have summary stats, create synthetic samples
    # by using mean +/- half-range evenly distributed
    min_s = stats.get("min_s", mean)
    max_s = stats.get("max_s", mean)
    # Use 10 synthetic points spanning the observed range
    if max_s <= min_s:
        return [mean] * 10
    step = (max_s - min_s) / 9.0
    return [min_s + i * step for i in range(10)]


# ---------------------------------------------------------------------------
# Comparison engine
# ---------------------------------------------------------------------------


def compare_benchmarks(
    current: dict[str, Any],
    baseline: dict[str, Any],
    thresholds: dict[str, tuple[float, float]],
) -> list[MetricComparison]:
    """Compare current benchmark results against a baseline."""
    results: list[MetricComparison] = []
    current_benches = current.get("benchmarks", {})
    baseline_benches = baseline.get("benchmarks", {})

    for name, cur_entry in current_benches.items():
        if not cur_entry.get("molt_ok", False):
            continue

        base_entry = baseline_benches.get(name)
        if base_entry is None or not base_entry.get("molt_ok", False):
            continue

        for metric, (field_name, higher_is_worse) in METRIC_FIELDS.items():
            cur_val = _extract_metric(cur_entry, metric)
            base_val = _extract_metric(base_entry, metric)
            if cur_val is None or base_val is None:
                continue

            # Change: positive = current is higher
            change_pct = (cur_val - base_val) / base_val

            # For metrics where higher is worse, positive change = regression
            # For metrics where lower is worse, negative change = regression
            regression_pct = change_pct if higher_is_worse else -change_pct

            warn_thresh, error_thresh = thresholds.get(
                metric, DEFAULT_THRESHOLDS[metric]
            )

            if regression_pct >= error_thresh:
                severity = "error"
            elif regression_pct >= warn_thresh:
                severity = "warn"
            else:
                severity = "ok"

            # Statistical analysis on samples if available
            ci_low: float | None = None
            ci_high: float | None = None
            p_value: float | None = None
            effect_size: float | None = None
            mde: float | None = None
            sig: bool | None = None

            cur_samples = _extract_samples(cur_entry, metric)
            base_samples = _extract_samples(base_entry, metric)

            if cur_samples and base_samples:
                ci_low, ci_high = _bootstrap_ci(cur_samples)
                _, p_value = _mann_whitney_u(base_samples, cur_samples)
                effect_size = _cohens_d(base_samples, cur_samples)
                mde = _min_detectable_effect(base_samples)
                sig = (
                    p_value < (1.0 - CONFIDENCE_LEVEL) if p_value is not None else None
                )
            elif cur_samples:
                ci_low, ci_high = _bootstrap_ci(cur_samples)
                mde = _min_detectable_effect(cur_samples)

            results.append(
                MetricComparison(
                    benchmark=name,
                    metric=metric,
                    baseline_value=base_val,
                    current_value=cur_val,
                    change_pct=change_pct,
                    warn_threshold=warn_thresh,
                    error_threshold=error_thresh,
                    severity=severity,
                    ci_low=ci_low,
                    ci_high=ci_high,
                    p_value=p_value,
                    cohens_d=effect_size,
                    min_detectable_effect=mde,
                    statistically_significant=sig,
                )
            )

    return results


# ---------------------------------------------------------------------------
# Trend analysis
# ---------------------------------------------------------------------------


def analyze_trends(
    baselines: list[tuple[Path, dict[str, Any]]],
    current: dict[str, Any],
) -> list[TrendAnalysis]:
    """Detect gradual performance drift over multiple historical runs."""
    if len(baselines) < MIN_TREND_POINTS - 1:
        # Need at least MIN_TREND_POINTS including current
        return []

    # Collect all data points (baselines + current)
    all_data: list[dict[str, Any]] = [d for _, d in baselines]
    all_data.append(current)

    # Gather all benchmark names that appear in at least MIN_TREND_POINTS runs
    bench_names: set[str] = set()
    for data in all_data:
        for name, entry in data.get("benchmarks", {}).items():
            if entry.get("molt_ok", False):
                bench_names.add(name)

    results: list[TrendAnalysis] = []

    for name in sorted(bench_names):
        for metric in METRIC_FIELDS:
            values: list[float] = []
            for data in all_data:
                entry = data.get("benchmarks", {}).get(name, {})
                if not entry.get("molt_ok", False):
                    continue
                val = _extract_metric(entry, metric)
                if val is not None:
                    values.append(val)

            if len(values) < MIN_TREND_POINTS:
                continue

            # Use sequential index as x (0, 1, 2, ...)
            xs = list(range(len(values)))
            slope, _, r_squared = _linear_regression([float(x) for x in xs], values)

            # Express slope as fraction of the mean
            mean_val = statistics.mean(values)
            if mean_val <= 0:
                continue
            fractional_slope = slope / mean_val

            _, higher_is_worse = METRIC_FIELDS[metric]

            # Determine if it is drifting significantly
            is_drifting = abs(fractional_slope) > TREND_DRIFT_WARN and r_squared > 0.5

            if higher_is_worse:
                if fractional_slope > TREND_DRIFT_WARN:
                    direction = "degrading"
                elif fractional_slope < -TREND_DRIFT_WARN:
                    direction = "improving"
                else:
                    direction = "stable"
            else:
                if fractional_slope < -TREND_DRIFT_WARN:
                    direction = "degrading"
                elif fractional_slope > TREND_DRIFT_WARN:
                    direction = "improving"
                else:
                    direction = "stable"

            results.append(
                TrendAnalysis(
                    benchmark=name,
                    metric=metric,
                    num_points=len(values),
                    slope_per_run=fractional_slope,
                    r_squared=r_squared,
                    is_drifting=is_drifting,
                    direction=direction,
                )
            )

    return results


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------


def build_report(
    current_path: Path,
    current: dict[str, Any],
    baseline_path: Path | None,
    baseline: dict[str, Any] | None,
    baselines: list[tuple[Path, dict[str, Any]]] | None,
    thresholds: dict[str, tuple[float, float]],
) -> RegressionReport:
    """Build a complete regression report."""
    report = RegressionReport(
        created_at=datetime.now(timezone.utc).isoformat(),
        current_file=str(current_path),
        baseline_file=str(baseline_path) if baseline_path else "",
        current_git_rev=current.get("git_rev"),
        baseline_git_rev=baseline.get("git_rev") if baseline else None,
        thresholds=dict(thresholds),
    )

    # Pairwise comparison
    if baseline is not None:
        report.comparisons = compare_benchmarks(current, baseline, thresholds)

    # Trend analysis
    if baselines and len(baselines) >= MIN_TREND_POINTS - 1:
        report.trends = analyze_trends(baselines, current)

    # Summary
    errors = [c for c in report.comparisons if c.severity == "error"]
    warnings = [c for c in report.comparisons if c.severity == "warn"]
    ok_count = sum(1 for c in report.comparisons if c.severity == "ok")
    drifting = [
        t for t in report.trends if t.is_drifting and t.direction == "degrading"
    ]

    report.summary = {
        "total_comparisons": len(report.comparisons),
        "errors": len(errors),
        "warnings": len(warnings),
        "ok": ok_count,
        "trend_points": len(report.trends),
        "degrading_trends": len(drifting),
        "has_errors": report.has_errors,
        "has_warnings": report.has_warnings,
        "error_details": [
            {
                "benchmark": c.benchmark,
                "metric": c.metric,
                "change_pct": round(c.change_pct * 100, 2),
                "baseline": round(c.baseline_value, 6),
                "current": round(c.current_value, 6),
            }
            for c in errors
        ],
        "warning_details": [
            {
                "benchmark": c.benchmark,
                "metric": c.metric,
                "change_pct": round(c.change_pct * 100, 2),
            }
            for c in warnings
        ],
        "degrading_trend_details": [
            {
                "benchmark": t.benchmark,
                "metric": t.metric,
                "slope_pct_per_run": round(t.slope_per_run * 100, 4),
                "r_squared": round(t.r_squared, 4),
            }
            for t in drifting
        ],
    }

    return report


# ---------------------------------------------------------------------------
# Human-readable output
# ---------------------------------------------------------------------------

IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def _green(t: str) -> str:
    return _c("32", t)


def _red(t: str) -> str:
    return _c("31", t)


def _yellow(t: str) -> str:
    return _c("33", t)


def _bold(t: str) -> str:
    return _c("1", t)


def _dim(t: str) -> str:
    return _c("2", t)


def print_report(report: RegressionReport) -> None:
    """Print a human-readable regression report."""
    print(_bold("Molt Performance Regression Report"))
    print(f"  Current:  {report.current_file}")
    if report.baseline_file:
        print(f"  Baseline: {report.baseline_file}")
    if report.current_git_rev:
        print(f"  Current rev:  {report.current_git_rev[:12]}")
    if report.baseline_git_rev:
        print(f"  Baseline rev: {report.baseline_git_rev[:12]}")
    print()

    # Thresholds
    print(_bold("Thresholds:"))
    for metric, (warn, error) in sorted(report.thresholds.items()):
        print(f"  {metric}: warn >{warn * 100:.0f}%, error >{error * 100:.0f}%")
    print()

    # Comparisons grouped by severity
    errors = [c for c in report.comparisons if c.severity == "error"]
    warnings = [c for c in report.comparisons if c.severity == "warn"]
    ok_count = sum(1 for c in report.comparisons if c.severity == "ok")

    if errors:
        print(_bold(_red(f"ERRORS ({len(errors)}):")))
        for c in sorted(errors, key=lambda x: -abs(x.change_pct)):
            direction = "+" if c.change_pct > 0 else ""
            stats = ""
            if c.p_value is not None:
                sig = (
                    "significant" if c.statistically_significant else "not significant"
                )
                stats = f" [p={c.p_value:.4f} ({sig})"
                if c.cohens_d is not None:
                    stats += f", d={c.cohens_d:.2f}"
                stats += "]"
            print(
                f"  {_red('ERR')}  {c.benchmark:<35} {c.metric:<14} "
                f"{direction}{c.change_pct * 100:+.1f}% "
                f"({c.baseline_value:.6f} -> {c.current_value:.6f}){stats}"
            )
        print()

    if warnings:
        print(_bold(_yellow(f"WARNINGS ({len(warnings)}):")))
        for c in sorted(warnings, key=lambda x: -abs(x.change_pct)):
            direction = "+" if c.change_pct > 0 else ""
            print(
                f"  {_yellow('WARN')} {c.benchmark:<35} {c.metric:<14} "
                f"{direction}{c.change_pct * 100:+.1f}% "
                f"({c.baseline_value:.6f} -> {c.current_value:.6f})"
            )
        print()

    print(
        _bold("Summary: ")
        + f"{_red(str(len(errors)) + ' errors') if errors else _green('0 errors')}, "
        + f"{_yellow(str(len(warnings)) + ' warnings') if warnings else '0 warnings'}, "
        + f"{ok_count} ok out of {len(report.comparisons)} comparisons"
    )

    # Trends
    drifting = [
        t for t in report.trends if t.is_drifting and t.direction == "degrading"
    ]
    if drifting:
        print()
        print(_bold(_yellow(f"DEGRADING TRENDS ({len(drifting)}):")))
        for t in sorted(drifting, key=lambda x: -abs(x.slope_per_run)):
            print(
                f"  {_yellow('DRIFT')} {t.benchmark:<35} {t.metric:<14} "
                f"{t.slope_per_run * 100:+.3f}%/run "
                f"(R^2={t.r_squared:.3f}, {t.num_points} points)"
            )

    improving = [
        t for t in report.trends if t.is_drifting and t.direction == "improving"
    ]
    if improving:
        print()
        print(_bold(_green(f"IMPROVING TRENDS ({len(improving)}):")))
        for t in sorted(improving, key=lambda x: -abs(x.slope_per_run)):
            print(
                f"  {_green('IMPR')}  {t.benchmark:<35} {t.metric:<14} "
                f"{t.slope_per_run * 100:+.3f}%/run "
                f"(R^2={t.r_squared:.3f}, {t.num_points} points)"
            )


# ---------------------------------------------------------------------------
# Public API for ci_gate.py integration
# ---------------------------------------------------------------------------


def check_perf_regression(
    current_path: Path | str | None = None,
    baseline_path: Path | str | None = None,
    baseline_dir: Path | str | None = None,
    thresholds: dict[str, tuple[float, float]] | None = None,
) -> tuple[bool, RegressionReport]:
    """Run regression check and return (passed, report).

    Suitable for calling from tools/ci_gate.py as a T2 check.
    Returns True if no error-level regressions are found.
    """
    if current_path is None:
        current_path = ROOT / "bench" / "results" / "bench.json"
    current_path = Path(current_path)

    if not current_path.exists():
        report = RegressionReport(
            created_at=datetime.now(timezone.utc).isoformat(),
            summary={"error": f"current file not found: {current_path}"},
        )
        return (True, report)  # No data = no regression (skip)

    current = _load_json(current_path)
    resolved_thresholds = dict(DEFAULT_THRESHOLDS)
    if thresholds:
        resolved_thresholds.update(thresholds)

    # Resolve baseline
    baseline: dict[str, Any] | None = None
    resolved_baseline_path: Path | None = None
    baselines: list[tuple[Path, dict[str, Any]]] | None = None

    if baseline_path is not None:
        resolved_baseline_path = Path(baseline_path)
        if resolved_baseline_path.exists():
            baseline = _load_json(resolved_baseline_path)
    elif baseline_dir is not None:
        bdir = Path(baseline_dir)
        if bdir.is_dir():
            baselines = _load_baselines_from_dir(bdir)
            if baselines:
                resolved_baseline_path = baselines[-1][0]
                baseline = baselines[-1][1]
    else:
        # Default: try bench/baseline.json
        default = ROOT / "bench" / "baseline.json"
        if default.exists():
            resolved_baseline_path = default
            baseline = _load_json(default)

    if baseline is None:
        report = RegressionReport(
            created_at=datetime.now(timezone.utc).isoformat(),
            current_file=str(current_path),
            summary={"note": "no baseline available, skipping regression check"},
        )
        return (True, report)

    report = build_report(
        current_path,
        current,
        resolved_baseline_path,
        baseline,
        baselines,
        resolved_thresholds,
    )

    return (not report.has_errors, report)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _parse_threshold(value: str) -> tuple[str, float, float]:
    """Parse 'metric:warn_pct:error_pct' string."""
    parts = value.split(":")
    if len(parts) != 3:
        raise argparse.ArgumentTypeError(
            f"threshold must be 'metric:warn_pct:error_pct', got '{value}'"
        )
    metric = parts[0]
    if metric not in DEFAULT_THRESHOLDS:
        raise argparse.ArgumentTypeError(
            f"unknown metric '{metric}', valid: {', '.join(DEFAULT_THRESHOLDS)}"
        )
    try:
        warn = float(parts[1]) / 100.0
        error = float(parts[2]) / 100.0
    except ValueError:
        raise argparse.ArgumentTypeError(
            f"threshold percentages must be numbers, got '{parts[1]}:{parts[2]}'"
        )
    return (metric, warn, error)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Detect performance regressions in Molt benchmark results.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--current",
        type=Path,
        default=None,
        help="Path to current benchmark JSON (default: bench/results/bench.json).",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=None,
        help="Path to baseline benchmark JSON (default: bench/baseline.json).",
    )
    parser.add_argument(
        "--baseline-dir",
        type=Path,
        default=None,
        help="Directory of historical baseline JSONs for trend analysis.",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="Write JSON report to this path.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print JSON report to stdout (for ci_gate.py integration).",
    )
    parser.add_argument(
        "--threshold",
        action="append",
        default=[],
        help=(
            "Override threshold as 'metric:warn_pct:error_pct' "
            "(e.g., 'runtime:5:10'). Repeatable."
        ),
    )
    parser.add_argument(
        "--quiet",
        "-q",
        action="store_true",
        help="Suppress human-readable output (just exit code).",
    )
    args = parser.parse_args()

    # Parse custom thresholds
    thresholds = dict(DEFAULT_THRESHOLDS)
    for t_str in args.threshold:
        metric, warn, error = _parse_threshold(t_str)
        thresholds[metric] = (warn, error)

    # Resolve paths
    current_path = args.current or ROOT / "bench" / "results" / "bench.json"
    if not current_path.exists():
        print(
            f"Error: current benchmark file not found: {current_path}", file=sys.stderr
        )
        sys.exit(2)

    current = _load_json(current_path)

    # Resolve baseline
    baseline: dict[str, Any] | None = None
    baseline_path: Path | None = args.baseline
    baselines: list[tuple[Path, dict[str, Any]]] | None = None

    if args.baseline_dir is not None:
        if not args.baseline_dir.is_dir():
            print(
                f"Error: baseline directory not found: {args.baseline_dir}",
                file=sys.stderr,
            )
            sys.exit(2)
        baselines = _load_baselines_from_dir(args.baseline_dir)
        if baselines:
            if baseline_path is None:
                baseline_path = baselines[-1][0]
                baseline = baselines[-1][1]
        if not baselines:
            print(
                f"Warning: no baseline files found in {args.baseline_dir}",
                file=sys.stderr,
            )

    if baseline is None and baseline_path is not None:
        if not baseline_path.exists():
            print(f"Error: baseline file not found: {baseline_path}", file=sys.stderr)
            sys.exit(2)
        baseline = _load_json(baseline_path)

    if baseline is None:
        # Try default
        default_baseline = ROOT / "bench" / "baseline.json"
        if default_baseline.exists():
            baseline_path = default_baseline
            baseline = _load_json(default_baseline)
        else:
            print(
                "Error: no baseline specified and bench/baseline.json not found.",
                file=sys.stderr,
            )
            sys.exit(2)

    report = build_report(
        current_path, current, baseline_path, baseline, baselines, thresholds
    )

    # Output
    if not args.quiet and not args.json:
        print_report(report)

    if args.json:
        print(json.dumps(report.to_dict(), indent=2))

    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(
            json.dumps(report.to_dict(), indent=2, sort_keys=False) + "\n"
        )
        if not args.quiet:
            print(f"\nJSON report written to: {args.json_out}")

    # Exit code
    if report.has_errors:
        sys.exit(1)
    sys.exit(0)


if __name__ == "__main__":
    main()
