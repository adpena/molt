"""Deterministic logging module for Molt."""

from __future__ import annotations

from typing import Any, Callable, TextIO, cast
from types import ModuleType

import io as _io
import os as _os
import string as _string
import sys as _sys
import time as _time
import traceback as _traceback
import warnings as _warnings

from _intrinsics import require_intrinsic as _require_intrinsic

# ── Intrinsic bindings ───────────────────────────────────────────────────────

_LOGGING_PERCENT_STYLE_FORMAT = cast(
    Callable[[str, dict[str, Any]], str],
    _require_intrinsic("molt_logging_percent_style_format"),
)

# LogRecord intrinsics
_RECORD_NEW = _require_intrinsic("molt_logging_record_new")
_RECORD_GET_MESSAGE = _require_intrinsic("molt_logging_record_get_message")
_RECORD_GET_ATTR = _require_intrinsic("molt_logging_record_get_attr")
_RECORD_DROP = _require_intrinsic("molt_logging_record_drop")

# Formatter intrinsics
_FORMATTER_NEW = _require_intrinsic("molt_logging_formatter_new")
_FORMATTER_FORMAT = _require_intrinsic("molt_logging_formatter_format")
_FORMATTER_FORMAT_TIME = _require_intrinsic("molt_logging_formatter_format_time")
_FORMATTER_DROP = _require_intrinsic("molt_logging_formatter_drop")

# Handler intrinsics
_HANDLER_NEW = _require_intrinsic("molt_logging_handler_new")
_HANDLER_EMIT = _require_intrinsic("molt_logging_handler_emit")
_HANDLER_SET_LEVEL = _require_intrinsic("molt_logging_handler_set_level")
_HANDLER_SET_FORMATTER = _require_intrinsic("molt_logging_handler_set_formatter")
_HANDLER_FLUSH = _require_intrinsic("molt_logging_handler_flush")
_HANDLER_CLOSE = _require_intrinsic("molt_logging_handler_close")
_HANDLER_DROP = _require_intrinsic("molt_logging_handler_drop")

# StreamHandler intrinsics
_STREAM_HANDLER_NEW = _require_intrinsic("molt_logging_stream_handler_new")
_STREAM_HANDLER_EMIT = _require_intrinsic("molt_logging_stream_handler_emit")

# Logger intrinsics
_LOGGER_NEW = _require_intrinsic("molt_logging_logger_new")
_LOGGER_SET_LEVEL = _require_intrinsic("molt_logging_logger_set_level")
_LOGGER_ADD_HANDLER = _require_intrinsic("molt_logging_logger_add_handler")
_LOGGER_REMOVE_HANDLER = _require_intrinsic("molt_logging_logger_remove_handler")
_LOGGER_LOG = _require_intrinsic("molt_logging_logger_log")
_LOGGER_IS_ENABLED_FOR = _require_intrinsic("molt_logging_logger_is_enabled_for")
_LOGGER_GET_EFFECTIVE_LEVEL = _require_intrinsic(
    "molt_logging_logger_get_effective_level"
)
_LOGGER_DROP = _require_intrinsic("molt_logging_logger_drop")

# Manager / root intrinsics
_MANAGER_GET_LOGGER = _require_intrinsic("molt_logging_manager_get_logger")
_ROOT_LOGGER = _require_intrinsic("molt_logging_root_logger")

# Config / shutdown intrinsics
_BASIC_CONFIG = _require_intrinsic("molt_logging_basic_config")
_SHUTDOWN = _require_intrinsic("molt_logging_shutdown")

# Level utility intrinsics
_GET_LEVEL_NAME = _require_intrinsic("molt_logging_get_level_name")
_ADD_LEVEL_NAME = _require_intrinsic("molt_logging_add_level_name")
_LEVEL_TO_INT = _require_intrinsic("molt_logging_level_to_int")

# Filter intrinsic
_FILTER_CHECK = _require_intrinsic("molt_logging_filter_check")

# FileHandler intrinsic
_FILE_HANDLER_EMIT = _require_intrinsic("molt_logging_file_handler_emit")

# Logging runtime readiness check
_LOGGING_RUNTIME_READY = _require_intrinsic("molt_logging_runtime_ready")

# Capability gate
_CAP_REQUIRE = _require_intrinsic("molt_capabilities_require")

# Lock intrinsics (for handler synchronization)
_RLOCK_NEW = _require_intrinsic("molt_rlock_new")
_RLOCK_ACQUIRE = _require_intrinsic("molt_rlock_acquire")
_RLOCK_RELEASE = _require_intrinsic("molt_rlock_release")
_RLOCK_LOCKED = _require_intrinsic("molt_rlock_locked")
_RLOCK_DROP = _require_intrinsic("molt_rlock_drop")

# Thread/process intrinsics for LogRecord metadata
_THREAD_CURRENT_IDENT = _require_intrinsic("molt_thread_current_ident")
_GETPID = _require_intrinsic("molt_getpid")

__all__ = [
    "BASIC_FORMAT",
    "CRITICAL",
    "DEBUG",
    "ERROR",
    "FATAL",
    "INFO",
    "NOTSET",
    "WARN",
    "WARNING",
    "basicConfig",
    "captureWarnings",
    "critical",
    "debug",
    "error",
    "exception",
    "fatal",
    "getLevelName",
    "getLogger",
    "getLoggerClass",
    "info",
    "log",
    "makeLogRecord",
    "setLoggerClass",
    "shutdown",
    "warn",
    "warning",
    "addLevelName",
    "Filter",
    "Formatter",
    "Handler",
    "LogRecord",
    "Logger",
    "LoggerAdapter",
    "NullHandler",
    "StreamHandler",
    "FileHandler",
    "BaseRotatingHandler",
    "TimedRotatingFileHandler",
    "QueueHandler",
    "QueueListener",
    "handlers",
]

# Allow `import logging.handlers` to treat this module as package-like.
__path__ = [_os.path.dirname(__file__)]  # type: ignore[var-annotated]


CRITICAL = 50
ERROR = 40
WARNING = 30
WARN = WARNING
INFO = 20
DEBUG = 10
NOTSET = 0
FATAL = CRITICAL

BASIC_FORMAT = "%(levelname)s:%(name)s:%(message)s"

_level_to_name = {
    CRITICAL: "CRITICAL",
    ERROR: "ERROR",
    WARNING: "WARNING",
    INFO: "INFO",
    DEBUG: "DEBUG",
    NOTSET: "NOTSET",
}
_name_to_level = {name: level for level, name in _level_to_name.items()}

_handler_list: list["Handler"] = []
_start_time = 0.0


def _init_start_time() -> float:
    try:
        return float(_time.time())
    except Exception:
        return 0.0


_start_time = _init_start_time()


def _require_fs_write() -> None:
    if _CAP_REQUIRE is None:
        return None
    _CAP_REQUIRE("fs.write")


def addLevelName(level: int, level_name: str) -> None:
    _ADD_LEVEL_NAME(level, level_name)
    _level_to_name[level] = level_name
    _name_to_level[level_name] = level


def getLevelName(level: int | str) -> str | int:
    if isinstance(level, int):
        return str(_GET_LEVEL_NAME(level))
    if isinstance(level, str):
        return _name_to_level.get(level, level)
    return _level_to_name.get(level, f"Level {level}")


def _check_level(level: int | str | None) -> int:
    if level is None:
        return NOTSET
    if isinstance(level, int):
        return int(level)
    if isinstance(level, str):
        if level.isdigit():
            return int(level)
        try:
            return int(_LEVEL_TO_INT(level))
        except Exception:
            resolved = _name_to_level.get(level)
            if resolved is None:
                raise ValueError(f"Unknown level: {level}")
            return resolved
    raise TypeError("Level must be an int or str")


def _percent_fallback(fmt: str, mapping: dict[str, Any]) -> str:
    return _LOGGING_PERCENT_STYLE_FORMAT(fmt, mapping)


class _RLock:
    def __init__(self) -> None:
        self._handle = _RLOCK_NEW()

    def acquire(self, blocking: bool = True, timeout: float = -1.0) -> bool:
        return bool(_RLOCK_ACQUIRE(self._handle, blocking, timeout))

    def release(self) -> None:
        _RLOCK_RELEASE(self._handle)

    def locked(self) -> bool:
        return bool(_RLOCK_LOCKED(self._handle))

    def __enter__(self) -> "_RLock":
        self.acquire()
        return self

    def __exit__(self, _exc_type, _exc, _tb) -> None:  # type: ignore[no-untyped-def]
        self.release()

    def __del__(self) -> None:
        try:
            if _RLOCK_DROP is not None:
                _RLOCK_DROP(self._handle)
        except Exception:
            return


class Filter:
    def __init__(self, name: str = "") -> None:
        self.name = name
        self.nlen = len(name)

    def filter(self, record: "LogRecord") -> bool:
        return bool(_FILTER_CHECK(self.name, record.name))


class Filterer:
    def __init__(self) -> None:
        self.filters: list[Filter] = []

    def addFilter(self, filt: Filter) -> None:
        if filt not in self.filters:
            self.filters.append(filt)

    def removeFilter(self, filt: Filter) -> None:
        if filt in self.filters:
            self.filters.remove(filt)

    def filter(self, record: "LogRecord") -> bool:
        for filt in self.filters:
            try:
                if not filt.filter(record):
                    return False
            except Exception:
                return False
        return True


class LogRecord:
    def __init__(
        self,
        name: str,
        level: int,
        pathname: str,
        lineno: int,
        msg: Any,
        args: Any,
        exc_info: Any,
        func: str | None = None,
        sinfo: str | None = None,
    ) -> None:
        # Create a Rust-side record handle.
        self._handle = _RECORD_NEW(
            name,
            level,
            pathname,
            lineno,
            str(msg) if msg is not None else "",
            str(args) if args else "",
            str(exc_info) if exc_info else "",
        )
        # Keep Python-side attributes for compatibility with __dict__ access,
        # extra fields from makeRecord, and direct attribute writes.
        self.name = name
        self.msg = msg
        self.args = args
        self.levelno = int(level)
        self.levelname = str(_GET_LEVEL_NAME(level))
        self.pathname = pathname
        self.filename = _os.path.basename(pathname)
        self.module = _os.path.splitext(self.filename)[0]
        self.lineno = int(lineno)
        self.funcName = func
        self.created = _init_start_time()
        try:
            self.msecs = (self.created - int(self.created)) * 1000.0
        except Exception:
            self.msecs = 0.0
        self.relativeCreated = (self.created - _start_time) * 1000.0
        self.exc_info = exc_info
        self.exc_text = None
        self.stack_info = sinfo
        thread_ident = int(_THREAD_CURRENT_IDENT())
        self.thread = thread_ident
        _main_ident = int(_THREAD_CURRENT_IDENT())
        if thread_ident == _main_ident:
            self.threadName = "MainThread"
        else:
            self.threadName = f"Thread-{thread_ident}"
        process_id = int(_GETPID())
        self.process = process_id
        self.processName = "MainProcess"
        self.message: str | None = None
        self.asctime: str | None = None

    def getMessage(self) -> str:
        try:
            result = _RECORD_GET_MESSAGE(self._handle)
            if result is not None and str(result):
                return str(result)
        except Exception:
            pass
        # Fallback for records with Python-side args not captured by intrinsic.
        msg = self.msg
        if isinstance(msg, str):
            if self.args:
                try:
                    return msg % self.args
                except Exception:
                    return msg
            return msg
        try:
            return str(msg)
        except Exception:
            return "<unprintable message>"

    def __del__(self) -> None:
        try:
            if _RECORD_DROP is not None:
                _RECORD_DROP(self._handle)
        except Exception:
            return


_RECORD_FORMAT_FIELDS = (
    "name",
    "msg",
    "args",
    "levelname",
    "levelno",
    "pathname",
    "filename",
    "module",
    "lineno",
    "funcName",
    "created",
    "msecs",
    "relativeCreated",
    "thread",
    "threadName",
    "process",
    "processName",
    "message",
    "asctime",
    "exc_text",
    "stack_info",
)


def _record_format_mapping(record: LogRecord) -> dict[str, Any]:
    mapping: dict[str, Any] = {}
    raw = getattr(record, "__dict__", None)
    if isinstance(raw, dict):
        mapping.update(raw)
    for key in _RECORD_FORMAT_FIELDS:
        if key in mapping:
            continue
        try:
            mapping[key] = getattr(record, key)
        except Exception:
            continue
    return mapping


class _Style:
    default_format = "%(message)s"
    asctime_format = "%(asctime)s"

    def __init__(self, fmt: str | None) -> None:
        self._fmt = fmt or self.default_format

    def usesTime(self) -> bool:
        return False

    def format(self, record: LogRecord) -> str:
        return self._fmt


class PercentStyle(_Style):
    def usesTime(self) -> bool:
        return "%(asctime)" in self._fmt

    def format(self, record: LogRecord) -> str:
        fmt = self._fmt
        mapping = _record_format_mapping(record)
        try:
            mapping["message"] = record.getMessage()
        except Exception:
            mapping["message"] = ""
        try:
            return _percent_fallback(fmt, mapping)
        except Exception:
            return fmt


class StrFormatStyle(_Style):
    default_format = "{message}"
    asctime_format = "{asctime}"

    def usesTime(self) -> bool:
        return "{asctime" in self._fmt

    def format(self, record: LogRecord) -> str:
        return self._fmt.format(**_record_format_mapping(record))


class StringTemplateStyle(_Style):
    default_format = "${message}"
    asctime_format = "${asctime}"

    def usesTime(self) -> bool:
        return "$asctime" in self._fmt

    def format(self, record: LogRecord) -> str:
        return _string.Template(self._fmt).substitute(_record_format_mapping(record))


class Formatter:
    default_time_format = "%Y-%m-%d %H:%M:%S"
    default_msec_format = "%s,%03d"

    def __init__(
        self,
        fmt: str | None = None,
        datefmt: str | None = None,
        style: str = "%",
    ) -> None:
        # Create Rust-side formatter handle.
        self._handle = _FORMATTER_NEW(fmt, datefmt, style, True)
        if style == "%":
            self._style: _Style = PercentStyle(fmt)
        elif style == "{":
            self._style = StrFormatStyle(fmt)
        elif style == "$":
            self._style = StringTemplateStyle(fmt)
        else:
            raise ValueError("Invalid format style")
        self._fmt = self._style._fmt
        self.datefmt = datefmt

    def usesTime(self) -> bool:
        return self._style.usesTime()

    def formatTime(self, record: LogRecord, datefmt: str | None = None) -> str:
        try:
            return str(_FORMATTER_FORMAT_TIME(self._handle, record._handle))
        except Exception:
            pass
        # Fallback for edge cases.
        localtime = getattr(_time, "localtime", None)
        strftime = getattr(_time, "strftime", None)
        if callable(localtime) and callable(strftime):
            try:
                ct = localtime(record.created)
            except Exception:
                ct = localtime(0.0)
            if datefmt:
                return strftime(datefmt, ct)
            t = strftime(self.default_time_format, ct)
            return self.default_msec_format % (t, record.msecs)
        if datefmt:
            return datefmt
        return f"{record.created:.3f}"

    def formatException(self, exc_info: Any) -> str:
        return "".join(_traceback.format_exception(*exc_info)).rstrip()

    def formatStack(self, stack_info: str) -> str:
        return stack_info

    def format(self, record: LogRecord) -> str:
        try:
            result = _FORMATTER_FORMAT(self._handle, record._handle)
            if result is not None:
                return str(result)
        except Exception:
            pass
        # Fallback: Python-side formatting.
        record.message = record.getMessage()
        if self.usesTime():
            record.asctime = self.formatTime(record, self.datefmt)
        if self._fmt.find("%(") != -1:
            mapping = _record_format_mapping(record)
            mapping["message"] = record.message
            s = _percent_fallback(self._fmt, mapping)
        else:
            s = self._style.format(record)
        if record.exc_info:
            if not record.exc_text:
                record.exc_text = self.formatException(record.exc_info)
            if record.exc_text:
                s = s + "\n" + record.exc_text
        if record.stack_info:
            s = s + "\n" + self.formatStack(record.stack_info)
        return s

    def __del__(self) -> None:
        try:
            if _FORMATTER_DROP is not None:
                _FORMATTER_DROP(self._handle)
        except Exception:
            return


class Handler(Filterer):
    def __init__(self, level: int = NOTSET) -> None:
        super().__init__()
        self.level = _check_level(level)
        self._handle = _HANDLER_NEW(self.level)
        self.formatter: Formatter | None = None
        self.lock = _RLock()
        _handler_list.append(self)

    def setLevel(self, level: int | str) -> None:
        self.level = _check_level(level)
        _HANDLER_SET_LEVEL(self._handle, self.level)

    def setFormatter(self, fmt: Formatter | None) -> None:
        self.formatter = fmt
        fmt_handle = fmt._handle if fmt is not None else None
        _HANDLER_SET_FORMATTER(self._handle, fmt_handle)

    def emit(self, record: LogRecord) -> None:
        _HANDLER_EMIT(self._handle, record._handle)

    def handle(self, record: LogRecord) -> bool:
        if self.filter(record):
            self.acquire()
            try:
                self.emit(record)
            finally:
                self.release()
            return True
        return False

    def acquire(self) -> None:
        self.lock.acquire()

    def release(self) -> None:
        self.lock.release()

    def format(self, record: LogRecord) -> str:
        if self.formatter:
            return self.formatter.format(record)
        return Formatter(BASIC_FORMAT).format(record)

    def handleError(self, record: LogRecord) -> None:
        try:
            ei = _sys.exc_info()
            try:
                stream = getattr(_sys, "stderr", None)
                if stream is not None:
                    _traceback.print_exception(ei[0], ei[1], ei[2], file=stream)
            finally:
                del ei
        except Exception:
            pass

    def flush(self) -> None:
        _HANDLER_FLUSH(self._handle)

    def close(self) -> None:
        _HANDLER_CLOSE(self._handle)
        try:
            if self in _handler_list:
                _handler_list.remove(self)
        except Exception:
            pass

    def __del__(self) -> None:
        try:
            if _HANDLER_DROP is not None:
                _HANDLER_DROP(self._handle)
        except Exception:
            return


class StreamHandler(Handler):
    terminator = "\n"

    def __init__(self, stream: Any | None = None) -> None:
        super().__init__()
        if stream is None:
            stream = getattr(_sys, "stderr", None)
        self.stream = stream
        # Create a Rust-side stream handler.
        stream_name = None
        if stream is getattr(_sys, "stderr", None):
            stream_name = "stderr"
        elif stream is getattr(_sys, "stdout", None):
            stream_name = "stdout"
        self._stream_handle = _STREAM_HANDLER_NEW(stream_name, None, self.level)

    def emit(self, record: LogRecord) -> None:
        try:
            _STREAM_HANDLER_EMIT(self._stream_handle, record._handle)
        except Exception:
            # Fallback: Python-side emit.
            msg = self.format(record)
            if self.stream is None:
                return None
            try:
                self.stream.write(msg + self.terminator)
                self.flush()
            except Exception:
                return None

    def flush(self) -> None:
        if self.stream is None:
            return None
        try:
            flush = getattr(self.stream, "flush", None)
            if callable(flush):
                flush()
        except Exception:
            return None


class FileHandler(StreamHandler):
    def __init__(
        self,
        filename: str,
        mode: str = "a",
        encoding: str | None = None,
        delay: bool = False,
    ) -> None:
        self.baseFilename = _os.fspath(filename)
        self.mode = mode
        self.encoding = encoding
        self.delay = delay
        self.stream: Any | None = None
        super().__init__(None)
        if not delay:
            self._open()

    def _open(self) -> Any:
        _require_fs_write()
        self.stream = _io.open(self.baseFilename, self.mode, encoding=self.encoding)
        return self.stream

    def emit(self, record: LogRecord) -> None:
        msg = self.format(record)
        try:
            _FILE_HANDLER_EMIT(msg, self.baseFilename, self.mode, self.encoding)
        except Exception:
            # Fallback: Python-side stream write.
            if self.stream is None:
                self._open()
            if self.stream is None:
                return None
            try:
                self.stream.write(msg + self.terminator)
                self.flush()
            except Exception:
                return None

    def close(self) -> None:
        try:
            if self.stream is not None:
                try:
                    self.stream.close()
                except Exception:
                    pass
            self.stream = None
        finally:
            super().close()


class BaseRotatingHandler(FileHandler):
    def __init__(
        self,
        filename: str,
        mode: str = "a",
        encoding: str | None = None,
        delay: bool = False,
    ) -> None:
        super().__init__(filename, mode, encoding, delay)
        self.namer: Any = None
        self.rotator: Any = None

    def shouldRollover(self, record: LogRecord) -> bool:
        return False

    def doRollover(self) -> None:
        pass

    def emit(self, record: LogRecord) -> None:
        try:
            if self.shouldRollover(record):
                self.doRollover()
            super().emit(record)
        except Exception:
            self.handleError(record)

    def rotation_filename(self, default_name: str) -> str:
        if self.namer is None:
            return default_name
        return self.namer(default_name)

    def rotate(self, source: str, dest: str) -> None:
        if self.rotator is None:
            if _os.path.exists(source):
                _os.rename(source, dest)
        else:
            self.rotator(source, dest)


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
    ) -> None:
        super().__init__(str(filename), "a", encoding, delay)
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

    def shouldRollover(self, record: LogRecord) -> bool:
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
        self.stream = self._open()
        self.rolloverAt = self._computeRollover(currentTime)


class NullHandler(Handler):
    def emit(self, record: LogRecord) -> None:
        return None


class QueueHandler(Handler):
    def __init__(self, queue: Any) -> None:
        super().__init__()
        self.queue = queue

    def enqueue(self, record: LogRecord) -> None:
        put_nowait = getattr(self.queue, "put_nowait", None)
        if callable(put_nowait):
            put_nowait(record)
            return None
        put = getattr(self.queue, "put", None)
        if callable(put):
            put(record)
            return None
        raise TypeError("queue object does not support put/put_nowait")

    def prepare(self, record: LogRecord) -> LogRecord:
        return record

    def emit(self, record: LogRecord) -> None:
        self.enqueue(self.prepare(record))


class QueueListener:
    def __init__(
        self, queue: Any, *handlers: Handler, respect_handler_level: bool = False
    ) -> None:
        self.queue = queue
        self.handlers: list[Handler] = list(handlers)
        self.respect_handler_level = bool(respect_handler_level)
        self._running = False

    def _dequeue(self) -> LogRecord | None:
        get_nowait = getattr(self.queue, "get_nowait", None)
        if callable(get_nowait):
            try:
                return cast(LogRecord, get_nowait())
            except Exception:
                return None
        get = getattr(self.queue, "get", None)
        if callable(get):
            try:
                return cast(LogRecord, get(False))
            except Exception:
                return None
        return None

    def _handle(self, record: LogRecord) -> None:
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


class Logger(Filterer):
    __slots__ = (
        "name",
        "level",
        "parent",
        "handlers",
        "propagate",
        "disabled",
        "_handle",
        "__dict__",
        "__weakref__",
    )
    manager: "Manager"

    def __init__(self, name: str, level: int = NOTSET) -> None:
        super().__init__()
        self.name = name
        self.level = _check_level(level)
        self._handle = _LOGGER_NEW(name, self.level)
        self.parent: Logger | None = None
        self.handlers: list[Handler] = []
        self.propagate = True
        self.disabled = False

    def setLevel(self, level: int | str) -> None:
        self.level = _check_level(level)
        _LOGGER_SET_LEVEL(self._handle, self.level)

    def addHandler(self, hdlr: Handler) -> None:
        if hdlr not in self.handlers:
            self.handlers.append(hdlr)
            _LOGGER_ADD_HANDLER(self._handle, hdlr._handle)

    def removeHandler(self, hdlr: Handler) -> None:
        if hdlr in self.handlers:
            self.handlers.remove(hdlr)
            _LOGGER_REMOVE_HANDLER(self._handle, hdlr._handle)

    def hasHandlers(self) -> bool:
        logger: Logger | None = self
        while logger:
            if logger.handlers:
                return True
            if not logger.propagate:
                break
            logger = logger.parent
        return False

    def findCaller(self, stacklevel: int = 1) -> tuple[str, int, str | None]:
        f = getattr(_sys, "_getframe", None)
        if f is None:
            return ("", 0, None)
        frame = f(2)
        for _ in range(stacklevel - 1):
            if frame is None:
                break
            frame = getattr(frame, "f_back", None)
        if frame is None:
            return ("", 0, None)
        code = getattr(frame, "f_code", None)
        if code is None:
            return ("", 0, None)
        return (
            getattr(code, "co_filename", ""),
            int(getattr(frame, "f_lineno", 0)),
            getattr(code, "co_name", None),
        )

    def makeRecord(
        self,
        name: str,
        level: int,
        fn: str,
        lno: int,
        msg: Any,
        args: Any,
        exc_info: Any,
        func: str | None = None,
        extra: dict[str, Any] | None = None,
        sinfo: str | None = None,
    ) -> LogRecord:
        record = LogRecord(name, level, fn, lno, msg, args, exc_info, func, sinfo)
        if extra:
            for key, value in extra.items():
                if key in record.__dict__:
                    raise KeyError(f"Attempt to overwrite {key} in LogRecord")
                record.__dict__[key] = value
        return record

    def handle(self, record: LogRecord) -> None:
        if self.disabled:
            return None
        if not self.filter(record):
            return None
        self.callHandlers(record)

    def callHandlers(self, record: LogRecord) -> None:
        logger: Logger | None = self
        found = False
        while logger:
            for handler in logger.handlers:
                if record.levelno >= handler.level:
                    handler.handle(record)
                    found = True
            if not logger.propagate:
                break
            logger = logger.parent
        if not found and lastResort is not None:
            if record.levelno >= lastResort.level:
                lastResort.handle(record)

    def getEffectiveLevel(self) -> int:
        try:
            return int(_LOGGER_GET_EFFECTIVE_LEVEL(self._handle))
        except Exception:
            pass
        logger: Logger | None = self
        while logger:
            if logger.level:
                return logger.level
            logger = logger.parent
        return NOTSET

    def isEnabledFor(self, level: int) -> bool:
        if self.disabled:
            return False
        try:
            return bool(_LOGGER_IS_ENABLED_FOR(self._handle, level))
        except Exception:
            return level >= self.getEffectiveLevel()

    def _log(
        self,
        level: int,
        msg: Any,
        args: Any,
        exc_info: Any = None,
        extra: dict[str, Any] | None = None,
        stack_info: bool = False,
        stacklevel: int = 1,
    ) -> None:
        if not self.isEnabledFor(level):
            return None
        fn, lno, func = self.findCaller(stacklevel + 1)
        sinfo = None
        if stack_info:
            sinfo = "".join(_traceback.format_stack())
        record = self.makeRecord(
            self.name,
            level,
            fn,
            lno,
            msg,
            args,
            exc_info,
            func,
            extra,
            sinfo,
        )
        self.handle(record)

    def debug(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        self._log(DEBUG, msg, args, **kwargs)

    def info(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        self._log(INFO, msg, args, **kwargs)

    def warning(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        self._log(WARNING, msg, args, **kwargs)

    warn = warning

    def error(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        self._log(ERROR, msg, args, **kwargs)

    def exception(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        kwargs["exc_info"] = True
        self._log(ERROR, msg, args, **kwargs)

    def critical(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        self._log(CRITICAL, msg, args, **kwargs)

    fatal = critical

    def log(self, level: int, msg: Any, *args: Any, **kwargs: Any) -> None:
        self._log(level, msg, args, **kwargs)

    def __del__(self) -> None:
        try:
            if _LOGGER_DROP is not None:
                _LOGGER_DROP(self._handle)
        except Exception:
            return


class RootLogger(Logger):
    def __init__(self, level: int) -> None:
        super().__init__("root", level)


class Manager:
    def __init__(self, root: RootLogger) -> None:
        self.root = root
        self.loggerDict: dict[str, Logger] = {}

    def getLogger(self, name: str) -> Logger:
        if not name or name == "root":
            return self.root
        if name in self.loggerDict:
            return self.loggerDict[name]
        logger = _logger_class(name)
        i = name.rfind(".")
        parent = self.root
        while i > 0:
            parent_name = name[:i]
            parent = self.loggerDict.get(parent_name, parent)
            if parent is not self.root:
                break
            i = parent_name.rfind(".")
        logger.parent = parent
        self.loggerDict[name] = logger
        return logger


class LoggerAdapter:
    def __init__(self, logger: Logger, extra: dict[str, Any]) -> None:
        self.logger = logger
        self.extra = extra

    def process(self, msg: Any, kwargs: dict[str, Any]) -> tuple[Any, dict[str, Any]]:
        extra = kwargs.get("extra", {})
        merged = dict(self.extra)
        merged.update(extra)
        kwargs["extra"] = merged
        return msg, kwargs

    def log(self, level: int, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.log(level, msg, *args, **kwargs)

    def debug(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.debug(msg, *args, **kwargs)

    def info(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.info(msg, *args, **kwargs)

    def warning(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.warning(msg, *args, **kwargs)

    def error(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.error(msg, *args, **kwargs)

    def exception(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.exception(msg, *args, **kwargs)

    def critical(self, msg: Any, *args: Any, **kwargs: Any) -> None:
        msg, kwargs = self.process(msg, kwargs)
        self.logger.critical(msg, *args, **kwargs)


root = RootLogger(WARNING)
_manager = Manager(root)
Logger.manager = _manager
_logger_class: type[Logger] = Logger

lastResort = StreamHandler()
lastResort.setLevel(WARNING)


def getLogger(name: str | None = None) -> Logger:
    if name is None:
        return root
    return _manager.getLogger(name)


def getLoggerClass() -> type[Logger]:
    return _logger_class


def setLoggerClass(klass: type[Logger]) -> None:
    global _logger_class
    if not issubclass(klass, Logger):
        raise TypeError("logger class must be subclass of Logger")
    _logger_class = klass
    klass.manager = _manager


def basicConfig(**kwargs: Any) -> None:
    force = bool(kwargs.pop("force", False))
    handlers_arg = kwargs.pop("handlers", None)
    if kwargs and "level" in kwargs:
        level = kwargs.pop("level")
    else:
        level = None
    if kwargs and "format" in kwargs:
        fmt = kwargs.pop("format")
    else:
        fmt = None
    datefmt = kwargs.pop("datefmt", None)
    style = kwargs.pop("style", "%")
    stream = kwargs.pop("stream", None)
    filename = kwargs.pop("filename", None)
    filemode = kwargs.pop("filemode", "a")
    if kwargs:
        raise ValueError("Unrecognized argument(s): " + ", ".join(kwargs.keys()))
    if force:
        for h in list(root.handlers):
            root.removeHandler(h)
    if root.handlers and not force:
        return None
    # Try the Rust intrinsic for simple cases (no handlers arg, no filename).
    if handlers_arg is None and filename is None:
        try:
            _BASIC_CONFIG(level, fmt, datefmt, filename, filemode, style)
        except Exception:
            pass
    if handlers_arg is None:
        if filename is not None:
            handler: Handler = FileHandler(filename, filemode)
        else:
            handler = StreamHandler(stream)
        handlers_arg = [handler]
    if fmt is not None or datefmt is not None or style is not None:
        formatter = Formatter(fmt, datefmt, style)
        for h in handlers_arg:
            h.setFormatter(formatter)
    for h in handlers_arg:
        root.addHandler(h)
    if level is not None:
        root.setLevel(level)


def makeLogRecord(dict_: dict[str, Any]) -> LogRecord:
    record = LogRecord("", NOTSET, "", 0, "", (), None)
    record.__dict__.update(dict_)
    return record


def shutdown() -> None:
    try:
        _SHUTDOWN()
    except Exception:
        pass
    for h in list(_handler_list):
        try:
            h.flush()
            h.close()
        except Exception:
            pass


def _log_root(level: int, msg: Any, args: Any, kwargs: dict[str, Any]) -> None:
    root.log(level, msg, *args, **kwargs)


def debug(msg: Any, *args: Any, **kwargs: Any) -> None:
    _log_root(DEBUG, msg, args, kwargs)


def info(msg: Any, *args: Any, **kwargs: Any) -> None:
    _log_root(INFO, msg, args, kwargs)


def warning(msg: Any, *args: Any, **kwargs: Any) -> None:
    _log_root(WARNING, msg, args, kwargs)


warn = warning


def error(msg: Any, *args: Any, **kwargs: Any) -> None:
    _log_root(ERROR, msg, args, kwargs)


def exception(msg: Any, *args: Any, **kwargs: Any) -> None:
    kwargs["exc_info"] = True
    _log_root(ERROR, msg, args, kwargs)


def critical(msg: Any, *args: Any, **kwargs: Any) -> None:
    _log_root(CRITICAL, msg, args, kwargs)


fatal = critical


def log(level: int, msg: Any, *args: Any, **kwargs: Any) -> None:
    _log_root(level, msg, args, kwargs)


def _formatwarning(
    message: Warning | str,
    category: type[Warning],
    filename: str,
    lineno: int,
    line: str | None,
) -> str:
    formatter = getattr(_traceback, "formatwarning", None)
    if callable(formatter):
        return formatter(message, category, filename, lineno, line)
    exc = message if isinstance(message, BaseException) else category(message)
    return "".join(_traceback.format_exception_only(category, exc))


ShowWarning = Callable[
    [Warning | str, type[Warning], str, int, TextIO | None, str | None], None
]
_warnings_showwarning: ShowWarning | None = None


def _set_warning_capture_streams(warnings_mod: ModuleType, logger: Logger) -> None:
    streams: list[tuple[Any, str]] = []
    handlers_list = list(getattr(logger, "handlers", []))
    if not handlers_list and getattr(logger, "propagate", False):
        parent = getattr(logger, "parent", None)
        if parent is not None:
            handlers_list = list(getattr(parent, "handlers", []))
    for handler in handlers_list:
        if isinstance(handler, StreamHandler):
            streams.append((handler.stream, handler.terminator))
    setattr(warnings_mod, "_molt_capture_streams", streams)


def _showwarning(
    message: Warning | str,
    category: type[Warning],
    filename: str,
    lineno: int,
    file: TextIO | None = None,
    line: str | None = None,
) -> None:
    logger = getLogger("py.warnings")
    msg = _formatwarning(message, category, filename, lineno, line)
    if getattr(logger, "disabled", False):
        return None
    try:
        if not logger.isEnabledFor(WARNING):
            return None
    except Exception:
        pass
    rendered = msg.rstrip()
    handlers_list = list(getattr(logger, "handlers", []))
    if not handlers_list and getattr(logger, "propagate", False):
        parent = getattr(logger, "parent", None)
        if parent is not None:
            handlers_list = list(getattr(parent, "handlers", []))
    for handler in handlers_list:
        try:
            if WARNING < getattr(handler, "level", NOTSET):
                continue
            if isinstance(handler, StreamHandler):
                stream = handler.stream or getattr(_sys, "stderr", None)
                if stream is not None and hasattr(stream, "write"):
                    stream.write(rendered + handler.terminator)
                    handler.flush()
                    continue
            handler.handle(
                logger.makeRecord(
                    logger.name,
                    WARNING,
                    filename,
                    lineno,
                    rendered,
                    (),
                    None,
                )
            )
        except Exception:
            continue


def captureWarnings(capture: bool) -> None:
    global _warnings_showwarning
    if capture:
        if _warnings_showwarning is None:
            _warnings_showwarning = _warnings.showwarning
            setattr(cast(Any, _warnings), "showwarning", _showwarning)
        try:
            _set_warning_capture_streams(_warnings, getLogger("py.warnings"))
        except Exception:
            pass
    else:
        if _warnings_showwarning is not None:
            setattr(cast(Any, _warnings), "showwarning", _warnings_showwarning)
            _warnings_showwarning = None
        try:
            if hasattr(_warnings, "_molt_capture_streams"):
                delattr(cast(Any, _warnings), "_molt_capture_streams")
        except Exception:
            pass


def _install_handlers_submodule() -> ModuleType:
    canonical_name = "logging.handlers"
    local_name = f"{__name__}.handlers"
    existing = _sys.modules.get(canonical_name) or _sys.modules.get(local_name)
    if isinstance(existing, ModuleType):
        return existing
    mod = ModuleType(canonical_name)
    mod.__dict__.update(
        {
            "BaseRotatingHandler": BaseRotatingHandler,
            "TimedRotatingFileHandler": TimedRotatingFileHandler,
            "QueueHandler": QueueHandler,
            "QueueListener": QueueListener,
        }
    )
    _sys.modules[canonical_name] = mod
    _sys.modules[local_name] = mod
    parent = _sys.modules.get(__name__)
    if parent is not None:
        setattr(parent, "handlers", mod)
    return mod


handlers = _install_handlers_submodule()

globals().pop("_require_intrinsic", None)
