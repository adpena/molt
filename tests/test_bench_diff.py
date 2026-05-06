from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
BENCH_DIFF_PATH = REPO_ROOT / "tools" / "bench_diff.py"
BENCH_DIFF_SPEC = importlib.util.spec_from_file_location(
    "bench_diff_under_test", BENCH_DIFF_PATH
)
assert BENCH_DIFF_SPEC is not None and BENCH_DIFF_SPEC.loader is not None
bench_diff = importlib.util.module_from_spec(BENCH_DIFF_SPEC)
sys.modules[BENCH_DIFF_SPEC.name] = bench_diff
BENCH_DIFF_SPEC.loader.exec_module(bench_diff)


def _benchmarks(entry: dict) -> dict[str, dict]:
    return {"bench.py": entry}


def test_failed_molt_ok_excludes_native_timing_metrics() -> None:
    old_bench = _benchmarks(
        {
            "molt_ok": False,
            "molt_time_s": 0.01,
            "molt_speedup": 100.0,
            "molt_cpython_ratio": 0.01,
            "molt_build_s": 1.0,
        }
    )
    new_bench = _benchmarks(
        {
            "molt_ok": False,
            "molt_time_s": 0.02,
            "molt_speedup": 50.0,
            "molt_cpython_ratio": 0.02,
            "molt_build_s": 1.2,
        }
    )

    assert "molt_time_s" not in bench_diff._available_metrics(old_bench, new_bench)
    assert "molt_speedup" not in bench_diff._available_metrics(old_bench, new_bench)
    assert "molt_cpython_ratio" not in bench_diff._available_metrics(
        old_bench, new_bench
    )
    assert "molt_build_s" in bench_diff._available_metrics(old_bench, new_bench)
    assert bench_diff._compute_metric_diffs("molt_time_s", old_bench, new_bench) == []


def test_ok_molt_rows_remain_comparable() -> None:
    old_bench = _benchmarks({"molt_ok": True, "molt_time_s": 1.0})
    new_bench = _benchmarks({"molt_ok": True, "molt_time_s": 1.5})

    rows = bench_diff._compute_metric_diffs("molt_time_s", old_bench, new_bench)

    assert len(rows) == 1
    assert rows[0].trend == "regressed"


def test_failed_wasm_ok_excludes_wasm_runtime_metrics() -> None:
    old_bench = _benchmarks(
        {
            "molt_wasm_ok": False,
            "molt_wasm_time_s": 0.0,
            "molt_wasm_size_kb": 100.0,
        }
    )
    new_bench = _benchmarks(
        {
            "molt_wasm_ok": False,
            "molt_wasm_time_s": 0.25,
            "molt_wasm_size_kb": 99.0,
        }
    )

    available = bench_diff._available_metrics(old_bench, new_bench)

    assert "molt_wasm_time_s" not in available
    assert "molt_wasm_size_kb" in available
    assert (
        bench_diff._compute_metric_diffs("molt_wasm_time_s", old_bench, new_bench) == []
    )


def test_ratio_metrics_require_all_runtime_gates() -> None:
    old_entry = {
        "codon_ok": True,
        "molt_ok": True,
        "nuitka_ok": True,
        "pypy_ok": False,
        "pyodide_ok": True,
        "molt_codon_ratio": 0.9,
        "molt_nuitka_ratio": 0.9,
        "molt_pypy_ratio": 0.9,
        "molt_pyodide_ratio": 0.9,
    }
    new_entry = {
        "codon_ok": False,
        "molt_ok": True,
        "nuitka_ok": False,
        "pypy_ok": True,
        "pyodide_ok": False,
        "molt_codon_ratio": 1.1,
        "molt_nuitka_ratio": 1.1,
        "molt_pypy_ratio": 1.1,
        "molt_pyodide_ratio": 1.1,
    }
    old_bench = _benchmarks(old_entry)
    new_bench = _benchmarks(new_entry)

    available = bench_diff._available_metrics(old_bench, new_bench)

    assert "molt_codon_ratio" not in available
    assert "molt_nuitka_ratio" not in available
    assert "molt_pypy_ratio" not in available
    assert "molt_pyodide_ratio" not in available


def test_explicit_metric_reports_no_rows_when_only_failed_gated_rows_exist(
    monkeypatch, tmp_path: Path
) -> None:
    old_json = tmp_path / "old.json"
    new_json = tmp_path / "new.json"
    old_json.write_text(
        json.dumps(
            {
                "benchmarks": {
                    "bench.py": {
                        "molt_wasm_ok": False,
                        "molt_wasm_time_s": 0.0,
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    new_json.write_text(
        json.dumps(
            {
                "benchmarks": {
                    "bench.py": {
                        "molt_wasm_ok": False,
                        "molt_wasm_time_s": 0.1,
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        "sys.argv",
        [
            "tools/bench_diff.py",
            str(old_json),
            str(new_json),
            "--metrics",
            "molt_wasm_time_s",
        ],
    )

    assert bench_diff.main() == 0


def test_regression_gate_ignores_failed_gated_metrics(
    monkeypatch, tmp_path: Path
) -> None:
    old_json = tmp_path / "old.json"
    new_json = tmp_path / "new.json"
    out_json = tmp_path / "diff.json"
    old_json.write_text(
        json.dumps(
            {
                "benchmarks": {
                    "bench.py": {
                        "molt_wasm_ok": False,
                        "molt_wasm_time_s": 0.01,
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    new_json.write_text(
        json.dumps(
            {
                "benchmarks": {
                    "bench.py": {
                        "molt_wasm_ok": False,
                        "molt_wasm_time_s": 1.0,
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        "sys.argv",
        [
            "tools/bench_diff.py",
            str(old_json),
            str(new_json),
            "--metrics",
            "molt_wasm_time_s",
            "--fail-regression-count",
            "0",
            "--json-out",
            str(out_json),
        ],
    )

    assert bench_diff.main() == 0
    payload = json.loads(out_json.read_text(encoding="utf-8"))
    assert payload["metrics"][0]["rows"] == 0
    assert payload["gates"]["regressed_rows"] == 0
    assert payload["gates"]["failed"] is False
