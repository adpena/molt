"""Queue-based handlers for the stdlib `logging` package."""

from __future__ import annotations

from typing import Any, cast

import logging as _logging

from _intrinsics import require_intrinsic as _require_intrinsic

# TODO(stdlib, owner:runtime, milestone:TL3, priority:P2, status:planned):
# Extend queue-backed logging handler parity for advanced listener lifecycle and
# queue edge cases after baseline stdlib queue support stabilizes.
_MOLT_LOGGING_RUNTIME_READY = _require_intrinsic(
    "molt_logging_runtime_ready", globals()
)

__all__ = ["QueueHandler", "QueueListener"]


class QueueHandler(_logging.Handler):
    def __init__(self, queue: Any) -> None:
        super().__init__()
        self.queue = queue

    def enqueue(self, record: _logging.LogRecord) -> None:
        put_nowait = getattr(self.queue, "put_nowait", None)
        if callable(put_nowait):
            put_nowait(record)
            return None
        put = getattr(self.queue, "put", None)
        if callable(put):
            put(record)
            return None
        raise TypeError("queue object does not support put/put_nowait")

    def prepare(self, record: _logging.LogRecord) -> _logging.LogRecord:
        return record

    def emit(self, record: _logging.LogRecord) -> None:
        self.enqueue(self.prepare(record))


class QueueListener:
    def __init__(
        self,
        queue: Any,
        *handlers: _logging.Handler,
        respect_handler_level: bool = False,
    ) -> None:
        self.queue = queue
        self.handlers: list[_logging.Handler] = list(handlers)
        self.respect_handler_level = bool(respect_handler_level)
        self._running = False

    def _dequeue(self) -> _logging.LogRecord | None:
        get_nowait = getattr(self.queue, "get_nowait", None)
        if callable(get_nowait):
            try:
                return cast(_logging.LogRecord, get_nowait())
            except Exception:
                return None
        get = getattr(self.queue, "get", None)
        if callable(get):
            try:
                return cast(_logging.LogRecord, get(False))
            except Exception:
                return None
        return None

    def _handle(self, record: _logging.LogRecord) -> None:
        for handler in self.handlers:
            if self.respect_handler_level and record.levelno < handler.level:
                continue
            handler.handle(record)

    def start(self) -> None:
        self._running = True

    def stop(self) -> None:
        while True:
            record = self._dequeue()
            if record is None:
                break
            self._handle(record)
        self._running = False
