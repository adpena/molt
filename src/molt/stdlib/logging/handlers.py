"""Handlers for the stdlib `logging` package."""

from __future__ import annotations

import io as _io
import os as _os
import time as _time
from typing import Any, cast

import logging as _logging


# TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend queue-backed logging handler parity for advanced listener lifecycle and queue edge cases after baseline stdlib queue support stabilizes.

__all__ = [
    "BaseRotatingHandler",
    "RotatingFileHandler",
    "TimedRotatingFileHandler",
    "QueueHandler",
    "QueueListener",
]


class BaseRotatingHandler(_logging.FileHandler):
    namer: Any = None
    rotator: Any = None

    def __init__(
        self,
        filename: Any,
        mode: str,
        encoding: str | None = None,
        delay: bool = False,
        errors: str | None = None,
    ) -> None:
        super().__init__(filename, mode, encoding, delay, errors)
        self.mode = mode
        self.encoding = encoding
        self.errors = errors

    def emit(self, record: _logging.LogRecord) -> None:
        try:
            if self.shouldRollover(record):
                self.doRollover()
            super().emit(record)
        except Exception:
            self.handleError(record)

    def rotation_filename(self, default_name: str) -> str:
        if not callable(self.namer):
            return default_name
        return self.namer(default_name)

    def rotate(self, source: str, dest: str) -> None:
        if not callable(self.rotator):
            if _os.path.exists(source):
                _os.rename(source, dest)
        else:
            self.rotator(source, dest)


class RotatingFileHandler(BaseRotatingHandler):
    def __init__(
        self,
        filename: Any,
        mode: str = "a",
        maxBytes: int = 0,
        backupCount: int = 0,
        encoding: str | None = None,
        delay: bool = False,
        errors: str | None = None,
    ) -> None:
        if maxBytes > 0:
            mode = "a"
        if "b" not in mode:
            encoding = _io.text_encoding(encoding)
        super().__init__(filename, mode, encoding, delay, errors)
        self.maxBytes = maxBytes
        self.backupCount = backupCount

    def shouldRollover(self, record: _logging.LogRecord) -> bool:
        if self.stream is None:
            self.stream = self._open()
        if self.maxBytes > 0:
            pos = self.stream.tell()
            if not pos:
                return False
            message = "%s\n" % self.format(record)
            if pos + len(message) >= self.maxBytes:
                if _os.path.exists(self.baseFilename) and not _os.path.isfile(
                    self.baseFilename
                ):
                    return False
                return True
        return False

    def doRollover(self) -> None:
        if self.stream:
            self.stream.close()
            self.stream = None
        if self.backupCount > 0:
            for index in range(self.backupCount - 1, 0, -1):
                source = self.rotation_filename("%s.%d" % (self.baseFilename, index))
                dest = self.rotation_filename("%s.%d" % (self.baseFilename, index + 1))
                if _os.path.exists(source):
                    if _os.path.exists(dest):
                        _os.remove(dest)
                    _os.rename(source, dest)
            dest = self.rotation_filename(self.baseFilename + ".1")
            if _os.path.exists(dest):
                _os.remove(dest)
            self.rotate(self.baseFilename, dest)
        if not self.delay:
            self.stream = self._open()


class TimedRotatingFileHandler(BaseRotatingHandler):
    _WHEN_MAP = {
        "S": (1, "%Y-%m-%d_%H-%M-%S"),
        "M": (60, "%Y-%m-%d_%H-%M"),
        "H": (3600, "%Y-%m-%d_%H"),
        "D": (86400, "%Y-%m-%d"),
        "MIDNIGHT": (86400, "%Y-%m-%d"),
    }

    def __init__(
        self,
        filename: Any,
        when: str = "h",
        interval: int = 1,
        backupCount: int = 0,
        encoding: str | None = None,
        delay: bool = False,
        utc: bool = False,
        atTime: Any = None,
        errors: str | None = None,
    ) -> None:
        encoding = _io.text_encoding(encoding)
        super().__init__(filename, "a", encoding, delay, errors)
        self.when = when.upper()
        entry = self._WHEN_MAP.get(self.when)
        if entry is None:
            raise ValueError("Invalid rollover interval specified: %s" % self.when)
        base_interval, self.suffix = entry
        self.interval = base_interval * max(interval, 1)
        self.backupCount = backupCount
        self.utc = utc
        self.atTime = atTime
        self.rolloverAt = self._computeRollover(_time.time())

    def _computeRollover(self, currentTime: float) -> float:
        return currentTime + self.interval

    def shouldRollover(self, record: _logging.LogRecord) -> bool:
        return _time.time() >= self.rolloverAt

    def getFilesToDelete(self) -> list[str]:
        dirName = _os.path.dirname(self.baseFilename)
        baseName = _os.path.basename(self.baseFilename)
        if not dirName:
            dirName = "."
        result: list[str] = []
        try:
            entries = _os.listdir(dirName)
        except OSError:
            return result
        prefix = baseName + "."
        for entry in entries:
            if entry[: len(prefix)] == prefix and len(entry) > len(prefix):
                result.append(_os.path.join(dirName, entry))
        result.sort()
        if len(result) <= self.backupCount:
            return []
        return result[: len(result) - self.backupCount]

    def doRollover(self) -> None:
        if self.stream is not None:
            self.stream.close()
            self.stream = None
        currentTime = int(_time.time())
        if self.utc:
            timeTuple = _time.gmtime(currentTime)
        else:
            timeTuple = _time.localtime(currentTime)
        dfn = self.rotation_filename(
            self.baseFilename + "." + _time.strftime(self.suffix, timeTuple)
        )
        if _os.path.exists(dfn):
            _os.remove(dfn)
        self.rotate(self.baseFilename, dfn)
        for s in self.getFilesToDelete():
            _os.remove(s)
        if not self.delay:
            self.stream = self._open()
        self.rolloverAt = self._computeRollover(currentTime)


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
