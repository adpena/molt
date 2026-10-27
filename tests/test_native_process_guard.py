from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from molt.backend_environment import DEBUG_ARTIFACT_DIR_ENV
from molt.cargo_execution_policy import PROOF_COMMAND_TIMEOUT_ENV
from molt.process_environment import ProcessEnvironment
from tests import native_process_guard
from tests.process_guard_common import current_thread_only


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
    root = preparation._REPO_ROOT
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


def _compiled_preparation_result(args, **kwargs):
    """A build boundary double that materializes its advertised output/streams."""
    if "molt.cli" in args:
        source = next(Path(arg) for arg in args if str(arg).endswith(".py"))
        assert source.is_file()
        output = source
        if "--out-dir" in args:
            directory = Path(args[args.index("--out-dir") + 1])
            directory.mkdir(parents=True, exist_ok=True)
            output = directory / (
                "output.wasm" if "--split-runtime" in args else "output_linked.wasm"
            )
            output.write_bytes(b"\0asm\x01\0\0\0")
            if "--split-runtime" in args:
                (directory / "manifest.json").write_text("{}", encoding="utf-8")
        payload = {"status": "ok", "data": {"output": str(output)}}
    else:
        payload = {}
    stdout = json.dumps(payload)
    for key, content in (("stdout_capture_path", stdout), ("stderr_capture_path", "")):
        if path := kwargs.get(key):
            Path(path).write_text(content, encoding="utf-8")
    return subprocess.CompletedProcess(args, 0, stdout, "")


def test_compiled_preparation_projects_host_and_shares_one_elapsed_budget(
    monkeypatch, tmp_path: Path
) -> None:
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
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    times = iter(
        [0.0, 1.0, 200.0, 500.0, 550.0, 600.0, 650.0, 700.0, 750.0, 800.0, 850.0, 900.0]
    )
    monkeypatch.setattr(preparation, "monotonic", lambda: next(times))
    calls = []
    source_paths = []

    def record(args, **kwargs):
        calls.append((tuple(args), kwargs))
        if "molt.cli" in args:
            source = next(Path(arg) for arg in args if str(arg).endswith(".py"))
            source_paths.append(source)
            assert source.is_file()
        return _compiled_preparation_result(args, **kwargs)

    monkeypatch.setattr(
        preparation._COMMANDS.__class__,
        "run",
        lambda self, args, **kw: record(args, **kw),
    )
    output = tmp_path / "evidence"
    preparation.prepare(plan, output)
    assert calls[0][0] == (*host.argv, "--verbose")
    assert [kw["timeout"] for _, kw in calls] == [
        1199.0,
        1000.0,
        700.0,
        650.0,
        600.0,
        550.0,
        500.0,
        450.0,
        400.0,
        350.0,
        300.0,
    ]
    assert all(kw["check"] and kw["capture_output"] for _, kw in calls)
    for args, _ in calls[1:3]:
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
    records = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in output.glob("wasm-*/command.json")
    ]
    assert {Path(record["source"]).parent.name for record in records} == {
        "wasm-syntax-run",
        "wasm-asyncio-linked",
        "wasm-finder-linked",
        "wasm-finder-split",
        "wasm-final-tir-linked",
        "wasm-final-tir-split",
        "wasm-zipimport-linked",
        "wasm-smtplib-linked",
    }
    for record in records:
        retained = Path(record["retained_sources"])
        assert (retained / Path(record["source"]).name).is_file()
        payload = json.loads(
            (retained.parent / "build.json").read_text(encoding="utf-8")
        )
        assert Path(payload["data"]["output"]).is_file()
        assert not Path(record["products"]).is_relative_to(
            Path(record["source"]).parent
        )
        assert "--runtime-feature" not in record["argv"]


@pytest.mark.parametrize("stage", [0, 1, 2, 3, 10])
@pytest.mark.parametrize("failure", ["timeout", "exit"])
def test_compiled_preparation_preserves_failure_and_never_runs_later_stage(
    monkeypatch, tmp_path: Path, stage: int, failure: str
) -> None:
    from tools import prepare_compiled_contracts as preparation

    plan = preparation.proof_plan.ProofPlan.load()
    monkeypatch.setenv(PROOF_COMMAND_TIMEOUT_ENV, "1200")
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
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
            sources.append(next(Path(arg) for arg in args if str(arg).endswith(".py")))
        if len(calls) == stage + 1:
            raise error
        return _compiled_preparation_result(args, **kwargs)

    monkeypatch.setattr(preparation._COMMANDS.__class__, "run", run)
    with pytest.raises(type(error)) as caught:
        preparation.prepare(plan, tmp_path / "evidence")
    assert caught.value is error
    assert caught.value.stdout == "out" and caught.value.stderr == "err"
    assert getattr(caught.value, "guarded_result") is receipt
    assert caught.value.__notes__ == ["original closed guard"]
    assert len(calls) == stage + 1
    assert all(not source.exists() for source in sources)
    if stage >= 3:
        retained = list((tmp_path / "evidence").glob("wasm-*/sources/*.py"))
        assert retained, "failed WASM build must retain its actual source inputs"


def test_compiled_preparation_uses_changed_linked_and_split_command_authorities(
    monkeypatch, tmp_path: Path
) -> None:
    from tools import prepare_compiled_contracts as preparation
    from tests.wasm_runtime_heavy_witnesses import heavy_wasm_witness

    selected = preparation.WasmTestBuildCommand(
        ("changed-authority", "--new-contract"),
        tmp_path,
        {"authority": "changed"},
        tmp_path / "published",
    )
    calls = []

    def linked(root, source, output, *, extra_args):
        calls.append((root, source, output, extra_args))
        return selected

    def split(source, output, *, extra_args):
        calls.append((source, output, extra_args))
        return selected

    monkeypatch.setattr(preparation, "wasm_linked_build_command", linked)
    monkeypatch.setattr(preparation, "wasm_split_build_command", split)
    root, source, output = tmp_path / "root", tmp_path / "entry.py", tmp_path / "out"
    assert (
        preparation.wasm_build_command(
            root, heavy_wasm_witness("finder"), source, output
        )
        is selected
    )
    assert (
        preparation.wasm_build_command(
            root, heavy_wasm_witness("finder", "split"), source, output
        )
        is selected
    )
    assert calls == [
        (root, source, output, ["--no-trusted", "--json"]),
        (source, output, ["--build-profile", "dev", "--no-trusted", "--json"]),
    ]


def test_compiled_policy_retains_budgets_and_default_linux_host_coverage() -> None:
    from tools import prepare_compiled_contracts as preparation

    plan = preparation.proof_plan.ProofPlan.load()
    rows = {row.id: row for row in plan.commands}
    owner = rows["python.unit.compiled-contracts.prepare.macos"]
    body = rows["python.unit.compiled-contracts.macos"]
    linux = rows["wasm.integration.host-boundaries"]
    assert owner.data["timeout_seconds"] == 1200
    assert body.data["timeout_seconds"] == 600
    assert body.data["env"] == {"MOLT_WASM_COMPILE_FAST": "1"}
    assert "MOLT_WASM_COMPILE_FAST" not in linux.data.get("env", {})
    assert body.data["pytest"] == [
        "tests/test_resource_enforcement_e2e.py",
        "tests/test_wasm_runtime_heavy_regressions.py",
    ]
    assert "tests/test_wasm_runtime_heavy_regressions.py" in linux.data["pytest"]
    assert set(plan.required_toolchains(body)) <= set(plan.required_toolchains(owner))
    family = next(family for family in plan.families if family.name == "python_unit")
    assert family.data["timeout_minutes"] == 43


def test_compiled_preparation_refuses_missing_wasm_toolchain_before_children(
    monkeypatch, tmp_path: Path
) -> None:
    from dataclasses import replace
    from tools import prepare_compiled_contracts as preparation

    plan = preparation.proof_plan.ProofPlan.load()
    owner = next(
        row
        for row in plan.commands
        if row.id == "python.unit.compiled-contracts.prepare.macos"
    )
    changed = replace(
        owner,
        data={
            **owner.data,
            "toolchains": [tool for tool in owner.toolchains if tool != "wasm-tools"],
        },
    )
    plan = replace(
        plan,
        commands=tuple(changed if row.id == owner.id else row for row in plan.commands),
    )
    monkeypatch.setattr(
        preparation._COMMANDS.__class__,
        "run",
        lambda *args, **kwargs: pytest.fail("missing WASM closure launched a child"),
    )
    with pytest.raises(ValueError, match="WASM toolchain closure"):
        preparation.prepare(plan, tmp_path / "evidence")
    assert not (tmp_path / "evidence").exists()


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
            source, witness, compiler_root=preparation._REPO_ROOT
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
        data["toolchains"] = [*host.toolchains, "rustfmt"]
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


def _bytes_result(command, returncode=0, *, stdout=b"", stderr=b"", **kwargs):
    return native_process_guard.harness_memory_guard.GuardedCompletedProcess(
        command,
        returncode,
        stdout,
        stderr,
        elapsed_s=0.1,
        child_stderr=kwargs.pop("child_stderr", stderr),
        child_returncode=returncode,
        descendants_closed=True,
        **kwargs,
    )


def _failure_inputs(tmp_path: Path) -> dict[str, Any]:
    binary = tmp_path / "program"
    binary.write_bytes(b"independent executable generation\x00\xff")
    env = {
        DEBUG_ARTIFACT_DIR_ENV: str(tmp_path / "debug"),
        "PYTEST_CURRENT_TEST": "independent-test[dev-llvm] (call)",
    }
    return {
        "phase": "run",
        "source": b"print('original source')\n",
        "source_path": tmp_path / "input.py",
        "executable_path": binary,
        "executable": native_process_guard.observe_native_test_executable(binary, env),
        "backend": "llvm",
        "profile": "dev",
        "build_command": ["compiler", "input.py"],
        "build_cwd": tmp_path,
        "run_command": [str(binary)],
        "build": _bytes_result(["compiler", "input.py"], stdout=b"built\n"),
        "run": _bytes_result(
            [str(binary)], 7, stdout=b"out\x00\xff", stderr=b"err\xfe"
        ),
        "env": env,
    }


def test_native_failure_retains_real_child_bytes_and_unique_invocations(
    tmp_path: Path,
) -> None:
    inputs = _failure_inputs(tmp_path)
    binary = Path(sys.executable).resolve(strict=True)
    inputs["executable_path"] = binary
    inputs["executable"] = native_process_guard.observe_native_test_executable(
        binary, inputs["env"]
    )
    command = [
        str(binary),
        "-c",
        "import os; os.write(1,b'out\\x00\\xff'); os.write(2,b'err\\xfe'); raise SystemExit(7)",
    ]
    child = native_process_guard.run_native_test_process(command, text=False)
    assert child.returncode == 7 and child.descendants_closed
    inputs.update(run_command=command, run=child)
    original_bytes = binary.read_bytes()
    for _ in range(2):
        error = AssertionError("original guest failure")
        native_process_guard.retain_native_test_failure(error, **inputs)
        assert len(error.__notes__) == 1
        assert "diagnostics:" in error.__notes__[0]
    manifests = list(
        (tmp_path / "debug/native-execution-failures").glob("*/manifest.json")
    )
    assert len(manifests) == 2
    for path in manifests:
        manifest = json.loads(path.read_text(encoding="utf-8"))
        root = path.parent
        assert manifest["backend"] == "llvm" and manifest["profile"] == "dev"
        assert manifest["test"] == "independent-test[dev-llvm] (call)"
        assert (root / manifest["source_input"]["path"]).read_bytes() == inputs[
            "source"
        ]
        assert (root / "executable").read_bytes() == original_bytes
        assert (
            manifest["executable"]["sha256"]
            == hashlib.sha256(original_bytes).hexdigest()
        )
        assert manifest["executable"]["bytes"] == len(original_bytes)
        result = manifest["run"]["result"]
        assert result["returncode"] == result["child_returncode"] == 7
        assert result["descendants_closed"] is True
        assert result["timed_out"] is False
        assert result["command"] == command
        assert (root / result["stdout"]["path"]).read_bytes() == b"out\x00\xff"
        assert (root / result["child_stderr"]["path"]).read_bytes() == b"err\xfe"
        assert manifest["build"]["cwd"] == str(tmp_path)
        assert manifest["run"]["cwd"] is None
        assert manifest["run_cwd_observed_before_guest"] == str(Path.cwd())


@pytest.mark.parametrize("change", ["overwrite", "replace", "remove"])
def test_native_failure_never_copies_a_later_executable_generation(
    tmp_path: Path, change: str
) -> None:
    inputs = _failure_inputs(tmp_path)
    binary = inputs["executable_path"]
    if change == "overwrite":
        binary.write_bytes(b"later generation")
    elif change == "replace":
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"later generation")
        replacement.replace(binary)
    else:
        binary.unlink()
    error = AssertionError("original failure")
    native_process_guard.retain_native_test_failure(error, **inputs)
    [path] = (tmp_path / "debug/native-execution-failures").glob("*/manifest.json")
    manifest = json.loads(path.read_text(encoding="utf-8"))
    assert manifest["executable"]["retained"] is False
    assert manifest["executable"]["unavailable_reason"]
    assert not (path.parent / "executable").exists()
    assert (
        manifest["executable"]["sha256"]
        == hashlib.sha256(b"independent executable generation\x00\xff").hexdigest()
    )


def test_native_witness_refuses_oversize_before_hash_and_has_no_disabled_io(
    tmp_path: Path, monkeypatch
) -> None:
    from molt import toolchain_identity

    def forbidden(*args, **kwargs):
        pytest.fail("diagnostics read bytes despite disabled capture or size refusal")

    inputs = _failure_inputs(tmp_path)
    monkeypatch.setattr(
        toolchain_identity,
        "_sha256_stream",
        current_thread_only(forbidden, toolchain_identity._sha256_stream),
    )
    monkeypatch.setattr(native_process_guard, "_MAX_EXECUTABLE_BYTES", 4)
    observed = native_process_guard.observe_native_test_executable(
        inputs["executable_path"], inputs["env"]
    )
    assert observed.identity is None
    assert "exceeds size limit" in observed.unavailable_reason
    monkeypatch.setattr(native_process_guard, "open_stable_regular_file", forbidden)
    monkeypatch.setattr(native_process_guard, "backend_debug_artifact_dir", forbidden)
    assert (
        native_process_guard.observe_native_test_executable(
            inputs["executable_path"], {}
        )
        is None
    )
    inputs["env"] = {}
    original = AssertionError("original")
    native_process_guard.retain_native_test_failure(original, **inputs)
    assert not hasattr(original, "__notes__")
    assert not (tmp_path / "debug").exists()


def test_native_witness_bounds_streams_and_separates_guard_stderr(
    tmp_path: Path, monkeypatch
) -> None:
    inputs = _failure_inputs(tmp_path)
    monkeypatch.setattr(native_process_guard, "_MAX_CAPTURE_BYTES", 4)
    inputs["run"] = _bytes_result(
        ["program"],
        7,
        stdout=b"0123456789",
        stderr=b"raw\xfe + guard",
        child_stderr=b"raw\xfe",
    )
    native_process_guard.retain_native_test_failure(
        AssertionError("original"), **inputs
    )
    [path] = (tmp_path / "debug/native-execution-failures").glob("*/manifest.json")
    manifest = json.loads(path.read_text(encoding="utf-8"))
    output = manifest["run"]["result"]["stdout"]
    assert (path.parent / output["path"]).read_bytes() == b"0123"
    assert output == {
        "path": "run.stdout",
        "bytes": 10,
        "sha256": hashlib.sha256(b"0123456789").hexdigest(),
        "retained_bytes": 4,
        "retained_sha256": hashlib.sha256(b"0123").hexdigest(),
        "truncated": True,
    }
    result = manifest["run"]["result"]
    assert result["stderr"]["truncated"] is True
    assert result["child_stderr"]["truncated"] is False
    assert (path.parent / result["child_stderr"]["path"]).read_bytes() == b"raw\xfe"


@pytest.mark.parametrize(
    "failure",
    [
        "build-exit",
        "build-timeout",
        "missing-binary",
        "run-exit",
        "run-timeout",
        "success",
    ],
)
def test_loop_join_consumer_preserves_failures_and_binary_capture(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    from tests import test_native_lir_loop_join_semantics as consumer

    monkeypatch.setattr(
        consumer,
        "development_artifact_env",
        lambda root, env, **kwargs: ProcessEnvironment(
            {DEBUG_ARTIFACT_DIR_ENV: str(tmp_path / "debug")}
        ),
    )
    calls = []
    timeout = subprocess.TimeoutExpired(
        "original command", 30, output=b"partial\xff", stderr=b"raw\xfe"
    )

    def boundary(command, **kwargs):
        calls.append((list(command), kwargs))
        assert kwargs["text"] is False
        build = "--out-dir" in command
        phase = "build" if build else "run"
        result = _bytes_result(
            command,
            (9 if build else -11) if failure == f"{phase}-exit" else 0,
            stdout=b"ok\r\nnext\rline\n",
            stderr=b"failure\xfe" if failure == f"{phase}-exit" else b"",
        )
        if failure == f"{phase}-timeout":
            result.timed_out = True
            result.returncode = 124
            result.child_returncode = -9
            result.stdout, result.stderr, result.child_stderr = (
                b"partial\xff",
                b"raw\xfe + guard",
                b"raw\xfe",
            )
            timeout.guarded_result = result
            raise timeout
        if build and failure != "missing-binary":
            source_path = Path(command[command.index("--out-dir") - 1])
            assert source_path.read_bytes() == b"print('source')\n"
            source_path.with_name(source_path.stem + "_molt").write_bytes(
                b"compiler boundary image\x00\xff"
            )
        return result

    monkeypatch.setattr(consumer, "run_native_test_process", boundary)
    if failure == "success":
        assert (
            consumer._compile_and_run("print('source')\n", "release", backend="llvm")
            == "ok\nnext\nline"
        )
        assert not (tmp_path / "debug").exists()
    else:
        expected = (
            subprocess.TimeoutExpired if failure.endswith("timeout") else AssertionError
        )
        with pytest.raises(expected) as caught:
            consumer._compile_and_run("print('source')\n", "release", backend="llvm")
        if failure.endswith("timeout"):
            assert caught.value is timeout
        [path] = (tmp_path / "debug/native-execution-failures").glob("*/manifest.json")
        manifest = json.loads(path.read_text(encoding="utf-8"))
        phase = (
            "build"
            if failure.startswith("build") or failure == "missing-binary"
            else "run"
        )
        assert manifest["phase"] == phase
        assert manifest["executable"]["retained"] == (phase == "run")
        assert (path.parent / "source.py").read_bytes() == b"print('source')\n"
        if phase == "run":
            assert (
                path.parent / "executable"
            ).read_bytes() == b"compiler boundary image\x00\xff"
        if failure == "run-exit":
            assert manifest["run"]["result"]["child_returncode"] == -11
        if failure.endswith("timeout"):
            assert manifest[phase]["result"]["timed_out"] is True
            assert manifest[phase]["result"]["returncode"] == 124
            assert manifest[phase]["result"]["child_returncode"] == -9
    assert calls[0][1]["cwd"] == consumer.ROOT
    if len(calls) == 2:
        assert "cwd" not in calls[1][1] and "env" not in calls[1][1]


def test_native_witness_publication_failure_preserves_original(
    tmp_path: Path, monkeypatch
) -> None:
    inputs = _failure_inputs(tmp_path)
    error = subprocess.TimeoutExpired("original command", 30)

    def fail_publication(*args, **kwargs):
        raise OSError("injected publication failure")

    monkeypatch.setattr(native_process_guard, "atomic_write_json", fail_publication)
    native_process_guard.retain_native_test_failure(error, **inputs)
    assert error.cmd == "original command" and error.timeout == 30
    assert "injected publication failure" in error.__notes__[0]
    assert not list((tmp_path / "debug").rglob("manifest.json"))
