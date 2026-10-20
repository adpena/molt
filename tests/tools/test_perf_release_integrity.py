"""Adversarial checks for canonical evidence acceptance (no benchmark builds)."""

import copy
from pathlib import Path
import sys
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tools.perf_authority import release_cell_problems, CANONICAL_PERF_BENCHMARKS  # noqa: E402
from molt.process_environment import NameRule, ProcessEnvironment  # noqa: E402


def winning_cell():
    from tools import perf_schema

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
    from tools.perf_board import _gate_cpython

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
            methodology_schema="molt-perf-methodology-v1",
            methodology_version=1,
            methodology_sha256="b" * 64,
            methodology_locked=True,
            require_quiescent=True,
            quiescent=True,
            diverges_from_origin=False,
            backend_binary_identity={backend + "/release-fast": "binary-" + backend},
        ),
    )
    doc["generated_at"] = "2026-09-30T00:00:00+00:00"
    doc["git_rev"] = "a" * 40
    return doc


def _merge(tmp_path, documents):
    import json
    from tools import perf_scoreboard as ps

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
        b["provenance"]["methodology_sha256"] = "c" * 64
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
    from tools import perf_scoreboard as ps

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    cell = next(
        iter(next(iter(next(iter(doc["scoreboard"].values())).values())).values())
    )
    next(iter(cell.values()))["repeat_ci_lo"] = 0.99
    doc["summary"]["gate_fails"] = False
    assert ps._gate_exit_code(doc, no_gate=False) == 1


@pytest.mark.parametrize("value", [None, True, -1, float("nan"), float("inf"), "100"])
def test_missing_or_invalid_startup_budget_is_not_accepted(value):
    from tools.perf_scoreboard_model import _budget_ms_for

    with pytest.raises(ValueError, match="budget missing or invalid"):
        _budget_ms_for(
            {"budgets": {"native/release-output": {"budget_ms": value}}},
            "native",
            "release-output",
        )


def test_missing_budget_policy_fails_closed(monkeypatch, tmp_path):
    from tools import perf_scoreboard_model as model

    monkeypatch.setattr(model, "COLD_START_BUDGET_PATH", tmp_path / "missing.json")
    with pytest.raises(ValueError, match="budget policy"):
        model._load_cold_start_budgets()


def test_summary_rebuild_cannot_fill_unrecorded_measurement_identities(
    tmp_path, monkeypatch
):
    import json
    from tools import perf_scoreboard as ps

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    doc["provenance"]["backend_binary_identity"]["native/release-fast"] = None
    doc["provenance"]["runtime_identity"] = {"native/release-fast": None}
    path = tmp_path / "prior.json"
    path.write_text(json.dumps(doc), encoding="utf-8")
    # Present-day artifacts are deliberately available: they cannot attest old samples.
    monkeypatch.setattr(ps, "_backend_binary_identity_for", lambda *args: "new-binary")
    rc = ps._rebuild_summary(path, no_gate=True)
    assert rc == 0
    rebuilt = json.loads(path.read_text(encoding="utf-8"))
    assert (
        rebuilt["provenance"]["backend_binary_identity"]["native/release-fast"] is None
    )
    assert rebuilt["provenance"]["runtime_identity"] == {"native/release-fast": None}


def test_native_measurement_passes_oracle_minor_to_build(monkeypatch, tmp_path):
    from tools import perf_scoreboard_measure as measure

    captured = {}

    def build(*args, **kwargs):
        captured.update(kwargs)
        return None

    monkeypatch.setattr(measure, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(
        measure, "_perfscore_build_env", lambda spec, profile: ProcessEnvironment({})
    )
    monkeypatch.setattr(measure.bench, "prepare_molt_binary", build)
    script = tmp_path / "bench_probe.py"
    script.write_text("print(1)", encoding="utf-8")
    measure.measure_cell(
        script_path=script,
        spec=measure.BackendSpec("native", "native", "native"),
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
    from tools import perf_scoreboard_measure as measure

    captured = []
    from molt import dx
    from tests.test_dx_run_context import (
        _clear_run_context_env,
        _github_actions_custody_env,
    )

    source = tmp_path / "runner-work/molt/molt"
    source.mkdir(parents=True)
    sha = "b" * 40
    facts = _github_actions_custody_env(source, tmp_path / "runner-temp", sha=sha)
    _clear_run_context_env(monkeypatch)
    monkeypatch.delenv("MOLT_BENCH_TMP_ROOT", raising=False)
    for key, value in facts.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)
    monkeypatch.setattr(measure.bench, "REPO_ROOT", source)
    build_env = measure._perfscore_build_env(
        measure.BackendSpec("wasm", "wasm", "wasm"), "wasm-release"
    )

    def guard(cmd, **kwargs):
        captured.extend(cmd)
        return subprocess.CompletedProcess(cmd, 1, "", "fixture build failure")

    monkeypatch.setattr(
        measure.bench.harness_memory_guard, "guarded_completed_process", guard
    )
    monkeypatch.setattr(measure.bench, "_prune_backend_daemons", lambda env: None)
    measure.bench.prepare_molt_binary(
        str(source / "probe.py"),
        env=build_env,
        build_profile="release",
        build_target="wasm",
        extra_args=["--python-version", "3.13"],
    )
    output = Path(captured[captured.index("--out-dir") + 1])
    assert output.is_relative_to(tmp_path / "runner-temp/molt-custody")
    assert not output.exists()
    assert not (source / "tmp").exists()
    assert captured[captured.index("--python-version") + 1] == "3.13"
    assert captured[captured.index("--target") + 1] == "wasm"


def test_board_must_name_a_locked_methodology():
    from tools.perf_authority import perf_methodology_problems

    locked = {
        "methodology_schema": "molt-perf-methodology-v1",
        "methodology_version": 1,
        "methodology_sha256": "b" * 64,
        "methodology_locked": True,
    }
    assert perf_methodology_problems(locked) == []
    assert perf_methodology_problems({"benchmark_tool_sha": "b" * 64})
    assert perf_methodology_problems({**locked, "methodology_locked": False}) == [
        "methodology content was not locked to its version"
    ]
    assert perf_methodology_problems({**locked, "methodology_version": True})
    assert perf_methodology_problems({**locked, "methodology_sha256": "b" * 40})


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
    from tools import perf_scoreboard_cli as cli

    seen = {}

    def run(command, **kwargs):
        seen.update(command=command, kwargs=kwargs)
        return subprocess.CompletedProcess(command, 0, '{"captured": true}', "")

    monkeypatch.setattr(cli.harness_memory_guard, "guarded_completed_process", run)
    monkeypatch.setattr(
        cli, "_cpython_run_env", lambda: ProcessEnvironment({"ORACLE_ENV": "matched"})
    )
    monkeypatch.setattr(cli, "validate_python_runtime_identity", lambda value: value)
    assert cli._observe_cpython_runtime(("selected-python",)) == {"captured": True}
    assert seen["command"][0] == "selected-python"
    assert "capture_current_python_runtime" in seen["command"][-1]
    environment = seen["kwargs"]["env"]
    assert isinstance(environment, ProcessEnvironment)
    assert environment.rule is NameRule.host()
    assert dict(environment) == {"ORACLE_ENV": "matched"}
    assert seen["kwargs"]["timeout"] == 180.0


def test_oracle_runtime_probe_rejects_error_and_untyped_runtime(monkeypatch):
    import subprocess
    from tools import perf_scoreboard_cli as cli

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
    from tools import perf_scoreboard as ps

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    if forged_verified:
        from tools.perf_schema import flatten_cells

        for cell in flatten_cells(doc):
            cell["build_observation"] = {
                "kind": "molt-build-observation-v1",
                "compiled_with_verified": True,
            }
    assert ps._gate_exit_code(doc, no_gate=False) == 1
    assert ps._gate_exit_code(doc, no_gate=True) == 0


def test_exploratory_board_marks_statistical_scope_and_unadmitted_e2():
    from tools.perf_board import project_all

    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    boards = project_all(doc)
    for board in boards.values():
        assert board["status_scope"] == "statistical-comparison"
        assert board["e2_eligibility"]["eligible"] is False
        assert any(
            "observation is missing" in reason
            for reason in board["e2_eligibility"]["problems"]
        )


def _revision_problems(doc) -> list[str]:
    import datetime as dt

    from tools import perf_authority as pa

    return [
        problem
        for problem in pa.release_scoreboard_problems(
            doc,
            expected_source_sha="a" * 40,
            now=dt.datetime(2026, 10, 1, tzinfo=dt.timezone.utc),
        )
        if any(
            field in problem
            for field in ("origin_sha", "merge_base_sha", "local_head_sha", "git_rev")
        )
    ]


def test_release_board_is_the_requested_landed_revision_not_the_main_tip():
    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    assert _revision_problems(doc) == []
    moved_on = copy.deepcopy(doc)
    moved_on["provenance"]["origin_sha"] = "d" * 40
    assert _revision_problems(moved_on) == []
    unrecorded = copy.deepcopy(doc)
    unrecorded["provenance"]["origin_sha"] = None
    assert any("must record origin/main" in p for p in _revision_problems(unrecorded))
    off_main = copy.deepcopy(doc)
    off_main["provenance"]["merge_base_sha"] = "e" * 40
    assert any("merge_base_sha" in p for p in _revision_problems(off_main))


def test_shards_measured_while_main_moved_on_still_merge(tmp_path):
    a = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    b = _merge_source("llvm", CANONICAL_PERF_BENCHMARKS[1])
    b["provenance"]["origin_sha"] = "d" * 40
    rc, out = _merge(tmp_path, [a, b])
    assert rc == 0
    assert out is not None and out["git_rev"] == "a" * 40


@pytest.mark.parametrize(
    "phase",
    [
        "success",
        "unbound",
        "binding-error",
        "sample-error",
        "cancel",
        "cycle-error",
        "log-error",
        "unbound-log-error",
    ],
)
@pytest.mark.parametrize("cleanup_failure", [False, True])
def test_measure_cell_closes_actual_binary_owner_on_every_exit(
    monkeypatch, tmp_path, phase, cleanup_failure
):
    from tools import perf_scoreboard as scoreboard
    from tools import perf_scoreboard_measure as measure
    from tools import perf_scoreboard_build_profiles as profiles
    from molt import temporary_artifacts

    owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
    artifact = Path(owner.name) / "program"
    artifact.write_bytes(b"independent artifact")
    unbound = phase.startswith("unbound")
    target = "native"
    original = (
        KeyboardInterrupt("independent measurement cancellation")
        if phase == "cancel"
        else OSError("independent " + phase)
    )
    binary = measure.bench.MoltBinary(
        artifact,
        owner,
        0.1,
        1.0,
        None
        if unbound
        else {
            "selected_profiles": {
                "backend": "native",
                "guest_profile": "release",
                "compiler_profile": "release",
                "runtime_profile": "release-fast",
                "target": target,
            }
        },
    )
    monkeypatch.setattr(measure, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(
        measure, "_perfscore_build_env", lambda spec, profile: ProcessEnvironment({})
    )
    monkeypatch.setattr(measure, "_cpython_run_env", lambda: ProcessEnvironment({}))
    monkeypatch.setattr(measure.bench, "prepare_molt_binary", lambda *a, **kw: binary)
    monkeypatch.setattr(measure.bench, "resolve_benchmark_run_args", lambda path: [])

    def fail(*args, **kwargs):
        assert artifact.is_file()
        raise original

    if phase == "binding-error":
        monkeypatch.setattr(profiles, "profile_binding_problems", fail)
    if phase.endswith("log-error"):
        monkeypatch.setattr(measure, "_write_log", fail)
    monkeypatch.setattr(scoreboard, "capture_cycle_profile", fail)

    def sample(*args, **kwargs):
        assert artifact.is_file()
        if phase in {"sample-error", "cancel"}:
            raise original
        return measure.RunOutcome(True, 0.1, 1.0, "ok", 0, stdout="same\n", stderr="")

    monkeypatch.setattr(measure, "_run_measured_command", sample)

    unrelated = RuntimeError("unrelated caller exception")

    def run():
        try:
            raise unrelated
        except RuntimeError:
            return measure.measure_cell(
                script_path=tmp_path / "bench_probe.py",
                spec=measure.BackendSpec(target, target, target),
                profile="release-fast",
                samples=1,
                warmup=0,
                rss_mb=64,
                timeout_s=1.0,
                batch_server=None,
                cpython_cmd=("fixture-cpython",),
                target_python_version="3.12",
                log_dir=tmp_path / "logs",
                emit_cycle_profile=phase == "cycle-error",
            )

    failed = phase not in {"success", "unbound"}
    with monkeypatch.context() as scoped:
        if cleanup_failure:
            scoped.setattr(
                temporary_artifacts,
                "delete_path",
                lambda path: (False, "independent deletion refusal"),
            )
        if failed or cleanup_failure:
            with pytest.raises(type(original) if failed else OSError) as caught:
                run()
            if failed:
                assert caught.value is original
            if cleanup_failure:
                assert caught.value.benchmark_temporary_directories == (owner,)
                assert artifact.is_file()
        else:
            cell = run()
            assert cell.build_ok
            assert cell.run_blocked is False
            assert (tmp_path / "logs").is_dir()
    if cleanup_failure:
        owner.cleanup()
    assert not hasattr(unrelated, "benchmark_temporary_directories")
    assert not Path(owner.name).exists()


def test_measure_cell_preserves_replaced_allocation_and_original_error(
    monkeypatch, tmp_path
):
    from tools import perf_scoreboard_measure as measure
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    owner = OwnedTemporaryDirectory(dir=tmp_path)
    allocated = Path(owner.name)
    artifact = allocated / "program"
    artifact.write_bytes(b"owned bytes")
    moved = tmp_path / "retained-original"
    original = OSError("independent binding failure")
    binary = measure.bench.MoltBinary(artifact, owner, 0.1, 1.0, None)
    monkeypatch.setattr(measure, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(
        measure, "_perfscore_build_env", lambda spec, profile: ProcessEnvironment({})
    )
    monkeypatch.setattr(measure.bench, "prepare_molt_binary", lambda *a, **kw: binary)

    def log(*args):
        allocated.rename(moved)
        allocated.mkdir()
        (allocated / "foreign").write_bytes(b"not this allocation")
        raise original

    monkeypatch.setattr(measure, "_write_log", log)
    with pytest.raises(OSError) as caught:
        measure.measure_cell(
            script_path=tmp_path / "bench_probe.py",
            spec=measure.BackendSpec("native", "native", "native"),
            profile="release-fast",
            samples=1,
            warmup=0,
            rss_mb=64,
            timeout_s=1.0,
            batch_server=None,
            cpython_cmd=("fixture-cpython",),
            target_python_version="3.12",
            log_dir=tmp_path / "logs",
        )
    assert caught.value is original
    assert original.benchmark_temporary_directories == (owner,)
    assert (allocated / "foreign").read_bytes() == b"not this allocation"
    assert (moved / "program").read_bytes() == b"owned bytes"
    (allocated / "foreign").unlink()
    allocated.rmdir()
    moved.rename(allocated)
    owner.cleanup()


def _wasm_merge_source(benchmark, engine):
    from tools import perf_schema

    doc = _merge_source("wasm", benchmark)
    cell = perf_schema.flatten_cells(doc)[0]
    cell["execution_host"] = copy.deepcopy(engine)
    cell["target"] = "wasm"
    cell["profile"] = "wasm-release"
    doc["scoreboard"] = {benchmark: {"wasm": {"wasm": {"wasm-release": cell}}}}
    doc["provenance"]["backend_binary_identity"] = {"wasm/wasm-release": "binary-wasm"}
    doc["provenance"]["runtime_identity"] = {}
    doc["host"]["execution_host"] = copy.deepcopy(engine)
    return doc


@pytest.mark.parametrize(
    "difference",
    ["bytes", "version", "missing", "relocated", "native-union", "missing-budget"],
)
def test_merge_execution_engine_comparability(
    monkeypatch, tmp_path, capsys, difference
):
    from tests.tools.test_perf_schema import _node_observation
    from tools import perf_schema

    import json
    from tools import perf_scoreboard_model as model

    # Exercise the actual policy-file loader and lookup. This independently
    # supplied test policy is not a new live WASM budget or a lookup bypass.
    policy = {"budgets": {"native/release-fast": {"budget_ms": 1000}}}
    if difference != "missing-budget":
        policy["budgets"]["wasm/wasm-release"] = {"budget_ms": 1000}
    policy_path = tmp_path / "synthetic-cold-policy.json"
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    monkeypatch.setattr(model, "COLD_START_BUDGET_PATH", policy_path)

    engine = _node_observation()
    a = _wasm_merge_source(CANONICAL_PERF_BENCHMARKS[0], engine)
    b = _wasm_merge_source(CANONICAL_PERF_BENCHMARKS[1], engine)
    if difference == "native-union":
        a = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    elif difference == "missing":
        b["host"].pop("execution_host")
        perf_schema.flatten_cells(b)[0].pop("execution_host")
    else:
        changed = b["host"]["execution_host"]
        if difference == "bytes":
            changed["identity"]["sha256"] = "b" * 64
        elif difference == "version":
            changed["version"] = "24.2.0"
        elif difference == "relocated":
            changed["executable"] = "D:/node/renamed.exe"
            changed["identity"]["entrypoint"] = "renamed.exe"
            changed["identity"]["content_filename"] = "renamed.exe"
        perf_schema.flatten_cells(b)[0]["execution_host"] = copy.deepcopy(changed)
    rc, merged = _merge(tmp_path, [a, b])
    if difference in {"bytes", "version", "missing", "missing-budget"}:
        assert rc == 3 and merged is None
        if difference == "missing-budget":
            assert (
                "cold-start budget missing or invalid for wasm/wasm-release"
                in capsys.readouterr().err
            )
    else:
        assert rc == 0 and perf_schema.validate_board(merged) == []
        cells = perf_schema.flatten_cells(merged)
        assert (
            cells[1]["execution_host"]["executable"]
            == b["host"]["execution_host"]["executable"]
        )
        assert merged["host"]["execution_host"] is not None


def test_release_receiver_requires_engine_facts_without_relaxing_other_holds():
    from tests.tools.test_perf_schema import _node_observation
    from tools import perf_authority, perf_schema

    cell = winning_cell()
    cell["backend"] = "wasm"
    assert any("execution_host" in problem for problem in release_cell_problems(cell))
    cell["execution_host"] = _node_observation()
    assert release_cell_problems(cell) == []
    doc = _wasm_merge_source(CANONICAL_PERF_BENCHMARKS[0], cell["execution_host"])
    assert not perf_authority.scoreboard_release_eligibility(doc)["eligible"]
    perf_schema.flatten_cells(doc)[0]["execution_host"]["version"] = "24.2.0"
    assert any(
        "execution_host differs" in p
        for p in perf_authority.scoreboard_observed_toolchain_problems(doc)
    )


def test_schema_four_is_never_promoted_by_merge(tmp_path):
    doc = _merge_source("native", CANONICAL_PERF_BENCHMARKS[0])
    doc["schema_version"] = 4
    rc, merged = _merge(tmp_path, [doc])
    assert rc == 3 and merged is None
