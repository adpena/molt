from __future__ import annotations

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
