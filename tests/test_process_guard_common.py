from __future__ import annotations

import subprocess
import json
from typing import Any

import pytest

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
    assert (
        captured["kwargs"]["timeout"]
        == process_guard_common.DEFAULT_BUILD_PROCESS_TIMEOUT_SEC
    )


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
