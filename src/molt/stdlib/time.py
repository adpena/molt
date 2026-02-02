"""Minimal time shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

import sys as _sys
import builtins as _builtins

try:
    from _intrinsics import load_intrinsic as _load_intrinsic
except Exception:
    _load_intrinsic = None

try:
    import importlib as _importlib
except Exception:
    _importlib = None

_py_time = None
_MOLT_TIME_MONOTONIC = None
_MOLT_TIME_MONOTONIC_NS = None
_MOLT_TIME_TIME = None
_MOLT_TIME_TIME_NS = None
if _load_intrinsic is not None:
    _MOLT_TIME_MONOTONIC = _load_intrinsic("_molt_time_monotonic", globals())
    if _MOLT_TIME_MONOTONIC is None:
        _MOLT_TIME_MONOTONIC = _load_intrinsic("molt_time_monotonic", globals())
    _MOLT_TIME_MONOTONIC_NS = _load_intrinsic("_molt_time_monotonic_ns", globals())
    if _MOLT_TIME_MONOTONIC_NS is None:
        _MOLT_TIME_MONOTONIC_NS = _load_intrinsic("molt_time_monotonic_ns", globals())
    _MOLT_TIME_TIME = _load_intrinsic("_molt_time_time", globals())
    if _MOLT_TIME_TIME is None:
        _MOLT_TIME_TIME = _load_intrinsic("molt_time_time", globals())
    _MOLT_TIME_TIME_NS = _load_intrinsic("_molt_time_time_ns", globals())
    if _MOLT_TIME_TIME_NS is None:
        _MOLT_TIME_TIME_NS = _load_intrinsic("molt_time_time_ns", globals())


def _import_std_time() -> Any | None:
    if _importlib is None:
        return None
    if _sys.modules.get(__name__) is not None and __name__ == "time":
        saved = _sys.modules.pop("time", None)
        try:
            return _importlib.import_module("time")
        except Exception:
            return None
        finally:
            if saved is not None:
                _sys.modules["time"] = saved
    try:
        return _importlib.import_module("time")
    except Exception:
        return None


def _ensure_intrinsics() -> None:
    global \
        _MOLT_TIME_MONOTONIC, \
        _MOLT_TIME_MONOTONIC_NS, \
        _MOLT_TIME_TIME, \
        _MOLT_TIME_TIME_NS
    if _MOLT_TIME_MONOTONIC is None:
        _MOLT_TIME_MONOTONIC = getattr(
            _builtins, "_molt_time_monotonic", None
        ) or getattr(_builtins, "molt_time_monotonic", None)
    if _MOLT_TIME_MONOTONIC_NS is None:
        _MOLT_TIME_MONOTONIC_NS = getattr(
            _builtins, "_molt_time_monotonic_ns", None
        ) or getattr(_builtins, "molt_time_monotonic_ns", None)
    if _MOLT_TIME_TIME is None:
        _MOLT_TIME_TIME = getattr(_builtins, "_molt_time_time", None) or getattr(
            _builtins, "molt_time_time", None
        )
    if _MOLT_TIME_TIME_NS is None:
        _MOLT_TIME_TIME_NS = getattr(_builtins, "_molt_time_time_ns", None) or getattr(
            _builtins, "molt_time_time_ns", None
        )


if _MOLT_TIME_MONOTONIC is None and _MOLT_TIME_TIME is None:
    _py_time = _import_std_time()

_capabilities: ModuleType | None
try:
    _capabilities = _importlib.import_module("molt.capabilities")
except Exception:
    _capabilities = None

__all__ = [
    "ClockInfo",
    "get_clock_info",
    "time",
    "time_ns",
    "monotonic",
    "monotonic_ns",
    "perf_counter",
    "perf_counter_ns",
    "sleep",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement full time module surface (timezone, tzname, struct_time, process_time).

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


class ClockInfo:
    def __init__(
        self,
        name: str,
        implementation: str,
        resolution: float,
        monotonic: bool,
        adjustable: bool,
    ) -> None:
        self.name = name
        self.implementation = implementation
        self.resolution = float(resolution)
        self.monotonic = bool(monotonic)
        self.adjustable = bool(adjustable)

    def __repr__(self) -> str:
        return (
            "ClockInfo(name="
            + repr(self.name)
            + ", implementation="
            + repr(self.implementation)
            + ", resolution="
            + repr(self.resolution)
            + ", monotonic="
            + repr(self.monotonic)
            + ", adjustable="
            + repr(self.adjustable)
            + ")"
        )


def _wrap_clock_info(info: Any, name: str) -> ClockInfo:
    return ClockInfo(
        getattr(info, "name", name),
        getattr(info, "implementation", "molt"),
        getattr(info, "resolution", 1e-9),
        getattr(info, "monotonic", False),
        getattr(info, "adjustable", False),
    )


def get_clock_info(name: str) -> ClockInfo:
    name = str(name)
    if name in ("monotonic", "perf_counter"):
        return ClockInfo(name, "molt", 1e-9, True, False)
    if name == "time":
        _require_time_wall()
        return ClockInfo(name, "molt", 1e-6, False, True)
    raise ValueError("unknown clock")


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
    _ensure_intrinsics()
    if _MOLT_TIME_MONOTONIC is not None:
        value = float(_MOLT_TIME_MONOTONIC())
        if value > 0:
            return value
    try:
        value = float(_molt_time_monotonic())  # type: ignore[name-defined]
        if value > 0:
            return value
    except NameError:
        pass
    if _py_time is not None:
        return float(_py_time.monotonic())
    if _MOLT_TIME_TIME is not None:
        return float(_MOLT_TIME_TIME())
    try:
        return float(_molt_time_time())  # type: ignore[name-defined]
    except NameError:
        pass
    raise NotImplementedError("time.monotonic unavailable")


def monotonic_ns() -> int:
    _ensure_intrinsics()
    if _MOLT_TIME_MONOTONIC_NS is not None:
        return int(_MOLT_TIME_MONOTONIC_NS())
    try:
        return int(_molt_time_monotonic_ns())  # type: ignore[name-defined]
    except NameError:
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
    _ensure_intrinsics()
    if _MOLT_TIME_TIME is not None:
        return float(_MOLT_TIME_TIME())
    try:
        return float(_molt_time_time())  # type: ignore[name-defined]
    except NameError:
        pass
    if _py_time is not None:
        return float(_py_time.time())
    raise NotImplementedError("time.time unavailable")


def time_ns() -> int:
    _require_time_wall()
    _ensure_intrinsics()
    if _MOLT_TIME_TIME_NS is not None:
        return int(_MOLT_TIME_TIME_NS())
    try:
        return int(_molt_time_time_ns())  # type: ignore[name-defined]
    except NameError:
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
