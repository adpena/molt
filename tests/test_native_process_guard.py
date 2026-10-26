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
