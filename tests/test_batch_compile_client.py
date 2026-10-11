from __future__ import annotations

import io
import queue
import threading
import time

import pytest

from tools import batch_compile_client
from tools.batch_compile_client import (
    BatchCompileProtocolError,
    BatchCompileServerClient,
)
import subprocess
from tests.process_guard_common import install_module_view


class _FakeProc:
    def __init__(self) -> None:
        self.stdin = io.StringIO()
        self.stderr = None

    def poll(self) -> None:
        return None


def _bare_client() -> BatchCompileServerClient:
    client = BatchCompileServerClient.__new__(BatchCompileServerClient)
    client._proc = _FakeProc()
    client._request_lock = threading.Lock()
    client._next_id = 1
    client._poisoned = False
    client._response_queue = queue.Queue()
    client.child_process = (
        batch_compile_client.harness_memory_guard.memory_guard.GuardedChildProcess(
            11, 11, 11, ("fixture",), "fixture", 1000
        )
    )
    return client


def test_batch_compile_client_rejects_response_id_mismatch(monkeypatch) -> None:
    client = _bare_client()
    monkeypatch.setattr(client, "_readline", lambda timeout: '{"id": 2, "ok": true}')

    with pytest.raises(BatchCompileProtocolError) as exc_info:
        client.request("ping", timeout=1.0)
    message = str(exc_info.value)
    assert "batch compile response id mismatch" in message
    assert "expected id 1" in message
    assert "got id 2" in message
    assert "op 'ping'" in message
    assert '{"id": 2, "ok": true}' in message
    assert client._poisoned is True


def test_batch_compile_client_requires_restart_after_response_timeout(
    monkeypatch,
) -> None:
    client = _bare_client()

    def _timeout(timeout):
        del timeout
        raise TimeoutError("batch compile server response timed out")

    monkeypatch.setattr(client, "_readline", _timeout)

    with pytest.raises(TimeoutError, match="response timed out"):
        client.request("ping", timeout=1.0)
    with pytest.raises(RuntimeError, match="restart required"):
        client.request("ping", timeout=1.0)


def test_batch_compile_client_readline_timeout_is_bounded() -> None:
    client = _bare_client()

    with pytest.raises(TimeoutError, match="response timed out"):
        client._readline(0.01)


def test_batch_request_custody_is_immutable_across_responses_and_errors(monkeypatch):
    client = _bare_client()
    times = iter((100, 200, 300))
    install_module_view(
        monkeypatch,
        "time",
        time,
        batch_compile_client,
        monotonic_ns=lambda: next(times),
    )
    replies = iter(
        (
            '{"id": 1, "ok": true, "returncode": 0}',
            '{"id": 2, "ok": false, "returncode": 5}',
        )
    )
    monkeypatch.setattr(client, "_readline", lambda timeout: next(replies))
    first = client.request("build", timeout=1)
    second = client.request("build", timeout=1)
    assert first.custody.request_started_at_ns == 100
    assert second.custody.request_started_at_ns == 200
    assert first.custody.returncode == 0
    assert second.custody.returncode == 5

    def closed(timeout):
        raise RuntimeError("server died before response")

    monkeypatch.setattr(client, "_readline", closed)
    with pytest.raises(RuntimeError) as error:
        client.request("build", timeout=1)
    assert error.value.batch_request_custody.request_started_at_ns == 300
    assert error.value.batch_request_custody.child_process.started_at_ns == 1000
    assert first.custody.request_started_at_ns == 100


def test_batch_compile_client_owns_guard_context_by_default(
    monkeypatch,
    tmp_path,
) -> None:
    events: list[tuple[str, object]] = []

    class FakeSentinel:
        def __exit__(self, exc_type, exc, tb) -> None:
            events.append(("sentinel_exit", exc_type))

    class FakeContext:
        env = {"PATH": "/usr/bin", "TMPDIR": str(tmp_path / "tmp")}

        class Limits:
            enabled = True

        limits = Limits()

        def process_group_kwargs(self) -> dict[str, object]:
            events.append(("process_group_kwargs", True))
            return {"start_new_session": True}

        def force_close_process_group(self, proc) -> None:
            events.append(("force_close", proc))

        def start_repo_sentinel(self, *, label: str, **kwargs):
            events.append(("sentinel_label", label))
            return FakeSentinel()

    def fake_from_env(prefix, env, *, repo_root):
        events.append(("from_env", (prefix, dict(env), repo_root)))
        return FakeContext()

    class FakeProc:
        def __init__(self, *args, **kwargs) -> None:
            events.append(("popen_kwargs", kwargs))
            self.pid = 12345
            self.stdin = io.StringIO()
            self.stdout = io.StringIO("")
            self.stderr = io.StringIO("")

        def poll(self) -> int | None:
            return 0

        def terminate(self) -> None:
            events.append(("terminate", True))

        def wait(self, timeout=None) -> int:
            return 0

    monkeypatch.setattr(
        batch_compile_client.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        fake_from_env,
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, batch_compile_client, Popen=FakeProc
    )
    guard = batch_compile_client.harness_memory_guard.memory_guard
    monkeypatch.setattr(guard._process_model, "process_started_at_ns", lambda pid: 1000)
    monkeypatch.setattr(
        guard, "windows_process_handle_started_at_ns", lambda handle: 1000
    )
    monkeypatch.setattr(guard, "_safe_getpgid", lambda pid: pid)
    monkeypatch.setattr(guard, "_safe_getsid", lambda pid: pid)

    client = BatchCompileServerClient(
        ["molt", "internal-batch-build-server"],
        cwd=tmp_path,
        env={"PATH": "/usr/bin"},
    )
    client.close(force=True)

    assert events[0][0] == "from_env"
    assert ("process_group_kwargs", True) in events
    assert ("sentinel_label", "molt_batch_server") in events
    popen_kwargs = next(value for key, value in events if key == "popen_kwargs")
    assert popen_kwargs["env"]["TMPDIR"] == str(tmp_path / "tmp")
    assert popen_kwargs["start_new_session"] is True
    assert any(key == "force_close" for key, _ in events)
    assert ("sentinel_exit", None) in events
