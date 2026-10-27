from __future__ import annotations

from tests.tools.perf_scoreboard_fixtures import synthetic_workload
import pytest

import tools.bench_dashboard as bench_dashboard


def test_dashboard_suppresses_stale_failed_native_values() -> None:
    entry = {
        "molt_ok": False,
        "cpython_time_s": 1.0,
        "molt_time_s": 0.01,
        "molt_speedup": 100.0,
        "molt_cpython_ratio": 0.01,
    }

    assert bench_dashboard.status_label(entry) == "FAIL"
    assert bench_dashboard.speedup_value(entry) is None
    assert bench_dashboard.ratio_value(entry) is None
    assert bench_dashboard.build_rows({"bench.py": entry}) == [
        {
            "name": "bench.py",
            "metric_evidence": entry,
            "cpython_time": 1.0,
            "molt_time": None,
            "speedup": None,
            "status": "FAIL",
        }
    ]


def test_dashboard_uses_valid_native_evidence() -> None:
    entry = {
        "molt_ok": True,
        "cpython_time_s": 1.0,
        "molt_time_s": 0.25,
        "molt_speedup": 4.0,
        "molt_cpython_ratio": 0.25,
    }

    assert bench_dashboard.status_label(entry) == "PASS"
    assert bench_dashboard.speedup_value(entry) == 4.0
    assert bench_dashboard.ratio_value(entry) == 0.25


def _bound_dashboard_entry(speedup: float, runtime: str = "release-output") -> dict:
    return {
        "workload": synthetic_workload("bench.py"),
        "molt_ok": True,
        "molt_time_s": 1.0 / speedup,
        "cpython_time_s": 1.0,
        "molt_speedup": speedup,
        "requested_profile": "release",
        "build_observation": {
            "kind": "molt-build-observation-v1",
            "compiled_with_verified": False,
            "selected_profiles": {
                "backend": "native",
                "target": "native",
                "guest_profile": "release",
                "compiler_profile": "release",
                "runtime_profile": runtime,
            },
        },
    }


def test_dashboard_refuses_unbound_baseline_and_preserves_raw_display() -> None:
    entry = {
        "workload": synthetic_workload("bench.py"),
        "molt_ok": True,
        "molt_time_s": 0.5,
        "cpython_time_s": 1.0,
    }
    rows = bench_dashboard.build_rows({"bench.py": entry})
    assert rows[0]["speedup"] == 2.0 and rows[0]["molt_time"] == 0.5
    with pytest.raises(ValueError, match="selected profiles unbound"):
        bench_dashboard.compute_baseline_diff(rows, {"bench.py": entry})
    summary = bench_dashboard.compute_summary(rows)
    assert "observed=`unbound`" in bench_dashboard.render_markdown(rows, summary, None)
    assert "observed=unbound" in bench_dashboard.render_html(rows, summary, None)


def test_dashboard_compares_only_matching_observed_profile_identity() -> None:
    current = _bound_dashboard_entry(2.0)
    baseline = _bound_dashboard_entry(1.0)
    rows = bench_dashboard.build_rows({"bench.py": current})
    diffs = bench_dashboard.compute_baseline_diff(rows, {"bench.py": baseline})
    assert diffs[0]["tag"] == "IMPROVED" and diffs[0]["pct_change"] == 100.0
    assert rows[0]["metric_evidence"] is current
    with pytest.raises(ValueError, match="selected build profiles differ"):
        bench_dashboard.compute_baseline_diff(
            rows, {"bench.py": _bound_dashboard_entry(1.0, "release-fast")}
        )


@pytest.mark.parametrize("mixed_current", [True, False])
def test_dashboard_keeps_raw_evidence_for_asymmetric_mixed_cell_refusal(mixed_current):
    current, baseline = _bound_dashboard_entry(2.0), _bound_dashboard_entry(1.0)
    mixed = current if mixed_current else baseline
    mixed["molt_wasm_time_s"] = 2.0
    mixed["molt_wasm_ok"] = True
    rows = bench_dashboard.build_rows({"bench.py": current})
    assert rows[0]["metric_evidence"] is current
    with pytest.raises(ValueError, match="mixed native/WASM metric cell"):
        bench_dashboard.compute_baseline_diff(rows, {"bench.py": baseline})


def test_dashboard_validates_nonshared_workloads_when_comparing_metrics():
    rows = bench_dashboard.build_rows({"bench.py": _bound_dashboard_entry(2.0)})
    baseline = {
        "bench.py": _bound_dashboard_entry(1.0),
        "retired.py": {"molt_ok": True, "molt_speedup": 1.0},
    }
    with pytest.raises(ValueError, match="baseline retired.py.*workload"):
        bench_dashboard.compute_baseline_diff(rows, baseline)


def test_dashboard_keeps_added_removed_display_without_comparison_claim():
    current = _bound_dashboard_entry(2.0)
    current["workload"] = synthetic_workload("tests/benchmarks/bench.py")
    rows = bench_dashboard.build_rows({"tests/benchmarks/bench.py": current})
    diffs = bench_dashboard.compute_baseline_diff(
        rows, {"bench.py": {"molt_ok": True, "molt_speedup": 1.0}}
    )
    assert [(d["name"], d["tag"], d["pct_change"]) for d in diffs] == [
        ("tests/benchmarks/bench.py", "NEW", None),
        ("bench.py", "REMOVED", None),
    ]
