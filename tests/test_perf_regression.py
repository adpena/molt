from __future__ import annotations

import json

import pytest

import tools.perf_regression as perf_regression


def _payload(
    *,
    value: float,
    samples_s: list[object] | None = None,
    top_level_samples_s: list[object] | None = None,
    molt_ok: bool = True,
    summary_only: bool = False,
) -> dict:
    entry: dict = {
        "molt_ok": molt_ok,
        "molt_time_s": value,
        "molt_build_s": 1.0,
        "molt_size_kb": 100.0,
    }
    if top_level_samples_s is not None:
        entry["molt_samples_s"] = top_level_samples_s
    if samples_s is not None or summary_only:
        stats = {
            "mean_s": value,
            "median_s": value,
            "variance_s": 0.01,
            "range_s": 0.2,
            "min_s": value - 0.1,
            "max_s": value + 0.1,
        }
        if samples_s is not None:
            stats["samples_s"] = samples_s
        entry["super_stats"] = {"molt": stats}
    return {
        "timing_mode": "warm_throughput",
        "warmup": 1,
        "samples": 3,
        "benchmarks": {"bench.py": entry},
    }


def test_summary_only_super_stats_do_not_emit_statistical_fields() -> None:
    comparisons = perf_regression.compare_benchmarks(
        _payload(value=1.2, summary_only=True),
        _payload(value=1.0, summary_only=True),
        perf_regression.DEFAULT_THRESHOLDS,
    )

    runtime = next(c for c in comparisons if c.metric == "runtime")
    assert runtime.severity == "error"
    assert runtime.ci_low is None
    assert runtime.ci_high is None
    assert runtime.p_value is None
    assert runtime.cohens_d is None
    assert runtime.min_detectable_effect is None
    assert runtime.statistically_significant is None


def test_raw_super_stats_samples_enable_statistical_fields() -> None:
    comparisons = perf_regression.compare_benchmarks(
        _payload(value=1.2, samples_s=[1.1, 1.2, 1.3]),
        _payload(value=1.0, samples_s=[0.9, 1.0, 1.1]),
        perf_regression.DEFAULT_THRESHOLDS,
    )

    runtime = next(c for c in comparisons if c.metric == "runtime")
    assert runtime.ci_low is not None
    assert runtime.ci_high is not None
    assert runtime.p_value is not None
    assert runtime.cohens_d is not None
    assert runtime.min_detectable_effect is not None
    assert runtime.statistically_significant is not None


def test_top_level_raw_samples_enable_statistical_fields() -> None:
    comparisons = perf_regression.compare_benchmarks(
        _payload(value=1.2, top_level_samples_s=[1.1, 1.2, 1.3]),
        _payload(value=1.0, top_level_samples_s=[0.9, 1.0, 1.1]),
        perf_regression.DEFAULT_THRESHOLDS,
    )

    runtime = next(c for c in comparisons if c.metric == "runtime")
    assert runtime.ci_low is not None
    assert runtime.p_value is not None


def test_failed_molt_ok_stale_runtime_and_samples_are_ignored() -> None:
    comparisons = perf_regression.compare_benchmarks(
        _payload(value=1.2, top_level_samples_s=[1.1, 1.2, 1.3], molt_ok=False),
        _payload(value=1.0, top_level_samples_s=[0.9, 1.0, 1.1]),
        perf_regression.DEFAULT_THRESHOLDS,
    )

    assert comparisons == []


def test_incompatible_run_metadata_fails_closed() -> None:
    current = _payload(value=1.2)
    baseline = _payload(value=1.0)
    baseline["timing_mode"] = "cold_first_run"
    baseline["warmup"] = 0

    with pytest.raises(ValueError, match="incompatible benchmark baseline"):
        perf_regression.compare_benchmarks(
            current,
            baseline,
            perf_regression.DEFAULT_THRESHOLDS,
        )


def test_check_perf_regression_reports_incompatible_metadata(tmp_path) -> None:
    current_path = tmp_path / "current.json"
    baseline_path = tmp_path / "baseline.json"
    current_path.write_text(json.dumps(_payload(value=1.2)), encoding="utf-8")
    baseline = _payload(value=1.0)
    baseline["samples"] = 1
    baseline_path.write_text(json.dumps(baseline), encoding="utf-8")

    passed, report = perf_regression.check_perf_regression(
        current_path=current_path,
        baseline_path=baseline_path,
    )

    assert not passed
    assert "incompatible benchmark baseline" in report.summary["error"]


@pytest.mark.parametrize(
    "bad_sample",
    [True, "1.0", float("nan"), float("inf"), -1.0, 0.0],
)
def test_malformed_present_raw_samples_fail_closed(bad_sample: object) -> None:
    with pytest.raises(ValueError, match="invalid raw sample"):
        perf_regression.compare_benchmarks(
            _payload(value=1.2, samples_s=[1.1, bad_sample, 1.3]),
            _payload(value=1.0, samples_s=[0.9, 1.0, 1.1]),
            perf_regression.DEFAULT_THRESHOLDS,
        )
