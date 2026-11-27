from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

import pytest

from molt.python_interpreter import PythonInterpreter, PythonInterpreterError
from molt.process_environment import ProcessEnvironment
from molt.target_python import resolve_target_python_for_oracle
from tests.process_guard_common import run_guarded_test_process
from tests.tools.perf_scoreboard_fixtures import (
    benchmark_oracle_interpreter,
    benchmark_oracle_metadata,
)
from tests.wasm_execution_manifest import (
    EMPTY_WASM_MODULE,
    write_wasm_execution_manifest,
)
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
        {"stderr": "[molt runtime_init] +    10us (d    4us) state_allocated\n"},
        {"stderr": "[molt runtime_init] +    12us (d    6us) state_allocated\n"},
        {"stderr": "[molt runtime_init] +    11us (d    5us) state_allocated\n"},
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


def test_node_probe_records_compile_before_refusing_an_invalid_linked_abi(
    tmp_path: Path,
) -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for JavaScript startup phase evidence")
    module = tmp_path / "linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    manifest = write_wasm_execution_manifest(tmp_path, linked=module)
    result = run_guarded_test_process(
        [
            node,
            "--require",
            str(startup_bench.NODE_PROBE),
            str(startup_bench.WASM_RUNNER),
            str(manifest),
        ],
        cwd=startup_bench.ROOT,
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )

    assert result.returncode != 0
    assert "application missing exact attached execution ABI section" in result.stderr
    phases = startup_bench._phase_stats([{"stderr": result.stderr}])
    assert phases["node_to_preload_ms"]["count"] == 1
    assert phases["runner:manifest_and_module"]["median_ms"] >= 0
    assert phases["runner:linked_engine_compile"]["median_ms"] >= 0
    assert phases["runner:host_dispose"]["median_ms"] >= 0
    assert "runner:linked_instantiate_and_abi" not in phases
    assert "runner:linked_host_bootstrap" not in phases
    assert "runner:linked_guest_entry" not in phases


def test_baseline_attestation_does_not_claim_variant_ii_improvement() -> None:
    def sample(ms: float) -> dict[str, object]:
        return {"ok": True, "stats": {"count": 5, "median_ms": ms}}

    report = {
        "baseline_python": benchmark_oracle_metadata(),
        "probes": [
            {
                "cpython": sample(10),
                "native": {"run": sample(5)},
                "wasm": {"linked": sample(100)},
            },
            {
                "cpython": sample(20),
                "native": {"run": sample(15)},
                "wasm": {"linked": sample(120)},
            },
        ],
    }
    attestation = startup_bench._attestation(report, 5)
    assert attestation["accepted"] is True
    assert "before/after" in attestation["variant_ii"]
    report["probes"][1]["cpython"]["stats"]["count"] = 4
    assert startup_bench._attestation(report, 5)["accepted"] is False


def test_baseline_identity_uses_captured_environment_not_ambient_override(
    monkeypatch, tmp_path
) -> None:
    env = dict(os.environ)
    env.pop("MOLT_STARTUP_PYTHON", None)
    monkeypatch.setenv("MOLT_STARTUP_PYTHON", str(tmp_path / "absent-python"))
    selected = benchmark_oracle_interpreter()
    calls = []

    def resolve(**kwargs: object) -> PythonInterpreter:
        calls.append(kwargs)
        return selected

    monkeypatch.setattr(startup_bench, "resolve_benchmark_python", resolve)
    captured = ProcessEnvironment(env)
    assert startup_bench._baseline_python(captured) is selected
    assert calls[0]["override"] is None
    assert calls[0]["env"] is captured
    explicit = captured.updated({"MOLT_STARTUP_PYTHON": "managed/python3"})
    assert startup_bench._baseline_python(explicit) is selected
    assert calls[1]["override"] == str(startup_bench.ROOT / "managed/python3")


def test_invalid_baseline_override_never_substitutes_running_python(tmp_path) -> None:
    env = {**os.environ, "MOLT_STARTUP_PYTHON": str(tmp_path / "absent-python")}
    with pytest.raises(
        PythonInterpreterError, match="benchmark CPython executable is unavailable"
    ):
        startup_bench._baseline_python(ProcessEnvironment(env))


def test_invalid_baseline_fails_before_measurement_or_output_creation(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(sys, "argv", ["startup_bench.py"])
    monkeypatch.setenv("MOLT_STARTUP_PYTHON", str(tmp_path / "absent-python"))
    monkeypatch.setattr(
        startup_bench.output_audit, "_canonical_env", ProcessEnvironment
    )
    scratch, results = tmp_path / "scratch", tmp_path / "results"
    monkeypatch.setattr(startup_bench, "TMP", scratch)
    monkeypatch.setattr(startup_bench, "RESULTS", results)
    monkeypatch.setattr(
        startup_bench,
        "_measure",
        lambda *args, **kwargs: pytest.fail("measurement before baseline admission"),
    )
    with pytest.raises(
        PythonInterpreterError, match="benchmark CPython executable is unavailable"
    ):
        startup_bench.main()
    assert not scratch.exists()
    assert not results.exists()


def test_startup_and_import_samples_share_the_verified_baseline(monkeypatch, tmp_path):
    baseline = benchmark_oracle_interpreter()
    env = ProcessEnvironment(
        {
            "MOLT_STARTUP_PYTHON": str(tmp_path / "later-selector"),
            "PYTHONPATH": "repo/src",
            "PYTHONHOME": "bad",
            "PYTHONHASHSEED": "0",
            "KEEP": "1",
        }
    )
    commands = []
    build_targets = []
    script = tmp_path / "probe.py"
    script.write_text("print('probe')\n", encoding="utf-8")

    def measure(command: list[str], **kwargs: object) -> dict[str, list[object]]:
        commands.append((command, kwargs["env"]))
        return {"records": []}

    def build(**kwargs: object) -> startup_bench.output_audit.BuildResult:
        case = kwargs["case"]
        assert isinstance(case, startup_bench.output_audit.MatrixCase)
        build_targets.append((case.target, kwargs["extra_molt_args"]))
        return startup_bench.output_audit.BuildResult(
            case, [], script, {}, 0, 0.1, "", "", None
        )

    monkeypatch.setattr(startup_bench, "_measure", measure)
    monkeypatch.setattr(startup_bench.output_audit, "_build_molt_artifact", build)
    monkeypatch.setattr(startup_bench, "_node_command", lambda _: ["fixture-node"])
    target = resolve_target_python_for_oracle((3, 12))
    startup_bench._measure_probe(
        "hello",
        script,
        env=env,
        baseline=baseline,
        target_python=target,
        samples=3,
        timeout=10,
        build_timeout=10,
    )
    assert [args for args, _ in commands[:2]] == [
        [*baseline.command, str(script)],
        [*baseline.command, "-X", "importtime", str(script)],
    ]
    assert all(
        process_env["KEEP"] == "1"
        and not {"PYTHONPATH", "PYTHONHOME", "PYTHONHASHSEED"}.intersection(process_env)
        for _, process_env in commands[:2]
    )
    assert build_targets == [
        ("native", ["--python-version", target.short]),
        ("wasm", ["--python-version", target.short]),
    ]
