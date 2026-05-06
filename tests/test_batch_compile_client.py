from __future__ import annotations

import io
import queue

import pytest

from tools.batch_compile_client import BatchCompileServerClient


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
    client._response_queue = queue.Queue()
    return client


def test_batch_compile_client_rejects_response_id_mismatch(monkeypatch) -> None:
    client = _bare_client()
    monkeypatch.setattr(client, "_readline", lambda timeout: '{"id": 2, "ok": true}')

    with pytest.raises(RuntimeError, match="response id mismatch"):
        client.request("ping", timeout=1.0)


def test_batch_compile_client_readline_timeout_is_bounded() -> None:
    client = _bare_client()

    with pytest.raises(TimeoutError, match="response timed out"):
        client._readline(0.01)
