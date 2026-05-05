from __future__ import annotations

import pytest

import tools.perf_regression as perf_regression


def _payload(
    *,
    value: float,
    samples_s: list[object] | None = None,
    summary_only: bool = False,
) -> dict:
    entry: dict = {
        "molt_ok": True,
        "molt_time_s": value,
        "molt_build_s": 1.0,
        "molt_size_kb": 100.0,
    }
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
    return {"benchmarks": {"bench.py": entry}}


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
