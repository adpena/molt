"""Minimal time shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_TIME_MONOTONIC = _require_intrinsic("molt_time_monotonic", globals())
_MOLT_TIME_MONOTONIC_NS = _require_intrinsic("molt_time_monotonic_ns", globals())
_MOLT_TIME_PERF_COUNTER = _require_intrinsic("molt_time_perf_counter", globals())
_MOLT_TIME_PERF_COUNTER_NS = _require_intrinsic("molt_time_perf_counter_ns", globals())
_MOLT_TIME_TIME = _require_intrinsic("molt_time_time", globals())
_MOLT_TIME_TIME_NS = _require_intrinsic("molt_time_time_ns", globals())
_MOLT_TIME_PROCESS_TIME = _require_intrinsic("molt_time_process_time", globals())
_MOLT_TIME_PROCESS_TIME_NS = _require_intrinsic("molt_time_process_time_ns", globals())
_MOLT_TIME_LOCALTIME = _require_intrinsic("molt_time_localtime", globals())
_MOLT_TIME_GMTIME = _require_intrinsic("molt_time_gmtime", globals())
_MOLT_TIME_STRFTIME = _require_intrinsic("molt_time_strftime", globals())
_MOLT_TIME_TIMEZONE = _require_intrinsic("molt_time_timezone", globals())
_MOLT_TIME_TZNAME = _require_intrinsic("molt_time_tzname", globals())
_MOLT_TIME_ASCTIME = _require_intrinsic("molt_time_asctime", globals())
_MOLT_TIME_GET_CLOCK_INFO = _require_intrinsic("molt_time_get_clock_info", globals())
_MOLT_ASYNC_SLEEP = _require_intrinsic("molt_async_sleep", globals())
_MOLT_BLOCK_ON = _require_intrinsic("molt_block_on", globals())

_CAP_TRUSTED = None
_CAP_HAS = None


def _ensure_capabilities() -> None:
    global _CAP_TRUSTED, _CAP_HAS
    if _CAP_TRUSTED is not None or _CAP_HAS is not None:
        return
    _CAP_TRUSTED = _require_intrinsic("molt_capabilities_trusted", globals())
    _CAP_HAS = _require_intrinsic("molt_capabilities_has", globals())


__all__ = [
    "ClockInfo",
    "struct_time",
    "asctime",
    "ctime",
    "get_clock_info",
    "time",
    "time_ns",
    "monotonic",
    "monotonic_ns",
    "perf_counter",
    "perf_counter_ns",
    "process_time",
    "process_time_ns",
    "sleep",
    "localtime",
    "gmtime",
    "strftime",
    "timezone",
    "tzname",
]

if TYPE_CHECKING:

    def _molt_time_monotonic() -> float:
        return 0.0

    def _molt_time_monotonic_ns() -> int:
        return 0

    def _molt_time_time() -> float:
        return 0.0

    def _molt_time_time_ns() -> int:
        return 0

    def _molt_time_perf_counter() -> float:
        return 0.0

    def _molt_time_perf_counter_ns() -> int:
        return 0

    def _molt_time_process_time() -> float:
        return 0.0

    def _molt_time_process_time_ns() -> int:
        return 0

    def _molt_time_localtime(secs: float | None = None) -> Any:
        return None

    def _molt_time_gmtime(secs: float | None = None) -> Any:
        return None

    def _molt_time_strftime(fmt: str, time_tuple: Any) -> str:
        return ""

    def _molt_time_timezone() -> int:
        return 0

    def _molt_time_tzname() -> Any:
        return ("UTC", "UTC")

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


class struct_time(tuple):
    __slots__ = ()

    n_fields = 9
    n_sequence_fields = 9
    n_unnamed_fields = 0

    def __new__(cls, seq: Any) -> "struct_time":
        try:
            items = tuple(seq)
        except Exception:
            raise TypeError("time tuple must be a 9-element sequence")
        if len(items) != 9:
            raise TypeError("time tuple must be a 9-element sequence")
        values = []
        for item in items:
            try:
                values.append(int(item))
            except Exception:
                raise TypeError("time tuple elements must be integers")
        return tuple.__new__(cls, values)

    @property
    def tm_year(self) -> int:
        return int(self[0])

    @property
    def tm_mon(self) -> int:
        return int(self[1])

    @property
    def tm_mday(self) -> int:
        return int(self[2])

    @property
    def tm_hour(self) -> int:
        return int(self[3])

    @property
    def tm_min(self) -> int:
        return int(self[4])

    @property
    def tm_sec(self) -> int:
        return int(self[5])

    @property
    def tm_wday(self) -> int:
        return int(self[6])

    @property
    def tm_yday(self) -> int:
        return int(self[7])

    @property
    def tm_isdst(self) -> int:
        return int(self[8])

    @property
    def tm_zone(self) -> None:
        return None

    @property
    def tm_gmtoff(self) -> None:
        return None

    def __repr__(self) -> str:
        return (
            "time.struct_time(tm_year="
            + repr(self.tm_year)
            + ", tm_mon="
            + repr(self.tm_mon)
            + ", tm_mday="
            + repr(self.tm_mday)
            + ", tm_hour="
            + repr(self.tm_hour)
            + ", tm_min="
            + repr(self.tm_min)
            + ", tm_sec="
            + repr(self.tm_sec)
            + ", tm_wday="
            + repr(self.tm_wday)
            + ", tm_yday="
            + repr(self.tm_yday)
            + ", tm_isdst="
            + repr(self.tm_isdst)
            + ")"
        )


def _coerce_time_tuple(value: Any) -> tuple[int, ...]:
    if isinstance(value, struct_time):
        return tuple(value)
    if isinstance(value, tuple):
        return value
    return tuple(value)


def _init_timezone() -> int:
    try:
        return int(_MOLT_TIME_TIMEZONE())
    except Exception as exc:
        raise RuntimeError("time timezone intrinsic failed") from exc


def _init_tzname() -> tuple[str, str]:
    raw = _MOLT_TIME_TZNAME()
    if not isinstance(raw, (tuple, list)) or len(raw) != 2:
        raise RuntimeError("time tzname intrinsic returned invalid value")
    left, right = raw
    if not isinstance(left, str) or not isinstance(right, str):
        raise RuntimeError("time tzname intrinsic returned invalid value")
    return (str(left), str(right))


timezone = _init_timezone()
tzname = _init_tzname()


def _wrap_clock_info(info: Any, name: str) -> ClockInfo:
    if not isinstance(info, (tuple, list)) or len(info) != 5:
        raise RuntimeError("time get_clock_info intrinsic returned invalid value")
    try:
        return ClockInfo(
            str(info[0]),
            str(info[1]),
            float(info[2]),
            bool(info[3]),
            bool(info[4]),
        )
    except Exception as exc:
        raise RuntimeError(
            "time get_clock_info intrinsic returned invalid value"
        ) from exc


def get_clock_info(name: str) -> ClockInfo:
    name = str(name)
    if name == "time":
        _require_time_wall()
    info = _MOLT_TIME_GET_CLOCK_INFO(name)
    return _wrap_clock_info(info, name)


def _has_time_wall() -> bool:
    _ensure_capabilities()
    if _CAP_TRUSTED is None or _CAP_HAS is None:
        return True
    try:
        if _CAP_TRUSTED():
            return True
        return bool(_CAP_HAS("time.wall") or _CAP_HAS("time"))
    except Exception:
        return False


def _require_time_wall() -> None:
    if not _has_time_wall():
        raise PermissionError("Missing capability")


def monotonic() -> float:
    return float(_MOLT_TIME_MONOTONIC())


def monotonic_ns() -> int:
    return int(_MOLT_TIME_MONOTONIC_NS())


def perf_counter() -> float:
    return float(_MOLT_TIME_PERF_COUNTER())


def perf_counter_ns() -> int:
    return int(_MOLT_TIME_PERF_COUNTER_NS())


def process_time() -> float:
    return float(_MOLT_TIME_PROCESS_TIME())


def process_time_ns() -> int:
    return int(_MOLT_TIME_PROCESS_TIME_NS())


def time() -> float:
    _require_time_wall()
    return float(_MOLT_TIME_TIME())


def time_ns() -> int:
    _require_time_wall()
    return int(_MOLT_TIME_TIME_NS())


def sleep(secs: float) -> None:
    try:
        delay = float(secs)
    except (TypeError, ValueError):
        raise TypeError("an integer or float is required")
    if delay < 0:
        raise ValueError("sleep length must be non-negative")
    fut = _MOLT_ASYNC_SLEEP(delay, None)
    _MOLT_BLOCK_ON(fut)
    return None


def localtime(secs: float | None = None) -> struct_time:
    if secs is None:
        _require_time_wall()
    parts = _MOLT_TIME_LOCALTIME(secs)
    return struct_time(parts)


def gmtime(secs: float | None = None) -> struct_time:
    if secs is None:
        _require_time_wall()
    parts = _MOLT_TIME_GMTIME(secs)
    return struct_time(parts)


def strftime(fmt: str, t: Any | None = None) -> str:
    if not isinstance(fmt, str):
        name = type(fmt).__name__
        raise TypeError(f"strftime() format must be str, not {name}")
    if t is None:
        t = localtime()
    tuple_val = _coerce_time_tuple(t)
    if len(tuple_val) != 9:
        raise TypeError("time tuple must be a 9-element sequence")
    return str(_MOLT_TIME_STRFTIME(fmt, tuple_val))


def asctime(t: Any | None = None) -> str:
    if t is None:
        t = localtime()
    tt = struct_time(t)
    return str(_MOLT_TIME_ASCTIME(tuple(tt)))


def ctime(secs: float | None = None) -> str:
    if secs is None:
        return asctime()
    return asctime(localtime(secs))
