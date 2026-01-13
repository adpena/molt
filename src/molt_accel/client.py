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
from molt_accel.framing import CancelledError, read_frame, write_frame


_STATUS_ERRORS: dict[str, type[Exception]] = {
    "InvalidInput": MoltInvalidInput,
    "Busy": MoltBusy,
    "Timeout": MoltTimeout,
    "Cancelled": MoltCancelled,
    "InternalError": MoltInternalError,
}

Hook = Callable[[dict[str, Any]], None]
CancelCheck = Callable[[], bool]


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
        self._lock = threading.Lock()
        self._next_id = 1

    def __enter__(self) -> "MoltClient":
        self._ensure_process()
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    def close(self) -> None:
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

    def _ensure_process(self) -> None:
        if self._proc is not None and self._proc.poll() is None:
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

    def call(
        self,
        *,
        entry: str,
        payload: Any,
        codec: str = "msgpack",
        timeout_ms: int = 250,
        idempotent: bool = False,
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
        before_send: Hook | None,
        after_recv: Hook | None,
        metrics_hook: Hook | None,
        cancel_check: CancelCheck | None,
    ) -> Any:
        self._ensure_process()
        if self._stdin is None or self._stdout is None:
            raise MoltWorkerUnavailable("Worker pipes unavailable")

        try:
            payload_bytes = encode_payload(payload, codec)
        except CodecUnavailableError as exc:
            raise MoltInvalidInput(str(exc)) from exc

        request_id = self._next_id
        self._next_id += 1
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
        with self._lock:
            try:
                write_frame(self._stdin, wire_payload, max_size=self._max_frame_size)
                response_bytes = read_frame(
                    self._stdout,
                    timeout=timeout_s,
                    max_size=self._max_frame_size,
                    cancel_check=cancel_check,
                )
            except CancelledError as exc:
                self._send_cancel_locked(request_id)
                self.close()
                raise MoltCancelled("Request cancelled") from exc
            except TimeoutError as exc:
                self._send_cancel_locked(request_id)
                self.close()
                raise MoltTimeout("Worker response timed out") from exc
            except EOFError as exc:
                raise MoltWorkerUnavailable("Worker closed the stream") from exc

        response = decode_message(response_bytes, self._wire)
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
        return decode_payload(payload_data, response_codec)

    def _send_cancel_locked(self, request_id: int) -> None:
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
            write_frame(self._stdin, wire_payload, max_size=self._max_frame_size)
        except Exception:
            return

    def ping(self, timeout_ms: int = 100) -> float:
        start = time.monotonic()
        self.call(entry="__ping__", payload=b"", codec="raw", timeout_ms=timeout_ms)
        return time.monotonic() - start
