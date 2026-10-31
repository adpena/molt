from __future__ import annotations

import os
import sys
import threading
import time
from pathlib import Path

import pytest

import molt_accel.client as client_module
from molt_accel.client import MoltClient, MoltClientPool
from molt_accel.codec import decode_payload, encode_payload
from molt_accel.errors import (
    MoltBusy,
    MoltCancelled,
    MoltInvalidInput,
    MoltTimeout,
    MoltWorkerUnavailable,
)
from tests.process_guard_common import install_module_view


def _worker_cmd() -> list[str]:
    root = Path(__file__).resolve().parents[1]
    worker = root / "tests" / "fixtures" / "molt_worker_stub.py"
    return [sys.executable, "-u", str(worker)]


def _worker_env() -> dict[str, str]:
    root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_WORKER_WIRE"] = "json"
    return env


@pytest.mark.slow
def test_client_echo_roundtrip() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    payload = {"ok": True, "count": 3}
    result = client.call(entry="echo", payload=payload, codec="json", timeout_ms=500)
    assert result == payload
    client.close()


@pytest.mark.parametrize("replace", [False, True])
@pytest.mark.slow
def test_dead_worker_buffered_stdin_cleanup_preserves_replacement(
    replace: bool,
) -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    worker = client._ensure_process()
    try:
        assert worker.proc.stdin is not None
        # A partial buffered frame can remain when a worker dies during send.
        worker.proc.stdin.write(b"partial frame")
        worker.proc.terminate()
        worker.proc.wait(timeout=5)
        if replace:
            successor = client._ensure_process()
            assert successor is not worker
            assert client.call(
                entry="echo", payload={"alive": True}, codec="json", timeout_ms=500
            ) == {"alive": True}
            assert client._worker is successor
        else:
            client.close()
            assert client._worker is None
        assert worker.proc.stdin.closed
    finally:
        client.close()


@pytest.mark.slow
def test_client_unknown_entry() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    with pytest.raises(MoltInvalidInput):
        client.call(entry="missing", payload={}, codec="json", timeout_ms=500)
    client.close()


@pytest.mark.slow
def test_client_hooks_and_metrics() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    events: list[str] = []
    metrics: dict[str, float] = {}

    def before_send(meta: dict[str, object]) -> None:
        events.append(f"before:{meta.get('entry')}")

    def after_recv(meta: dict[str, object]) -> None:
        events.append(f"after:{meta.get('status')}")

    def on_metrics(meta: dict[str, object]) -> None:
        for key in ("client_ms", "payload_bytes", "response_bytes"):
            value = meta.get(key)
            if isinstance(value, (int, float)):
                metrics[key] = float(value)

    result = client.call(
        entry="echo",
        payload={"ok": True},
        codec="json",
        timeout_ms=500,
        before_send=before_send,
        after_recv=after_recv,
        metrics_hook=on_metrics,
    )
    assert result == {"ok": True}
    assert events[0] == "before:echo"
    assert events[-1] == "after:Ok"
    assert "client_ms" in metrics
    assert "payload_bytes" in metrics
    assert "response_bytes" in metrics
    client.close()


@pytest.mark.parametrize("hook", ["before_send", "cancel_check"])
@pytest.mark.slow
def test_client_hook_failure_releases_request_for_timeout_restart(
    hook: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(client_module, "_STARTUP_GRACE_MS", 0)
    env = _worker_env()
    env["MOLT_STUB_STARTUP_DELAY_S"] = "30"
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=env)

    def fail(*_):
        raise ValueError("hook failed")

    try:
        with pytest.raises(ValueError, match="hook failed"):
            client.call(entry="echo", payload={}, codec="json", **{hook: fail})
        worker = client._worker
        assert worker is not None
        with pytest.raises(MoltTimeout):
            client.call(entry="echo", payload={}, codec="json", timeout_ms=20)
        # The aborted hook sent no request. It must not keep this process
        # alive after its only real request times out and drains.
        worker.proc.wait(timeout=1)
    finally:
        client.close()


@pytest.mark.slow
def test_client_cancel_check() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    with pytest.raises(MoltCancelled):
        client.call(
            entry="echo",
            payload={"ok": True},
            codec="json",
            timeout_ms=500,
            cancel_check=lambda: True,
        )
    client.close()


def test_client_retry_on_timeout_with_backoff(monkeypatch: pytest.MonkeyPatch) -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    calls = {"count": 0}
    sleeps: list[float] = []

    def fake_call_once(**_kwargs: object) -> dict[str, bool]:
        calls["count"] += 1
        if calls["count"] == 1:
            raise MoltTimeout("slow")
        return {"ok": True}

    monkeypatch.setattr(client, "_call_once", fake_call_once)
    install_module_view(monkeypatch, "time", time, client_module, sleep=sleeps.append)

    result = client.call(
        entry="echo",
        payload={"ok": True},
        codec="json",
        timeout_ms=500,
        idempotent=True,
        retry_on_timeout=True,
        retry_backoff_ms=7,
        retry_backoff_max_ms=20,
    )
    assert result == {"ok": True}
    assert calls["count"] == 2
    assert sleeps == [0.007]


def test_client_retry_on_busy_opt_in(monkeypatch: pytest.MonkeyPatch) -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    calls = {"count": 0}

    def fake_call_once(**_kwargs: object) -> dict[str, bool]:
        calls["count"] += 1
        if calls["count"] == 1:
            raise MoltBusy("busy")
        return {"ok": True}

    monkeypatch.setattr(client, "_call_once", fake_call_once)
    monkeypatch.setattr(client, "close", lambda: None)

    result = client.call(
        entry="echo",
        payload={"ok": True},
        codec="json",
        timeout_ms=500,
        idempotent=True,
        retry_on_busy=True,
    )
    assert result == {"ok": True}
    assert calls["count"] == 2


def test_client_timeout_not_retried_by_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())

    def fake_call_once(**_kwargs: object) -> dict[str, bool]:
        raise MoltTimeout("slow")

    monkeypatch.setattr(client, "_call_once", fake_call_once)
    with pytest.raises(MoltTimeout):
        client.call(
            entry="echo",
            payload={"ok": True},
            codec="json",
            timeout_ms=500,
            idempotent=True,
        )


@pytest.mark.slow
def test_client_decode_response_false() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    payload = {"ok": True, "count": 3}
    result = client.call(
        entry="echo",
        payload=payload,
        codec="json",
        timeout_ms=500,
        decode_response=False,
    )
    assert isinstance(result, (bytes, bytearray))
    assert decode_payload(result, "json") == payload
    client.close()


def test_arrow_ipc_codec_passthrough() -> None:
    payload = b"arrow-bytes"
    assert encode_payload(payload, "arrow_ipc") == payload
    assert decode_payload(payload, "arrow_ipc") == payload


@pytest.mark.slow
def test_client_concurrent_calls() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    results: list[int] = []
    errors: list[BaseException] = []
    lock = threading.Lock()

    def worker(idx: int) -> None:
        try:
            payload = {"id": idx}
            response = client.call(
                entry="echo",
                payload=payload,
                codec="json",
                timeout_ms=500,
            )
            with lock:
                results.append(response["id"])
        except BaseException as exc:
            with lock:
                errors.append(exc)

    threads = [threading.Thread(target=worker, args=(idx,)) for idx in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not errors
    assert sorted(results) == [0, 1, 2, 3]
    client.close()


@pytest.mark.slow
def test_startup_grace_covers_every_request_sent_before_first_response(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Worker startup outlasts the request budget but not the startup grace.
    # Every concurrent request reaches the worker before it can answer, so
    # each one waits for startup, not only the request that started it.
    monkeypatch.setattr(client_module, "_STARTUP_GRACE_MS", 60_000)
    env = _worker_env()
    env["MOLT_STUB_STARTUP_DELAY_S"] = "1.0"
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=env)
    barrier = threading.Barrier(4)
    results: list[int] = []
    errors: list[BaseException] = []
    lock = threading.Lock()

    def worker(idx: int) -> None:
        try:
            barrier.wait()
            response = client.call(
                entry="echo", payload={"id": idx}, codec="json", timeout_ms=250
            )
            with lock:
                results.append(response["id"])
        except BaseException as exc:
            with lock:
                errors.append(exc)

    threads = [threading.Thread(target=worker, args=(idx,)) for idx in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    try:
        assert errors == []
        assert sorted(results) == [0, 1, 2, 3]
    finally:
        client.close()


@pytest.mark.slow
def test_client_pool_round_robin() -> None:
    pool = MoltClientPool(
        worker_cmd=_worker_cmd(), wire="json", env=_worker_env(), pool_size=2
    )
    first = pool.call(entry="echo", payload={"id": 1}, codec="json", timeout_ms=500)
    second = pool.call(entry="echo", payload={"id": 2}, codec="json", timeout_ms=500)
    assert first["id"] == 1
    assert second["id"] == 2
    clients = pool.clients
    assert len(clients) == 2
    assert clients[0]._worker is not None
    assert clients[1]._worker is not None
    pool.close()


@pytest.mark.parametrize("retry", [False, True])
@pytest.mark.slow
def test_dead_worker_replacement_fails_its_requests_before_delayed_eof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, retry: bool
) -> None:
    # Hold A's EOF after the real process dies. B must get its own reader,
    # and A's eventual EOF must not fail B's in-flight request.
    reader_at_eof = threading.Event()
    release_eof = threading.Event()
    first_done = threading.Event()
    second_sent = threading.Event()
    second_done = threading.Event()
    retry_sent = threading.Event()
    successor_response = threading.Event()
    release_response = threading.Event()
    first_stream = []
    read_frame = client_module.read_frame

    def delayed_eof(stream, **kwargs):
        if not first_stream:
            first_stream.append(stream)
        try:
            response = read_frame(stream, **kwargs)
        except EOFError:
            if not reader_at_eof.is_set():
                reader_at_eof.set()
                assert release_eof.wait(10), "test did not release old reader"
            raise
        if stream is not first_stream[0]:
            successor_response.set()
            assert release_response.wait(10), "test did not release successor response"
        return response

    monkeypatch.setattr(client_module, "read_frame", delayed_eof)
    env = _worker_env()
    env["MOLT_STUB_FAIL_ONCE_ENTRY"] = "echo"
    env["MOLT_STUB_FAIL_ONCE_SENTINEL"] = str(tmp_path / "failed-once")
    env["MOLT_STUB_STARTUP_DELAY_S"] = "0.1"
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=env, max_restarts=1)
    outcomes: dict[str, object] = {}

    def first_before_send(_):
        if reader_at_eof.is_set():
            retry_sent.set()

    def call_worker(name, done, before_send=None):
        try:
            outcomes[name] = client.call(
                entry="echo",
                payload={"owner": name},
                codec="json",
                timeout_ms=10_000,
                before_send=before_send,
                idempotent=retry and name == "A",
            )
        except BaseException as exc:
            outcomes[name] = exc
        finally:
            done.set()

    first = threading.Thread(
        target=call_worker, args=("A", first_done, first_before_send)
    )
    second = threading.Thread(
        target=call_worker,
        args=("B", second_done, lambda _: second_sent.set()),
    )
    first.start()
    try:
        assert reader_at_eof.wait(10), "worker A did not reach EOF"
        old_worker = client._worker
        assert old_worker is not None
        old_proc = old_worker.proc
        assert old_proc is not None
        assert old_proc.wait(timeout=5) == 89
        replacement = client._ensure_process()
        assert replacement is not old_worker
        second.start()
        assert second_sent.wait(5), "worker B request was not sent"
        if retry:
            assert retry_sent.wait(5), "replacing A did not unblock its retry"
        else:
            assert first_done.wait(1), "replacing A left its request pending"
            assert isinstance(outcomes["A"], MoltWorkerUnavailable)
        assert successor_response.wait(5), "worker B did not get its own reader"
        release_eof.set()
        assert old_worker.reader is not None
        old_worker.reader.join(timeout=5)
        assert not old_worker.reader.is_alive()
        assert client._worker is replacement, "A's retry retired its successor"
        assert not second_done.is_set(), "A's EOF failed B's pending request"
        release_response.set()
        assert second_done.wait(5), "worker B did not get its own reader"
        assert outcomes["B"] == {"owner": "B"}
        if retry:
            assert first_done.wait(5)
            assert outcomes["A"] == {"owner": "A"}
    finally:
        release_eof.set()
        release_response.set()
        client.close()
        first.join(timeout=5)
        if second.ident is not None:
            second.join(timeout=5)
        assert not first.is_alive()
        assert not second.is_alive()
