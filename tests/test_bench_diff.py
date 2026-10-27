from __future__ import annotations

from tests.tools.perf_scoreboard_fixtures import synthetic_workload
import json
from pathlib import Path

import pytest


from tools import bench_diff


def _benchmarks(entry: dict) -> dict[str, dict]:
    target = "wasm" if "molt_wasm_ok" in entry else "native"
    return {
        "bench.py": {
            "workload": synthetic_workload("bench.py"),
            "requested_profile": "release",
            "build_observation": {
                "kind": "molt-build-observation-v1",
                "compiled_with_verified": False,
                "selected_profiles": {
                    "backend": "native",
                    "target": target,
                    "guest_profile": "release",
                    "compiler_profile": "release",
                    "runtime_profile": "wasm-release"
                    if target == "wasm"
                    else "release-output",
                },
            },
            **entry,
        }
    }


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
                        "workload": synthetic_workload("bench.py"),
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
                        "workload": synthetic_workload("bench.py"),
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

    assert bench_diff.main() == 2
    payload = json.loads(out_json.read_text(encoding="utf-8"))
    assert payload["metrics"][0]["rows"] == 0
    assert payload["gates"]["regressed_rows"] == 0
    assert payload["gates"]["failed"] is True
    assert payload["gates"]["failure_reasons"] == ["no comparable benchmark pairs"]


def test_diff_refuses_legacy_molt_timing_but_keeps_external_metrics() -> None:
    old = {
        "bench.py": {
            "workload": synthetic_workload("bench.py"),
            "molt_ok": True,
            "molt_time_s": 1.0,
            "cpython_time_s": 2.0,
        }
    }
    new = {
        "bench.py": {
            "workload": synthetic_workload("bench.py"),
            "molt_ok": True,
            "molt_time_s": 1.5,
            "cpython_time_s": 3.0,
        }
    }
    with pytest.raises(ValueError, match="selected profiles unbound"):
        bench_diff._compute_metric_diffs("molt_time_s", old, new)
    external = bench_diff._compute_metric_diffs("cpython_time_s", old, new)
    assert len(external) == 1 and external[0].new == 3.0


@pytest.mark.parametrize(
    "coordinate",
    ["backend", "target", "guest_profile", "runtime_profile", "compiler_profile"],
)
def test_diff_refuses_changed_observed_profile_coordinate(coordinate: str) -> None:
    old = _benchmarks({"molt_ok": True, "molt_time_s": 1.0})
    new = _benchmarks({"molt_ok": True, "molt_time_s": 1.5})
    new["bench.py"]["build_observation"]["selected_profiles"][coordinate] = "different"
    with pytest.raises(
        ValueError,
        match="selected build profiles differ|requested guest profile differs",
    ):
        bench_diff._compute_metric_diffs("molt_time_s", old, new)


def test_diff_cli_refuses_unbound_comparison_before_writing_success(
    monkeypatch, tmp_path: Path
) -> None:
    old, new, output = (
        tmp_path / name for name in ("old.json", "new.json", "diff.json")
    )
    for path, value in ((old, 1.0), (new, 1.5)):
        path.write_text(
            json.dumps(
                {"benchmarks": {"bench.py": {"molt_ok": True, "molt_time_s": value}}}
            ),
            encoding="utf-8",
        )
    monkeypatch.setattr(
        "sys.argv",
        [
            "bench_diff.py",
            str(old),
            str(new),
            "--metrics",
            "molt_time_s",
            "--json-out",
            str(output),
        ],
    )
    assert bench_diff.main() == 2
    assert not output.exists()


@pytest.mark.parametrize(
    "metric,observed_target",
    [
        ("molt_wasm_time_s", "native"),
        ("molt_time_s", "wasm"),
    ],
)
def test_diff_refuses_consistently_wrong_lane_even_when_observations_match(
    metric, observed_target
):
    def entry(value):
        cell = _benchmarks({metric: value, "molt_ok": True, "molt_wasm_ok": True})[
            "bench.py"
        ]
        cell["build_observation"]["selected_profiles"]["target"] = observed_target
        return {"bench.py": cell}

    with pytest.raises(ValueError, match="requires .* target"):
        bench_diff._compute_metric_diffs(metric, entry(1.0), entry(2.0))


@pytest.mark.parametrize("metric", ["molt_time_s", "molt_wasm_time_s"])
def test_diff_refuses_mixed_metric_cell_with_one_observation(metric):
    old = _benchmarks(
        {
            "molt_ok": True,
            "molt_wasm_ok": True,
            "molt_time_s": 1.0,
            "molt_wasm_time_s": 2.0,
        }
    )
    new = _benchmarks(
        {
            "molt_ok": True,
            "molt_wasm_ok": True,
            "molt_time_s": 2.0,
            "molt_wasm_time_s": 3.0,
        }
    )
    with pytest.raises(ValueError, match="mixed native/WASM metric cell"):
        bench_diff._compute_metric_diffs(metric, old, new)


def test_every_declared_molt_diff_metric_has_one_shared_policy():
    from tools.bench_evidence import METRIC_POLICIES

    declared = bench_diff._LOWER_IS_BETTER | bench_diff._HIGHER_IS_BETTER
    assert {
        name for name in declared if name.startswith("molt_")
    } <= METRIC_POLICIES.keys()


def test_external_only_diff_refuses_changed_source_without_molt_profiles():
    from copy import deepcopy

    old = {
        "program.py": {
            "workload": synthetic_workload("program.py"),
            "codon_ok": True,
            "codon_time_s": 1.0,
        }
    }
    new = deepcopy(old)
    new["program.py"]["workload"]["sources"][0]["sha256"] = "a" * 64
    with pytest.raises(ValueError, match="workload identities differ"):
        bench_diff._compute_metric_diffs("codon_time_s", old, new)
    rows = bench_diff._compute_metric_diffs("codon_time_s", old, old)
    assert len(rows) == 1 and rows[0].delta == 0


@pytest.mark.parametrize("bound", [False, True])
@pytest.mark.parametrize("gated", [False, True])
def test_added_removed_report_cannot_be_zero_comparison_gate(
    tmp_path, monkeypatch, capsys, bound, gated
):
    old_entry = {"cpython_time_s": 1.0}
    if bound:
        old_entry["workload"] = synthetic_workload("bench_sum.py")
    old = {"benchmarks": {"bench_sum.py": old_entry}}
    name = "tests/benchmarks/bench_sum.py"
    new = {
        "benchmarks": {
            name: {"workload": synthetic_workload(name), "cpython_time_s": 1.0}
        }
    }
    old_path, new_path = tmp_path / "old.json", tmp_path / "new.json"
    old_path.write_text(json.dumps(old), encoding="utf-8")
    new_path.write_text(json.dumps(new), encoding="utf-8")
    argv = ["tools/bench_diff.py", str(old_path), str(new_path)]
    if gated:
        argv += ["--fail-regression-count", "0"]
    monkeypatch.setattr("sys.argv", argv)
    assert bench_diff.main() == (2 if gated else 0)
    captured = capsys.readouterr()
    if gated:
        assert (
            "no comparable benchmark pairs" if bound else "workload"
        ) in captured.err
    else:
        assert "Added benchmarks:" in captured.out
        assert "Removed benchmarks:" in captured.out


def test_external_comparison_validates_nonshared_entries_before_comparing():
    old = {
        "bench.py": {"workload": synthetic_workload("bench.py"), "cpython_time_s": 1.0},
        "retired.py": {"cpython_time_s": 2.0},
    }
    new = {
        "bench.py": {"workload": synthetic_workload("bench.py"), "cpython_time_s": 1.0}
    }
    with pytest.raises(ValueError, match="baseline retired.py.*workload"):
        bench_diff._compute_metric_diffs("cpython_time_s", old, new)


@pytest.mark.parametrize("cells", [{"program.py": 1}, {7: {"cpython_time_s": 1.0}}])
def test_diff_cannot_discard_malformed_workloads_before_comparison(cells):
    with pytest.raises(
        SystemExit, match="names must be strings and cells must be objects"
    ):
        bench_diff._benchmark_map({"benchmarks": cells})
