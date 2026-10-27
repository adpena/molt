from __future__ import annotations

from tests.tools.perf_scoreboard_fixtures import synthetic_workload

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
        "workload": synthetic_workload("bench.py"),
        "requested_profile": "release",
        "build_observation": {
            "kind": "molt-build-observation-v1",
            "compiled_with_verified": False,
            "selected_profiles": {
                "backend": "native",
                "target": "native",
                "guest_profile": "release",
                "runtime_profile": "release-output",
                "compiler_profile": "release",
            },
        },
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
    with pytest.raises(ValueError, match="no comparable benchmark pairs"):
        perf_regression.compare_benchmarks(
            _payload(value=1.2, top_level_samples_s=[1.1, 1.2, 1.3], molt_ok=False),
            _payload(value=1.0, top_level_samples_s=[0.9, 1.0, 1.1]),
            perf_regression.DEFAULT_THRESHOLDS,
        )


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


@pytest.mark.parametrize(
    "coordinate", ["runtime_profile", "compiler_profile", "target"]
)
def test_regression_refuses_different_observed_build_profiles(coordinate: str) -> None:
    current = _payload(value=1.2)
    baseline = _payload(value=1.0)
    current["benchmarks"]["bench.py"]["build_observation"]["selected_profiles"][
        coordinate
    ] = "different"

    with pytest.raises(ValueError, match="selected build profiles differ"):
        perf_regression.compare_benchmarks(
            current, baseline, perf_regression.DEFAULT_THRESHOLDS
        )


def test_regression_refuses_legacy_profile_label_without_observation() -> None:
    current = _payload(value=1.2)
    baseline = _payload(value=1.0)
    baseline["benchmarks"]["bench.py"].pop("build_observation")
    baseline["provenance"] = {"profile": "release-fast"}

    with pytest.raises(ValueError, match="baseline selected profiles unbound"):
        perf_regression.compare_benchmarks(
            current, baseline, perf_regression.DEFAULT_THRESHOLDS
        )


@pytest.mark.parametrize("historical_only", [False, True])
def test_trends_require_every_point_to_bind_the_same_profile(
    historical_only: bool,
) -> None:
    from pathlib import Path

    older = _payload(value=1.0)
    recent = _payload(value=1.1)
    current = _payload(value=1.2)
    baselines = [(Path("older.json"), older), (Path("recent.json"), recent)]
    if historical_only:
        baselines.append((Path("last.json"), current))
        current = {**current, "benchmarks": {}}

    trends = perf_regression.analyze_trends(baselines, current)
    runtime = next(trend for trend in trends if trend.metric == "runtime")
    assert runtime.num_points == 3
    assert runtime.is_drifting and runtime.direction == "degrading"

    older["benchmarks"]["bench.py"]["build_observation"]["selected_profiles"][
        "runtime_profile"
    ] = "release-fast"
    with pytest.raises(
        ValueError, match="incompatible benchmark trend.*selected build profiles differ"
    ):
        perf_regression.analyze_trends(baselines, current)
    older["benchmarks"]["bench.py"].pop("build_observation")
    with pytest.raises(ValueError, match="selected profiles unbound"):
        perf_regression.analyze_trends(baselines, current)


@pytest.mark.parametrize("bound", [False, True])
def test_regression_gate_refuses_renamed_disjoint_workloads(bound):
    current, baseline = _payload(value=1.2), _payload(value=1.0)
    cell = current["benchmarks"].pop("bench.py")
    cell["workload"] = synthetic_workload("tests/benchmarks/bench.py")
    current["benchmarks"]["tests/benchmarks/bench.py"] = cell
    if not bound:
        baseline["benchmarks"]["bench.py"].pop("workload")
    with pytest.raises(
        ValueError, match="no comparable benchmark pairs" if bound else "workload"
    ):
        perf_regression.compare_benchmarks(
            current, baseline, perf_regression.DEFAULT_THRESHOLDS
        )
