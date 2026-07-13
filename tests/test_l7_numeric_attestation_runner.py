from __future__ import annotations

import copy
import importlib.util
import math
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
RUNNER_PATH = ROOT / "tools" / "bench" / "run_l7_numeric_attestation.py"
SPEC = importlib.util.spec_from_file_location(
    "l7_numeric_attestation_runner", RUNNER_PATH
)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = runner
SPEC.loader.exec_module(runner)

SHA = "a" * 64
NONCE = "b" * 32


def _reported(values: list[float]) -> dict[str, float]:
    summary = runner._summary(values)
    return {"median": summary["median"], "cv": summary["cv"]}


def _case(name: str, family: str, input_data: dict, invariant: str) -> dict:
    samples = [
        {
            "ns_per_op": 10.0,
            "allocations_per_op": 1.0,
            "allocated_bytes_per_op": 8.0,
            "peak_live_bytes": 8.0,
            invariant: 1.0,
        }
        for _ in range(runner.SAMPLE_COUNT)
    ]
    metrics = runner.BASE_METRICS + (invariant,)
    return {
        "name": name,
        "family": family,
        "input": copy.deepcopy(input_data),
        "iterations_per_sample": 2_000_000,
        "observer_iterations_per_sample": 64,
        "calibration_target_ns": 20_000_000,
        "timing_scope": runner.TIMING_SCOPE,
        "sample_count": runner.SAMPLE_COUNT,
        "summary": {
            metric: _reported([float(sample[metric]) for sample in samples])
            for metric in metrics
        },
        "samples": samples,
    }


def _quiescence() -> dict:
    return {
        "certified": True,
        "load1": 0.1,
        "load_per_core": 0.01,
        "competing_builds": 0,
        "detail": "test fixture",
    }


def _snapshot() -> dict:
    snapshot = {
        "git_commit": "commit",
        "git_dirty": False,
        "status_sha256": SHA,
        "worktree_diff_sha256": SHA,
        "untracked_sha256": SHA,
    }
    snapshot["fingerprint"] = runner._sha256_bytes(runner._canonical_bytes(snapshot))
    return snapshot


def _component_fixture(component: str, config: dict, runs: int) -> tuple[dict, list]:
    configuration = {
        "package": config["package"],
        "test": config["test"],
        "profile": "release",
        "features": list(config["features"]),
        "rustc": "rustc fixture",
        "timing_instrumentation": config["timing_instrumentation"],
        "environment": {},
    }
    configuration_fingerprint, artifact_fingerprint = runner._build_fingerprints(
        configuration,
        source_fingerprint=_snapshot()["fingerprint"],
        cargo_lock_sha256=SHA,
        executable_sha256=SHA,
    )
    build = {
        "command": ["cargo", "test"],
        "stderr_sha256": SHA,
        "executable_sha256": SHA,
        "configuration": configuration,
        "configuration_fingerprint": configuration_fingerprint,
        "artifact_fingerprint": artifact_fingerprint,
    }
    source = {
        "git_commit": "commit",
        "git_dirty": False,
        "rustc": "rustc fixture",
        "build_fingerprint": artifact_fingerprint,
        "run_nonce": NONCE,
    }
    attestations = []
    process_runs = []
    for index in range(1, runs + 1):
        attestation = {
            "schema_version": 1,
            "kind": config["kind"],
            "profile": "release",
            "allocator_scope": config["allocator_scope"],
            "sample_count": runner.SAMPLE_COUNT,
            "host": {"os": "windows", "arch": "x86_64", "logical_cpus": 8},
            "source": source,
            "scope": {
                "native": True,
                "wasm32": False,
                "assembly": False,
                "code_size": False,
                "component_rss_only": True,
            },
            "coverage": {},
            "cases": [
                _case(name, family, input_data, config["invariant_metric"])
                for name, family, input_data in config["cases"]
            ],
        }
        attestations.append(attestation)
        process_runs.append(
            {
                "run": index,
                "elapsed_ms": 100.0,
                "peak_rss_bytes": 1_000_000,
                "attestation_sha256": runner._sha256_bytes(
                    runner._canonical_bytes(attestation)
                ),
                "quiescence_before": _quiescence(),
                "quiescence_after": _quiescence(),
                "capsule_path": f"logs/capsule-{component}-{index}.json",
            }
        )
    process = {
        "component": component,
        "measurement": "whole child harness RSS; provenance commands excluded",
        "coverage": {
            "timing_scope": runner.TIMING_SCOPE,
            "timing_instrumentation": config["timing_instrumentation"],
            "claim": "relative comparison only",
        },
        "elapsed_ms": runner._summary([100.0] * runs),
        "peak_rss_bytes": runner._summary([1_000_000.0] * runs),
        "runs": process_runs,
        "runner": {
            "direct_executable": "fixture.exe",
            "argv": ["fixture.exe", "--exact"],
            "runs": runs,
            "build": build,
        },
    }
    return process, attestations


def _bundle() -> dict:
    runs = 7
    process = {}
    attestations = {}
    for component, config in runner.COMPONENTS.items():
        process[component], attestations[component] = _component_fixture(
            component, config, runs
        )
    policy = {
        "max_cv": 0.1,
        "max_time_regression": 0.15,
        "max_allocation_regression": 0.0,
        "max_allocated_bytes_regression": 0.0,
        "max_peak_live_regression": 0.15,
        "max_rss_regression": 0.15,
        "max_measured_rss_bytes": None,
    }
    bundle = {
        "schema_version": runner.BUNDLE_SCHEMA_VERSION,
        "kind": runner.BUNDLE_KIND,
        "generated_at_utc": "2026-07-13T00:00:00+00:00",
        "runner": {
            "path": "tools/bench/run_l7_numeric_attestation.py",
            "runs_per_component": runs,
            "timing_scope": runner.TIMING_SCOPE,
            "case_order": "exact",
            "scope": "native",
            "policy": policy,
        },
        "source": {
            "run_nonce": NONCE,
            "rustc": "rustc fixture",
            "cargo_lock_sha256": SHA,
            "runner_sha256": SHA,
            "schema_sha256": SHA,
            "start": _snapshot(),
            "end": _snapshot(),
        },
        "host": {
            "fingerprint": {
                "os": "Windows",
                "arch": "AMD64",
                "cpu": "fixture",
                "logical_cores": 8,
                "python_version": "3.13",
                "key": "fixture",
            },
            "quiescence_policy": "before and after every run",
        },
        "process": process,
        "attestations": attestations,
        "aggregated_cases": {},
        "validation": {"valid": True, "max_cv": 0.1, "errors": []},
        "comparison": {
            "status": "evidence_only",
            "performance_claim": False,
            "errors": ["baseline required"],
            "rows": [],
            "process_rows": [],
            "violations": [],
        },
    }
    aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert errors == []
    bundle["aggregated_cases"] = aggregated
    return bundle


def test_summary_uses_sample_standard_deviation() -> None:
    summary = runner._summary([1.0, 2.0, 3.0])
    assert summary["mean"] == 2.0
    assert summary["cv"] == 0.5


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("timeout", math.nan),
        ("max_cv", math.inf),
        ("max_time_regression", -0.01),
        ("max_rss_regression", 1.01),
    ],
)
def test_policy_rejects_nonfinite_and_out_of_range(field: str, value: float) -> None:
    policy = {
        "runs": 7,
        "timeout": 1.0,
        "max_cv": 0.1,
        "max_time_regression": 0.15,
        "max_allocation_regression": 0.0,
        "max_allocated_bytes_regression": 0.0,
        "max_peak_live_regression": 0.15,
        "max_rss_regression": 0.15,
        "max_measured_rss_bytes": None,
    }
    policy[field] = value
    with pytest.raises(ValueError):
        runner._validate_policy(**policy)


def test_checked_in_schema_accepts_complete_semantic_fixture() -> None:
    bundle = _bundle()
    schema = runner._load_schema()
    assert runner._schema_errors(bundle, schema, root=schema) == []


def test_checked_in_schema_rejects_unknown_fields() -> None:
    bundle = _bundle()
    bundle["unknown"] = True
    schema = runner._load_schema()
    assert any(
        "unknown property 'unknown'" in error
        for error in runner._schema_errors(bundle, schema, root=schema)
    )


def test_aggregate_recomputes_raw_samples_and_rejects_forged_summary() -> None:
    bundle = _bundle()
    case = bundle["attestations"]["abi_boundary"][0]["cases"][0]
    case["summary"]["ns_per_op"]["median"] = 1.0
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert any("reported 1.0 != recomputed 10.0" in error for error in errors)


def test_aggregate_requires_calibrated_duration_reached() -> None:
    bundle = _bundle()
    case = bundle["attestations"]["abi_boundary"][0]["cases"][0]
    for sample in case["samples"]:
        sample["ns_per_op"] = 5.0
    case["summary"]["ns_per_op"] = _reported([5.0] * runner.SAMPLE_COUNT)
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert any("calibrated duration" in error for error in errors)


def test_aggregate_requires_exact_ordered_case_manifest() -> None:
    bundle = _bundle()
    cases = bundle["attestations"]["abi_boundary"][0]["cases"]
    cases[0], cases[1] = cases[1], cases[0]
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert any("ordered case manifest drift" in error for error in errors)


def test_aggregate_rejects_consistent_input_drift_without_baseline() -> None:
    bundle = _bundle()
    for attestation in bundle["attestations"]["abi_boundary"]:
        attestation["cases"][0]["input"]["digits"] = 24
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert any("ordered case manifest drift" in error for error in errors)


def test_aggregate_requires_nonce_bound_parent_provenance() -> None:
    bundle = _bundle()
    bundle["attestations"]["runtime_bigint"][0]["source"]["run_nonce"] = "c" * 32
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert any("parent provenance echo mismatch" in error for error in errors)


def test_aggregate_requires_before_and_after_quiescence() -> None:
    bundle = _bundle()
    bundle["process"]["abi_boundary"]["runs"][0]["quiescence_after"]["certified"] = (
        False
    )
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1)
    assert any("post-run quiescence not certified" in error for error in errors)


def test_baseline_requires_identical_build_configuration() -> None:
    current = _bundle()
    baseline = copy.deepcopy(current)
    baseline["process"]["runtime_bigint"]["runner"]["build"][
        "configuration_fingerprint"
    ] = "d" * 64
    comparison = runner._compare_to_baseline(
        current,
        baseline,
        schema=runner._load_schema(),
        max_cv=0.1,
        max_time_regression=0.15,
        max_allocation_regression=0.0,
        max_allocated_bytes_regression=0.0,
        max_peak_live_regression=0.15,
        max_rss_regression=0.15,
    )
    assert comparison["status"] == "invalid"
    assert any(
        "build configuration fingerprint differs" in error
        for error in comparison["errors"]
    )
