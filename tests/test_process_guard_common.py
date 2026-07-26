from __future__ import annotations

import subprocess
import json
from typing import Any

import pytest

from molt.cargo_execution_policy import PROOF_COMMAND_TIMEOUT_ENV
from tests import process_guard_common


def test_run_guarded_test_process_preserves_prefix_and_timeout(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = process_guard_common.run_guarded_test_process(
        ["python3", "-c", "print('ok')"],
        prefix="MOLT_UNIT_TEST",
        env={"MOLT_UNIT_TEST_TIMEOUT_SEC": "12"},
    )

    assert result.returncode == 0
    assert captured["cmd"] == ["python3", "-c", "print('ok')"]
    assert captured["kwargs"]["prefix"] == "MOLT_UNIT_TEST"
    assert captured["kwargs"]["operation_role"] == "execution"
    assert captured["kwargs"]["timeout"] == 12


@pytest.mark.parametrize(
    ("command", "expected"),
    [
        (["python3", "-m", "molt.cli", "build", "probe.py"], "build"),
        (["python3", "-m", "molt", "build", "probe.py"], "build"),
        (["uv", "run", "python", "-m", "molt.cli", "build", "probe.py"], "build"),
        (["molt", "build", "probe.py"], "build"),
        (["cargo", "build", "--locked"], "build"),
        (["cmake", "--build", "out"], "build"),
        (["clang", "probe.c", "-o", "probe"], "build"),
        (["rustc", "probe.rs"], "build"),
        (["clang", "--version"], "execution"),
        (["rustc", "-vV"], "execution"),
        (["rustc", "--print", "target-list"], "execution"),
        (["python3", "build"], "execution"),
        (["node", "build"], "execution"),
    ],
)
def test_guarded_process_role_uses_realized_command_grammar(
    command: list[str], expected: str
) -> None:
    assert process_guard_common.guarded_process_role(command).value == expected


def test_build_role_preserves_family_custody_and_uses_shared_default(
    monkeypatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    command = ["python3", "-m", "molt.cli", "build", "probe.py"]
    launch = process_guard_common.run_guarded_test_process
    launch(command, prefix="MOLT_WASM_TEST", env={})

    assert captured["kwargs"]["prefix"] == "MOLT_WASM_TEST"
    assert captured["kwargs"]["operation_role"] == "build"
    assert captured["kwargs"][
        "timeout"
    ] == process_guard_common.default_nested_process_timeout_seconds("build")


@pytest.mark.parametrize(
    ("env", "expected"),
    [
        ({"MOLT_WASM_TEST_BUILD_TIMEOUT_SEC": "111"}, 111),
        ({"MOLT_WASM_TEST_TIMEOUT_SEC": "222", "MOLT_BUILD_TIMEOUT_SEC": "333"}, 222),
        ({"MOLT_BUILD_TIMEOUT_SEC": "333"}, 333),
        ({"MOLT_TEST_PROCESS_TIMEOUT_SEC": "444"}, 444),
    ],
)
def test_build_timeout_policy_is_compositional(
    monkeypatch, env: dict[str, str], expected: float
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    process_guard_common.run_guarded_test_process(
        ["cargo", "build"], prefix="MOLT_WASM_TEST", env=env
    )
    assert captured["kwargs"]["timeout"] == expected


@pytest.mark.parametrize(
    ("prefix", "command"),
    [
        ("MOLT_NATIVE_TEST", ["python3", "tools/bench.py"]),
        ("MOLT_WASM_TEST", ["cargo", "build", "--locked"]),
        ("MOLT_CLI_TEST", ["python3", "tool.py"]),
        ("MOLT_RUST_TEST", ["rustc", "probe.rs"]),
        ("MOLT_COMPLIANCE", ["python3", "probe.py"]),
        ("MOLT_MUTATION", ["python3", "probe.py"]),
        ("MOLT_SURFACE_TEST", ["python3", "probe.py"]),
    ],
)
def test_nested_guard_defaults_cannot_undercut_owning_proof_budget(
    monkeypatch,
    prefix: str,
    command: list[str],
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    process_guard_common.run_guarded_test_process(
        command,
        prefix=prefix,
        env={
            PROOF_COMMAND_TIMEOUT_ENV: "1200",
            f"{prefix}_TIMEOUT_SEC": "300",
        },
    )

    assert captured["kwargs"]["timeout"] == 1200


def test_explicit_nested_operation_timeout_remains_narrower_than_owner(
    monkeypatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    process_guard_common.run_guarded_test_process(
        ["node", "probe.js"],
        prefix="MOLT_WASM_TEST",
        env={PROOF_COMMAND_TIMEOUT_ENV: "1200"},
        timeout=10,
    )

    assert captured["kwargs"]["timeout"] == 10


@pytest.mark.parametrize("raw", ["0", "nan", "invalid"])
def test_invalid_owning_proof_timeout_fails_closed(raw: str) -> None:
    with pytest.raises(ValueError, match=PROOF_COMMAND_TIMEOUT_ENV):
        process_guard_common._timeout_from_role_env(
            "MOLT_NATIVE_TEST",
            process_guard_common.GuardedProcessRole.EXECUTION,
            {PROOF_COMMAND_TIMEOUT_ENV: raw},
            explicit=None,
            default=None,
        )


def test_run_guarded_test_process_preserves_check_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(cmd, 17, "out", "err")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard_common.run_guarded_test_process(
            ["false"],
            prefix="MOLT_UNIT_TEST",
            check=True,
        )
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 17
        assert exc.output == "out"
        assert exc.stderr == "err"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")


def test_run_guarded_test_process_preserves_timeout_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(
            cmd,
            process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            "",
            "memory_guard: timeout after 5s\n",
        )

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard_common.run_guarded_test_process(
            ["sleep", "10"],
            prefix="MOLT_UNIT_TEST",
            timeout=5,
        )
    except subprocess.TimeoutExpired as exc:
        assert exc.timeout == 5
        assert exc.stderr == "memory_guard: timeout after 5s\n"
        receipt = json.loads(exc.__notes__[0])
        assert receipt["schema"] == "molt.test-process-timeout.v1"
        assert receipt["stderr_tail"] == "memory_guard: timeout after 5s\n"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")


def test_cleanup_failure_is_attached_without_replacing_primary() -> None:
    primary = subprocess.TimeoutExpired(["compiler"], 5)

    try:
        with process_guard_common.preserve_primary_during_cleanup(
            lambda: (_ for _ in ()).throw(NotADirectoryError("repro root changed")),
            label="tmp/repro",
        ):
            raise primary
    except subprocess.TimeoutExpired as exc:
        assert exc is primary
        receipt = json.loads(exc.__notes__[0])
        assert receipt == {
            "cleanup_error": "NotADirectoryError: repro root changed",
            "label": "tmp/repro",
            "schema": "molt.test-process-cleanup.v1",
        }
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")
