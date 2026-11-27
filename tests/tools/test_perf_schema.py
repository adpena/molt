from __future__ import annotations

from tests.tools.perf_scoreboard_fixtures import (
    benchmark_oracle_metadata,
    synthetic_workload,
)

import json
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"


from tools import perf_schema as schema  # noqa: E402
from tools import perf_scoreboard as scoreboard  # noqa: E402
from tools import perf_authority as pa  # noqa: E402


def _matching_output_parity() -> dict[str, object]:
    return schema.output_parity_evidence(
        reference_observations=[("cpython:cold", "result\n", "", 0)],
        molt_observations=[("molt:cold", "result\n", "", 0)],
    )


def _cell(**overrides: object) -> dict[str, object]:
    cell: dict[str, object] = {
        "workload": synthetic_workload(),
        "benchmark": "tests/benchmarks/bench_fib.py",
        "target": "native",
        "backend": "native",
        "profile": "release-fast",
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "cold_molt_s": 0.12,
        "cold_cpython_s": 0.24,
        "warm_molt_s": 0.10,
        "warm_cpython_s": 0.20,
        "warm_speedup": 2.0,
        "cold_speedup": 2.0,
        "startup_tax_ms": 5.0,
        "verdict": schema.VERDICT_GREEN,
        "binary_size_kib": 512.0,
        "molt_peak_rss_mib": 18.0,
        "compile_time_s": 0.4,
        "stable": True,
        "pypy_ratio": None,
        "codon_ratio": None,
        "codon_equivalent": None,
        "cpython_peak_rss_mib": 15.0,
        "output_parity": _matching_output_parity(),
        "log_artifact": "bench/scoreboard/logs/fib.log",
        "classification": schema.CLASS_GREEN,
        "measured_quiescent": True,
        "fact_class": "repr_tir_type_lattice",
        "suspected_missing_fact": "Repr/TirType numeric lane",
        "pypy_advantage_class": "loop_specialization",
        "reference_class": "numeric",
        "codon_semantics": "equivalent",
        "attribution_confidence": 0.8,
    }
    cell.update(overrides)
    if "workload" not in overrides:
        cell["workload"] = synthetic_workload(cell["benchmark"])
    return cell


def _minimal_host() -> dict[str, object]:
    return {
        "platform": "test",
        "python_runner": "3.12.13",
        "cpython_baseline": "3.14.3",
    }


def test_native_runtime_variant_binds_per_cell_without_promoting_legacy_data() -> None:
    from tools.perf_scoreboard_build_profiles import measured_runtime_coordinate

    backend, profile = "native", "release-fast"
    runtime = {
        "identity": {"filename": "libmolt_runtime.a", "size": 123, "sha256": "a" * 64},
        "runtime_features": ["stdlib_micro"],
    }
    observation = {"runtime": runtime}
    cell = _cell(build_observation=observation)
    key, digest = measured_runtime_coordinate(
        observation, backend=backend, profile=profile
    )
    doc = {"provenance": {"runtime_identity": {key: digest}}}
    assert not schema.runtime_board_binding_problems(
        doc, [cell], require_native_variants=True
    )
    runtime["runtime_features"] = ["stdlib_edge"]
    assert schema.runtime_board_binding_problems(doc, [cell])
    del runtime["runtime_features"]
    assert not schema.runtime_board_binding_problems(doc, [cell])
    assert schema.runtime_board_binding_problems(
        doc, [cell], require_native_variants=True
    )


def _modern_host(**overrides: object) -> dict[str, object]:
    oracle = benchmark_oracle_metadata(
        "C:/Python314/python.exe", "3.14.3", sys_platform="win32", machine="AMD64"
    )
    host: dict[str, object] = {
        "platform": oracle["sys_platform"],
        "machine": oracle["machine"],
        "arch": oracle["arch"],
        "pointer_bits": oracle["pointer_bits"],
        "python_runner": "3.12.13",
        "cpython_baseline": oracle["version"],
        "molt_target_python": "3.14",
        "cpython_oracle": oracle,
        "pypy": None,
        "codon": None,
    }
    host.update(overrides)
    return host


def _doc(cell: dict[str, object]) -> dict[str, object]:
    coordinate = schema.planned_cell_from_observation(cell)
    return {
        "schema_version": schema.SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": "2026-06-24T00:00:00+00:00",
        "git_rev": "a" * 40,
        "provenance": {
            "origin_sha": "a" * 40,
            "local_head_sha": "a" * 40,
            "merge_base_sha": "a" * 40,
            "dirty_tree": False,
            "methodology_schema": "molt-perf-methodology-v1",
            "methodology_version": 1,
            "methodology_sha256": "d" * 64,
            "methodology_locked": True,
            "backend_binary_identity": {"native/release-fast": "sha|1|2"},
            "runtime_identity": {},
            "authoritative": True,
        },
        "host": _modern_host(),
        "direction": "speedup = cpython_time / molt_time",
        "red_threshold": 1.0,
        "verdict_legend": {},
        "methodology": {"process_timing_scope": schema.process_timing_scope()},
        "planned_cells": [coordinate.as_record()],
        "run_state": "complete",
        "coverage": schema.scoreboard_coverage(
            (coordinate,), [cell], authoritative=True
        ),
        "reserved_columns": {},
        "summary": {
            "cells_fail_engine": 0,
            "cells_fail_cold_budget": 0,
            "cells_warn_cold_floor": 0,
            "cells_fail_stale": 0,
            "verdict_breakdown": {},
            "gate_fails": False,
        },
        "benchmarks_run": [cell["benchmark"]],
        "benchmarks_deferred": [],
        "scoreboard": {
            cell["benchmark"]: {
                cell["target"]: {cell["backend"]: {cell["profile"]: cell}}
            }
        },
    }


def test_perf_scoreboard_uses_schema_vocabulary_authority() -> None:
    assert scoreboard.SCHEMA_VERSION == schema.SCHEMA_VERSION
    assert scoreboard.VERDICT_FAIL_ENGINE == schema.VERDICT_FAIL_ENGINE
    assert scoreboard.CLASS_RED_STABLE == schema.CLASS_RED_STABLE
    assert scoreboard.verdict_fails_gate is schema.verdict_fails_gate


def test_schema_accepts_valid_board_and_materializes_cell() -> None:
    cell = _cell()
    doc = _doc(cell)

    assert schema.validate_board(doc) == []
    flattened = schema.flatten_cells(doc)
    assert flattened == [cell]
    perf_cell = schema.PerfCell.from_payload(flattened[0])
    assert perf_cell.benchmark == "tests/benchmarks/bench_fib.py"
    assert perf_cell.workload == synthetic_workload(perf_cell.benchmark)
    assert perf_cell.verdict == schema.VERDICT_GREEN
    assert perf_cell.stable is True
    assert perf_cell.warm_speedup == 2.0
    assert perf_cell.fact_class == "repr_tir_type_lattice"
    assert perf_cell.attribution_confidence == 0.8


def test_coverage_keeps_failed_stale_and_unrun_coordinates_in_selected_total() -> None:
    good = _cell()
    stale = _cell(backend="llvm", verdict=schema.VERDICT_FAIL_STALE)
    failed = _cell(
        benchmark="tests/benchmarks/bench_sum.py",
        build_ok=False,
        molt_ok=False,
        verdict=schema.VERDICT_BUILD_FAILED,
        output_parity=None,
    )
    unrun = schema.PlannedCell(
        "tests/benchmarks/bench_sum.py", "native", "llvm", "release-fast"
    )
    planned = tuple(
        schema.planned_cell_from_observation(c) for c in (good, stale, failed)
    ) + (unrun,)

    assert schema.scoreboard_coverage(
        planned, [good, stale, failed], authoritative=False
    ) == {
        "selected": 4,
        "attempted": 3,
        "parity_qualified": 1,
        "measured": 1,
        "release_qualified": 0,
        "not_run": [unrun.as_record()],
    }


def test_coverage_refuses_duplicate_or_undeclared_observations() -> None:
    cell = _cell()
    coordinate = schema.planned_cell_from_observation(cell)
    with pytest.raises(ValueError, match="unique"):
        schema.scoreboard_coverage((coordinate, coordinate), [], authoritative=False)
    with pytest.raises(ValueError, match="duplicate measured"):
        schema.scoreboard_coverage((coordinate,), [cell, cell], authoritative=False)
    with pytest.raises(ValueError, match="outside the declared plan"):
        schema.scoreboard_coverage(
            (coordinate,), [_cell(backend="llvm")], authoritative=False
        )


def test_schema_preserves_zero_attempt_partial_plan_without_measurements() -> None:
    cell = _cell()
    doc = _doc(cell)
    planned = (schema.planned_cell_from_observation(cell),)
    doc["run_state"] = "partial"
    doc["provenance"]["authoritative"] = False
    doc["scoreboard"] = {}
    doc["benchmarks_run"] = []
    doc["coverage"] = schema.scoreboard_coverage(planned, [], authoritative=False)

    assert schema.validate_board(doc) == []
    assert schema.flatten_cells(doc) == []
    assert doc["coverage"]["attempted"] == 0
    assert doc["coverage"]["not_run"] == [planned[0].as_record()]
    doc["run_state"] = "complete"
    assert any("unrun planned" in error for error in schema.validate_board(doc))


@pytest.mark.parametrize("field", ["planned_cells", "coverage", "run_state"])
def test_schema_refuses_missing_selection_evidence(field: str) -> None:
    doc = _doc(_cell())
    del doc[field]
    assert schema.validate_board(doc)


def test_schema_does_not_accept_forged_coverage_or_startup_scope() -> None:
    doc = _doc(_cell())
    doc["coverage"]["attempted"] = True
    assert any("coverage differs" in error for error in schema.validate_board(doc))
    doc = _doc(_cell())
    doc["methodology"]["process_timing_scope"]["cache_state"] = "cold"
    assert any(
        "fresh-process timing scope" in error for error in schema.validate_board(doc)
    )


@pytest.mark.parametrize("quiescent", [False, None])
def test_schema_refuses_stable_win_without_measured_quiescence(
    quiescent: bool | None,
) -> None:
    assert any(
        "GREEN_STABLE without measured_quiescent=true" in error
        for error in schema.validate_cell(_cell(measured_quiescent=quiescent))
    )


def test_schema_rejects_historical_board_version() -> None:
    doc = _doc(_cell())
    doc["schema_version"] = schema.SCHEMA_VERSION - 1

    problems = schema.validate_board(doc)

    assert any("schema_version must be" in problem for problem in problems)


def test_schema_accepts_modern_cpython_oracle_host_metadata() -> None:
    cell = _cell()
    doc = _doc(cell)
    doc["host"] = _modern_host()

    assert schema.validate_board(doc) == []


def test_schema_rejects_incomplete_or_inconsistent_cpython_oracle_host() -> None:
    cell = _cell()
    doc = _doc(cell)
    doc["host"] = _modern_host(arch="aarch64")

    problems = schema.validate_board(doc)

    assert any("host.cpython_oracle.arch must match host.arch" in p for p in problems)

    doc["host"] = _modern_host(cpython_oracle=None)
    problems = schema.validate_board(doc)

    assert any("host.cpython_oracle must be an object" in p for p in problems)


def test_current_board_cannot_omit_all_managed_host_fields() -> None:
    doc = _doc(_cell())
    doc["host"] = _minimal_host()

    assert any("cpython_oracle" in problem for problem in schema.validate_board(doc))


@pytest.mark.parametrize(
    ("mutation", "field"),
    [
        ("missing_origin", "managed_origin"),
        ("wrong_origin", "managed_origin.kind"),
        ("wrong_request", "requested_release"),
        ("wrong_managed_path", "managed_origin.executable"),
        ("system_lookup", "lookup_command"),
        ("relative_uv", "lookup_command"),
        ("nonisolated", "cmd"),
        ("seeded_hash", "launch_policy"),
        ("minor_only", "version"),
    ],
)
def test_retained_oracle_rejects_missing_or_forged_protocol(
    mutation: str, field: str
) -> None:
    authentic = benchmark_oracle_metadata()
    oracle: dict[str, object] = dict(authentic)
    origin: dict[str, object] = dict(authentic["managed_origin"])
    oracle["managed_origin"] = origin
    assert schema.benchmark_python_metadata_problems(oracle) == []

    if mutation == "missing_origin":
        oracle.pop("managed_origin")
    elif mutation == "wrong_origin":
        origin["kind"] = "system"
    elif mutation == "wrong_request":
        origin["requested_release"] = "3.12.15"
    elif mutation == "wrong_managed_path":
        origin["executable"] = "/another/python"
    elif mutation == "system_lookup":
        origin["lookup_command"] = [
            token
            for token in authentic["managed_origin"]["lookup_command"]
            if token != "--managed-python"
        ]
    elif mutation == "relative_uv":
        origin["lookup_command"] = [
            "uv",
            *authentic["managed_origin"]["lookup_command"][1:],
        ]
    elif mutation == "nonisolated":
        oracle["cmd"] = [authentic["executable"], "-S", "-B"]
    elif mutation == "seeded_hash":
        oracle["launch_policy"] = "fixed-hash"
    else:
        oracle["version"] = "3.12"

    assert any(
        field in problem
        for problem in schema.benchmark_python_metadata_problems(oracle)
    )


def test_schema_rejects_cpython_oracle_launcher_command() -> None:
    cell = _cell()
    doc = _doc(cell)
    host = _modern_host()
    oracle = dict(host["cpython_oracle"])
    oracle["cmd"] = ["py", "-3.14"]
    host["cpython_oracle"] = oracle
    doc["host"] = host

    problems = schema.validate_board(doc)

    assert any("cmd[0] must be the resolved executable" in p for p in problems)


def test_schema_rejects_unknown_verdict_and_classification() -> None:
    cell = _cell(verdict="MAYBE_FAST", classification="SORT_OF_GREEN")

    problems = schema.validate_cell(cell)

    assert any("unknown verdict" in problem for problem in problems)
    assert any("unknown classification" in problem for problem in problems)


def test_schema_rejects_legacy_or_missing_parity_on_measured_cells() -> None:
    legacy = _cell(output_parity=True)
    assert any(
        "output_parity must be an object" in p for p in schema.validate_cell(legacy)
    )

    missing = _cell(output_parity=None)
    assert any("output_parity is missing" in p for p in schema.validate_cell(missing))


def test_schema_rejects_forged_affirmative_parity_without_observations() -> None:
    zero = _matching_output_parity()
    zero["reference_observation_count"] = 0
    zero["molt_observation_count"] = 0

    problems = schema.validate_cell(_cell(output_parity=zero))

    assert any("reference_observation_count must be at least 1" in p for p in problems)
    assert any("molt_observation_count must be at least 1" in p for p in problems)

    unequal = _matching_output_parity()
    unequal["molt_observation_count"] = 2
    assert any(
        "reference and molt observation counts must match" in p
        for p in schema.validate_cell(_cell(output_parity=unequal))
    )


@pytest.mark.parametrize(
    ("field", "forged_value", "problem_text"),
    [
        (
            "molt_stdout_sha256",
            "f" * 64,
            "stdout_match=true contradicts unequal representative stdout hashes",
        ),
        (
            "molt_stderr_sha256",
            "f" * 64,
            "stderr_match=true contradicts unequal representative stderr hashes",
        ),
        (
            "molt_returncode",
            7,
            "exit_match=true contradicts unequal representative return codes",
        ),
    ],
)
def test_schema_rejects_affirmative_axes_that_contradict_persisted_evidence(
    field: str,
    forged_value: object,
    problem_text: str,
) -> None:
    parity = _matching_output_parity()
    parity[field] = forged_value
    cell = _cell(output_parity=parity)

    problems = schema.validate_cell(cell)

    assert any(problem_text in problem for problem in problems)
    assert any(problem_text in problem for problem in schema.validate_board(_doc(cell)))
    with pytest.raises(ValueError, match=problem_text):
        schema.PerfCell.from_payload(cell)


def test_schema_does_not_treat_equal_hashes_as_the_comparison_authority() -> None:
    parity = schema.output_parity_evidence(
        reference_observations=[("cpython:cold", "reference\n", "", 0)],
        molt_observations=[("molt:cold", "different\n", "", 0)],
    )
    assert parity["stdout_match"] is False
    parity["molt_stdout_sha256"] = parity["reference_stdout_sha256"]

    assert schema.output_parity_evidence_problems(parity) == []


def test_output_parity_evidence_tracks_warm_instability() -> None:
    parity = schema.output_parity_evidence(
        reference_observations=[
            ("cpython:cold", "same\n", "", 0),
            ("cpython:warm:p0:s0", "same\n", "", 0),
        ],
        molt_observations=[
            ("molt:cold", "same\n", "", 0),
            ("molt:warm:p0:s0", "changed\n", "", 0),
        ],
    )

    assert parity["checked"] is True
    assert parity["ok"] is False
    assert parity["reason"] == "molt_unstable"
    assert parity["mismatch_observation"] == "molt:warm:p0:s0"
    assert schema.output_parity_evidence_problems(parity) == []


def test_schema_rejects_invalid_attribution_fields() -> None:
    cell = _cell(attribution_confidence=1.5)
    problems = schema.validate_cell(cell)
    assert any("out-of-range attribution_confidence" in problem for problem in problems)

    cell = _cell(suspected_missing_fact="")
    problems = schema.validate_cell(cell)
    assert any("invalid suspected_missing_fact" in problem for problem in problems)

    cell = _cell(fact_class="shape_facts", suspected_missing_fact=None)
    problems = schema.validate_cell(cell)
    assert any(
        "fact_class without suspected_missing_fact" in problem for problem in problems
    )


def test_schema_requires_failure_payload_for_failed_molt_cells() -> None:
    cell = _cell(
        build_ok=False,
        molt_ok=False,
        cpython_ok=False,
        verdict=schema.VERDICT_BUILD_FAILED,
        binary_size_kib=None,
        compile_time_s=None,
        molt_peak_rss_mib=None,
        cpython_peak_rss_mib=None,
        output_parity=None,
    )

    problems = schema.validate_cell(cell)
    assert any("without a molt_failure payload" in problem for problem in problems)

    failure = {
        "phase": "build",
        "status": "daemon_crash",
        "detail": "backend_daemon_empty_response",
        "message": "backend daemon returned empty response",
        "stdout_tail": None,
        "stderr_tail": "backend daemon returned empty response",
        "returncode": 1,
        "timed_out": False,
        "elapsed_s": 2.5,
        "signal": None,
        "guard_violation": None,
        "orphaned_process_groups": [],
    }
    cell.update(
        {
            "molt_failure": failure,
            "molt_failure_phase": "build",
            "molt_failure_status": "daemon_crash",
            "molt_failure_detail": "backend_daemon_empty_response",
            "molt_failure_message": "backend daemon returned empty response",
            "molt_failure_returncode": 1,
            "molt_failure_timed_out": False,
            "molt_failure_elapsed_s": 2.5,
            "molt_failure_signal": None,
            "molt_failure_guard_violation": None,
            "molt_failure_orphaned_process_groups": [],
        }
    )

    assert schema.validate_cell(cell) == []
    perf_cell = schema.PerfCell.from_payload(cell)
    assert perf_cell.molt_failure_phase == "build"
    assert perf_cell.molt_failure_status == "daemon_crash"
    assert perf_cell.molt_failure_returncode == 1
    assert perf_cell.molt_failure_timed_out is False
    assert perf_cell.molt_failure_elapsed_s == 2.5
    assert perf_cell.molt_failure_orphaned_process_groups == []


def test_schema_rejects_measured_verdict_without_method_facts() -> None:
    cell = _cell(warm_molt_s=None)

    problems = schema.validate_cell(cell)

    assert any("missing numeric facts" in problem for problem in problems)


def test_schema_rejects_red_stable_without_quiescent_repeat_ci() -> None:
    cell = _cell(
        verdict=schema.VERDICT_FAIL_ENGINE,
        classification=schema.CLASS_RED_STABLE,
        measured_quiescent=False,
        repeat_ci_lo=None,
        repeat_ci_hi=None,
        warm_speedup=0.8,
        warm_molt_s=0.20,
        warm_cpython_s=0.16,
    )

    problems = schema.validate_cell(cell)

    assert any("measured_quiescent=true" in problem for problem in problems)
    assert any("numeric repeat CI" in problem for problem in problems)


def test_checked_in_quiet_native_is_historical_projection_fixture() -> None:
    doc = json.loads(
        (REPO_ROOT / "bench" / "scoreboard" / "quiet_native.json").read_text(
            encoding="utf-8"
        )
    )

    assert pa.is_stale_snapshot_metadata(doc.get(pa.STALE_METADATA_KEY))
    cells = schema.flatten_cells(doc)
    assert cells
    assert all(cell.get("benchmark") for cell in cells)


def _node_observation(*, path="/opt/node", version="24.1.0", digest="a" * 64):
    return {
        "kind": "node",
        "executable": path,
        "version": version,
        "identity": {
            "entrypoint": "node",
            "content_filename": "node",
            "size": 1024,
            "sha256": digest,
        },
    }


def test_wasm_execution_observation_survives_schema_and_typed_projection():
    engine = _node_observation()
    cell = _cell(
        target="wasm", backend="wasm", profile="wasm-release", execution_host=engine
    )
    doc = _doc(cell)
    doc["host"]["execution_host"] = engine
    assert schema.validate_board(doc) == []
    assert schema.PerfCell.from_payload(cell).execution_host == engine


@pytest.mark.parametrize("missing", ["cell", "host", "both"])
def test_missing_historical_wasm_host_is_unqualified(missing):
    engine = _node_observation()
    cell = _cell(target="wasm", backend="wasm", execution_host=engine)
    doc = _doc(cell)
    doc["host"]["execution_host"] = engine
    if missing in {"cell", "both"}:
        cell.pop("execution_host")
    if missing in {"host", "both"}:
        doc["host"].pop("execution_host")
    assert any("execution_host" in p for p in schema.validate_board(doc))


@pytest.mark.parametrize("changed", ["version", "digest", "size"])
def test_cell_engine_must_match_board_admission(changed):
    engine = _node_observation()
    cell = _cell(target="wasm", backend="wasm", execution_host=_node_observation())
    if changed == "version":
        cell["execution_host"]["version"] = "24.2.0"
    elif changed == "digest":
        cell["execution_host"]["identity"]["sha256"] = "b" * 64
    else:
        cell["execution_host"]["identity"]["size"] += 1
    doc = _doc(cell)
    doc["host"]["execution_host"] = engine
    assert (
        "WASM cell execution_host differs from board execution_host"
        in schema.validate_board(doc)
    )


def test_native_host_cannot_masquerade_as_node():
    assert schema.validate_cell(_cell(execution_host=_node_observation()))


def test_engine_comparability_ignores_install_path_and_filename():
    left = _node_observation()
    right = _node_observation(path="D:/tools/node-copy.exe")
    right["identity"]["entrypoint"] = "node-copy.exe"
    right["identity"]["content_filename"] = "node-copy.exe"
    assert schema.execution_host_key(left) == schema.execution_host_key(right)


@pytest.mark.parametrize("mutation", ["missing", "hash", "argv", "duplicate"])
def test_workload_identity_is_required_for_comparison_and_admission(mutation):
    import copy
    from tools.bench_evidence import workloads_comparable

    left = _cell()
    right = copy.deepcopy(left)
    if mutation == "missing":
        right.pop("workload")
    elif mutation == "hash":
        right["workload"]["sources"][0]["sha256"] = "a" * 64
    elif mutation == "argv":
        right["workload"]["run_args"] = ["different-input"]
    else:
        right["workload"]["sources"] *= 2
    assert not workloads_comparable(left["workload"], right.get("workload"))
    assert not schema.cells_comparable(left, right)
    if mutation in {"missing", "duplicate"}:
        assert any("workload" in p for p in schema.validate_cell(right))


def test_external_same_basename_cannot_be_retagged_as_canonical_workload():
    from tools.bench_evidence import workload_identity_problems

    record = synthetic_workload("/private/foreign/bench_fib.py")
    record["sources"][0]["sha256"] = "f" * 64
    record["entry"] = "tests/benchmarks/bench_fib.py"
    assert workload_identity_problems(record) == [
        "workload entry is absent from its captured source paths"
    ]
