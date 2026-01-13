from __future__ import annotations

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


class _PendingResponse:
    def __init__(self) -> None:
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
        self._proc: subprocess.Popen[bytes] | None = None
        self._stdin: IO[bytes] | None = None
        self._stdout: IO[bytes] | None = None
        self._proc_lock = threading.Lock()
        self._send_lock = threading.Lock()
        self._pending_lock = threading.Lock()
        self._pending: dict[int, _PendingResponse] = {}
        self._reader: threading.Thread | None = None
        self._reader_error: BaseException | None = None
        self._restart_after_drain = False
        self._next_id = 1

    def __enter__(self) -> "MoltClient":
        self._ensure_process()
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    def close(self) -> None:
        with self._proc_lock:
            self._close_locked()

    def _close_locked(self) -> None:
        if self._proc is None:
            return
        try:
            self._proc.terminate()
            self._proc.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            self._proc.wait(timeout=1)
        finally:
            self._proc = None
            self._stdin = None
            self._stdout = None
            self._reader = None
            self._reader_error = None
            self._restart_after_drain = False
            self._clear_pending(MoltWorkerUnavailable("Worker closed"))

    def _ensure_process(self) -> None:
        with self._proc_lock:
            if self._reader_error is not None:
                self._close_locked()
            if self._proc is not None and self._proc.poll() is None:
                self._ensure_reader()
                return
            try:
                self._proc = subprocess.Popen(
                    self._worker_cmd,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=None,
                    cwd=self._cwd,
                    env=self._env,
                )
            except OSError as exc:
                raise MoltWorkerUnavailable("Failed to start worker") from exc
            if self._proc.stdin is None or self._proc.stdout is None:
                raise MoltWorkerUnavailable("Worker pipes unavailable")
            self._stdin = self._proc.stdin
            self._stdout = self._proc.stdout
            self._ensure_reader()

    def _ensure_reader(self) -> None:
        if self._reader is not None and self._reader.is_alive():
            return
        self._reader = threading.Thread(target=self._reader_loop, daemon=True)
        self._reader.start()

    def _reader_loop(self) -> None:
        while True:
            stdout = self._stdout
            if stdout is None:
                return
            try:
                response_bytes = read_frame(stdout, max_size=self._max_frame_size)
            except EOFError:
                self._set_reader_error(
                    MoltWorkerUnavailable("Worker closed the stream")
                )
                return
            except Exception as exc:
                self._set_reader_error(MoltWorkerUnavailable(str(exc)))
                return
            try:
                response = decode_message(response_bytes, self._wire)
            except Exception as exc:
                self._set_reader_error(MoltProtocolError(str(exc)))
                return
            request_id = response.get("request_id")
            if not isinstance(request_id, int):
                self._set_reader_error(MoltProtocolError("Missing response id"))
                return
            pending = self._pop_pending(request_id)
            if pending is None:
                continue
            pending.set_response(response)

    def _set_reader_error(self, error: BaseException) -> None:
        self._reader_error = error
        self._clear_pending(error)

    def _clear_pending(self, error: BaseException) -> None:
        with self._pending_lock:
            pending = list(self._pending.values())
            self._pending.clear()
        for item in pending:
            item.set_error(error)

    def _pop_pending(self, request_id: int) -> _PendingResponse | None:
        with self._pending_lock:
            return self._pending.pop(request_id, None)

    def _wait_for_response(
        self,
        request_id: int,
        pending: _PendingResponse,
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
                self._send_cancel(request_id)
                raise MoltCancelled("Request cancelled")
            if deadline is not None and time.monotonic() >= deadline:
                self._abandon_pending(request_id)
                self._send_cancel(request_id)
                self._mark_restart_after_drain()
                raise MoltTimeout("Worker response timed out")
            if self._reader_error is not None:
                raise MoltWorkerUnavailable(
                    "Worker reader failed"
                ) from self._reader_error

    def _abandon_pending(self, request_id: int) -> None:
        with self._pending_lock:
            self._pending.pop(request_id, None)

    def _mark_restart_after_drain(self) -> None:
        if not self._restart_on_failure:
            return
        with self._pending_lock:
            if self._pending:
                self._restart_after_drain = True
                return
        self.close()

    def _maybe_restart_if_idle(self) -> None:
        if not self._restart_after_drain:
            return
        with self._pending_lock:
            if self._pending:
                return
            self._restart_after_drain = False
        self.close()

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
    ) -> Any:
        attempts = 0
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
            except MoltWorkerUnavailable:
                if (
                    not idempotent
                    or not self._restart_on_failure
                    or attempts >= self._max_restarts
                ):
                    raise
                attempts += 1
                self.close()

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
        self._ensure_process()
        if self._stdin is None or self._stdout is None:
            raise MoltWorkerUnavailable("Worker pipes unavailable")
        if self._reader_error is not None:
            raise MoltWorkerUnavailable("Worker reader failed") from self._reader_error

        try:
            payload_bytes = encode_payload(payload, codec)
        except CodecUnavailableError as exc:
            raise MoltInvalidInput(str(exc)) from exc

        pending = _PendingResponse()
        with self._pending_lock:
            request_id = self._next_id
            self._next_id += 1
            self._pending[request_id] = pending
        message = {
            "request_id": request_id,
            "entry": entry,
            "timeout_ms": timeout_ms,
            "codec": codec,
            "payload": payload_bytes,
        }
        wire_payload = encode_message(message, self._wire)
        if cancel_check is not None and cancel_check():
            self._abandon_pending(request_id)
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
                write_frame(self._stdin, wire_payload, max_size=self._max_frame_size)
        except Exception as exc:
            self._abandon_pending(request_id)
            raise MoltWorkerUnavailable("Failed to send request") from exc

        try:
            response = self._wait_for_response(
                request_id, pending, timeout_s, cancel_check
            )
        finally:
            self._maybe_restart_if_idle()
        elapsed_ms = int((time.monotonic() - start) * 1000)
        if response.get("request_id") != request_id:
            raise MoltProtocolError("Mismatched response id")
        status = response.get("status", "InternalError")
        if status != "Ok":
            error_cls = _STATUS_ERRORS.get(status, MoltInternalError)
            raise error_cls(response.get("error", status))

        response_metrics = response.get("metrics")
        if isinstance(response_metrics, dict):
            response_metrics = dict(response_metrics)
        else:
            response_metrics = {}
        response_metrics["client_ms"] = elapsed_ms
        if metrics_hook is not None:
            metrics_hook(dict(response_metrics))
        if after_recv is not None:
            recv_meta = dict(request_meta)
            recv_meta.update(
                {
                    "status": status,
                    "client_ms": elapsed_ms,
                    "metrics": response_metrics,
                }
            )
            after_recv(recv_meta)

        payload_data = response.get("payload", b"")
        response_codec = response.get("codec", codec)
        if not decode_response:
            return payload_data
        return decode_payload(payload_data, response_codec)

    def _send_cancel(self, request_id: int) -> None:
        if self._stdin is None:
            return
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
                write_frame(self._stdin, wire_payload, max_size=self._max_frame_size)
        except Exception:
            return

    def ping(self, timeout_ms: int = 100) -> float:
        start = time.monotonic()
        self.call(entry="__ping__", payload=b"", codec="raw", timeout_ms=timeout_ms)
        return time.monotonic() - start
