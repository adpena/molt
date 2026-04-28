"""Minimal time shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

TYPE_CHECKING = False
if TYPE_CHECKING:
    from typing import Any
else:
    Any = object


_MOLT_TIME_MONOTONIC = _require_intrinsic("molt_time_monotonic")
_MOLT_TIME_MONOTONIC_NS = _require_intrinsic("molt_time_monotonic_ns")
_MOLT_TIME_PERF_COUNTER = _require_intrinsic("molt_time_perf_counter")
_MOLT_TIME_PERF_COUNTER_NS = _require_intrinsic("molt_time_perf_counter_ns")
_MOLT_TIME_TIME = _require_intrinsic("molt_time_time")
_MOLT_TIME_TIME_NS = _require_intrinsic("molt_time_time_ns")
_MOLT_TIME_SLEEP = _require_intrinsic("molt_time_sleep")
_MOLT_TIME_PROCESS_TIME = _require_intrinsic("molt_time_process_time")
_MOLT_TIME_PROCESS_TIME_NS = _require_intrinsic("molt_time_process_time_ns")
_MOLT_TIME_LOCALTIME = _require_intrinsic("molt_time_localtime")
_MOLT_TIME_GMTIME = _require_intrinsic("molt_time_gmtime")
_MOLT_TIME_STRFTIME = _require_intrinsic("molt_time_strftime")
_MOLT_TIME_TIMEZONE = _require_intrinsic("molt_time_timezone")
_MOLT_TIME_DAYLIGHT = _require_intrinsic("molt_time_daylight")
_MOLT_TIME_ALTZONE = _require_intrinsic("molt_time_altzone")
_MOLT_TIME_TZNAME = _require_intrinsic("molt_time_tzname")
_MOLT_TIME_ASCTIME = _require_intrinsic("molt_time_asctime")
_MOLT_TIME_MKTIME = _require_intrinsic("molt_time_mktime")
_MOLT_TIME_TIMEGM = _require_intrinsic("molt_time_timegm")
_MOLT_TIME_GET_CLOCK_INFO = _require_intrinsic("molt_time_get_clock_info")

_CAP_TRUSTED = None
_CAP_HAS = None


def _ensure_capabilities() -> None:
    global _CAP_TRUSTED, _CAP_HAS
    if _CAP_TRUSTED is not None or _CAP_HAS is not None:
        return
    _CAP_TRUSTED = _require_intrinsic("molt_capabilities_trusted")
    _CAP_HAS = _require_intrinsic("molt_capabilities_has")


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
    "strptime",
    "timezone",
    "daylight",
    "altzone",
    "tzname",
    "tzset",
    "mktime",
    "timegm",
]


# Number of fields the struct_time exposes — _strptime indexes into the
# raw 11-tuple it builds before constructing struct_time, so this must
# match struct_time.__new__'s required length.
_STRUCT_TM_ITEMS = 9

if TYPE_CHECKING:

    def _molt_time_monotonic() -> float:
        return 0.0

    def _molt_time_monotonic_ns() -> int:
        return 0

    def _molt_time_time() -> float:
        return 0.0

    def _molt_time_time_ns() -> int:
        return 0

    def _molt_time_sleep(secs: float) -> None:
        return None

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


def _init_daylight() -> int:
    try:
        return int(_MOLT_TIME_DAYLIGHT())
    except Exception as exc:
        raise RuntimeError("time daylight intrinsic failed") from exc


def _init_altzone() -> int:
    try:
        return int(_MOLT_TIME_ALTZONE())
    except Exception as exc:
        raise RuntimeError("time altzone intrinsic failed") from exc


timezone = _init_timezone()
daylight = _init_daylight()
altzone = _init_altzone()
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
    _MOLT_TIME_SLEEP(delay)
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


def mktime(t: Any) -> float:
    tuple_val = _coerce_time_tuple(t)
    if len(tuple_val) != 9:
        raise TypeError("mktime(): illegal time tuple argument")
    return float(_MOLT_TIME_MKTIME(tuple(tuple_val)))


def timegm(t: Any) -> int:
    tuple_val = _coerce_time_tuple(t)
    if len(tuple_val) < 6:
        raise ValueError(
            f"not enough values to unpack (expected 6, got {len(tuple_val)})"
        )
    return int(_MOLT_TIME_TIMEGM(tuple(tuple_val)))


def tzset() -> None:
    """Reset the timezone information from the TZ environment variable.

    Molt's deterministic compiled-binary contract has no dynamic timezone
    state — the runtime is initialized from the host environment at
    startup and stays fixed. Provided as a no-op for compatibility with
    code that calls tzset() defensively (e.g. _strptime.LocaleTime).
    """
    return None


def strptime(data_string: str, format: str = "%a %b %d %H:%M:%S %Y") -> "struct_time":
    """Parse a string according to a format string and return a struct_time.

    Mirrors CPython's `time.strptime` — a thin wrapper over the
    `_strptime` module's `_strptime_time` entry point.
    """
    import _strptime as _strptime_mod  # imported lazily to avoid circular boot
    return _strptime_mod._strptime_time(data_string, format)
