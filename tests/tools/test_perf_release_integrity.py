"""Adversarial checks for canonical evidence acceptance (no benchmark builds)."""

import copy
from pathlib import Path
import sys
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from perf_authority import release_cell_problems, CANONICAL_PERF_BENCHMARKS  # noqa: E402


def winning_cell():
    import perf_schema

    return dict(
        build_ok=True,
        molt_ok=True,
        cpython_ok=True,
        stable=True,
        measured_quiescent=True,
        verdict="GREEN",
        classification="GREEN_STABLE",
        repeat_stability="STABLE_ABOVE",
        repeat_passes=5,
        repeat_ci_lo=1.01,
        repeat_ci_hi=1.2,
        output_parity=perf_schema.output_parity_evidence(
            reference_observations=[("cpython", "result", "", 0)],
            molt_observations=[("molt", "result", "", 0)],
        ),
    )


def test_actual_statistical_win():
    assert release_cell_problems(winning_cell()) == []


@pytest.mark.parametrize(
    "field,value",
    [
        ("verdict", "CPY_INCOMPAT"),
        ("verdict", "WARN_COLD_FLOOR"),
        ("classification", "TIE"),
        ("repeat_stability", "STRADDLES"),
        ("repeat_passes", 1),
        ("repeat_ci_lo", 1.0),
        ("repeat_ci_lo", float("nan")),
        ("repeat_ci_hi", 1.0),
        ("measured_quiescent", False),
        ("cpython_ok", False),
    ],
)
def test_median_or_unknown_cannot_establish_release_win(field, value):
    cell = winning_cell()
    cell[field] = value
    assert release_cell_problems(cell)


def test_authoritative_projection_uses_same_required_cell_rule():
    from perf_board import _gate_cpython

    cell = winning_cell()
    assert _gate_cpython(cell, board_authoritative=True).verdict == "PASS"
    cell["repeat_ci_lo"] = 0.98
    assert _gate_cpython(cell, board_authoritative=True).verdict == "FAIL"


def _merge_source(backend, benchmark):
    import runpy

    helpers = runpy.run_path(str(ROOT / "tests/tools/test_perf_scoreboard.py"))
    cell = helpers["_cell"](
        backend=backend,
        benchmark=benchmark,
        warm_molt_s=0.1,
        warm_cpython_s=0.2,
        cold_molt_s=0.1,
        cold_cpython_s=0.2,
    )
    cell.finalize(budget_ms=1000, authoritative=True)
    helpers["_release_evidence"](cell)
    doc = helpers["_board"](
        [cell],
        provenance=dict(
            benchmark_tool_identity_schema="molt-perf-tool-family-v1",
            benchmark_tool_sha="b" * 64,
            require_quiescent=True,
            quiescent=True,
            benchmark_tool_modified=False,
            diverges_from_origin=False,
            backend_binary_identity={backend + "/release-fast": "binary-" + backend},
        ),
    )
    doc["generated_at"] = "2026-09-30T00:00:00+00:00"
    doc["git_rev"] = "a" * 40
    return doc


def _merge(tmp_path, documents):
    import json
    import perf_scoreboard as ps

    paths = []
    for i, doc in enumerate(documents):
        path = tmp_path / f"source-{i}.json"
        path.write_text(json.dumps(doc), encoding="utf-8")
        paths.append(path)
    out = tmp_path / "merged.json"
    rc = ps._merge_boards(paths, out, no_gate=True)
    return rc, json.loads(out.read_text(encoding="utf-8")) if out.exists() else None


@pytest.mark.parametrize(
    "mutation", ["revision", "host", "method", "dirty", "quiet", "tool", "duplicate"]
)
def test_merge_rejects_provenance_laundering_and_overwrite(tmp_path, mutation):
    a = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    b = _merge_source("llvm", CANONICAL_PERF_BENCHMARKS[1])
    if mutation == "revision":
        b["git_rev"] = "c" * 40
    elif mutation == "host":
        b["host"]["cpython_baseline"] = "3.13"
    elif mutation == "method":
        b["methodology"]["samples_per_phase"] = 6
    elif mutation == "dirty":
        a["provenance"]["dirty_tree"] = True
    elif mutation == "quiet":
        a["provenance"]["quiescent"] = False
    elif mutation == "tool":
        b["provenance"]["benchmark_tool_sha"] = "c" * 64
    else:
        b = copy.deepcopy(a)
    rc, out = _merge(tmp_path, [a, b])
    assert rc == 3
    assert out is None


def test_merge_preserves_canonical_order_oldest_age_and_backend_union(tmp_path):
    a = _merge_source("native", CANONICAL_PERF_BENCHMARKS[1])
    b = _merge_source("llvm", CANONICAL_PERF_BENCHMARKS[0])
    a["generated_at"] = "2026-09-29T00:00:00+00:00"
    rc, out = _merge(tmp_path, [a, b])
    assert rc == 0
    assert out["benchmarks_run"] == list(CANONICAL_PERF_BENCHMARKS[:2])
    assert out["generated_at"] == a["generated_at"]
    assert out["provenance"]["backend_binary_identity"] == {
        "native/release-fast": "binary-native",
        "llvm/release-fast": "binary-llvm",
    }


def test_gate_does_not_trust_forged_summary():
    import perf_scoreboard as ps

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    cell = next(
        iter(next(iter(next(iter(doc["scoreboard"].values())).values())).values())
    )
    next(iter(cell.values()))["repeat_ci_lo"] = 0.99
    doc["summary"]["gate_fails"] = False
    assert ps._gate_exit_code(doc, no_gate=False) == 1


def test_tool_family_identity_changes_when_sibling_gate_changes(monkeypatch):
    import perf_scoreboard as ps

    changed = False

    def git(args):
        if args[0] == "log":
            return "a" * 40
        if (
            changed
            and args[0] == "hash-object"
            and args[1] == "--path=tools/perf_authority.py"
        ):
            return "c" * 40
        return "b" * 40

    monkeypatch.setattr(ps, "_git_output", git)
    original = ps._benchmark_tool_identity()
    assert original["modified_vs_head"] == "false"
    changed = True
    modified = ps._benchmark_tool_identity()
    assert modified["modified_vs_head"] == "true"
    assert modified["ondisk_blob_sha"] != original["ondisk_blob_sha"]


@pytest.mark.parametrize("value", [None, True, -1, float("nan"), float("inf"), "100"])
def test_missing_or_invalid_startup_budget_is_not_accepted(value):
    from perf_scoreboard_model import _budget_ms_for

    with pytest.raises(ValueError, match="budget missing or invalid"):
        _budget_ms_for(
            {"budgets": {"native/release-output": {"budget_ms": value}}},
            "native",
            "release-output",
        )


def test_missing_budget_policy_fails_closed(monkeypatch, tmp_path):
    import perf_scoreboard_model as model

    monkeypatch.setattr(model, "COLD_START_BUDGET_PATH", tmp_path / "missing.json")
    with pytest.raises(ValueError, match="budget policy"):
        model._load_cold_start_budgets()


def test_summary_rebuild_cannot_fill_unrecorded_measurement_identities(
    tmp_path, monkeypatch
):
    import json
    import perf_scoreboard as ps

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    doc["provenance"]["backend_binary_identity"]["native/release-fast"] = None
    doc["provenance"]["stdlib_cache_key"] = None
    path = tmp_path / "prior.json"
    path.write_text(json.dumps(doc), encoding="utf-8")
    # Present-day artifacts are deliberately available: they cannot attest old samples.
    monkeypatch.setattr(ps, "_backend_binary_identity_for", lambda *args: "new-binary")
    monkeypatch.setattr(ps, "_stdlib_cache_key_signal", lambda: "new-cache")
    rc = ps._rebuild_summary(path, no_gate=True)
    assert rc == 0
    rebuilt = json.loads(path.read_text(encoding="utf-8"))
    assert (
        rebuilt["provenance"]["backend_binary_identity"]["native/release-fast"] is None
    )
    assert rebuilt["provenance"]["stdlib_cache_key"] is None


def test_native_measurement_passes_oracle_minor_to_build(monkeypatch, tmp_path):
    import perf_scoreboard_measure as measure

    captured = {}

    def build(*args, **kwargs):
        captured.update(kwargs)
        return None

    monkeypatch.setattr(measure, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(measure, "_perfscore_build_env", lambda spec: {})
    monkeypatch.setattr(measure.bench, "prepare_molt_binary", build)
    script = tmp_path / "bench_probe.py"
    script.write_text("print(1)", encoding="utf-8")
    measure.measure_cell(
        script_path=script,
        spec=measure.BackendSpec("native", "native", None, "native"),
        profile="release-fast",
        samples=5,
        warmup=2,
        rss_mb=64,
        timeout_s=1.0,
        batch_server=None,
        cpython_cmd=("cpython-3.14",),
        target_python_version="3.14",
        log_dir=tmp_path / "logs",
    )
    args = captured["extra_args"]
    assert args[args.index("--python-version") + 1] == "3.14"


def test_wasm_measurement_build_uses_same_oracle_minor(monkeypatch, tmp_path):
    import subprocess
    import perf_scoreboard_measure as measure

    captured = []
    monkeypatch.setattr(measure.bench, "BENCH_TMP_ROOT", tmp_path)

    def guard(cmd, **kwargs):
        captured.extend(cmd)
        return subprocess.CompletedProcess(cmd, 1, "", "fixture build failure")

    monkeypatch.setattr(
        measure.harness_memory_guard, "guarded_completed_process", guard
    )
    measure._build_wasm_only(tmp_path / "probe.py", {}, "release", [], "3.13")
    assert captured[captured.index("--python-version") + 1] == "3.13"
    assert captured[captured.index("--target") + 1] == "wasm"


def test_legacy_entrypoint_hash_cannot_attest_tool_family():
    from perf_authority import perf_tool_identity_problems

    assert perf_tool_identity_problems({"benchmark_tool_sha": "b" * 40})
    assert not perf_tool_identity_problems(
        {
            "benchmark_tool_sha": "b" * 64,
            "benchmark_tool_identity_schema": "molt-perf-tool-family-v1",
        }
    )


def test_build_observation_hashes_selected_bytes_without_claiming_daemon_attestation(
    tmp_path,
):
    from molt.cli.build_results import _observed_build_toolchain
    import hashlib

    backend = tmp_path / "backend.exe"
    runtime = tmp_path / "runtime.lib"
    artifact = tmp_path / "program.exe"
    for path, content in (
        (backend, b"compiler"),
        (runtime, b"runtime"),
        (artifact, b"program"),
    ):
        path.write_bytes(content)
    facts = _observed_build_toolchain(
        backend_bin=backend, runtime_lib=runtime, output=artifact
    )
    assert facts["compiled_with_verified"] is False
    for name, path in (
        ("compiler", backend),
        ("runtime", runtime),
        ("artifact", artifact),
    ):
        assert (
            facts[name]["identity"]["sha256"]
            == hashlib.sha256(path.read_bytes()).hexdigest()
        )
    runtime.write_bytes(b"changed")
    changed = _observed_build_toolchain(
        backend_bin=backend, runtime_lib=runtime, output=artifact
    )
    assert changed["runtime"]["identity"] != facts["runtime"]["identity"]


def test_build_observation_retains_unknowns_without_candidate_path_fallback(tmp_path):
    from molt.cli.build_results import _observed_build_toolchain

    facts = _observed_build_toolchain(
        backend_bin=None,
        runtime_lib=tmp_path / "missing",
        output=tmp_path / "missing-output",
    )
    assert facts["compiler"] is None
    assert "error" in facts["runtime"]
    assert "identity" not in facts["artifact"]
    assert facts["compiled_with_verified"] is False


def test_publication_observation_cannot_be_laundered_into_e2_attestation():
    from tools.perf_authority import scoreboard_observed_toolchain_problems

    identity = {"sha256": "a" * 64, "size": 10}
    facts = {"kind": "molt-build-observation-v1", "compiled_with_verified": True}
    facts.update(
        {name: {"identity": identity} for name in ("compiler", "runtime", "artifact")}
    )
    doc = {
        "cells": [{"build_observation": facts}],
        "host": {
            "cpython_oracle": {
                "command_executable_identity": identity,
                "base_executable_identity": identity,
            }
        },
        "provenance": {
            "producer_invocations": [
                {
                    "argv": ["python", "tools/perf_scoreboard.py"],
                    "command_interpreter": identity,
                    "base_interpreter": identity,
                }
            ]
        },
    }
    doc["scoreboard"] = {
        "bench.py": {"native": {"native": {"release": doc.pop("cells")[0]}}}
    }
    assert (
        "cell compiler/runtime used-byte admission receipt is unavailable"
        in scoreboard_observed_toolchain_problems(doc)
    )
    assert any(
        "canonical Python runtime closure is invalid" in p
        for p in scoreboard_observed_toolchain_problems(doc)
    )


def test_oracle_runtime_probe_uses_selected_command_and_baseline_environment(
    monkeypatch,
):
    import subprocess
    import perf_scoreboard_cli as cli

    seen = {}

    def run(command, **kwargs):
        seen.update(command=command, kwargs=kwargs)
        return subprocess.CompletedProcess(command, 0, '{"captured": true}', "")

    monkeypatch.setattr(cli.harness_memory_guard, "guarded_completed_process", run)
    monkeypatch.setattr(cli, "_cpython_run_env", lambda: {"ORACLE_ENV": "matched"})
    monkeypatch.setattr(cli, "validate_python_runtime_identity", lambda value: value)
    assert cli._observe_cpython_runtime(("selected-python",)) == {"captured": True}
    assert seen["command"][0] == "selected-python"
    assert "capture_current_python_runtime" in seen["command"][-1]
    assert seen["kwargs"]["env"] == {"ORACLE_ENV": "matched"}
    assert seen["kwargs"]["timeout"] == 180.0


def test_oracle_runtime_probe_rejects_error_and_untyped_runtime(monkeypatch):
    import subprocess
    import perf_scoreboard_cli as cli

    for code, payload in ((1, "{}"), (0, "{}"), (0, "not-json")):
        monkeypatch.setattr(
            cli.harness_memory_guard,
            "guarded_completed_process",
            lambda cmd, **kwargs: subprocess.CompletedProcess(
                cmd, code, payload, "capture failed"
            ),
        )
        with pytest.raises(RuntimeError, match="runtime closure observation"):
            cli._observe_cpython_runtime(("selected-python",))


@pytest.mark.parametrize("forged_verified", (False, True))
def test_documented_default_cli_gate_rejects_unadmitted_toolchain(forged_verified):
    import perf_scoreboard as ps

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    if forged_verified:
        from perf_schema import flatten_cells

        for cell in flatten_cells(doc):
            cell["build_observation"] = {
                "kind": "molt-build-observation-v1",
                "compiled_with_verified": True,
            }
    assert ps._gate_exit_code(doc, no_gate=False) == 1
    assert ps._gate_exit_code(doc, no_gate=True) == 0


def test_exploratory_board_marks_statistical_scope_and_unadmitted_e2():
    from perf_board import project_all

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    boards = project_all(doc)
    for board in boards.values():
        assert board["status_scope"] == "statistical-comparison"
        assert board["e2_eligibility"]["eligible"] is False
        assert any(
            "observation is missing" in reason
            for reason in board["e2_eligibility"]["problems"]
        )
