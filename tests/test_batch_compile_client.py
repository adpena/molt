from __future__ import annotations

import io
import queue

import pytest

from tools import batch_compile_client
from tools.batch_compile_client import (
    BatchCompileProtocolError,
    BatchCompileServerClient,
)


class _FakeProc:
    def __init__(self) -> None:
        self.stdin = io.StringIO()
        self.stderr = None

    def poll(self) -> None:
        return None


def _bare_client() -> BatchCompileServerClient:
    client = BatchCompileServerClient.__new__(BatchCompileServerClient)
    client._proc = _FakeProc()
    client._next_id = 1
    client._poisoned = False
    client._response_queue = queue.Queue()
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
    monkeypatch.setattr(batch_compile_client.subprocess, "Popen", FakeProc)

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
