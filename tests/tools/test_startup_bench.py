from __future__ import annotations

import os
import platform
import sys
from pathlib import Path

import pytest

from molt.python_interpreter import PythonInterpreterError
from tools import startup_bench


def test_stats_use_median_and_preserve_samples() -> None:
    assert startup_bench._stats([9.0, 1.0, 5.0]) == {
        "count": 3,
        "median_ms": 5.0,
        "min_ms": 1.0,
        "max_ms": 9.0,
        "samples_ms": [9.0, 1.0, 5.0],
    }


def test_runtime_phase_parser_reports_median_deltas() -> None:
    records = [
        {"stderr": "[molt runtime_init] +10us (d4us) state_allocated\n"},
        {"stderr": "[molt runtime_init] +12us (d6us) state_allocated\n"},
        {"stderr": "[molt runtime_init] +11us (d5us) state_allocated\n"},
    ]
    assert startup_bench._runtime_phases(records) == {
        "phase_median_ms": {"state_allocated": 0.005},
        "total_median_ms": 0.005,
    }


def test_node_phase_parser_reads_marker() -> None:
    payload = startup_bench._parse_node_phases(
        'noise\nmolt_startup_phases={"preload_to_exit_ms":1.5,"reads":[],"instantiations":[]}\n'
    )
    assert payload is not None
    assert payload["preload_to_exit_ms"] == 1.5


def test_baseline_attestation_does_not_claim_variant_ii_improvement() -> None:
    report = {
        "probes": [
            {
                "cpython": {"stats": {"median_ms": 10.0}},
                "native": {"run": {"stats": {"median_ms": 5.0}}},
                "wasm": {"linked": {"stats": {"median_ms": 100.0}}},
            },
            {
                "cpython": {"stats": {"median_ms": 20.0}},
                "native": {"run": {"stats": {"median_ms": 15.0}}},
                "wasm": {"linked": {"stats": {"median_ms": 120.0}}},
            },
        ]
    }
    attestation = startup_bench._attestation(report, 5)
    assert attestation["accepted"] is True
    assert "before/after" in attestation["variant_ii"]


def test_cpython_env_removes_project_startup_hooks() -> None:
    env = startup_bench._cpython_env(
        {
            "PYTHONPATH": "repo/src",
            "PYTHONHOME": "bad",
            "UV_PROJECT_ENVIRONMENT": "env",
            "KEEP": "1",
        }
    )
    assert env["KEEP"] == "1"
    assert env["PYTHONNOUSERSITE"] == "1"
    assert "PYTHONPATH" not in env
    assert "PYTHONHOME" not in env
    assert "UV_PROJECT_ENVIRONMENT" not in env


def test_baseline_identity_uses_captured_environment_not_ambient_override(
    monkeypatch, tmp_path
) -> None:
    env = dict(os.environ)
    env.pop("MOLT_STARTUP_PYTHON", None)
    monkeypatch.setenv("MOLT_STARTUP_PYTHON", str(tmp_path / "absent-python"))
    baseline = startup_bench._baseline_python(env)
    assert baseline.command == (sys.executable, "-I")
    assert Path(baseline.executable).samefile(sys.executable)
    assert baseline.version == platform.python_version()
    assert baseline.implementation == "CPython"


def test_invalid_baseline_override_never_substitutes_running_python(tmp_path) -> None:
    env = {**os.environ, "MOLT_STARTUP_PYTHON": str(tmp_path / "absent-python")}
    with pytest.raises(PythonInterpreterError, match="identity probe failed"):
        startup_bench._baseline_python(env)


def test_invalid_baseline_fails_before_measurement_or_output_creation(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(sys, "argv", ["startup_bench.py"])
    monkeypatch.setenv("MOLT_STARTUP_PYTHON", str(tmp_path / "absent-python"))
    monkeypatch.setattr(startup_bench.output_audit, "_canonical_env", lambda env: env)
    scratch, results = tmp_path / "scratch", tmp_path / "results"
    monkeypatch.setattr(startup_bench, "TMP", scratch)
    monkeypatch.setattr(startup_bench, "RESULTS", results)
    monkeypatch.setattr(
        startup_bench,
        "_measure",
        lambda *args, **kwargs: pytest.fail("measurement before baseline admission"),
    )
    with pytest.raises(PythonInterpreterError, match="identity probe failed"):
        startup_bench.main()
    assert not scratch.exists()
    assert not results.exists()


def test_startup_and_import_samples_share_the_verified_baseline(monkeypatch, tmp_path):
    env = dict(os.environ)
    env["MOLT_STARTUP_PYTHON"] = sys.executable
    baseline = startup_bench._baseline_python(env)
    env["MOLT_STARTUP_PYTHON"] = str(tmp_path / "later-selector")
    commands = []

    def measure(command, **kwargs):
        commands.append(command)
        return {}

    def build(*args, **kwargs):
        raise RuntimeError("compiled build outside reference selection test")

    monkeypatch.setattr(startup_bench, "_measure", measure)
    monkeypatch.setattr(startup_bench, "_build", build)
    script = tmp_path / "probe.py"
    row = startup_bench._measure_probe(
        "hello",
        script,
        env=env,
        baseline=baseline,
        samples=3,
        timeout=10,
        build_timeout=10,
    )
    assert commands == [
        [sys.executable, "-I", str(script)],
        [sys.executable, "-I", "-X", "importtime", str(script)],
    ]
    assert row["build_blocker"]["message"] == (
        "compiled build outside reference selection test"
    )
