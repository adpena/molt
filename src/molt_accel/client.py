from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
import subprocess
import threading
import time
from typing import Any, Callable, IO

from molt_accel.codec import (
    CodecUnavailableError,
    choose_wire,
    decode_message,
    decode_payload,
    encode_message,
    encode_payload,
)
from molt_accel.errors import (
    MoltBusy,
    MoltCancelled,
    MoltInternalError,
    MoltInvalidInput,
    MoltProtocolError,
    MoltTimeout,
    MoltWorkerUnavailable,
)
from molt_accel.framing import read_frame, write_frame


_STATUS_ERRORS: dict[str, type[Exception]] = {
    "InvalidInput": MoltInvalidInput,
    "Busy": MoltBusy,
    "Timeout": MoltTimeout,
    "Cancelled": MoltCancelled,
    "InternalError": MoltInternalError,
}

Hook = Callable[[dict[str, Any]], None]
CancelCheck = Callable[[], bool]

_POLL_INTERVAL = 0.01
_STARTUP_GRACE_MS = 1000


class _WorkerState(StrEnum):
    STARTING = "starting"
    READY = "ready"
    DEAD = "dead"
    REPLACED = "replaced"


@dataclass(eq=False)
class _Worker:
    proc: subprocess.Popen[bytes]
    state: _WorkerState = _WorkerState.STARTING
    reader: threading.Thread | None = None
    restart_after_drain: bool = False


class _PendingResponse:
    def __init__(self, proc: subprocess.Popen[bytes]) -> None:
        self.proc = proc
        self.event = threading.Event()
        self.response: dict[str, Any] | None = None
        self.error: BaseException | None = None

    def set_response(self, response: dict[str, Any]) -> None:
        self.response = response
        self.event.set()

    def set_error(self, error: BaseException) -> None:
        self.error = error
        self.event.set()


class MoltClient:
    """Blocking stdio client for the Molt worker (v0)."""

    def __init__(
        self,
        *,
        worker_cmd: list[str],
        wire: str | None = None,
        max_frame_size: int = 64 * 1024 * 1024,
        restart_on_failure: bool = True,
        max_restarts: int = 1,
        env: dict[str, str] | None = None,
        cwd: str | None = None,
    ) -> None:
        self._worker_cmd = list(worker_cmd)
        self._wire = choose_wire(wire)
        self._max_frame_size = max_frame_size
        self._restart_on_failure = restart_on_failure
        self._max_restarts = max(0, max_restarts)
        self._env = env
        self._cwd = cwd
        self._worker: _Worker | None = None
        self._proc_lock = threading.Lock()
        self._send_lock = threading.Lock()
        self._pending_lock = threading.Lock()
        self._pending: dict[int, _PendingResponse] = {}
        self._next_id = 1

    def __enter__(self) -> "MoltClient":
        self._ensure_process()
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    def close(self) -> None:
        with self._proc_lock:
            if self._worker is not None:
                self._close_locked(self._worker)

    def _transition_locked(
        self, worker: _Worker, state: _WorkerState, error: BaseException | None = None
    ) -> None:
        """The sole lifecycle transition authority; callers hold _proc_lock."""
        if worker.state is state:
            return
        allowed = {
            _WorkerState.STARTING: {
                _WorkerState.READY,
                _WorkerState.DEAD,
                _WorkerState.REPLACED,
            },
            _WorkerState.READY: {_WorkerState.DEAD, _WorkerState.REPLACED},
            _WorkerState.DEAD: {_WorkerState.REPLACED},
            _WorkerState.REPLACED: set(),
        }
        if state not in allowed[worker.state]:
            raise RuntimeError(f"invalid worker transition: {worker.state} -> {state}")
        worker.state = state
        if state in {_WorkerState.DEAD, _WorkerState.REPLACED}:
            assert error is not None
            with self._pending_lock:
                failed = [
                    self._pending.pop(request_id)
                    for request_id, pending in tuple(self._pending.items())
                    if pending.proc is worker.proc
                ]
            for pending in failed:
                pending.set_error(error)

    def _close_locked(self, worker: _Worker) -> None:
        self._transition_locked(
            worker, _WorkerState.REPLACED, MoltWorkerUnavailable("Worker closed")
        )
        try:
            if worker.proc.poll() is None:
                worker.proc.terminate()
                try:
                    worker.proc.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    worker.proc.kill()
                    worker.proc.wait(timeout=1)
        finally:
            try:
                if worker.proc.stdin is not None:
                    worker.proc.stdin.close()
            except BrokenPipeError:
                # Closing a buffered partial frame can flush after this
                # worker has died. Its requests already have terminal errors.
                pass
            finally:
                if self._worker is worker:
                    self._worker = None

    def _ensure_process(self) -> _Worker:
        with self._proc_lock:
            worker = self._worker
            if worker is not None:
                if (
                    worker.state in {_WorkerState.STARTING, _WorkerState.READY}
                    and worker.proc.poll() is None
                ):
                    return worker
                # A dead process can precede its reader's EOF. Retire its
                # requests and reader ownership before publishing a successor.
                self._close_locked(worker)
            try:
                proc = subprocess.Popen(
                    self._worker_cmd,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=None,
                    cwd=self._cwd,
                    env=self._env,
                )
            except OSError as exc:
                raise MoltWorkerUnavailable("Failed to start worker") from exc
            worker = _Worker(proc)
            self._worker = worker
            if proc.stdin is None or proc.stdout is None:
                self._close_locked(worker)
                raise MoltWorkerUnavailable("Worker pipes unavailable")
            worker.reader = threading.Thread(
                target=self._reader_loop, args=(worker, proc.stdout), daemon=True
            )
            worker.reader.start()
            return worker

    def _reader_loop(self, worker: _Worker, stdout: IO[bytes]) -> None:
        try:
            while True:
                try:
                    response_bytes = read_frame(stdout, max_size=self._max_frame_size)
                except EOFError:
                    self._set_reader_error(
                        worker, MoltWorkerUnavailable("Worker closed the stream")
                    )
                    return
                except Exception as exc:
                    self._set_reader_error(worker, MoltWorkerUnavailable(str(exc)))
                    return
                try:
                    response = decode_message(response_bytes, self._wire)
                except Exception as exc:
                    self._set_reader_error(worker, MoltProtocolError(str(exc)))
                    return
                request_id = response.get("request_id")
                if not isinstance(request_id, int):
                    self._set_reader_error(
                        worker, MoltProtocolError("Missing response id")
                    )
                    return
                with self._proc_lock:
                    if worker.state not in {_WorkerState.STARTING, _WorkerState.READY}:
                        return
                    self._transition_locked(worker, _WorkerState.READY)
                    with self._pending_lock:
                        pending = self._pending.get(request_id)
                        if pending is not None and pending.proc is worker.proc:
                            del self._pending[request_id]
                        else:
                            pending = None
                    if pending is not None:
                        pending.set_response(response)
        finally:
            stdout.close()

    def _set_reader_error(self, worker: _Worker, error: BaseException) -> None:
        with self._proc_lock:
            if worker.state is not _WorkerState.REPLACED:
                self._transition_locked(worker, _WorkerState.DEAD, error)

    def _wait_for_response(
        self,
        request_id: int,
        pending: _PendingResponse,
        worker: _Worker,
        timeout_s: float | None,
        cancel_check: CancelCheck | None,
    ) -> dict[str, Any]:
        deadline = None if timeout_s is None else time.monotonic() + timeout_s
        while True:
            if pending.event.wait(_POLL_INTERVAL):
                if pending.error is not None:
                    raise pending.error
                if pending.response is None:
                    raise MoltProtocolError("Missing response data")
                return pending.response
            if cancel_check is not None and cancel_check():
                self._abandon_pending(request_id)
                self._send_cancel(request_id, worker)
                raise MoltCancelled("Request cancelled")
            if deadline is not None and time.monotonic() >= deadline:
                self._abandon_pending(request_id)
                self._send_cancel(request_id, worker)
                self._mark_restart_after_drain(worker)
                raise MoltTimeout("Worker response timed out")

    def _abandon_pending(self, request_id: int) -> None:
        with self._pending_lock:
            self._pending.pop(request_id, None)

    def _mark_restart_after_drain(self, worker: _Worker) -> None:
        if not self._restart_on_failure:
            return
        with self._proc_lock:
            worker.restart_after_drain = True
            self._maybe_restart_locked(worker)

    def _maybe_restart_locked(self, worker: _Worker) -> None:
        if not worker.restart_after_drain:
            return
        with self._pending_lock:
            if any(pending.proc is worker.proc for pending in self._pending.values()):
                return
        self._close_locked(worker)

    def _maybe_restart_if_idle(self, worker: _Worker) -> None:
        with self._proc_lock:
            self._maybe_restart_locked(worker)

    def call(
        self,
        *,
        entry: str,
        payload: Any,
        codec: str = "msgpack",
        timeout_ms: int = 250,
        idempotent: bool = False,
        decode_response: bool = True,
        before_send: Hook | None = None,
        after_recv: Hook | None = None,
        metrics_hook: Hook | None = None,
        cancel_check: CancelCheck | None = None,
        retry_on_timeout: bool = False,
        retry_on_busy: bool = False,
        retry_backoff_ms: int = 0,
        retry_backoff_max_ms: int = 0,
    ) -> Any:
        attempts = 0
        backoff_ms = max(0, retry_backoff_ms)
        backoff_cap_ms = max(backoff_ms, retry_backoff_max_ms)
        while True:
            try:
                return self._call_once(
                    entry=entry,
                    payload=payload,
                    codec=codec,
                    timeout_ms=timeout_ms,
                    decode_response=decode_response,
                    before_send=before_send,
                    after_recv=after_recv,
                    metrics_hook=metrics_hook,
                    cancel_check=cancel_check,
                )
            except (MoltWorkerUnavailable, MoltTimeout, MoltBusy) as exc:
                retryable = (
                    isinstance(exc, MoltWorkerUnavailable)
                    or (retry_on_timeout and isinstance(exc, MoltTimeout))
                    or (retry_on_busy and isinstance(exc, MoltBusy))
                )
                if (
                    not idempotent
                    or not retryable
                    or not self._restart_on_failure
                    or attempts >= self._max_restarts
                ):
                    raise
                attempts += 1
                if backoff_ms > 0:
                    time.sleep(backoff_ms / 1000.0)
                    if backoff_cap_ms > 0:
                        backoff_ms = min(backoff_cap_ms, backoff_ms * 2)

    def _call_once(
        self,
        *,
        entry: str,
        payload: Any,
        codec: str,
        timeout_ms: int,
        decode_response: bool,
        before_send: Hook | None,
        after_recv: Hook | None,
        metrics_hook: Hook | None,
        cancel_check: CancelCheck | None,
    ) -> Any:
        worker = self._ensure_process()
        if timeout_ms > 0 and worker.state is _WorkerState.STARTING:
            # Every request sent before the worker first answers waits for its
            # startup, not only the request that started it.
            timeout_ms = max(timeout_ms, _STARTUP_GRACE_MS)

        try:
            payload_bytes = encode_payload(payload, codec)
        except CodecUnavailableError as exc:
            raise MoltInvalidInput(str(exc)) from exc

        pending = _PendingResponse(worker.proc)
        with self._proc_lock:
            if worker.state not in {_WorkerState.STARTING, _WorkerState.READY}:
                raise MoltWorkerUnavailable("Worker replaced before request admission")
            with self._pending_lock:
                request_id = self._next_id
                self._next_id += 1
                self._pending[request_id] = pending
        try:
            message = {
                "request_id": request_id,
                "entry": entry,
                "timeout_ms": timeout_ms,
                "codec": codec,
                "payload": payload_bytes,
            }
            wire_payload = encode_message(message, self._wire)
            if cancel_check is not None and cancel_check():
                raise MoltCancelled("Request cancelled before send")
            request_meta = {
                "request_id": request_id,
                "entry": entry,
                "codec": codec,
                "timeout_ms": timeout_ms,
                "payload_bytes": len(payload_bytes),
            }
            if before_send is not None:
                before_send(dict(request_meta))
            timeout_s = None if timeout_ms <= 0 else timeout_ms / 1000.0
            start = time.monotonic()
            try:
                with self._send_lock:
                    with self._proc_lock:
                        if worker.state not in {
                            _WorkerState.STARTING,
                            _WorkerState.READY,
                        }:
                            raise MoltWorkerUnavailable(
                                "Worker replaced before request send"
                            )
                        assert worker.proc.stdin is not None
                        stdin = worker.proc.stdin
                    write_frame(stdin, wire_payload, max_size=self._max_frame_size)
            except Exception as exc:
                self._set_reader_error(
                    worker, MoltWorkerUnavailable("Failed to send request")
                )
                raise MoltWorkerUnavailable("Failed to send request") from exc

            response = self._wait_for_response(
                request_id, pending, worker, timeout_s, cancel_check
            )
        finally:
            self._abandon_pending(request_id)
            self._maybe_restart_if_idle(worker)
        elapsed_ms = int((time.monotonic() - start) * 1000)
        if response.get("request_id") != request_id:
            raise MoltProtocolError("Mismatched response id")
        status = response.get("status", "InternalError")
        if status != "Ok":
            error_cls = _STATUS_ERRORS.get(status, MoltInternalError)
            raise error_cls(response.get("error", status))

        payload_data = response.get("payload", b"")
        if not isinstance(payload_data, (bytes, bytearray)):
            payload_data = b""
        response_codec = response.get("codec", codec)

        response_metrics = response.get("metrics")
        if isinstance(response_metrics, dict):
            response_metrics = dict(response_metrics)
        else:
            response_metrics = {}
        response_metrics.setdefault("payload_bytes", request_meta["payload_bytes"])
        response_metrics["response_bytes"] = len(payload_data)
        response_metrics["client_ms"] = elapsed_ms
        if metrics_hook is not None:
            metrics_hook(dict(response_metrics))
        if after_recv is not None:
            recv_meta: dict[str, object] = dict(request_meta)
            recv_meta.update(
                {
                    "status": status,
                    "client_ms": elapsed_ms,
                    "metrics": response_metrics,
                }
            )
            after_recv(recv_meta)

        if not decode_response:
            return payload_data
        return decode_payload(payload_data, response_codec)

    def _send_cancel(self, request_id: int, worker: _Worker) -> None:
        cancel_codec = "msgpack" if self._wire == "msgpack" else "json"
        try:
            payload = encode_payload({"request_id": request_id}, cancel_codec)
            message = {
                "request_id": request_id,
                "entry": "__cancel__",
                "timeout_ms": 0,
                "codec": cancel_codec,
                "payload": payload,
            }
            wire_payload = encode_message(message, self._wire)
            with self._send_lock:
                with self._proc_lock:
                    if worker.state not in {_WorkerState.STARTING, _WorkerState.READY}:
                        return
                    assert worker.proc.stdin is not None
                    stdin = worker.proc.stdin
                write_frame(stdin, wire_payload, max_size=self._max_frame_size)
        except (BrokenPipeError, OSError):
            # Worker process is gone — cancel cannot be delivered; this is
            # expected during shutdown or after a crash.  Other callers will
            # discover the dead worker on their next call().
            return
        except Exception as exc:
            import logging

            logging.getLogger("molt_accel").debug(
                "cancel request %d failed: %s", request_id, exc
            )
            return

    def ping(self, timeout_ms: int = 100) -> float:
        start = time.monotonic()
        self.call(entry="__ping__", payload=b"", codec="raw", timeout_ms=timeout_ms)
        return time.monotonic() - start


class MoltClientPool:
    """Round-robin pool of Molt workers for higher concurrency."""

    def __init__(
        self,
        *,
        worker_cmd: list[str],
        pool_size: int,
        wire: str | None = None,
        max_frame_size: int = 64 * 1024 * 1024,
        restart_on_failure: bool = True,
        max_restarts: int = 1,
        env: dict[str, str] | None = None,
        cwd: str | None = None,
    ) -> None:
        if pool_size < 1:
            raise ValueError("pool_size must be >= 1")
        self._clients = [
            MoltClient(
                worker_cmd=worker_cmd,
                wire=wire,
                max_frame_size=max_frame_size,
                restart_on_failure=restart_on_failure,
                max_restarts=max_restarts,
                env=env,
                cwd=cwd,
            )
            for _ in range(pool_size)
        ]
        self._lock = threading.Lock()
        self._next_index = 0

    @property
    def clients(self) -> tuple[MoltClient, ...]:
        return tuple(self._clients)

    def __enter__(self) -> "MoltClientPool":
        for client in self._clients:
            client.__enter__()
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    def close(self) -> None:
        for client in self._clients:
            client.close()

    def call(
        self,
        *,
        entry: str,
        payload: Any,
        codec: str = "msgpack",
        timeout_ms: int = 250,
        idempotent: bool = False,
        decode_response: bool = True,
        before_send: Hook | None = None,
        after_recv: Hook | None = None,
        metrics_hook: Hook | None = None,
        cancel_check: CancelCheck | None = None,
        retry_on_timeout: bool = False,
        retry_on_busy: bool = False,
        retry_backoff_ms: int = 0,
        retry_backoff_max_ms: int = 0,
    ) -> Any:
        with self._lock:
            client = self._clients[self._next_index]
            self._next_index = (self._next_index + 1) % len(self._clients)
        return client.call(
            entry=entry,
            payload=payload,
            codec=codec,
            timeout_ms=timeout_ms,
            idempotent=idempotent,
            decode_response=decode_response,
            before_send=before_send,
            after_recv=after_recv,
            metrics_hook=metrics_hook,
            cancel_check=cancel_check,
            retry_on_timeout=retry_on_timeout,
            retry_on_busy=retry_on_busy,
            retry_backoff_ms=retry_backoff_ms,
            retry_backoff_max_ms=retry_backoff_max_ms,
        )

    def ping(self, timeout_ms: int = 100) -> float:
        with self._lock:
            client = self._clients[self._next_index]
            self._next_index = (self._next_index + 1) % len(self._clients)
        return client.ping(timeout_ms=timeout_ms)
