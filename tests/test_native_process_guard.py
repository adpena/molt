from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

import pytest

from molt.cargo_execution_policy import PROOF_COMMAND_TIMEOUT_ENV
from tests import native_process_guard


def test_run_native_test_process_uses_shared_memory_guard(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = native_process_guard.run_native_test_process(
        ["python3", "-m", "molt.cli", "run", "probe.py"],
        cwd=tmp_path,
        env={"PYTHONPATH": "src"},
        timeout=60,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["python3", "-m", "molt.cli", "run", "probe.py"]
    assert captured["kwargs"]["prefix"] == "MOLT_NATIVE_TEST"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 60


def test_run_native_test_process_exposes_env_overridable_default_timeout(
    monkeypatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    native_process_guard.run_native_test_process(
        ["true"],
        env={"MOLT_NATIVE_TEST_TIMEOUT_SEC": "1200"},
        default_timeout=600,
    )

    assert captured["kwargs"]["timeout"] == 1200


def test_run_native_test_process_uses_custom_default_timeout(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    native_process_guard.run_native_test_process(["true"], default_timeout=600)

    assert captured["kwargs"]["timeout"] == 600


def test_native_nested_default_inherits_owning_proof_budget(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    native_process_guard.run_native_test_process(
        ["python3", "tools/bench.py"],
        env={PROOF_COMMAND_TIMEOUT_ENV: "1200"},
    )

    assert captured["kwargs"]["timeout"] == 1200


def test_run_native_test_process_preserves_check_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(cmd, 9, "out", "err")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        native_process_guard.run_native_test_process(["false"], check=True)
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 9
        assert exc.output == "out"
        assert exc.stderr == "err"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")


@pytest.mark.parametrize("helper", ["_run_molt", "_run_python"])
def test_resource_witness_preserves_the_original_guard_timeout(
    monkeypatch, helper: str
) -> None:
    from tests import test_resource_enforcement_e2e as resource_witness

    guarded_result = object()
    error = subprocess.TimeoutExpired(
        ["selected-child"],
        180,
        output="compiler reached link\n",
        stderr="child blocked\n",
    )
    error.add_note("molt.test-process-timeout.v1: original closed guard receipt")
    setattr(error, "guarded_result", guarded_result)
    sources: list[Path] = []

    def timed_out(args, *, capture_output, text, timeout, env=None):
        source = Path(args[-1])
        assert source.read_text(encoding="utf-8") == "print('witness')"
        assert capture_output and text and timeout == 180
        sources.append(source)
        raise error

    monkeypatch.setattr(resource_witness, "run_native_test_process", timed_out)
    with pytest.raises(subprocess.TimeoutExpired) as caught:
        getattr(resource_witness, helper)("print('witness')", timeout=180)
    assert caught.value is error
    assert caught.value.stdout == "compiler reached link\n"
    assert caught.value.stderr == "child blocked\n"
    assert caught.value.__notes__ == [
        "molt.test-process-timeout.v1: original closed guard receipt"
    ]
    assert getattr(caught.value, "guarded_result") is guarded_result
    assert len(sources) == 1 and not sources[0].exists()


@pytest.mark.parametrize("kind", ["child", "feedback"])
def test_prepared_native_environment_matches_actual_run_policy(
    monkeypatch, tmp_path: Path, kind: str
) -> None:
    from molt.cli import script_commands
    from tools import prepare_compiled_contracts as preparation
    from tests.resource_enforcement_witnesses import (
        child_resource_witness,
        feedback_destination_witness,
    )

    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "dev-fast")
    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "release")
    witness = (
        child_resource_witness("/selected/python λ")
        if kind == "child"
        else feedback_destination_witness(tmp_path / "canary λ.json")
    )
    source = tmp_path / "witness.py"
    source.write_text(
        "# MOLT_ENV: MOLT_BACKEND_PROFILE=dev\n" + witness.source, encoding="utf-8"
    )
    root = preparation.ROOT
    monkeypatch.setattr(script_commands, "compiler_source_root", lambda: root)
    monkeypatch.setattr(
        script_commands._build_inputs,
        "_resolve_wrapper_build_entry",
        lambda **kwargs: (object(), None),
    )
    observed = {}

    def stop_at_build(**kwargs):
        observed.update(kwargs)
        return None, 0.0, 71

    monkeypatch.setattr(script_commands, "_run_wrapper_build", stop_at_build)
    assert (
        script_commands.run_script(
            str(source), None, [], environment_overrides=witness.environment
        )
        == 71
    )
    cwd, environment = preparation.native_build_environment(
        source, witness, compiler_root=root
    )
    assert dict(environment) == dict(observed["env"])
    assert cwd == observed["project_root"]
    assert observed["build_args"] == []
    assert environment["MOLT_RUNTIME_BUILD_PROFILE"] == "dev-fast"
    assert environment["MOLT_BACKEND_PROFILE"] == "dev"
    if kind == "child":
        assert witness.environment["MOLT_RESOURCE_MAX_MEMORY"] == "64M"
        assert environment["MOLT_RESOURCE_MAX_MEMORY"] == "67108864"
    else:
        assert environment["MOLT_CAPABILITY_TIER"] == "none"
        assert environment["MOLT_CAPABILITIES"] == "env.write"
        assert environment["MOLT_RUNTIME_FEEDBACK"] == "0"


def test_compiled_preparation_projects_host_and_shares_one_elapsed_budget(
    monkeypatch, tmp_path: Path
) -> None:
    import json
    from dataclasses import replace
    from tools import prepare_compiled_contracts as preparation

    plan = preparation.proof_plan.ProofPlan.load()
    host = next(row for row in plan.commands if row.id == "wasm.build.host")
    # Change the canonical recipe: the preparer must project it rather than a
    # private copy. There is no real compiler or process launch in this control.
    changed = replace(host, data={**host.data, "argv": [*host.argv, "--verbose"]})
    plan = replace(
        plan,
        commands=tuple(changed if row.id == host.id else row for row in plan.commands),
    )
    monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, "1200")
    times = iter([0.0, 1.0, 200.0, 500.0])
    monkeypatch.setattr(preparation, "monotonic", lambda: next(times))
    calls = []
    source_paths = []

    def record(args, **kwargs):
        calls.append((tuple(args), kwargs))
        if "molt.cli" in args:
            source = Path(args[-1])
            source_paths.append(source)
            assert source.is_file()
        if "molt.cli" in args:
            payload = {"status": "ok", "data": {"output": str(source)}}
        else:
            payload = {}
        return subprocess.CompletedProcess(args, 0, json.dumps(payload), "")

    monkeypatch.setattr(
        preparation._COMMANDS.__class__,
        "run",
        lambda self, args, **kw: record(args, **kw),
    )
    output = tmp_path / "evidence"
    preparation.prepare(plan, output)
    assert calls[0][0] == (*host.argv, "--verbose")
    assert [kw["timeout"] for _, kw in calls] == [1199.0, 1000.0, 700.0]
    assert all(kw["check"] and kw["capture_output"] for _, kw in calls)
    for args, _ in calls[1:]:
        assert args[:5] == (
            preparation.sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--json",
        )
        assert len(args) == 6  # No guessed profile, tier or features.
    child = (output / "child-policy.py").read_text(encoding="utf-8")
    feedback = (output / "feedback.py").read_text(encoding="utf-8")
    assert "import subprocess" in child and repr(preparation.sys.executable) in child
    assert "start_new_session=extended" in child
    assert 'os.putenv("MOLT_RUNTIME_FEEDBACK", "1")' in feedback
    assert 'os.environ["MOLT_RUNTIME_FEEDBACK_FILE"]' in feedback
    assert all(not source.exists() for source in source_paths)


@pytest.mark.parametrize("stage", [0, 1, 2])
@pytest.mark.parametrize("failure", ["timeout", "exit"])
def test_compiled_preparation_preserves_failure_and_never_runs_later_stage(
    monkeypatch, tmp_path: Path, stage: int, failure: str
) -> None:
    from tools import prepare_compiled_contracts as preparation

    plan = preparation.proof_plan.ProofPlan.load()
    monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, "1200")
    error = (
        subprocess.TimeoutExpired(["selected-child"], 17, output="out", stderr="err")
        if failure == "timeout"
        else subprocess.CalledProcessError(
            9, ["selected-child"], output="out", stderr="err"
        )
    )
    receipt = object()
    setattr(error, "guarded_result", receipt)
    error.add_note("original closed guard")
    calls = []
    sources = []

    def run(self, args, **kwargs):
        calls.append(tuple(args))
        if "molt.cli" in args:
            sources.append(Path(args[-1]))
        if len(calls) == stage + 1:
            raise error
        import json

        payload = {"status": "ok", "data": {"output": str(args[-1])}}
        return subprocess.CompletedProcess(args, 0, json.dumps(payload), "")

    monkeypatch.setattr(preparation._COMMANDS.__class__, "run", run)
    with pytest.raises(type(error)) as caught:
        preparation.prepare(plan, tmp_path / "evidence")
    assert caught.value is error
    assert caught.value.stdout == "out" and caught.value.stderr == "err"
    assert getattr(caught.value, "guarded_result") is receipt
    assert caught.value.__notes__ == ["original closed guard"]
    assert len(calls) == stage + 1
    assert all(not source.exists() for source in sources)


@pytest.mark.parametrize("budget", [None, "600", "1201", "nan"])
def test_compiled_preparation_refuses_missing_or_changed_owner_budget(
    monkeypatch, tmp_path: Path, budget: str | None
) -> None:
    from tools import prepare_compiled_contracts as preparation

    if budget is None:
        monkeypatch.delenv(PROOF_COMMAND_TIMEOUT_ENV, raising=False)
    else:
        monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, budget)
    monkeypatch.setattr(
        preparation._COMMANDS.__class__,
        "run",
        lambda *args, **kwargs: pytest.fail("invalid budget launched a child"),
    )
    with pytest.raises(ValueError):
        preparation.prepare(
            preparation.proof_plan.ProofPlan.load(), tmp_path / "evidence"
        )
    assert not (tmp_path / "evidence").exists()


def test_compiled_preparation_elapsed_host_time_cannot_reset_child_budget(
    monkeypatch, tmp_path: Path
) -> None:
    from tools import prepare_compiled_contracts as preparation

    monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, "1200")
    times = iter([0.0, 1.0, 1201.0])
    monkeypatch.setattr(preparation, "monotonic", lambda: next(times))
    calls = []

    def run(self, args, **kwargs):
        calls.append(tuple(args))
        return subprocess.CompletedProcess(args, 0, "{}", "")

    monkeypatch.setattr(preparation._COMMANDS.__class__, "run", run)
    with pytest.raises(subprocess.TimeoutExpired) as caught:
        preparation.prepare(
            preparation.proof_plan.ProofPlan.load(), tmp_path / "evidence"
        )
    assert len(calls) == 1
    assert caught.value.timeout == 1200
    assert "molt.cli" in caught.value.cmd


def test_compiled_preparation_rejects_invalid_runtime_policy(
    tmp_path: Path, monkeypatch
) -> None:
    from tools import prepare_compiled_contracts as preparation
    from tests.resource_enforcement_witnesses import feedback_destination_witness

    monkeypatch.setenv("MOLT_RESOURCE_MAX_DURATION_MS", "invalid")
    source = tmp_path / "feedback.py"
    witness = feedback_destination_witness(tmp_path / "canary.json")
    source.write_text(witness.source, encoding="utf-8")
    with pytest.raises(ValueError, match="Invalid capability policy"):
        preparation.native_build_environment(
            source, witness, compiler_root=preparation.ROOT
        )


@pytest.mark.parametrize("mutation", ["dependency", "toolchain"])
def test_compiled_preparation_rejects_unprojected_host_requirements(
    monkeypatch, tmp_path: Path, mutation: str
) -> None:
    from dataclasses import replace
    from tools import prepare_compiled_contracts as preparation

    plan = preparation.proof_plan.ProofPlan.load()
    host = next(row for row in plan.commands if row.id == "wasm.build.host")
    data = dict(host.data)
    if mutation == "dependency":
        data["dependencies"] = ["wasm.build.backend"]
    else:
        data["toolchains"] = [*host.toolchains, "node"]
    changed = replace(host, data=data)
    plan = replace(
        plan,
        commands=tuple(changed if row.id == host.id else row for row in plan.commands),
    )
    monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, "1200")
    monkeypatch.setattr(
        preparation._COMMANDS.__class__,
        "run",
        lambda *args, **kwargs: pytest.fail("unprojected host requirement launched"),
    )
    with pytest.raises(ValueError, match="host"):
        preparation.prepare(plan, tmp_path / "evidence")


@pytest.mark.parametrize(
    "payload",
    ["{}", "not json", '{"status":"ok","data":{"output":"/missing/compiled-witness"}}'],
)
def test_compiled_preparation_zero_exit_does_not_replace_build_contract(
    monkeypatch, tmp_path: Path, payload: str
) -> None:
    from tools import prepare_compiled_contracts as preparation

    monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, "1200")
    calls = []

    def run(self, args, **kwargs):
        calls.append(tuple(args))
        return subprocess.CompletedProcess(args, 0, payload, "")

    monkeypatch.setattr(preparation._COMMANDS.__class__, "run", run)
    with pytest.raises(ValueError):
        preparation.prepare(
            preparation.proof_plan.ProofPlan.load(), tmp_path / "evidence"
        )
    assert len(calls) == 2
