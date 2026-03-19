"""Intrinsic-gated datetime subset for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_DATETIME_RUNTIME_READY = _require_intrinsic(
    "molt_datetime_runtime_ready")
_MOLT_DATETIME_RUNTIME_READY()

_MOLT_DT_VALIDATE_DATE = _require_intrinsic("molt_datetime_validate_date")
_MOLT_DT_VALIDATE_TIME = _require_intrinsic("molt_datetime_validate_time")
_MOLT_DT_IS_LEAP = _require_intrinsic("molt_datetime_is_leap")
_MOLT_DT_DAYS_IN_MONTH = _require_intrinsic("molt_datetime_days_in_month")
_MOLT_DT_YMD_TO_ORDINAL = _require_intrinsic("molt_datetime_ymd_to_ordinal")
_MOLT_DT_ORDINAL_TO_YMD = _require_intrinsic("molt_datetime_ordinal_to_ymd")
_MOLT_DT_TD_NORMALIZE = _require_intrinsic("molt_datetime_td_normalize")
_MOLT_DT_TD_TOTAL_SECONDS = _require_intrinsic(
    "molt_datetime_td_total_seconds")
_MOLT_DT_NOW_LOCAL = _require_intrinsic("molt_datetime_now_local")
_MOLT_DT_NOW_UTC = _require_intrinsic("molt_datetime_now_utc")
_MOLT_DT_FROMTIMESTAMP_LOCAL = _require_intrinsic(
    "molt_datetime_fromtimestamp_local")
_MOLT_DT_FROMTIMESTAMP_UTC = _require_intrinsic(
    "molt_datetime_fromtimestamp_utc")
_MOLT_DT_TO_TIMESTAMP = _require_intrinsic("molt_datetime_to_timestamp")
_MOLT_DT_STRFTIME = _require_intrinsic("molt_datetime_strftime")
_MOLT_DT_STRPTIME = _require_intrinsic("molt_datetime_strptime")
_MOLT_DT_FORMAT_ISODATE = _require_intrinsic("molt_datetime_format_isodate")
_MOLT_DT_FORMAT_ISOTIME = _require_intrinsic("molt_datetime_format_isotime")
_MOLT_DT_FORMAT_ISODATETIME = _require_intrinsic(
    "molt_datetime_format_isodatetime")
_MOLT_DT_PARSE_ISOFORMAT = _require_intrinsic(
    "molt_datetime_parse_isoformat")
_MOLT_DT_PARSE_ISOFORMAT_DATE = _require_intrinsic(
    "molt_datetime_parse_isoformat_date")
_MOLT_DT_PARSE_ISOFORMAT_TIME = _require_intrinsic(
    "molt_datetime_parse_isoformat_time")
_MOLT_DT_HASH_DATE = _require_intrinsic("molt_datetime_hash_date")
_MOLT_DT_HASH_TIME = _require_intrinsic("molt_datetime_hash_time")
_MOLT_DT_HASH_DATETIME = _require_intrinsic("molt_datetime_hash_datetime")
_MOLT_DT_HASH_TIMEDELTA = _require_intrinsic("molt_datetime_hash_timedelta")
_MOLT_DT_WEEKDAY = _require_intrinsic("molt_datetime_weekday")
_MOLT_DT_ISOWEEKDAY = _require_intrinsic("molt_datetime_isoweekday")
_MOLT_DT_ISOCALENDAR = _require_intrinsic("molt_datetime_isocalendar")
_MOLT_DT_CTIME = _require_intrinsic("molt_datetime_ctime")
_MOLT_DT_LOCAL_UTCOFFSET = _require_intrinsic(
    "molt_datetime_local_utcoffset")
_MOLT_TIMEDELTA_ABS = _require_intrinsic("molt_timedelta_abs")
_MOLT_TIMEDELTA_TRUEDIV_TD = _require_intrinsic("molt_timedelta_truediv_td")
_MOLT_TIMEDELTA_TRUEDIV_SCALAR = _require_intrinsic(
    "molt_timedelta_truediv_scalar")
_MOLT_TIMEDELTA_FLOORDIV_TD = _require_intrinsic(
    "molt_timedelta_floordiv_td")
_MOLT_TIMEDELTA_FLOORDIV_SCALAR = _require_intrinsic(
    "molt_timedelta_floordiv_scalar")
_MOLT_TIMEDELTA_MOD_TD = _require_intrinsic("molt_timedelta_mod_td")
_MOLT_DATE_FROMISOCALENDAR = _require_intrinsic("molt_date_fromisocalendar")
_MOLT_DATETIME_COMBINE = _require_intrinsic("molt_datetime_combine")
_MOLT_DT_AS_INT = _require_intrinsic("molt_datetime_as_int")
_MOLT_DT_FORMAT_TIME = _require_intrinsic("molt_datetime_format_time")
_MOLT_TIMEDELTA_REPR = _require_intrinsic("molt_timedelta_repr")
_MOLT_TIMEDELTA_STR = _require_intrinsic("molt_timedelta_str")
_MOLT_TIMEZONE_VALIDATE = _require_intrinsic("molt_timezone_validate")
_MOLT_TIMEZONE_TZNAME = _require_intrinsic("molt_timezone_tzname")
_MOLT_DT_DATE_REPR = _require_intrinsic("molt_datetime_date_repr")
_MOLT_DT_TIME_REPR = _require_intrinsic("molt_datetime_time_repr")
_MOLT_DT_DATETIME_REPR = _require_intrinsic("molt_datetime_datetime_repr")
_MOLT_DT_TIMETUPLE = _require_intrinsic("molt_datetime_timetuple")

__all__ = [
    "MINYEAR",
    "MAXYEAR",
    "tzinfo",
    "timezone",
    "timedelta",
    "date",
    "time",
    "datetime",
]

MINYEAR = 1
MAXYEAR = 9999
_DAY_SECONDS = 86_400
_EPOCH_ORDINAL_OFFSET = 719_468


def _as_int(value: Any) -> int:
    return int(_MOLT_DT_AS_INT(value))


def _is_leap(year: int) -> bool:
    return bool(_MOLT_DT_IS_LEAP(year))


def _days_in_month(year: int, month: int) -> int:
    return int(_MOLT_DT_DAYS_IN_MONTH(year, month))


def _validate_date(year: int, month: int, day: int) -> None:
    _MOLT_DT_VALIDATE_DATE(year, month, day)


def _validate_time(
    hour: int, minute: int, second: int, microsecond: int, fold: int
) -> None:
    _MOLT_DT_VALIDATE_TIME(hour, minute, second, microsecond, fold)


def _days_from_civil(year: int, month: int, day: int) -> int:
    return int(_MOLT_DT_YMD_TO_ORDINAL(year, month, day))


def _civil_from_days(days: int) -> tuple[int, int, int]:
    result = _MOLT_DT_ORDINAL_TO_YMD(days)
    return (int(result[0]), int(result[1]), int(result[2]))


def _normalize_day_second_micro(
    days: int, seconds: int, microseconds: int
) -> tuple[int, int, int]:
    result = _MOLT_DT_TD_NORMALIZE(days, seconds, microseconds, 0, 0, 0, 0)
    return (int(result[0]), int(result[1]), int(result[2]))


_UNSET = object()
_SENTINEL = object()


def _format_time(
    hour: int, minute: int, second: int, microsecond: int, timespec: str
) -> str:
    return str(_MOLT_DT_FORMAT_TIME(hour, minute, second, microsecond, timespec))


class timedelta:
    __slots__ = ("days", "seconds", "microseconds")

    def __init__(
        self,
        days: int = 0,
        seconds: int = 0,
        microseconds: int = 0,
        milliseconds: int = 0,
        minutes: int = 0,
        hours: int = 0,
        weeks: int = 0,
    ) -> None:
        result = _MOLT_DT_TD_NORMALIZE(
            days,
            seconds,
            microseconds,
            milliseconds,
            minutes,
            hours,
            weeks,
        )
        self.days = int(result[0])
        self.seconds = int(result[1])
        self.microseconds = int(result[2])

    @classmethod
    def _from_parts(cls, days: int, seconds: int, microseconds: int) -> timedelta:
        d, s, us = _normalize_day_second_micro(days, seconds, microseconds)
        obj = cls.__new__(cls)
        obj.days = d
        obj.seconds = s
        obj.microseconds = us
        return obj

    def total_seconds(self) -> float:
        return float(
            _MOLT_DT_TD_TOTAL_SECONDS(self.days, self.seconds, self.microseconds)
        )

    def __hash__(self) -> int:
        return int(_MOLT_DT_HASH_TIMEDELTA(self.days, self.seconds, self.microseconds))

    def __add__(self, other: object) -> timedelta:
        if not isinstance(other, timedelta):
            return NotImplemented
        return timedelta._from_parts(
            self.days + other.days,
            self.seconds + other.seconds,
            self.microseconds + other.microseconds,
        )

    def __sub__(self, other: object) -> timedelta:
        if not isinstance(other, timedelta):
            return NotImplemented
        return timedelta._from_parts(
            self.days - other.days,
            self.seconds - other.seconds,
            self.microseconds - other.microseconds,
        )

    def __neg__(self) -> timedelta:
        return timedelta._from_parts(-self.days, -self.seconds, -self.microseconds)

    def __abs__(self) -> timedelta:
        d, s, us = _MOLT_TIMEDELTA_ABS(self.days, self.seconds, self.microseconds)
        return timedelta(days=int(d), seconds=int(s), microseconds=int(us))

    def __truediv__(self, other: object) -> timedelta | float:
        if isinstance(other, timedelta):
            return float(
                _MOLT_TIMEDELTA_TRUEDIV_TD(
                    self.days,
                    self.seconds,
                    self.microseconds,
                    other.days,
                    other.seconds,
                    other.microseconds,
                )
            )
        if isinstance(other, (int, float)):
            d, s, us = _MOLT_TIMEDELTA_TRUEDIV_SCALAR(
                self.days, self.seconds, self.microseconds, other
            )
            return timedelta(days=int(d), seconds=int(s), microseconds=int(us))
        return NotImplemented

    def __floordiv__(self, other: object) -> timedelta | int:
        if isinstance(other, timedelta):
            return int(
                _MOLT_TIMEDELTA_FLOORDIV_TD(
                    self.days,
                    self.seconds,
                    self.microseconds,
                    other.days,
                    other.seconds,
                    other.microseconds,
                )
            )
        if isinstance(other, int):
            d, s, us = _MOLT_TIMEDELTA_FLOORDIV_SCALAR(
                self.days, self.seconds, self.microseconds, other
            )
            return timedelta(days=int(d), seconds=int(s), microseconds=int(us))
        return NotImplemented

    def __mod__(self, other: object) -> timedelta:
        if not isinstance(other, timedelta):
            return NotImplemented
        d, s, us = _MOLT_TIMEDELTA_MOD_TD(
            self.days,
            self.seconds,
            self.microseconds,
            other.days,
            other.seconds,
            other.microseconds,
        )
        return timedelta(days=int(d), seconds=int(s), microseconds=int(us))

    def __eq__(self, other: object) -> bool:
        return (
            isinstance(other, timedelta)
            and self.days == other.days
            and self.seconds == other.seconds
            and self.microseconds == other.microseconds
        )

    def __str__(self) -> str:
        return str(_MOLT_TIMEDELTA_STR(self.days, self.seconds, self.microseconds))

    def __repr__(self) -> str:
        return str(_MOLT_TIMEDELTA_REPR(self.days, self.seconds, self.microseconds))


class tzinfo:
    def utcoffset(self, dt: datetime | None) -> timedelta | None:  # noqa: ARG002
        return None

    def dst(self, dt: datetime | None) -> timedelta | None:  # noqa: ARG002
        return None

    def tzname(self, dt: datetime | None) -> str | None:  # noqa: ARG002
        return None


class timezone(tzinfo):
    __slots__ = ("_offset", "_name")

    def __init__(self, offset: timedelta, name: str | None = None) -> None:
        if not isinstance(offset, timedelta):
            raise TypeError("offset must be a timedelta")
        _MOLT_TIMEZONE_VALIDATE(offset.days, offset.seconds)
        self._offset = offset
        self._name = name

    def utcoffset(self, dt: datetime | None) -> timedelta:  # noqa: ARG002
        return self._offset

    def dst(self, dt: datetime | None) -> timedelta:  # noqa: ARG002
        return timedelta()

    def tzname(self, dt: datetime | None) -> str:  # noqa: ARG002
        if self._name is not None:
            return self._name
        return str(
            _MOLT_TIMEZONE_TZNAME(self._offset.days, self._offset.seconds)
        )


timezone.utc = timezone(timedelta())  # type: ignore[attr-defined]
_TZINFO_TYPE = tzinfo


class date:
    __slots__ = ("year", "month", "day")

    def __init__(self, year: int, month: int, day: int) -> None:
        year = _as_int(year)
        month = _as_int(month)
        day = _as_int(day)
        _validate_date(year, month, day)
        self.year = year
        self.month = month
        self.day = day

    def isoformat(self) -> str:
        return str(_MOLT_DT_FORMAT_ISODATE(self.year, self.month, self.day))

    def __str__(self) -> str:
        return self.isoformat()

    def __repr__(self) -> str:
        return str(_MOLT_DT_DATE_REPR(self.year, self.month, self.day))

    def __hash__(self) -> int:
        return int(_MOLT_DT_HASH_DATE(self.year, self.month, self.day))

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, date):
            return NotImplemented
        return (
            self.year == other.year
            and self.month == other.month
            and self.day == other.day
        )

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, date):
            return NotImplemented
        return (self.year, self.month, self.day) < (other.year, other.month, other.day)

    def __le__(self, other: object) -> bool:
        if not isinstance(other, date):
            return NotImplemented
        return (self.year, self.month, self.day) <= (other.year, other.month, other.day)

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, date):
            return NotImplemented
        return (self.year, self.month, self.day) > (other.year, other.month, other.day)

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, date):
            return NotImplemented
        return (self.year, self.month, self.day) >= (other.year, other.month, other.day)

    def weekday(self) -> int:
        return int(_MOLT_DT_WEEKDAY(self.year, self.month, self.day))

    def isoweekday(self) -> int:
        return int(_MOLT_DT_ISOWEEKDAY(self.year, self.month, self.day))

    def isocalendar(self) -> tuple[int, int, int]:
        result = _MOLT_DT_ISOCALENDAR(self.year, self.month, self.day)
        return (int(result[0]), int(result[1]), int(result[2]))

    def ctime(self) -> str:
        return str(_MOLT_DT_CTIME(self.year, self.month, self.day, 0, 0, 0))

    def toordinal(self) -> int:
        return _days_from_civil(self.year, self.month, self.day)

    @classmethod
    def fromordinal(cls, ordinal: int) -> date:
        y, m, d = _civil_from_days(ordinal)
        return cls(y, m, d)

    @classmethod
    def today(cls) -> date:
        result = _MOLT_DT_NOW_LOCAL()
        return cls(int(result[0]), int(result[1]), int(result[2]))

    @classmethod
    def fromisocalendar(cls, year: int, week: int, day: int) -> date:
        y, m, d = _MOLT_DATE_FROMISOCALENDAR(year, week, day)
        return cls(int(y), int(m), int(d))

    @classmethod
    def fromisoformat(cls, value: str) -> date:
        if not isinstance(value, str):
            raise TypeError("fromisoformat: argument must be str")
        result = _MOLT_DT_PARSE_ISOFORMAT_DATE(value)
        if not isinstance(result, (list, tuple)) or len(result) < 3:
            raise ValueError(f"Invalid isoformat string: {value!r}")
        return cls(int(result[0]), int(result[1]), int(result[2]))

    def replace(
        self,
        year: int | None = None,
        month: int | None = None,
        day: int | None = None,
    ) -> date:
        return date(
            year if year is not None else self.year,
            month if month is not None else self.month,
            day if day is not None else self.day,
        )

    def strftime(self, fmt: str) -> str:
        return str(
            _MOLT_DT_STRFTIME(self.year, self.month, self.day, 0, 0, 0, 0, "", fmt)
        )


class time:
    __slots__ = ("hour", "minute", "second", "microsecond", "tzinfo", "fold")

    def __init__(
        self,
        hour: int = 0,
        minute: int = 0,
        second: int = 0,
        microsecond: int = 0,
        tzinfo: tzinfo | None = None,
        *,
        fold: int = 0,
    ) -> None:
        hour = _as_int(hour)
        minute = _as_int(minute)
        second = _as_int(second)
        microsecond = _as_int(microsecond)
        fold = _as_int(fold)
        _validate_time(hour, minute, second, microsecond, fold)
        if tzinfo is not None and not isinstance(tzinfo, _TZINFO_TYPE):
            raise TypeError("tzinfo argument must be None or of a tzinfo subclass")
        self.hour = hour
        self.minute = minute
        self.second = second
        self.microsecond = microsecond
        self.tzinfo = tzinfo
        self.fold = fold

    def isoformat(self, timespec: str = "auto") -> str:
        return str(
            _MOLT_DT_FORMAT_ISOTIME(
                self.hour, self.minute, self.second, self.microsecond, timespec
            )
        )

    def __str__(self) -> str:
        return self.isoformat()

    def __repr__(self) -> str:
        return str(
            _MOLT_DT_TIME_REPR(self.hour, self.minute, self.second, self.microsecond)
        )

    def __hash__(self) -> int:
        return int(
            _MOLT_DT_HASH_TIME(self.hour, self.minute, self.second, self.microsecond)
        )

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, time):
            return NotImplemented
        return (
            self.hour == other.hour
            and self.minute == other.minute
            and self.second == other.second
            and self.microsecond == other.microsecond
        )

    def replace(
        self,
        hour: int | None = None,
        minute: int | None = None,
        second: int | None = None,
        microsecond: int | None = None,
        tzinfo: Any = _UNSET,
        *,
        fold: int | None = None,
    ) -> time:
        return time(
            hour if hour is not None else self.hour,
            minute if minute is not None else self.minute,
            second if second is not None else self.second,
            microsecond if microsecond is not None else self.microsecond,
            tzinfo=tzinfo if tzinfo is not _UNSET else self.tzinfo,
            fold=fold if fold is not None else self.fold,
        )

    @classmethod
    def fromisoformat(cls, value: str) -> time:
        if not isinstance(value, str):
            raise TypeError("fromisoformat: argument must be str")
        result = _MOLT_DT_PARSE_ISOFORMAT_TIME(value)
        if not isinstance(result, (list, tuple)) or len(result) < 4:
            raise ValueError(f"Invalid isoformat string: {value!r}")
        h, mi, sec, us = (int(result[i]) for i in range(4))
        tz: tzinfo | None = None
        if len(result) >= 5 and result[4] is not None:
            off_seconds = int(result[4])
            tz = timezone(timedelta(seconds=off_seconds))
        return cls(h, mi, sec, us, tzinfo=tz)


class datetime:
    __slots__ = (
        "year",
        "month",
        "day",
        "hour",
        "minute",
        "second",
        "microsecond",
        "tzinfo",
        "fold",
    )

    def __init__(
        self,
        year: int,
        month: int,
        day: int,
        hour: int = 0,
        minute: int = 0,
        second: int = 0,
        microsecond: int = 0,
        tzinfo: tzinfo | None = None,
        *,
        fold: int = 0,
    ) -> None:
        year = _as_int(year)
        month = _as_int(month)
        day = _as_int(day)
        hour = _as_int(hour)
        minute = _as_int(minute)
        second = _as_int(second)
        microsecond = _as_int(microsecond)
        fold = _as_int(fold)
        _validate_date(year, month, day)
        _validate_time(hour, minute, second, microsecond, fold)
        if tzinfo is not None and not isinstance(tzinfo, _TZINFO_TYPE):
            raise TypeError("tzinfo argument must be None or of a tzinfo subclass")
        self.year = year
        self.month = month
        self.day = day
        self.hour = hour
        self.minute = minute
        self.second = second
        self.microsecond = microsecond
        self.tzinfo = tzinfo
        self.fold = fold

    @classmethod
    def _from_parts(
        cls,
        days: int,
        seconds: int,
        microseconds: int,
        tz: tzinfo | None,
        fold: int = 0,
    ) -> datetime:
        days, seconds, microseconds = _normalize_day_second_micro(
            days, seconds, microseconds
        )
        year, month, day = _civil_from_days(days)
        hour, rem = divmod(seconds, 3600)
        minute, second = divmod(rem, 60)
        return cls(
            year,
            month,
            day,
            int(hour),
            int(minute),
            int(second),
            int(microseconds),
            tzinfo=tz,
            fold=fold,
        )

    def _parts_naive(self) -> tuple[int, int, int]:
        days = _days_from_civil(self.year, self.month, self.day)
        seconds = self.hour * 3600 + self.minute * 60 + self.second
        return (days, seconds, self.microsecond)

    def _parts_utc(self) -> tuple[int, int, int]:
        days, seconds, microseconds = self._parts_naive()
        offset = self.utcoffset()
        if offset is None:
            return (days, seconds, microseconds)
        return _normalize_day_second_micro(
            days - offset.days,
            seconds - offset.seconds,
            microseconds - offset.microseconds,
        )

    def date(self) -> date:
        return date(self.year, self.month, self.day)

    def utcoffset(self) -> timedelta | None:
        if self.tzinfo is None:
            return None
        out = self.tzinfo.utcoffset(self)
        if out is not None and not isinstance(out, timedelta):
            raise TypeError("tzinfo.utcoffset() must return timedelta or None")
        return out

    def dst(self) -> timedelta | None:
        if self.tzinfo is None:
            return None
        out = self.tzinfo.dst(self)
        if out is not None and not isinstance(out, timedelta):
            raise TypeError("tzinfo.dst() must return timedelta or None")
        return out

    def tzname(self) -> str | None:
        if self.tzinfo is None:
            return None
        return self.tzinfo.tzname(self)

    def isoformat(self, sep: str = "T", timespec: str = "auto") -> str:
        offset = self.utcoffset()
        tz_str = ""
        if offset is not None:
            total = offset.days * _DAY_SECONDS + offset.seconds
            sign = "+" if total >= 0 else "-"
            total = abs(total)
            hh, rem = divmod(total, 3600)
            mm, _ = divmod(rem, 60)
            tz_str = f"{sign}{hh:02d}:{mm:02d}"
        return str(
            _MOLT_DT_FORMAT_ISODATETIME(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
                self.microsecond,
                sep,
                timespec,
                tz_str,
            )
        )

    def __str__(self) -> str:
        return self.isoformat(sep=" ")

    @classmethod
    def fromisoformat(cls, value: str) -> datetime:
        if not isinstance(value, str):
            raise TypeError("fromisoformat: argument must be str")
        result = _MOLT_DT_PARSE_ISOFORMAT(value)
        if not isinstance(result, (list, tuple)) or len(result) < 7:
            raise ValueError("Invalid isoformat string")
        y, mo, d, hh, mm, ss, us = (int(result[i]) for i in range(7))
        tz: tzinfo | None = None
        if len(result) >= 8 and result[7] is not None:
            off_seconds = int(result[7])
            tz = timezone(timedelta(seconds=off_seconds))
        return cls(y, mo, d, hh, mm, ss, us, tzinfo=tz)

    def astimezone(self, tz: tzinfo | None = None) -> datetime:
        if tz is None:
            tz = timezone.utc
        if not isinstance(tz, _TZINFO_TYPE):
            raise TypeError("tz argument must be an instance of tzinfo")
        if self.tzinfo is None:
            return datetime(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
                self.microsecond,
                tzinfo=tz,
                fold=self.fold,
            )
        utc_days, utc_seconds, utc_micros = self._parts_utc()
        target_off = tz.utcoffset(None)
        if target_off is None:
            target_off = timedelta()
        return datetime._from_parts(
            utc_days + target_off.days,
            utc_seconds + target_off.seconds,
            utc_micros + target_off.microseconds,
            tz,
            fold=self.fold,
        )

    def __sub__(self, other: object) -> timedelta | datetime:
        if isinstance(other, timedelta):
            return datetime._from_parts(
                _days_from_civil(self.year, self.month, self.day) - other.days,
                self.hour * 3600 + self.minute * 60 + self.second - other.seconds,
                self.microsecond - other.microseconds,
                self.tzinfo,
                fold=self.fold,
            )
        if not isinstance(other, datetime):
            return NotImplemented
        self_aware = self.tzinfo is not None
        other_aware = other.tzinfo is not None
        if self_aware != other_aware:
            raise TypeError("can't subtract offset-naive and offset-aware datetimes")
        if self_aware:
            left = self._parts_utc()
            right = other._parts_utc()
        else:
            left = self._parts_naive()
            right = other._parts_naive()
        return timedelta._from_parts(
            left[0] - right[0],
            left[1] - right[1],
            left[2] - right[2],
        )

    def __add__(self, other: object) -> datetime:
        if not isinstance(other, timedelta):
            return NotImplemented
        d, s, us = self._parts_naive()
        return datetime._from_parts(
            d + other.days,
            s + other.seconds,
            us + other.microseconds,
            self.tzinfo,
            fold=self.fold,
        )

    __radd__ = __add__

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, datetime):
            return False
        self_aware = self.tzinfo is not None
        other_aware = other.tzinfo is not None
        if self_aware != other_aware:
            return False
        if self_aware:
            return self._parts_utc() == other._parts_utc()
        return self._parts_naive() == other._parts_naive()

    def _compare(self, other: datetime, op: str) -> bool:
        self_aware = self.tzinfo is not None
        other_aware = other.tzinfo is not None
        if self_aware != other_aware:
            raise TypeError("can't compare offset-naive and offset-aware datetimes")
        left = self._parts_utc() if self_aware else self._parts_naive()
        right = other._parts_utc() if other_aware else other._parts_naive()
        if op == "<":
            return left < right
        if op == "<=":
            return left <= right
        if op == ">":
            return left > right
        if op == ">=":
            return left >= right
        raise RuntimeError("invalid comparison")

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, datetime):
            return NotImplemented
        return self._compare(other, "<")

    def __le__(self, other: object) -> bool:
        if not isinstance(other, datetime):
            return NotImplemented
        return self._compare(other, "<=")

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, datetime):
            return NotImplemented
        return self._compare(other, ">")

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, datetime):
            return NotImplemented
        return self._compare(other, ">=")

    def strftime(self, fmt: str) -> str:
        if not isinstance(fmt, str):
            raise TypeError("strftime() argument 1 must be str")
        tz_str = self.tzname() or ""
        return str(
            _MOLT_DT_STRFTIME(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
                self.microsecond,
                tz_str,
                fmt,
            )
        )

    @classmethod
    def strptime(cls, text: str, fmt: str) -> datetime:
        result = _MOLT_DT_STRPTIME(text, fmt)
        if isinstance(result, (list, tuple)) and len(result) >= 7:
            y, mo, d, hh, mm, ss, us = (int(result[i]) for i in range(7))
            tz: tzinfo | None = None
            if len(result) >= 8 and result[7] is not None:
                off_seconds = int(result[7])
                tz = timezone(timedelta(seconds=off_seconds))
            return cls(y, mo, d, hh, mm, ss, us, tzinfo=tz)
        raise ValueError("time data does not match format")

    def __hash__(self) -> int:
        offset = self.utcoffset()
        off_secs = 0
        if offset is not None:
            off_secs = offset.days * _DAY_SECONDS + offset.seconds
        return int(
            _MOLT_DT_HASH_DATETIME(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
                self.microsecond,
                off_secs,
            )
        )

    def weekday(self) -> int:
        return int(_MOLT_DT_WEEKDAY(self.year, self.month, self.day))

    def isoweekday(self) -> int:
        return int(_MOLT_DT_ISOWEEKDAY(self.year, self.month, self.day))

    def isocalendar(self) -> tuple[int, int, int]:
        result = _MOLT_DT_ISOCALENDAR(self.year, self.month, self.day)
        return (int(result[0]), int(result[1]), int(result[2]))

    def ctime(self) -> str:
        return str(
            _MOLT_DT_CTIME(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
            )
        )

    def timestamp(self) -> float:
        offset = self.utcoffset()
        off_secs = None
        if offset is not None:
            off_secs = offset.days * _DAY_SECONDS + offset.seconds
        return float(
            _MOLT_DT_TO_TIMESTAMP(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
                self.microsecond,
                off_secs,
            )
        )

    @classmethod
    def now(cls, tz: tzinfo | None = None) -> datetime:
        if tz is None:
            result = _MOLT_DT_NOW_LOCAL()
        else:
            result = _MOLT_DT_NOW_UTC()
        dt = cls(
            int(result[0]),
            int(result[1]),
            int(result[2]),
            int(result[3]),
            int(result[4]),
            int(result[5]),
            int(result[6]),
        )
        if tz is not None:
            dt = dt.replace(tzinfo=timezone.utc).astimezone(tz)
        return dt

    @classmethod
    def utcnow(cls) -> datetime:
        result = _MOLT_DT_NOW_UTC()
        return cls(
            int(result[0]),
            int(result[1]),
            int(result[2]),
            int(result[3]),
            int(result[4]),
            int(result[5]),
            int(result[6]),
        )

    @classmethod
    def fromtimestamp(cls, ts: float, tz: tzinfo | None = None) -> datetime:
        if tz is None:
            result = _MOLT_DT_FROMTIMESTAMP_LOCAL(ts)
        else:
            result = _MOLT_DT_FROMTIMESTAMP_UTC(ts)
        dt = cls(
            int(result[0]),
            int(result[1]),
            int(result[2]),
            int(result[3]),
            int(result[4]),
            int(result[5]),
            int(result[6]),
        )
        if tz is not None:
            dt = dt.replace(tzinfo=timezone.utc).astimezone(tz)
        return dt

    @classmethod
    def combine(
        cls,
        date: date,
        time: time,
        tzinfo: tzinfo | None = None,
    ) -> datetime:
        tz = tzinfo if tzinfo is not None else time.tzinfo
        result = _MOLT_DATETIME_COMBINE(
            date.year,
            date.month,
            date.day,
            time.hour,
            time.minute,
            time.second,
            time.microsecond,
            getattr(time, "fold", 0),
        )
        return cls(
            int(result[0]),
            int(result[1]),
            int(result[2]),
            int(result[3]),
            int(result[4]),
            int(result[5]),
            int(result[6]),
            tzinfo=tz,
            fold=int(result[7]),
        )

    def replace(
        self,
        year: int | None = None,
        month: int | None = None,
        day: int | None = None,
        hour: int | None = None,
        minute: int | None = None,
        second: int | None = None,
        microsecond: int | None = None,
        tzinfo: Any = _SENTINEL,
        *,
        fold: int | None = None,
    ) -> datetime:
        return datetime(
            year if year is not None else self.year,
            month if month is not None else self.month,
            day if day is not None else self.day,
            hour if hour is not None else self.hour,
            minute if minute is not None else self.minute,
            second if second is not None else self.second,
            microsecond if microsecond is not None else self.microsecond,
            tzinfo=tzinfo if tzinfo is not _SENTINEL else self.tzinfo,
            fold=fold if fold is not None else self.fold,
        )

    def timetuple(self) -> tuple[int, ...]:
        yday = self.toordinal() - date(self.year, 1, 1).toordinal() + 1
        dst_flag = -1
        d = self.dst()
        if d is not None:
            dst_flag = 1 if d.total_seconds() > 0 else 0
        return _MOLT_DT_TIMETUPLE(
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.weekday(),
            yday,
            dst_flag,
        )

    def toordinal(self) -> int:
        return _days_from_civil(self.year, self.month, self.day)


timedelta.resolution = timedelta(microseconds=1)  # type: ignore[attr-defined]
timedelta.min = timedelta(days=-999999999)  # type: ignore[attr-defined]
timedelta.max = timedelta(
    days=999999999, hours=23, minutes=59, seconds=59, microseconds=999999
)  # type: ignore[attr-defined]
date.min = date(MINYEAR, 1, 1)  # type: ignore[attr-defined]
date.max = date(MAXYEAR, 12, 31)  # type: ignore[attr-defined]
date.resolution = timedelta(days=1)  # type: ignore[attr-defined]
