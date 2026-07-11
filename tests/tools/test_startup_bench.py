from __future__ import annotations

from tools import startup_bench


def test_stats_use_median_and_preserve_samples() -> None:
    assert startup_bench._stats([9.0, 1.0, 5.0]) == {
        "count": 3, "median_ms": 5.0, "min_ms": 1.0,
        "max_ms": 9.0, "samples_ms": [9.0, 1.0, 5.0],
    }


def test_runtime_phase_parser_reports_median_deltas() -> None:
    records = [
        {"stderr": "[molt runtime_init] +10us (d4us) state_allocated\n"},
        {"stderr": "[molt runtime_init] +12us (d6us) state_allocated\n"},
        {"stderr": "[molt runtime_init] +11us (d5us) state_allocated\n"},
    ]
    assert startup_bench._runtime_phases(records) == {
        "phase_median_ms": {"state_allocated": 0.005}, "total_median_ms": 0.005,
    }


def test_node_phase_parser_reads_marker() -> None:
    payload = startup_bench._parse_node_phases(
        'noise\nMOLT_STARTUP_PHASES={"preload_to_exit_ms":1.5,"reads":[],"instantiations":[]}\n'
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
        {"PYTHONPATH": "repo/src", "PYTHONHOME": "bad", "UV_PROJECT_ENVIRONMENT": "env", "KEEP": "1"}
    )
    assert env["KEEP"] == "1"
    assert env["PYTHONNOUSERSITE"] == "1"
    assert "PYTHONPATH" not in env
    assert "PYTHONHOME" not in env
    assert "UV_PROJECT_ENVIRONMENT" not in env
