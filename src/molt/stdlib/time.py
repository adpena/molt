"""Minimal time shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from sys import _struct_sequence_type, _struct_sequence_new

TYPE_CHECKING = False
if TYPE_CHECKING:
    from typing import Any as _Any
else:
    _Any = object


_MOLT_TIME_TIME = _require_intrinsic("molt_time_time")
_MOLT_TIME_TIME_NS = _require_intrinsic("molt_time_time_ns")
_MOLT_TIME_SLEEP = _require_intrinsic("molt_time_sleep")
_MOLT_TIME_LOCALTIME = _require_intrinsic("molt_time_localtime")
_MOLT_TIME_GMTIME = _require_intrinsic("molt_time_gmtime")
_MOLT_TIME_STRFTIME = _require_intrinsic("molt_time_strftime")
_MOLT_TIME_TIMEZONE = _require_intrinsic("molt_time_timezone")
_MOLT_TIME_DAYLIGHT = _require_intrinsic("molt_time_daylight")
_MOLT_TIME_ALTZONE = _require_intrinsic("molt_time_altzone")
_MOLT_TIME_TZNAME = _require_intrinsic("molt_time_tzname")
_MOLT_TIME_ASCTIME = _require_intrinsic("molt_time_asctime")
_MOLT_TIME_MKTIME = _require_intrinsic("molt_time_mktime")
_MOLT_TIME_GET_CLOCK_INFO = _require_intrinsic("molt_time_get_clock_info")

_CAP_HAS = None


def _ensure_capabilities() -> None:
    global _CAP_HAS
    if _CAP_HAS is not None:
        return
    _CAP_HAS = _require_intrinsic("molt_capabilities_has")


__all__ = [
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

    def _molt_time_localtime(secs: float | None = None) -> _Any:
        return None

    def _molt_time_gmtime(secs: float | None = None) -> _Any:
        return None

    def _molt_time_strftime(fmt: str, time_tuple: _Any) -> str:
        return ""

    def _molt_time_timezone() -> int:
        return 0

    def _molt_time_tzname() -> _Any:
        return ("UTC", "UTC")


class _ClockInfo:
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
            "_ClockInfo(name="
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


struct_time = _struct_sequence_type(
    "struct_time",
    (
        "tm_year",
        "tm_mon",
        "tm_mday",
        "tm_hour",
        "tm_min",
        "tm_sec",
        "tm_wday",
        "tm_yday",
        "tm_isdst",
        "tm_zone",
        "tm_gmtoff",
    ),
    module="time",
    visible=9,
    constructor=3,
    static_origin=False,
    docs=(
        "year, for example, 1993",
        "month of year, range [1, 12]",
        "day of month, range [1, 31]",
        "hours, range [0, 23]",
        "minutes, range [0, 59]",
        "seconds, range [0, 61])",
        "day of week, range [0, 6], Monday is 0",
        "day of year, range [1, 366]",
        "1 if summer time is in effect, 0 if not, and -1 if unknown",
        "abbreviation of timezone name",
        "offset from UTC in seconds",
    ),
    doc="The time value as returned by gmtime(), localtime(), and strptime(), and\n accepted by asctime(), mktime() and strftime().  May be considered as a\n sequence of 9 integers.\n\n Note that several fields' values are not the same as those defined by\n the C language standard for struct tm.  For example, the value of the\n field tm_year is the actual year, not year - 1900.  See individual\n fields' descriptions for details.",
)

_STRUCT_TIME = struct_time


def _coerce_time_tuple(value: _Any) -> tuple[int, ...]:
    if isinstance(value, struct_time):
        return value
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


def _wrap_clock_info(info: _Any, name: str) -> _ClockInfo:
    if not isinstance(info, (tuple, list)) or len(info) != 5:
        raise RuntimeError("time get_clock_info intrinsic returned invalid value")
    try:
        return _ClockInfo(
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


def get_clock_info(name: str, /) -> _ClockInfo:
    name = str(name)
    if name == "time":
        _require_time_wall()
    info = _MOLT_TIME_GET_CLOCK_INFO(name)
    return _wrap_clock_info(info, name)

# BEGIN PUBLIC OBJECT molt_time_get_clock_info
# Generated by tools/gen_intrinsics.py from public_objects.toml.
get_clock_info.__name__ = 'get_clock_info'
get_clock_info.__qualname__ = 'get_clock_info'
get_clock_info.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(get_clock_info)
# END PUBLIC OBJECT molt_time_get_clock_info


def _has_time_wall() -> bool:
    _ensure_capabilities()
    if _CAP_HAS is None:
        return False
    try:
        return bool(_CAP_HAS("time.wall"))
    except Exception:
        return False


def _require_time_wall() -> None:
    if not _has_time_wall():
        raise PermissionError("Missing capability")


# BEGIN PUBLIC OBJECT molt_time_monotonic
# Generated by tools/gen_intrinsics.py from public_objects.toml.
monotonic = _require_intrinsic('molt_time_monotonic')
# END PUBLIC OBJECT molt_time_monotonic


# BEGIN PUBLIC OBJECT molt_time_monotonic_ns
# Generated by tools/gen_intrinsics.py from public_objects.toml.
monotonic_ns = _require_intrinsic('molt_time_monotonic_ns')
# END PUBLIC OBJECT molt_time_monotonic_ns


# BEGIN PUBLIC OBJECT molt_time_perf_counter
# Generated by tools/gen_intrinsics.py from public_objects.toml.
perf_counter = _require_intrinsic('molt_time_perf_counter')
# END PUBLIC OBJECT molt_time_perf_counter


# BEGIN PUBLIC OBJECT molt_time_perf_counter_ns
# Generated by tools/gen_intrinsics.py from public_objects.toml.
perf_counter_ns = _require_intrinsic('molt_time_perf_counter_ns')
# END PUBLIC OBJECT molt_time_perf_counter_ns


# BEGIN PUBLIC OBJECT molt_time_process_time
# Generated by tools/gen_intrinsics.py from public_objects.toml.
process_time = _require_intrinsic('molt_time_process_time')
# END PUBLIC OBJECT molt_time_process_time


# BEGIN PUBLIC OBJECT molt_time_process_time_ns
# Generated by tools/gen_intrinsics.py from public_objects.toml.
process_time_ns = _require_intrinsic('molt_time_process_time_ns')
# END PUBLIC OBJECT molt_time_process_time_ns


def time() -> float:
    _require_time_wall()
    return float(_MOLT_TIME_TIME())

# BEGIN PUBLIC OBJECT molt_time_time
# Generated by tools/gen_intrinsics.py from public_objects.toml.
time.__name__ = 'time'
time.__qualname__ = 'time'
time.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(time)
# END PUBLIC OBJECT molt_time_time


def time_ns() -> int:
    _require_time_wall()
    return int(_MOLT_TIME_TIME_NS())

# BEGIN PUBLIC OBJECT molt_time_time_ns
# Generated by tools/gen_intrinsics.py from public_objects.toml.
time_ns.__name__ = 'time_ns'
time_ns.__qualname__ = 'time_ns'
time_ns.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(time_ns)
# END PUBLIC OBJECT molt_time_time_ns


def sleep(secs: float, /) -> None:
    try:
        delay = float(secs)
    except (TypeError, ValueError):
        raise TypeError("an integer or float is required")
    if delay < 0:
        raise ValueError("sleep length must be non-negative")
    _MOLT_TIME_SLEEP(delay)
    return None

# BEGIN PUBLIC OBJECT molt_time_sleep
# Generated by tools/gen_intrinsics.py from public_objects.toml.
sleep.__name__ = 'sleep'
sleep.__qualname__ = 'sleep'
sleep.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(sleep)
# END PUBLIC OBJECT molt_time_sleep


def localtime(secs: float | None = None, /) -> struct_time:
    if secs is None:
        _require_time_wall()
    parts = _MOLT_TIME_LOCALTIME(secs)
    return _struct_sequence_new(_STRUCT_TIME, parts)

# BEGIN PUBLIC OBJECT molt_time_localtime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
localtime.__name__ = 'localtime'
localtime.__qualname__ = 'localtime'
localtime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(localtime)
# END PUBLIC OBJECT molt_time_localtime


def gmtime(secs: float | None = None, /) -> struct_time:
    if secs is None:
        _require_time_wall()
    parts = _MOLT_TIME_GMTIME(secs)
    return _struct_sequence_new(_STRUCT_TIME, parts)

# BEGIN PUBLIC OBJECT molt_time_gmtime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
gmtime.__name__ = 'gmtime'
gmtime.__qualname__ = 'gmtime'
gmtime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(gmtime)
# END PUBLIC OBJECT molt_time_gmtime


def strftime(fmt: str, t: _Any | None = None, /) -> str:
    if not isinstance(fmt, str):
        name = type(fmt).__name__
        raise TypeError(f"strftime() format must be str, not {name}")
    if t is None:
        t = localtime()
    return str(_MOLT_TIME_STRFTIME(fmt, t))

# BEGIN PUBLIC OBJECT molt_time_strftime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
strftime.__name__ = 'strftime'
strftime.__qualname__ = 'strftime'
strftime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(strftime)
# END PUBLIC OBJECT molt_time_strftime


def asctime(t: _Any | None = None, /) -> str:
    if t is None:
        t = localtime()
    return str(_MOLT_TIME_ASCTIME(t))

# BEGIN PUBLIC OBJECT molt_time_asctime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
asctime.__name__ = 'asctime'
asctime.__qualname__ = 'asctime'
asctime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(asctime)
# END PUBLIC OBJECT molt_time_asctime


def ctime(secs: float | None = None, /) -> str:
    if secs is None:
        return asctime()
    return asctime(localtime(secs))

# BEGIN PUBLIC OBJECT molt_time_ctime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
ctime.__name__ = 'ctime'
ctime.__qualname__ = 'ctime'
ctime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(ctime)
# END PUBLIC OBJECT molt_time_ctime


def mktime(t: _Any, /) -> float:
    return float(_MOLT_TIME_MKTIME(t))

# BEGIN PUBLIC OBJECT molt_time_mktime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
mktime.__name__ = 'mktime'
mktime.__qualname__ = 'mktime'
mktime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(mktime)
# END PUBLIC OBJECT molt_time_mktime


def tzset() -> None:
    """Reset the timezone information from the TZ environment variable.

    Molt's deterministic compiled-binary contract has no dynamic timezone
    state — the runtime is initialized from the host environment at
    startup and stays fixed. Provided as a no-op for compatibility with
    code that calls tzset() defensively (e.g. _strptime.LocaleTime).
    """
    return None

# BEGIN PUBLIC OBJECT molt_time_tzset
# Generated by tools/gen_intrinsics.py from public_objects.toml.
tzset.__name__ = 'tzset'
tzset.__qualname__ = 'tzset'
tzset.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(tzset)
# END PUBLIC OBJECT molt_time_tzset


def strptime(data_string: str, format: str = "%a %b %d %H:%M:%S %Y", /) -> "struct_time":
    """Parse a string according to a format string and return a struct_time.

    Mirrors CPython's `time.strptime` — a thin wrapper over the
    `_strptime` module's `_strptime_time` entry point.
    """
    import _strptime as _strptime_mod  # imported lazily to avoid circular boot

    return _strptime_mod._strptime_time(data_string, format)

# BEGIN PUBLIC OBJECT molt_time_strptime
# Generated by tools/gen_intrinsics.py from public_objects.toml.
strptime.__name__ = 'strptime'
strptime.__qualname__ = 'strptime'
strptime.__module__ = 'time'
_require_intrinsic('molt_function_set_builtin')(strptime)
# END PUBLIC OBJECT molt_time_strptime


del TYPE_CHECKING
