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
    guarded_result = subprocess.CompletedProcess(["false"], 17, "out", "err")
    guarded_result.child_returncode = 0
    guarded_result.infrastructure_failure = object()

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del cmd, kwargs
        return guarded_result

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
        assert getattr(exc, "guarded_result") is guarded_result
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")


def test_run_guarded_test_process_preserves_timeout_semantics(monkeypatch) -> None:
    guarded_result = subprocess.CompletedProcess(
        ["sleep", "10"],
        process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
        "",
        "memory_guard: timeout after 5s\n",
    )
    guarded_result.timed_out = True

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del cmd, kwargs
        return guarded_result

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
        assert getattr(exc, "guarded_result") is guarded_result
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")


def test_run_guarded_test_process_does_not_infer_timeout_from_child_124(
    monkeypatch,
) -> None:
    guarded_result = subprocess.CompletedProcess(
        ["child"],
        process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
        "",
        "memory_guard: timeout after is child output only\n",
    )
    guarded_result.timed_out = False
    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        lambda *_args, **_kwargs: guarded_result,
    )

    result = process_guard_common.run_guarded_test_process(
        ["child"],
        prefix="MOLT_UNIT_TEST",
        timeout=5,
        check=False,
    )

    assert result is guarded_result
    assert result.returncode == 124


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


def test_module_os_view_keeps_patches_out_of_the_process_wide_os(monkeypatch):
    import os
    from pathlib import Path

    from molt import file_locks
    from tests.process_guard_common import install_module_os_view
    from tools.memory_guard_core import process_custody

    real_pid = os.getpid()
    real_name = os.name
    other_name = "posix" if real_name == "nt" else "nt"
    view = install_module_os_view(monkeypatch, process_custody, name=other_name)
    monkeypatch.setattr(process_custody.os, "getpid", lambda: 999)
    monkeypatch.delattr(process_custody.os, "killpg", raising=False)

    assert process_custody.os is view
    assert process_custody.os.getpid() == 999
    assert process_custody.os.name == other_name
    assert not hasattr(process_custody.os, "killpg")
    assert process_custody.os.environ is os.environ
    # Host consumers keep the real module: pathlib still builds host paths
    # and file-lock ownership still reads the real process id.
    assert os.name == real_name and os.getpid() == real_pid
    assert file_locks.os.getpid() == real_pid
    assert Path(".").resolve().is_absolute()
    monkeypatch.undo()
    assert process_custody.os is os


def test_current_thread_only_leaves_other_threads_on_the_original():
    import threading

    from tests.process_guard_common import current_thread_only

    calls = []
    dispatch = current_thread_only(
        lambda value: calls.append(("double", value)) or "double",
        lambda value: calls.append(("original", value)) or "original",
    )
    seen = []
    worker = threading.Thread(target=lambda: seen.append(dispatch("worker")))
    worker.start()
    worker.join(5)

    assert dispatch("owner") == "double"
    assert seen == ["original"]
    assert sorted(calls) == [("double", "owner"), ("original", "worker")]


def test_isolated_python_probe_excludes_concurrent_parent_allocations(tmp_path) -> None:
    from concurrent.futures import ThreadPoolExecutor
    import os
    import threading

    ready = tmp_path / "trace-started"
    polluted = tmp_path / "parent-allocation-held"
    stop = threading.Event()

    def pollute_parent() -> int:
        while not stop.wait(0.01):
            if ready.is_file():
                allocation = bytearray(8 * 1024 * 1024)
                polluted.write_text("held", encoding="utf-8")
                stop.wait()
                return len(allocation)
        return 0

    with ThreadPoolExecutor(max_workers=1) as pool:
        pollution = pool.submit(pollute_parent)
        try:
            observed = process_guard_common.run_isolated_python_probe(
                """
                import gc
                import json
                import os
                from pathlib import Path
                import sys
                import threading
                import time
                import tracemalloc
                from molt import exact_json

                payload = json.load(sys.stdin)
                ready, polluted = map(Path, sys.argv[1:])
                if tracemalloc.is_tracing():
                    raise RuntimeError("probe requires exclusive allocation tracing")
                gc.collect()
                tracemalloc.start()
                try:
                    ready.write_text("tracing", encoding="utf-8")
                    while not polluted.is_file():
                        time.sleep(0.01)
                    decoded = exact_json.loads_exact('{"value":7}')
                    _, peak = tracemalloc.get_traced_memory()
                finally:
                    tracemalloc.stop()
                print(json.dumps({
                    "pid": os.getpid(), "peak": peak, "decoded": decoded,
                    "payload": payload, "threads": threading.active_count(),
                    "pytest_imported": "pytest" in sys.modules,
                }))
                """,
                args=[ready, polluted],
                payload={"transport": "owned"},
            )
        finally:
            stop.set()
        assert pollution.result() == 8 * 1024 * 1024
    assert observed["pid"] != os.getpid()
    assert observed["threads"] == 1
    assert observed["pytest_imported"] is False
    assert observed["decoded"] == {"value": 7}
    assert observed["payload"] == {"transport": "owned"}
    assert observed["peak"] < 256 * 1024
