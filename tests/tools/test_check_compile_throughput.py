from __future__ import annotations

import json
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


def _run_check(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", "tools/check_compile_throughput.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def _write_json(path: Path, payload: dict) -> None:
    path.write_text(json.dumps(payload), encoding="utf-8")


def test_accepts_dict_benchmarks_keyed_by_name(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(
        baseline,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.0},
                "bench_b": {"metrics": {"molt_build_s": 2.0}},
            }
        },
    )
    _write_json(
        current,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.1},
                "bench_b": {"metrics": {"molt_build_s": 2.2}},
            }
        },
    )

    res = _run_check(
        "--baseline",
        str(baseline),
        "--current",
        str(current),
        "--max-regression-pct",
        "20",
    )

    assert res.returncode == 0, res.stderr
    assert "bench_a" in res.stdout
    assert "bench_b" in res.stdout


def test_accepts_legacy_list_benchmarks_format(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline_list.json"
    current = tmp_path / "current_list.json"
    _write_json(
        baseline,
        {"benchmarks": [{"name": "bench_a", "molt_build_s": 1.0}]},
    )
    _write_json(
        current,
        {"benchmarks": [{"benchmark": "bench_a", "molt_build_s": 1.05}]},
    )

    res = _run_check(
        "--baseline",
        str(baseline),
        "--current",
        str(current),
        "--max-regression-pct",
        "20",
    )

    assert res.returncode == 0, res.stderr
    assert "bench_a" in res.stdout


def test_missing_metric_is_hard_failure_by_default(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline_missing_metric.json"
    current = tmp_path / "current_missing_metric.json"
    _write_json(
        baseline,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.0},
                "bench_b": {"molt_build_s": 2.0},
            }
        },
    )
    _write_json(
        current,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.1},
                "bench_b": {"metrics": {}},
            }
        },
    )

    res = _run_check(
        "--baseline",
        str(baseline),
        "--current",
        str(current),
        "--max-regression-pct",
        "20",
    )

    assert res.returncode == 1
    assert "current missing metric 'molt_build_s' for: bench_b" in res.stderr
    assert "Missing metrics are a hard failure by default" in res.stderr


def test_missing_benchmark_key_is_hard_failure_by_default(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline_missing_key.json"
    current = tmp_path / "current_missing_key.json"
    _write_json(
        baseline,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.0},
                "bench_b": {"molt_build_s": 2.0},
            }
        },
    )
    _write_json(
        current,
        {"benchmarks": {"bench_a": {"molt_build_s": 1.1}}},
    )

    res = _run_check(
        "--baseline",
        str(baseline),
        "--current",
        str(current),
        "--max-regression-pct",
        "20",
    )

    assert res.returncode == 1
    assert (
        "current missing benchmark keys present in baseline metrics: bench_b"
        in res.stderr
    )


def test_allow_missing_metrics_opt_out(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline_allow_missing.json"
    current = tmp_path / "current_allow_missing.json"
    _write_json(
        baseline,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.0},
                "bench_b": {"molt_build_s": 2.0},
            }
        },
    )
    _write_json(
        current,
        {
            "benchmarks": {
                "bench_a": {"molt_build_s": 1.05},
                "bench_b": {"metrics": {}},
            }
        },
    )

    res = _run_check(
        "--baseline",
        str(baseline),
        "--current",
        str(current),
        "--max-regression-pct",
        "20",
        "--allow-missing-metrics",
    )

    assert res.returncode == 0, res.stderr
    assert "WARNING: current missing metric 'molt_build_s' for: bench_b" in res.stderr
    assert "PASSED: Compile throughput within acceptable bounds." in res.stdout
