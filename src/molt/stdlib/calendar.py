"""Calendar functions — Gregorian-calendar utilities matching CPython 3.12.

Direct port of the constants, enums, and pure utility functions that
external code (datetime.strptime, _strptime, time formatting) depends on.
The high-level printing classes (Calendar, TextCalendar, HTMLCalendar)
are not yet ported — those are presentation-layer and rarely needed in
compiled-binary deployments. They will be added when a real consumer
appears; until then `__getattr__` raises AttributeError matching
CPython's behavior for module-level missing names.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import datetime
import warnings
from enum import IntEnum, global_enum

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY

__all__ = [
    "IllegalMonthError",
    "IllegalWeekdayError",
    "setfirstweekday",
    "firstweekday",
    "isleap",
    "leapdays",
    "weekday",
    "monthrange",
    "monthcalendar",
    "timegm",
    "month_name",
    "month_abbr",
    "day_name",
    "day_abbr",
    "weekheader",
    "Day",
    "Month",
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
]


# Exception raised for bad input (with string parameter for details).
error = ValueError


class IllegalMonthError(ValueError, IndexError):
    def __init__(self, month):
        self.month = month

    def __str__(self):
        return "bad month number %r; must be 1-12" % self.month


class IllegalWeekdayError(ValueError):
    def __init__(self, weekday):
        self.weekday = weekday

    def __str__(self):
        return "bad weekday number %r; must be 0 (Monday) to 6 (Sunday)" % self.weekday


def __getattr__(name):
    if name in ("January", "February"):
        warnings.warn(
            f"The '{name}' attribute is deprecated, use '{name.upper()}' instead",
            DeprecationWarning,
            stacklevel=2,
        )
        return 1 if name == "January" else 2
    raise AttributeError(f"module 'calendar' has no attribute {name!r}")


# Month constants (also exported into module globals via @global_enum).
@global_enum
class Month(IntEnum):
    JANUARY = 1
    FEBRUARY = 2
    MARCH = 3
    APRIL = 4
    MAY = 5
    JUNE = 6
    JULY = 7
    AUGUST = 8
    SEPTEMBER = 9
    OCTOBER = 10
    NOVEMBER = 11
    DECEMBER = 12


# Day constants — Monday is 0, Sunday is 6 (CPython convention).
@global_enum
class Day(IntEnum):
    MONDAY = 0
    TUESDAY = 1
    WEDNESDAY = 2
    THURSDAY = 3
    FRIDAY = 4
    SATURDAY = 5
    SUNDAY = 6


# Number of days per month (except for February in leap years).
mdays = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]


# Locale-independent month/day name tables. CPython exposes these as
# locale-aware via datetime.strftime, but for the deterministic
# compiled-binary contract we use the canonical English C-locale names
# — same as `setlocale(LC_TIME, "C")`.
_FULL_MONTH_NAMES = [
    "",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
]

_ABBR_MONTH_NAMES = [
    "",
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
]

_FULL_DAY_NAMES = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
]

_ABBR_DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]


class _NameTable:
    """Read-only sequence with len-13 (months) or len-7 (days) shape."""

    __slots__ = ("_data",)

    def __init__(self, data):
        self._data = list(data)

    def __getitem__(self, i):
        return self._data[i]

    def __len__(self):
        return len(self._data)

    def __iter__(self):
        return iter(self._data)

    def __contains__(self, item):
        return item in self._data

    def __repr__(self):
        return repr(self._data)


month_name = _NameTable(_FULL_MONTH_NAMES)
month_abbr = _NameTable(_ABBR_MONTH_NAMES)
day_name = _NameTable(_FULL_DAY_NAMES)
day_abbr = _NameTable(_ABBR_DAY_NAMES)


# Module-level firstweekday (Monday by default).
_firstweekday = 0


def firstweekday():
    return _firstweekday


def setfirstweekday(weekday):
    """Set weekday (0=Monday, 6=Sunday) to start each week."""
    global _firstweekday
    if not (0 <= weekday <= 6):
        raise IllegalWeekdayError(weekday)
    _firstweekday = weekday


def isleap(year):
    """Return True for leap years, False for non-leap years."""
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)


def leapdays(y1, y2):
    """Return number of leap years in range [y1, y2). Assume y1 <= y2."""
    y1 -= 1
    y2 -= 1
    return (y2 // 4 - y1 // 4) - (y2 // 100 - y1 // 100) + (y2 // 400 - y1 // 400)


def weekday(year, month, day):
    """Return weekday (0-6 ~ Mon-Sun) for year (1970-...), month (1-12),
    day (1-31)."""
    if not datetime.MINYEAR <= year <= datetime.MAXYEAR:
        year = 2000 + year % 400
    return datetime.date(year, month, day).weekday()


def monthrange(year, month):
    """Return weekday of first day of the month and number of days in month,
    for the specified year and month."""
    if not 1 <= month <= 12:
        raise IllegalMonthError(month)
    day1 = weekday(year, month, 1)
    ndays = mdays[month] + (month == 2 and isleap(year))
    return day1, ndays


def _monthlen(year, month):
    return mdays[month] + (month == 2 and isleap(year))


def _prevmonth(year, month):
    if month == 1:
        return year - 1, 12
    return year, month - 1


def _nextmonth(year, month):
    if month == 12:
        return year + 1, 1
    return year, month + 1


def monthcalendar(year, month):
    """Return a matrix representing a month's calendar.

    Each row represents a week; days outside of the month are zero.
    """
    day1, ndays = monthrange(year, month)
    rows = []
    days = list(range(1, ndays + 1))
    leading_blanks = (day1 - _firstweekday) % 7
    week = [0] * leading_blanks
    for d in days:
        week.append(d)
        if len(week) == 7:
            rows.append(week)
            week = []
    if week:
        week.extend([0] * (7 - len(week)))
        rows.append(week)
    return rows


def weekheader(n):
    """Return a header for a week of width n columns."""
    names = day_abbr if n < 9 else day_name
    return " ".join(name[:n].center(n) for name in names)


_EPOCH = 1970


def timegm(tuple):
    """Unrelated but handy: convert a tuple representing UTC time to an
    epoch seconds value. Mirrors CPython's calendar.timegm."""
    year, month, day, hour, minute, second = tuple[:6]
    days = (
        datetime.date(year, month, day).toordinal()
        - datetime.date(_EPOCH, 1, 1).toordinal()
    )
    hours = days * 24 + hour
    minutes = hours * 60 + minute
    return minutes * 60 + second


globals().pop("_require_intrinsic", None)
