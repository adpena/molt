"""Minimal time shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

try:
    import importlib as _importlib

    _py_time = _importlib.import_module("time")
except Exception:
    _py_time = None

_capabilities: ModuleType | None
try:
    _capabilities = _importlib.import_module("molt.capabilities")
except Exception:
    _capabilities = None

__all__ = [
    "time",
    "time_ns",
    "monotonic",
    "monotonic_ns",
    "perf_counter",
    "perf_counter_ns",
    "sleep",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement full time module surface (timezone, tzname, struct_time, get_clock_info, process_time).

if TYPE_CHECKING:
    from types import ModuleType

    _capabilities: ModuleType | None

    def _molt_time_monotonic() -> float:
        return 0.0

    def _molt_time_monotonic_ns() -> int:
        return 0

    def _molt_time_time() -> float:
        return 0.0

    def _molt_time_time_ns() -> int:
        return 0

    def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any:
        return None

    def molt_block_on(task: Any) -> Any:
        return None


def _has_time_wall() -> bool:
    if _capabilities is None:
        return True
    if _capabilities.trusted():
        return True
    return _capabilities.has("time.wall") or _capabilities.has("time")


def _require_time_wall() -> None:
    if not _has_time_wall():
        raise PermissionError("Missing capability")


def monotonic() -> float:
    try:
        return float(_molt_time_monotonic())  # type: ignore[name-defined]
    except NameError:
        pass
    except Exception:
        pass
    if _py_time is not None:
        return float(_py_time.monotonic())
    raise NotImplementedError("time.monotonic unavailable")


def monotonic_ns() -> int:
    try:
        return int(_molt_time_monotonic_ns())  # type: ignore[name-defined]
    except NameError:
        pass
    except Exception:
        pass
    if _py_time is not None and hasattr(_py_time, "monotonic_ns"):
        return int(_py_time.monotonic_ns())
    return int(monotonic() * 1_000_000_000)


def perf_counter() -> float:
    return monotonic()


def perf_counter_ns() -> int:
    return monotonic_ns()


def time() -> float:
    _require_time_wall()
    try:
        return float(_molt_time_time())  # type: ignore[name-defined]
    except NameError:
        pass
    except Exception:
        pass
    if _py_time is not None:
        return float(_py_time.time())
    raise NotImplementedError("time.time unavailable")


def time_ns() -> int:
    _require_time_wall()
    try:
        return int(_molt_time_time_ns())  # type: ignore[name-defined]
    except NameError:
        pass
    except Exception:
        pass
    if _py_time is not None and hasattr(_py_time, "time_ns"):
        return int(_py_time.time_ns())
    return int(time() * 1_000_000_000)


def sleep(secs: float) -> None:
    try:
        delay = float(secs)
    except (TypeError, ValueError):
        raise TypeError("an integer or float is required")
    if delay < 0:
        raise ValueError("sleep length must be non-negative")
    try:
        fut = molt_async_sleep(delay, None)  # type: ignore[name-defined]
        molt_block_on(fut)  # type: ignore[name-defined]
        return None
    except NameError:
        pass
    if _py_time is not None:
        _py_time.sleep(delay)
        return None
    raise NotImplementedError("time.sleep unavailable")
