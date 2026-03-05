from __future__ import annotations

from pathlib import Path
import sys

import tools.symphony_perf as symphony_perf


def test_summary_uses_success_samples_for_latency_stats() -> None:
    samples = [
        symphony_perf.Sample(
            mode="python",
            iteration=1,
            returncode=0,
            duration_s=1.25,
            stdout_tail="",
            stderr_tail="",
        ),
        symphony_perf.Sample(
            mode="python",
            iteration=2,
            returncode=1,
            duration_s=9.5,
            stdout_tail="",
            stderr_tail="boom",
        ),
    ]
    summary = symphony_perf._summary(samples)
    lane = summary["python"]
    assert lane["samples"] == 2
    assert lane["successes"] == 1
    assert lane["failures"] == 1
    assert lane["avg_s"] == 1.25
    assert lane["max_s"] == 1.25


def test_main_returns_nonzero_when_any_sample_fails(
    monkeypatch: object,
    tmp_path: Path,
) -> None:
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    monkeypatch.setattr(symphony_perf.shutil, "which", lambda _: "/usr/bin/uv")

    def _fake_run_once(**_: object) -> symphony_perf.Sample:
        return symphony_perf.Sample(
            mode="python",
            iteration=1,
            returncode=1,
            duration_s=2.0,
            stdout_tail="",
            stderr_tail="failed",
        )

    monkeypatch.setattr(symphony_perf, "_run_once", _fake_run_once)
    rc = symphony_perf.main(
        [
            "WORKFLOW.md",
            "--modes",
            "python",
            "--iterations",
            "1",
            "--output-json",
            str(tmp_path / "out.json"),
        ]
    )
    assert rc == 2


def test_summarize_dashboard_state_samples_counts_200_and_304() -> None:
    rows = [
        symphony_perf.DashboardStateSample(
            iteration=1, status=200, latency_ms=8.0, had_etag=True
        ),
        symphony_perf.DashboardStateSample(
            iteration=2, status=304, latency_ms=3.0, had_etag=True
        ),
        symphony_perf.DashboardStateSample(
            iteration=3, status=-1, latency_ms=5.0, had_etag=False
        ),
    ]
    summary = symphony_perf._summarize_dashboard_state_samples(rows)
    assert summary["samples"] == 3
    assert summary["status_200"] == 1
    assert summary["status_304"] == 1
    assert summary["errors"] == 1
    assert summary["etag_seen"] == 2
    assert summary["avg_latency_ms"] is not None


def test_hash_bench_python_and_helper() -> None:
    payload = b"abcdef" * 64
    python_report = symphony_perf._bench_python_hash(payload=payload, iterations=10)
    assert python_report["mode"] == "python_blake2s"
    assert python_report["iterations"] == 10
    helper_cmd = f"{sys.executable} tools/symphony_state_hasher.py"
    helper_report = symphony_perf._bench_helper_hash(
        payload=payload,
        iterations=5,
        helper_cmd=helper_cmd,
    )
    assert helper_report["mode"] == "helper_stdio"
    assert helper_report["iterations"] == 5
    assert helper_report["iterations_completed"] == 5
    assert "error" not in helper_report


def test_hash_bench_helper_reports_partial_completion_on_invalid_output() -> None:
    payload = b"abcdef" * 64
    helper_cmd = f"{sys.executable} -c \"print('oops')\""
    helper_report = symphony_perf._bench_helper_hash(
        payload=payload,
        iterations=5,
        helper_cmd=helper_cmd,
    )
    assert helper_report["mode"] == "helper_stdio"
    assert helper_report["iterations"] == 5
    assert helper_report["iterations_completed"] == 0
    assert helper_report["hashes_per_second"] == 0.0
    assert helper_report.get("error") == "invalid_helper_output"


def test_hash_bench_helper_frame_mode() -> None:
    payload = b"abcdef" * 64
    helper_cmd = f"{sys.executable} tools/symphony_state_hasher.py --stdio-frame"
    helper_report = symphony_perf._bench_helper_hash(
        payload=payload,
        iterations=5,
        helper_cmd=helper_cmd,
    )
    assert helper_report["mode"] == "helper_framed"
    assert helper_report["iterations"] == 5
    assert helper_report["iterations_completed"] == 5
    assert "error" not in helper_report


def test_compare_reports_emits_mode_and_dashboard_deltas() -> None:
    current = {
        "generated_at": "2026-03-05T00:00:00Z",
        "summary": {
            "python": {"avg_s": 2.0, "p95_s": 2.5},
            "molt-run": {"avg_s": 1.2, "p95_s": 1.6},
        },
        "dashboard_state_api": {"avg_latency_ms": 12.0, "p95_latency_ms": 20.0},
    }
    baseline = {
        "generated_at": "2026-03-04T00:00:00Z",
        "summary": {
            "python": {"avg_s": 1.5, "p95_s": 2.2},
            "molt-run": {"avg_s": 1.4, "p95_s": 1.8},
        },
        "dashboard_state_api": {"avg_latency_ms": 10.0, "p95_latency_ms": 18.0},
    }
    report = symphony_perf._compare_reports(current, baseline)
    python = report["mode_comparison"]["python"]
    assert python["avg_delta_s"] == 0.5
    assert python["avg_delta_ratio"] == 0.3333
    assert python["p95_delta_s"] == 0.3
    dashboard = report["dashboard_state_api_comparison"]
    assert dashboard["avg_latency_delta_ms"] == 2.0
    assert dashboard["p95_latency_delta_ms"] == 2.0
