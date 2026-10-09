from __future__ import annotations

import copy
import importlib.util
import json
import math
import struct
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace

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


def test_candidate_entrypoint_selects_its_own_benchmark_modules(monkeypatch):
    from tools.import_file import load_module_from_path

    for name in (
        "run_l7_numeric_attestation",
        "run_list_delta_attestation",
        "tools._candidate_import_probe",
    ):
        foreign = ModuleType(name)
        foreign.__file__ = f"/foreign-checkout/{name}.py"
        monkeypatch.setitem(sys.modules, name, foreign)
    candidate = load_module_from_path(
        "tools._candidate_import_probe", ROOT / "tools/run_candidate_runtime_costs.py"
    )
    assert Path(candidate.l7.__file__).resolve() == RUNNER_PATH.resolve()
    assert (
        Path(candidate.lists.__file__).resolve()
        == (ROOT / "tools/bench/run_list_delta_attestation.py").resolve()
    )
    assert candidate.lists.l7 is candidate.l7
    assert sys.modules["run_l7_numeric_attestation"] is candidate.l7
    assert sys.modules["run_list_delta_attestation"] is candidate.lists


def _reported(values: list[float]) -> dict[str, float]:
    summary = runner._summary(values)
    return {
        "median": summary["median"],
        "cv": summary["cv"],
        "robust_cv": summary["robust_cv"],
    }


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
        "calibration_target_ns": 200_000_000,
        "minimum_sample_ns": 20_000_000,
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
            "schema_version": runner.CHILD_SCHEMA_VERSION,
            "kind": config["kind"],
            "profile": "release",
            "allocator_scope": config["allocator_scope"],
            "sample_count": runner.SAMPLE_COUNT,
            "host": {"os": "windows", "arch": "x86_64", "logical_cpus": 8},
            "execution_control": {
                "affinity_mask": "0x1",
                "scope": runner.AFFINITY_SCOPE,
            },
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
        "max_robust_cv": 0.1,
        "max_raw_cv": 0.25,
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
            "execution_control": {
                "affinity_mask": "0x1",
                "allowed_affinity_mask": "0xff",
                "logical_cpu": 0,
                "selection": "explicit",
                "selection_policy": runner.EXPLICIT_AFFINITY_POLICY,
                "scope": runner.AFFINITY_SCOPE,
            },
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
        "validation": {
            "valid": True,
            "max_robust_cv": 0.1,
            "max_raw_cv": 0.25,
            "errors": [],
        },
        "comparison": {
            "status": "evidence_only",
            "performance_claim": False,
            "errors": ["baseline required"],
            "rows": [],
            "process_rows": [],
            "violations": [],
        },
    }
    aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert errors == []
    bundle["aggregated_cases"] = aggregated
    return bundle


def test_summary_uses_sample_standard_deviation() -> None:
    summary = runner._summary([1.0, 2.0, 3.0])
    assert summary["mean"] == 2.0
    assert summary["cv"] == 0.5


def test_summary_preserves_raw_cv_and_reports_robust_cv() -> None:
    summary = runner._summary([1.0] * 8 + [2.0])
    assert summary["cv"] > 0.25
    assert summary["robust_cv"] == 0.0
    assert summary["samples"][-1] == 2.0


def test_dispersion_applies_independent_robust_and_raw_ceilings() -> None:
    errors: list[str] = []
    runner._validate_dispersion(
        {"cv": 0.30, "robust_cv": 0.11},
        context="fixture",
        max_robust_cv=0.10,
        max_raw_cv=0.25,
        errors=errors,
    )
    assert any("robust CV" in error for error in errors)
    assert any("raw CV" in error for error in errors)


@pytest.mark.parametrize("value", ["0x0", "0x3", "bogus"])
def test_affinity_requires_one_logical_cpu(value: str) -> None:
    with pytest.raises(ValueError):
        runner._normalize_affinity_mask(value)


@pytest.mark.parametrize("logical_count", [1, 3, 4, None])
def test_affinity_is_normalized_for_provenance(monkeypatch, logical_count) -> None:
    monkeypatch.setattr(runner, "os", SimpleNamespace(cpu_count=lambda: logical_count))
    assert runner._normalize_affinity_mask("16") == "0x10"


def test_affinity_normalization_matches_unsigned_native_mask_width() -> None:
    pointer_bits = struct.calcsize("P") * 8
    highest_bit = 1 << (pointer_bits - 1)
    assert runner._normalize_affinity_mask(str(highest_bit)) == hex(highest_bit)
    with pytest.raises(ValueError, match="native pointer width"):
        runner._normalize_affinity_mask(str(1 << pointer_bits))


@pytest.mark.parametrize(
    "allowed,expected",
    [({4, 17, 31}, "0x80000000"), ({4, 17}, "0x20000"), ({17}, "0x20000")],
)
def test_auto_affinity_selects_sparse_cpu_ids(monkeypatch, allowed, expected) -> None:
    # Exercise the native provider-to-selection boundary; a small reported
    # count cannot renumber the actual allowed logical IDs.
    monkeypatch.setattr(
        runner, "sys", SimpleNamespace(platform="linux", maxsize=sys.maxsize)
    )
    monkeypatch.setattr(
        runner,
        "os",
        SimpleNamespace(sched_getaffinity=lambda pid: allowed, cpu_count=lambda: 1),
    )
    result = runner._resolve_execution_control("auto")
    assert result["affinity_mask"] == expected
    assert result["allowed_affinity_mask"] == hex(sum(1 << cpu for cpu in allowed))


def test_explicit_affinity_preserves_highest_allowed_native_cpu(monkeypatch) -> None:
    pointer_bits = struct.calcsize("P") * 8
    selected = 1 << (pointer_bits - 1)
    monkeypatch.setattr(runner, "_allowed_affinity_mask", lambda: selected)
    monkeypatch.setattr(runner, "os", SimpleNamespace(cpu_count=lambda: 1))
    assert (
        runner._resolve_execution_control(hex(selected))["logical_cpu"]
        == pointer_bits - 1
    )
    with pytest.raises(ValueError, match="unavailable to this process"):
        runner._resolve_execution_control("0x1")


def test_allowed_affinity_excludes_unrepresentable_cpu_ids(monkeypatch) -> None:
    pointer_bits = struct.calcsize("P") * 8
    monkeypatch.setattr(
        runner, "sys", SimpleNamespace(platform="linux", maxsize=sys.maxsize)
    )
    monkeypatch.setattr(
        runner,
        "os",
        SimpleNamespace(sched_getaffinity=lambda pid: {pointer_bits - 1, pointer_bits}),
    )
    assert runner._allowed_affinity_mask() == 1 << (pointer_bits - 1)
    monkeypatch.setattr(
        runner, "os", SimpleNamespace(sched_getaffinity=lambda pid: {pointer_bits})
    )
    with pytest.raises(ValueError, match="no native-pointer-width logical CPU"):
        runner._allowed_affinity_mask()


def test_auto_affinity_avoids_primary_housekeeping_logicals(monkeypatch) -> None:
    monkeypatch.setattr(runner, "_allowed_affinity_mask", lambda: 0b11_1111)
    assert runner._resolve_execution_control("auto") == {
        "affinity_mask": "0x4",
        "allowed_affinity_mask": "0x3f",
        "logical_cpu": 2,
        "selection": "auto",
        "selection_policy": runner.AUTO_AFFINITY_POLICY,
        "scope": runner.AFFINITY_SCOPE,
    }


def test_auto_affinity_uses_last_cpu_when_fewer_than_three_are_allowed(
    monkeypatch,
) -> None:
    monkeypatch.setattr(runner, "_allowed_affinity_mask", lambda: 0b11)
    assert runner._resolve_execution_control("auto")["affinity_mask"] == "0x2"


def test_explicit_affinity_must_be_available_to_process(monkeypatch) -> None:
    monkeypatch.setattr(runner, "_allowed_affinity_mask", lambda: 0b101)
    with pytest.raises(ValueError, match="unavailable to this process"):
        runner._resolve_execution_control("0x2")


def test_explicit_affinity_records_allowed_topology(monkeypatch) -> None:
    monkeypatch.setattr(runner, "os", SimpleNamespace(cpu_count=lambda: 1))
    monkeypatch.setattr(runner, "_allowed_affinity_mask", lambda: 0b1_0101)
    assert runner._resolve_execution_control("0x10") == {
        "affinity_mask": "0x10",
        "allowed_affinity_mask": "0x15",
        "logical_cpu": 4,
        "selection": "explicit",
        "selection_policy": runner.EXPLICIT_AFFINITY_POLICY,
        "scope": runner.AFFINITY_SCOPE,
    }


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("timeout", math.nan),
        ("max_robust_cv", math.inf),
        ("max_time_regression", -0.01),
        ("max_rss_regression", 1.01),
    ],
)
def test_policy_rejects_nonfinite_and_out_of_range(field: str, value: float) -> None:
    policy = {
        "runs": 7,
        "timeout": 1.0,
        "max_robust_cv": 0.1,
        "max_raw_cv": 0.25,
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


def test_checked_in_schema_rejects_noncanonical_affinity() -> None:
    bundle = _bundle()
    bundle["runner"]["execution_control"]["affinity_mask"] = "1"
    schema = runner._load_schema()
    assert any(
        "required pattern" in error
        for error in runner._schema_errors(bundle, schema, root=schema)
    )


def test_aggregate_recomputes_raw_samples_and_rejects_forged_summary() -> None:
    bundle = _bundle()
    case = bundle["attestations"]["abi_boundary"][0]["cases"][0]
    case["summary"]["ns_per_op"]["median"] = 1.0
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("reported 1.0 != recomputed 10.0" in error for error in errors)


def test_aggregate_requires_calibrated_duration_reached() -> None:
    bundle = _bundle()
    case = bundle["attestations"]["abi_boundary"][0]["cases"][0]
    for sample in case["samples"]:
        sample["ns_per_op"] = 5.0
    case["summary"]["ns_per_op"] = _reported([5.0] * runner.SAMPLE_COUNT)
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("sample duration" in error for error in errors)


def test_aggregate_requires_calibration_headroom() -> None:
    bundle = _bundle()
    case = bundle["attestations"]["abi_boundary"][0]["cases"][0]
    case["calibration_target_ns"] = case["minimum_sample_ns"]
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("lacks 10x minimum headroom" in error for error in errors)


def test_aggregate_requires_child_execution_control() -> None:
    bundle = _bundle()
    bundle["attestations"]["abi_boundary"][0]["execution_control"]["affinity_mask"] = (
        "0x2"
    )
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("execution control drift" in error for error in errors)


def test_aggregate_requires_runner_affinity_within_recorded_allowed_mask() -> None:
    bundle = _bundle()
    bundle["runner"]["execution_control"]["allowed_affinity_mask"] = "0xfe"
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("absent from its recorded allowed mask" in error for error in errors)


def test_aggregate_recomputes_automatic_affinity_policy() -> None:
    bundle = _bundle()
    execution = bundle["runner"]["execution_control"]
    execution.update(
        {
            "affinity_mask": "0x1",
            "logical_cpu": 0,
            "selection": "auto",
            "selection_policy": runner.AUTO_AFFINITY_POLICY,
        }
    )
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("does not follow its recorded policy" in error for error in errors)


def test_aggregate_requires_exact_ordered_case_manifest() -> None:
    bundle = _bundle()
    cases = bundle["attestations"]["abi_boundary"][0]["cases"]
    cases[0], cases[1] = cases[1], cases[0]
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("ordered case manifest drift" in error for error in errors)


def test_aggregate_rejects_consistent_input_drift_without_baseline() -> None:
    bundle = _bundle()
    for attestation in bundle["attestations"]["abi_boundary"]:
        attestation["cases"][0]["input"]["digits"] = 24
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("ordered case manifest drift" in error for error in errors)


def test_aggregate_requires_nonce_bound_parent_provenance() -> None:
    bundle = _bundle()
    bundle["attestations"]["runtime_bigint"][0]["source"]["run_nonce"] = "c" * 32
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("parent provenance echo mismatch" in error for error in errors)


def test_aggregate_requires_before_and_after_quiescence() -> None:
    bundle = _bundle()
    bundle["process"]["abi_boundary"]["runs"][0]["quiescence_after"]["certified"] = (
        False
    )
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
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
        max_robust_cv=0.1,
        max_raw_cv=0.25,
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


def test_baseline_requires_identical_execution_control() -> None:
    current = _bundle()
    baseline = copy.deepcopy(current)
    baseline["runner"]["execution_control"]["affinity_mask"] = "0x2"
    for attestations in baseline["attestations"].values():
        for attestation in attestations:
            attestation["execution_control"]["affinity_mask"] = "0x2"
    comparison = runner._compare_to_baseline(
        current,
        baseline,
        schema=runner._load_schema(),
        max_robust_cv=0.1,
        max_raw_cv=0.25,
        max_time_regression=0.15,
        max_allocation_regression=0.0,
        max_allocated_bytes_regression=0.0,
        max_peak_live_regression=0.15,
        max_rss_regression=0.15,
    )
    assert comparison["status"] == "invalid"
    assert any("execution control differs" in error for error in comparison["errors"])


def test_guard_custody_is_not_a_build_input_but_rust_flags_are():
    first = {
        "MOLT_MEMORY_GUARD_PID": "101",
        "MOLT_MEMORY_GUARD_TOKEN": "a" * 32,
        "MOLT_MEMORY_GUARD_MARKER": "/guard/first.json",
        "MOLT_GUARD_SCRATCH_ROOT": "/scratch/first",
        "RUSTFLAGS": "-C target-cpu=x86-64",
        "MOLT_RUNTIME_PYTHON_VERSION": "3.12",
    }
    second = dict(first)
    second.update(
        {
            "MOLT_MEMORY_GUARD_PID": "202",
            "MOLT_MEMORY_GUARD_TOKEN": "b" * 32,
            "MOLT_MEMORY_GUARD_MARKER": "/guard/second.json",
            "MOLT_GUARD_SCRATCH_ROOT": "/scratch/second",
        }
    )
    expected = {
        "RUSTFLAGS": runner._sha256_bytes(first["RUSTFLAGS"].encode()),
        "MOLT_RUNTIME_PYTHON_VERSION": runner._sha256_bytes(b"3.12"),
    }
    assert runner._captured_environment(first) == expected
    assert runner._captured_environment(second) == expected
    second["RUSTFLAGS"] = "-C target-cpu=native"
    assert runner._captured_environment(second) != expected
    second = dict(first, MOLT_RUNTIME_PYTHON_VERSION="3.14")
    assert runner._captured_environment(second) != expected


def test_scalar_origin_cases_use_noncanonical_values_and_both_lifetimes() -> None:
    scalar_cases = tuple(
        row for row in runner.RUNTIME_CASES if row[1] == "runtime_scalar_bridge"
    )
    assert scalar_cases == (
        (
            "runtime.scalar.int.construct_extract_release",
            "runtime_scalar_bridge",
            {
                "scalar": "int",
                "value": 1000,
                "operation": "construct_extract_release",
                "real_runtime_hooks": True,
            },
        ),
        (
            "runtime.scalar.int.runtime_hold_roundtrip",
            "runtime_scalar_bridge",
            {
                "scalar": "int",
                "value": 1000,
                "operation": "runtime_hold_roundtrip",
                "real_runtime_hooks": True,
            },
        ),
        (
            "runtime.scalar.float.construct_extract_release",
            "runtime_scalar_bridge",
            {
                "scalar": "float",
                "value": 1.25,
                "operation": "construct_extract_release",
                "real_runtime_hooks": True,
            },
        ),
        (
            "runtime.scalar.float.runtime_hold_roundtrip",
            "runtime_scalar_bridge",
            {
                "scalar": "float",
                "value": 1.25,
                "operation": "runtime_hold_roundtrip",
                "real_runtime_hooks": True,
            },
        ),
    )


def test_aggregate_rejects_omitted_scalar_owner_roundtrip() -> None:
    bundle = _bundle()
    for attestation in bundle["attestations"]["runtime_bigint"]:
        attestation["cases"] = [
            case
            for case in attestation["cases"]
            if case["name"] != "runtime.scalar.float.runtime_hold_roundtrip"
        ]
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("ordered case manifest drift" in error for error in errors)


def test_aggregate_rejects_cached_integer_substitution_in_origin_case() -> None:
    bundle = _bundle()
    for attestation in bundle["attestations"]["runtime_bigint"]:
        for case in attestation["cases"]:
            if case["name"] == "runtime.scalar.int.runtime_hold_roundtrip":
                case["input"]["value"] = 42
    _aggregated, errors = runner._aggregate_bundle(bundle, 0.1, 0.25)
    assert any("ordered case manifest drift" in error for error in errors)


@pytest.mark.parametrize(
    "mutation",
    [
        "missing-test",
        "ignored-test",
        "missing-record",
        "duplicate-record",
        "source-drift",
        "affinity-drift",
        "unknown-schema",
        "empty-cases",
    ],
)
@pytest.mark.parametrize("payload_size", [0, 128_000])
def test_candidate_storage_capture_requires_actual_exact_completion(
    mutation, payload_size
):
    from tools import run_candidate_runtime_costs as candidate

    name = "shared_hash_storage_performance_attestation"
    source = {"run_nonce": "independent-source"}
    payload = {
        "schema_version": 1,
        "kind": "shared_hash_storage_performance_attestation",
        "profile": "release",
        "source": source,
        "affinity_mask": "0x1",
        "cases": [{"name": "owned storage workload", "detail": "x" * payload_size}],
    }
    record = "SHARED_HASH_STORAGE_ATTESTATION=" + json.dumps(payload) + "\n"
    prefix = f"running 1 test\ntest {name} ... "
    suffix = "ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
    argv = ["image", name, "--exact", "--ignored", "--nocapture", "--test-threads=1"]
    assert (
        candidate.storage_payload(prefix + record + suffix, argv, source, "0x1")
        == payload
    )
    if mutation == "missing-test":
        prefix = prefix.replace(name, "another_test")
    elif mutation == "ignored-test":
        suffix = (
            suffix.replace("ok\n", "ignored\n", 1)
            .replace("1 passed", "0 passed")
            .replace("0 ignored", "1 ignored")
        )
    elif mutation == "missing-record":
        record = ""
    elif mutation == "duplicate-record":
        record += record
    elif mutation == "source-drift":
        source = {"run_nonce": "different-source"}
    elif mutation == "affinity-drift":
        payload["affinity_mask"] = "0x2"
        record = "SHARED_HASH_STORAGE_ATTESTATION=" + json.dumps(payload) + "\n"
    elif mutation == "unknown-schema":
        payload["schema_version"] = 2
        record = "SHARED_HASH_STORAGE_ATTESTATION=" + json.dumps(payload) + "\n"
    else:
        payload["cases"] = []
        record = "SHARED_HASH_STORAGE_ATTESTATION=" + json.dumps(payload) + "\n"
    with pytest.raises(RuntimeError):
        candidate.storage_payload(prefix + record + suffix, argv, source, "0x1")


@pytest.mark.parametrize(
    "mutation",
    [
        "stream-drift",
        "foreign-run",
        "wrong-argv",
        "wrong-image",
        "duplicate-receipt",
        "failed-execution",
        "timed-out",
        "termination-drift",
        "guard-failure",
        "saved-row-drift",
    ],
)
def test_candidate_storage_reads_complete_bound_capture(tmp_path, mutation):
    import hashlib
    from tools import run_candidate_runtime_costs as candidate
    from tools.libtest_results import ACCOUNTING_SCHEMA, BINARY_RECEIPT_SCHEMA

    image = (tmp_path / "image").resolve()
    image.write_bytes(b"independent image")
    identity = {"schema": "molt.git-source.v1", "tree": "independent"}
    directory = tmp_path / "binaries"
    evidence = directory / "evidence" / "invocation"
    evidence.mkdir(parents=True)
    stream = evidence / "baseline.stdout.log"
    # This deliberately exceeds the wrapper's diagnostic tail. The actual
    # complete capture, not a console mirror, must supply the accepted bytes.
    content = "retained beginning\n" + "x" * 100_000 + "\nretained end\n"
    stream.write_text(content, encoding="utf-8")
    argv = [
        str(image),
        "shared_hash_storage_performance_attestation",
        "--exact",
        "--ignored",
    ]
    receipt = {
        "schema": BINARY_RECEIPT_SCHEMA,
        "invocation_id": "invocation",
        "run_id": "run",
        "source_identity": identity,
        "executable_resolved": str(image),
        "executable_size": image.stat().st_size,
        "executable_sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
        "status": "success",
        "returncode": 0,
        "test_results": [{"identity": argv[1], "status": "pass"}],
        "result_accounting": {
            "schema": ACCOUNTING_SCHEMA,
            "complete": True,
            "issues": [],
            "observed_results": 1,
            "declared_results": 1,
        },
        "executions": [
            {
                "argv": argv,
                "returncode": 0,
                "timed_out": False,
                "termination": {"kind": "exit", "returncode": 0},
                "infrastructure_failure": None,
                "stdout_evidence": str(stream),
                "stdout_bytes": stream.stat().st_size,
                "stdout_sha256": hashlib.sha256(stream.read_bytes()).hexdigest(),
            }
        ],
    }
    path = directory / "receipt.json"
    path.write_text(json.dumps(receipt), encoding="utf-8")
    assert candidate.storage_capture(directory, "run", identity, argv) == content
    if mutation == "stream-drift":
        stream.write_text(content + "changed", encoding="utf-8")
    elif mutation == "foreign-run":
        receipt["run_id"] = "foreign"
    elif mutation == "wrong-argv":
        receipt["executions"][0]["argv"] = [str(image), "another_test"]
    elif mutation == "wrong-image":
        other = tmp_path / "other-image"
        other.write_bytes(image.read_bytes())
        receipt["executable_resolved"] = str(other)
    elif mutation == "failed-execution":
        receipt["executions"][0]["returncode"] = 1
    elif mutation == "timed-out":
        receipt["executions"][0]["timed_out"] = True
    elif mutation == "termination-drift":
        receipt["executions"][0]["termination"] = {"kind": "signal", "returncode": -15}
    elif mutation == "saved-row-drift":
        receipt["test_results"] = [{"identity": "unrelated_test", "status": "pass"}]
    elif mutation == "guard-failure":
        receipt["executions"][0]["infrastructure_failure"] = {
            "phase": "rss_trip_evidence",
            "details": ["independent observer loss"],
        }
    else:
        (directory / "duplicate.json").write_text(json.dumps(receipt), encoding="utf-8")
    path.write_text(json.dumps(receipt), encoding="utf-8")
    with pytest.raises(RuntimeError):
        candidate.storage_capture(directory, "run", identity, argv)


@pytest.mark.parametrize("exit_code", [0, 2])
def test_candidate_list_exit_status_is_retained(tmp_path, monkeypatch, exit_code):
    from tools import run_candidate_runtime_costs as candidate

    monkeypatch.setattr(candidate, "OUTPUT", tmp_path)
    monkeypatch.setattr(candidate.lists, "main", lambda argv: exit_code)
    if exit_code:
        with pytest.raises(RuntimeError, match="candidate list attestation failed: 2"):
            candidate.run_operation("list")
    else:
        candidate.run_operation("list")
    result = json.loads((tmp_path / "list/result.json").read_text(encoding="utf-8"))
    assert result["status"] == ("failed" if exit_code else "evidence_only")
    assert result["performance_claim"] is False
    assert result["release_acceptance"] is False


@pytest.mark.parametrize("rejected", [(), ("l7",), ("list",), ("l7", "list")])
def test_candidate_completed_rejections_run_independent_operations(
    tmp_path, monkeypatch, rejected
):
    from tools import run_candidate_runtime_costs as candidate

    monkeypatch.setattr(candidate, "OUTPUT", tmp_path)
    calls = []

    def numeric(argv, *, raise_measurement_rejection):
        assert raise_measurement_rejection is True
        calls.append("l7")
        invalid = "l7" in rejected
        candidate.l7._write_json_atomic(
            Path(argv[argv.index("--output") + 1]),
            {
                "comparison": {
                    "status": "invalid" if invalid else "evidence_only",
                    "performance_claim": False,
                }
            },
        )
        if invalid:
            raise candidate.l7.EvidenceRejected("fixture measurement rejection")
        return 0

    def sequence(argv):
        calls.append("list")
        if "list" in rejected:
            raise candidate.l7.EvidenceRejected("fixture nonquiescent list admission")
        return 0

    def storage(_directory, _timeout):
        calls.append("storage")

    monkeypatch.setattr(candidate.l7, "main", numeric)
    monkeypatch.setattr(candidate.lists, "main", sequence)
    monkeypatch.setattr(candidate, "run_storage", storage)
    assert candidate.main([]) == (1 if rejected else 0)
    assert calls == ["l7", "list", "storage"]
    for operation in calls:
        result = json.loads(
            (tmp_path / operation / "result.json").read_text(encoding="utf-8")
        )
        assert result["status"] == (
            "failed" if operation in rejected else "evidence_only"
        )
        assert result["performance_claim"] is False
        assert result["release_acceptance"] is False


@pytest.mark.parametrize(
    "error", [RuntimeError("custody unresolved"), KeyboardInterrupt(), SystemExit(130)]
)
def test_candidate_does_not_continue_unknown_failure_or_interruption(
    tmp_path, monkeypatch, error
):
    from tools import run_candidate_runtime_costs as candidate

    monkeypatch.setattr(candidate, "OUTPUT", tmp_path)
    calls = []

    def numeric(_argv, *, raise_measurement_rejection):
        assert raise_measurement_rejection is True
        calls.append("l7")
        raise error

    monkeypatch.setattr(candidate.l7, "main", numeric)
    monkeypatch.setattr(candidate.lists, "main", lambda _argv: calls.append("list"))
    monkeypatch.setattr(candidate, "run_storage", lambda *args: calls.append("storage"))
    with pytest.raises(type(error)) as caught:
        candidate.main([])
    assert caught.value is error
    assert calls == ["l7"]
    assert (
        json.loads((tmp_path / "l7/result.json").read_text(encoding="utf-8"))["status"]
        == "failed"
    )
    assert not (tmp_path / "list").exists()
    assert not (tmp_path / "storage").exists()


def _list_closed_child_payload():
    # Literal workload/zero-gate denominator, independent of the runner's roster.
    names = (
        ("list.delta.append_pop", True),
        ("list.delta.indexed_replace", True),
        ("list.delta.reverse", True),
        ("list.construction.pylist_new_presized", False),
        ("tuple.steady.empty_singleton", True),
        ("tuple.steady.checked_raw_fast_items", True),
        ("tuple.steady.full_slice_repeat_one_identity", True),
        ("tuple.construction.pytuple_new_fill", False),
        ("list.removal.tail_128", False),
        ("list.removal.tail_1048576", False),
        ("list.removal.dense_8192", False),
        ("list.removal.projected_tail_65536", False),
        ("list.removal.projected_dense_8192", False),
    )
    cases = []
    for name, zero in names:
        sample = {
            "ns_per_op": 10.0,
            "allocations_per_op": 0.0 if zero else 1.0,
            "allocated_bytes_per_op": 0.0 if zero else 8.0,
            "peak_live_bytes": 0 if zero else 8,
        }
        cases.append(
            {
                "name": name,
                "family": "fixture",
                "input": {},
                "samples": [dict(sample) for _ in range(9)],
                "summary": {
                    metric: {"median": value, "cv": 0.0, "robust_cv": 0.0}
                    for metric, value in sample.items()
                },
                "gates": {
                    "semantic_witness": "pass",
                    "steady_state_zero_allocations": {"required": zero, "passed": True},
                    "allocator_probe_positive_control": {
                        "required": not zero,
                        "passed": True,
                    },
                },
            }
        )
    return {
        "schema_version": 2,
        "kind": "sequence_container_performance_attestation",
        "profile": "release",
        "source": {
            "git_commit": SHA,
            "git_dirty": False,
            "rustc": "rustc-fixture",
            "build_fingerprint": SHA,
            "run_nonce": NONCE,
        },
        "execution_control": {
            "affinity_mask": "0x4",
            "scope": "current_benchmark_thread",
        },
        "execution_mode": {
            "deterministic_default": True,
            "runtime_gil": "enabled",
            "free_threaded": False,
            "benchmark_threads": 1,
        },
        "sample_count": 9,
        "cases": cases,
    }


def test_list_owner_still_refuses_allocation_corruption_before_timing_classification():
    from tools import run_candidate_runtime_costs as candidate

    payload = _list_closed_child_payload()
    payload["cases"][0]["samples"][0]["allocations_per_op"] = 1.0
    with pytest.raises(RuntimeError, match="violated exact zero gate"):
        candidate.lists._validate_payload(
            payload,
            source={"git_commit": SHA, "git_dirty": False},
            build={"artifact_fingerprint": SHA},
            rustc="rustc-fixture",
            run_nonce=NONCE,
            execution_control={
                "affinity_mask": "0x4",
                "scope": "current_benchmark_thread",
            },
        )


@pytest.mark.parametrize("rejection", ["timing", "allocation"])
def test_list_rejection_type_separates_stability_from_invalid_counts(rejection):
    from tools import run_candidate_runtime_costs as candidate

    payload = _list_closed_child_payload()
    if rejection == "timing":
        values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]
        for sample, value in zip(payload["cases"][0]["samples"], values, strict=True):
            sample["ns_per_op"] = value
        payload["cases"][0]["summary"]["ns_per_op"] = candidate.l7._summary(values)
    else:
        payload["cases"][0]["samples"][0]["allocations_per_op"] = 1.0
    with pytest.raises(RuntimeError) as caught:
        candidate.lists._validate_payload(
            payload,
            source={"git_commit": SHA, "git_dirty": False},
            build={"artifact_fingerprint": SHA},
            rustc="rustc-fixture",
            run_nonce=NONCE,
            execution_control={
                "affinity_mask": "0x4",
                "scope": "current_benchmark_thread",
            },
        )
    assert isinstance(caught.value, candidate.l7.EvidenceRejected) is (
        rejection == "timing"
    )


@pytest.mark.parametrize("component", ["numeric", "list"])
def test_cost_owner_does_not_turn_lost_custody_into_continuable_evidence(
    tmp_path, monkeypatch, component
):
    from tools import run_candidate_runtime_costs as candidate

    l7 = candidate.l7
    owner = l7 if component == "numeric" else candidate.lists
    source = {"git_commit": SHA, "git_dirty": False}
    monkeypatch.setattr(owner, "CAPSULE_ACTIVE_DIR", tmp_path / "active")
    monkeypatch.setattr(owner, "CAPSULE_ARCHIVE_DIR", tmp_path / "custody")
    monkeypatch.setattr(
        owner, "secrets", SimpleNamespace(token_hex=lambda _count: NONCE)
    )
    monkeypatch.setattr(l7, "_source_snapshot", lambda: source)
    monkeypatch.setattr(l7, "_parent_command", lambda _argv: b"rustc-fixture")
    monkeypatch.setattr(l7, "_sha256_file", lambda _path: SHA)
    monkeypatch.setattr(
        l7, "_resolve_execution_control", lambda _mask: {"affinity_mask": "0x4"}
    )
    monkeypatch.setattr(
        l7,
        "_build_test_executable",
        lambda *args, **kwargs: (
            tmp_path / "native-image",
            {"artifact_fingerprint": SHA},
        ),
    )
    monkeypatch.setattr(
        l7.perf_calibration,
        "measure_quiescence",
        lambda: l7.perf_calibration.Quiescence(True, 0.1, 0.025, 0, "literal quiet"),
    )
    calls = []

    def measure(argv, **kwargs):
        calls.append(argv)
        # A successful child code cannot erase unresolved descendant custody.
        return l7.perf_calibration.RunMeasurement(
            returncode=0,
            child_returncode=0,
            elapsed_s=0.01,
            peak_rss_bytes=1024,
            peak_job_commit_bytes=2048,
            stdout="",
            stderr="",
            orphaned_process_groups=(123,),
        )

    monkeypatch.setattr(l7.perf_calibration, "run_and_measure", measure)
    with pytest.raises(RuntimeError, match="rc=0 timeout=False") as caught:
        if component == "numeric":
            l7._run_component(
                "runtime_bigint",
                l7.COMPONENTS["runtime_bigint"],
                runs=7,
                timeout=300.0,
                schema={},
                source=source,
                rustc="rustc-fixture",
                cargo_lock_sha256=SHA,
                run_nonce=NONCE,
                max_measured_rss_bytes=None,
                affinity_mask="0x4",
            )
        else:
            owner.run_attestation(
                runs=7,
                timeout=300.0,
                output=tmp_path / "attestation.json",
                affinity_request="auto",
            )
    assert not isinstance(caught.value, l7.EvidenceRejected)
    assert len(calls) == 1

    capsules = list((tmp_path / "custody").glob("*.json"))
    assert len(capsules) == 1
    assert json.loads(capsules[0].read_text(encoding="utf-8"))["status"] == "failed"


@pytest.mark.parametrize(
    "defect", [None, "source", "missing-component", "digest", "summary", "raw-error"]
)
def test_candidate_l7_policy_rejection_does_not_mask_integrity_failure(
    tmp_path, monkeypatch, defect
):
    from tools import run_candidate_runtime_costs as candidate

    l7 = candidate.l7
    bundle = _bundle()
    component = "abi_boundary"
    bundle["process"][component]["runs"][0]["quiescence_before"]["certified"] = False
    if defect == "source":
        bundle["source"]["end"]["git_commit"] = "different-source"
    elif defect == "missing-component":
        del bundle["attestations"]["runtime_bigint"]
    elif defect == "digest":
        bundle["process"][component]["runs"][0]["attestation_sha256"] = "0" * 64
    elif defect == "summary":
        bundle["attestations"][component][0]["cases"][0]["summary"]["ns_per_op"][
            "median"
        ] = 999.0
    aggregate, errors = l7._aggregate_bundle(bundle, 0.1, 0.25)
    assert errors
    if defect == "raw-error":
        # Serialized strings cannot confer the in-memory owner's classification.
        errors = [str(error) for error in errors]
    bundle["aggregated_cases"] = aggregate
    bundle["validation"]["errors"] = errors
    bundle["validation"]["valid"] = False
    bundle["comparison"] = {"status": "invalid", "performance_claim": False}
    monkeypatch.setattr(l7, "run_attestation", lambda *args, **kwargs: bundle)
    monkeypatch.setattr(
        l7,
        "_resolve_execution_control",
        lambda _mask: bundle["runner"]["execution_control"],
    )
    monkeypatch.setattr(candidate, "OUTPUT", tmp_path)
    later = []
    monkeypatch.setattr(
        candidate.lists, "main", lambda _args: later.append("list") or 0
    )
    monkeypatch.setattr(
        candidate, "run_storage", lambda *_args: later.append("storage")
    )
    if defect is None:
        assert candidate.main([]) == 1
        assert later == ["list", "storage"]
        for operation in later:
            assert (
                json.loads(
                    (tmp_path / operation / "result.json").read_text(encoding="utf-8")
                )["status"]
                == "evidence_only"
            )
    else:
        with pytest.raises(RuntimeError, match="must be valid evidence_only"):
            candidate.main([])
        assert later == []
        assert not (tmp_path / "list").exists()
        assert not (tmp_path / "storage").exists()
    assert (
        json.loads((tmp_path / "l7/result.json").read_text(encoding="utf-8"))["status"]
        == "failed"
    )
    assert (
        json.loads((tmp_path / "l7/attestation.json").read_text(encoding="utf-8"))[
            "comparison"
        ]["status"]
        == "invalid"
    )


@pytest.mark.parametrize("source_changed", [False, True])
def test_candidate_list_pre_refusal_revalidates_source_before_continuation(
    tmp_path, monkeypatch, source_changed
):
    from tools import run_candidate_runtime_costs as candidate

    l7 = candidate.l7
    lists = candidate.lists
    source = {"git_commit": SHA, "git_dirty": False}
    snapshots = iter(
        [source, {**source, "git_commit": "changed"} if source_changed else source]
    )
    monkeypatch.setattr(l7, "_source_snapshot", lambda: next(snapshots))
    monkeypatch.setattr(l7, "_parent_command", lambda _argv: b"rustc-fixture")
    monkeypatch.setattr(l7, "_sha256_file", lambda _path: SHA)
    monkeypatch.setattr(
        l7, "_resolve_execution_control", lambda _mask: {"affinity_mask": "0x4"}
    )
    monkeypatch.setattr(
        l7,
        "_build_test_executable",
        lambda *args, **kwargs: (tmp_path / "image", {"artifact_fingerprint": SHA}),
    )
    monkeypatch.setattr(lists, "CAPSULE_ACTIVE_DIR", tmp_path / "active")
    monkeypatch.setattr(lists, "CAPSULE_ARCHIVE_DIR", tmp_path / "custody")
    monkeypatch.setattr(
        l7.perf_calibration,
        "measure_quiescence",
        lambda: l7.perf_calibration.Quiescence(False, 4.0, 1.0, 0, "literal busy"),
    )
    monkeypatch.setattr(candidate, "OUTPUT", tmp_path / "operations")

    def numeric(argv, **_kwargs):
        l7._write_json_atomic(
            Path(argv[argv.index("--output") + 1]),
            {"comparison": {"status": "evidence_only", "performance_claim": False}},
        )
        return 0

    monkeypatch.setattr(l7, "main", numeric)
    children = []
    monkeypatch.setattr(
        l7.perf_calibration,
        "run_and_measure",
        lambda *args, **kwargs: children.append(args),
    )
    later = []
    monkeypatch.setattr(
        candidate, "run_storage", lambda *_args: later.append("storage")
    )
    if source_changed:
        with pytest.raises(RuntimeError, match="repository source changed"):
            candidate.main([])
        assert later == []
    else:
        assert candidate.main([]) == 1
        assert later == ["storage"]
    assert children == []
    assert (
        json.loads((candidate.OUTPUT / "list/result.json").read_text(encoding="utf-8"))[
            "status"
        ]
        == "failed"
    )


@pytest.mark.parametrize("defect", ["late-allocation", "forged-summary"])
def test_list_timing_rejection_cannot_mask_payload_corruption(defect):
    from tools import run_candidate_runtime_costs as candidate

    payload = _list_closed_child_payload()
    values = [float(value) for value in range(1, 10)]
    for sample, value in zip(payload["cases"][0]["samples"], values, strict=True):
        sample["ns_per_op"] = value
    payload["cases"][0]["summary"]["ns_per_op"] = candidate.l7._summary(values)
    if defect == "late-allocation":
        payload["cases"][1]["samples"][0]["allocations_per_op"] = 1.0
    else:
        payload["cases"][-1]["summary"]["ns_per_op"]["median"] = 999.0
    with pytest.raises(RuntimeError) as caught:
        candidate.lists._validate_payload(
            payload,
            source={"git_commit": SHA, "git_dirty": False},
            build={"artifact_fingerprint": SHA},
            rustc="rustc-fixture",
            run_nonce=NONCE,
            execution_control={
                "affinity_mask": "0x4",
                "scope": "current_benchmark_thread",
            },
        )
    assert not isinstance(caught.value, candidate.l7.EvidenceRejected)
