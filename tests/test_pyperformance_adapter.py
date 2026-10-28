from __future__ import annotations

import argparse
import json
import subprocess
import sys
import types
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process
from tools import pyperformance_adapter as adapter


REPO_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_SUITE = (
    REPO_ROOT / "tests/differential/pyperformance/fixtures/pyperformance_smoke"
)


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        [sys.executable, "tools/pyperformance_adapter.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_catalog_reports_expected_fixture_metadata() -> None:
    result = _run_tool("catalog", "--suite-root", str(FIXTURE_SUITE), "--json")
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["benchmark_count"] == 2
    assert payload["groups"] == ["default", "math"]
    assert payload["smoke_available"] == ["nbody", "fannkuch"]


def test_run_subset_executes_fixture_benchmarks() -> None:
    result = _run_tool("run-subset", "--suite-root", str(FIXTURE_SUITE), "--json")
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["benchmarks"] == ["nbody", "fannkuch"]
    assert len(payload["results"]) == 2
    for row in payload["results"]:
        assert float(row["elapsed_s"]) > 0.0
        assert str(row["result_fingerprint"]) != ""


@pytest.fixture
def pyperf_facade(monkeypatch):
    # The adapter replaces Runner; no optional pyperf attributes are used by
    # these executable workload fixtures. Exercise its real exec/exception
    # boundary without requiring the separate benchmark dependency group.
    monkeypatch.setitem(sys.modules, "pyperf", types.ModuleType("pyperf"))


def _workload(tmp_path: Path, body: str) -> adapter.BenchmarkSpec:
    script = tmp_path / "run_benchmark.py"
    script.write_text("import pyperf\n" + body, encoding="utf-8")
    return adapter.BenchmarkSpec("application", script, (), ())


@pytest.mark.parametrize("method", ["bench_func", "bench_time_func"])
def test_workload_runtime_error_is_a_failure(tmp_path, pyperf_facade, method):
    spec = _workload(
        tmp_path,
        "def application(*args):\n"
        "    raise RuntimeError('timeit is not supported in the molt in-process lane')\n"
        f"pyperf.Runner().{method}('application', application)\n",
    )
    row = adapter._run_full_benchmark_once(spec, rounds=1)
    assert row["status"] == "error"
    assert row["reason"].startswith("RuntimeError:")
    assert row["elapsed_s"] is None
    assert row["result_fingerprint"] == ""


@pytest.mark.parametrize("method", ["bench_async_func", "bench_command", "timeit"])
def test_runner_capability_refusal_remains_an_explicit_skip(
    tmp_path, pyperf_facade, method
):
    spec = _workload(tmp_path, f"pyperf.Runner().{method}('application')\n")
    row = adapter._run_full_benchmark_once(spec, rounds=1)
    assert row["status"] == "skipped"
    assert "in-process" in row["reason"]
    assert row["elapsed_s"] is None


@pytest.mark.parametrize("command", ["all", "group"])
@pytest.mark.parametrize("errors", [0, 1])
def test_cli_failure_status_reaches_process_exit(
    tmp_path, monkeypatch, capsys, pyperf_facade, command, errors
):
    statement = (
        "raise RuntimeError('application failed')\n"
        if errors
        else "pyperf.Runner().bench_func('application', lambda: 42)\n"
    )
    payload = adapter._run_specs((_workload(tmp_path, statement),), rounds=1)
    assert payload["summary"]["error"] == errors
    monkeypatch.setattr(adapter, "run_" + command, lambda *a, **kw: payload)
    args = argparse.Namespace(
        suite_root=str(tmp_path), rounds=1, json=True, group="apps"
    )
    status = getattr(adapter, "_cmd_run_" + command)(args)
    assert status == int(errors != 0)
    assert json.loads(capsys.readouterr().out) == payload


@pytest.mark.parametrize(
    "statement",
    [
        "raise ImportError('installed package is broken')",
        "raise ModuleNotFoundError('no module identity')",
        "raise SystemExit(0)",
        "raise SystemExit(2)",
        "import pickle; raise pickle.UnpicklingError('corrupt application data')",
        "class PicklingError(Exception): pass\nraise PicklingError('same spelling')",
    ],
)
def test_incidental_exception_does_not_infer_unsupported_capability(
    tmp_path, pyperf_facade, statement
):
    row = adapter._run_full_benchmark_once(_workload(tmp_path, statement), rounds=1)
    assert row["status"] == "error"
    assert row["elapsed_s"] is None


def test_named_missing_dependency_is_not_inferred_unsupported(tmp_path, pyperf_facade):
    spec = _workload(tmp_path, "import _molt_benchmark_missing_dependency_fixture\n")
    row = adapter._run_full_benchmark_once(spec, rounds=1)
    assert row["status"] == "error"
    assert row["reason"].startswith("ModuleNotFoundError:")
    assert "_molt_benchmark_missing_dependency_fixture" in row["reason"]


@pytest.mark.parametrize(
    "statement",
    [
        "import application_pkg\n",
        "import importlib; importlib.import_module('application_pkg')\n",
    ],
)
def test_installed_package_with_broken_internal_import_fails(
    tmp_path, pyperf_facade, statement
):
    package = tmp_path / "application_pkg"
    package.mkdir()
    (package / "__init__.py").write_text(
        "from .missing_internal import implementation\n", encoding="utf-8"
    )
    row = adapter._run_full_benchmark_once(_workload(tmp_path, statement), rounds=1)
    assert row["status"] == "error"
    assert "application_pkg.missing_internal" in row["reason"]


def test_unknown_result_status_cannot_produce_a_success_summary():
    assert adapter._summarize([{"status": "unrecognized"}]) == {
        "ok": 0,
        "skipped": 0,
        "error": 1,
    }
