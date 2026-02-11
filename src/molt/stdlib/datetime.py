"""Intrinsic-gated datetime subset for Molt."""

from __future__ import annotations

import re
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_DATETIME_RUNTIME_READY = _require_intrinsic(
    "molt_datetime_runtime_ready", globals()
)
_MOLT_DATETIME_RUNTIME_READY()

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:in_progress): lower datetime/date/time arithmetic and parsing primitives into dedicated Rust intrinsics.

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
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    raise TypeError(f"integer argument expected, got {type(value).__name__}")


def _is_leap(year: int) -> bool:
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)


def _days_in_month(year: int, month: int) -> int:
    if month == 2:
        return 29 if _is_leap(year) else 28
    if month in (4, 6, 9, 11):
        return 30
    return 31


def _validate_date(year: int, month: int, day: int) -> None:
    if not (MINYEAR <= year <= MAXYEAR):
        raise ValueError("year out of range")
    if not (1 <= month <= 12):
        raise ValueError("month must be in 1..12")
    dim = _days_in_month(year, month)
    if not (1 <= day <= dim):
        raise ValueError("day is out of range for month")


def _validate_time(
    hour: int, minute: int, second: int, microsecond: int, fold: int
) -> None:
    if not (0 <= hour <= 23):
        raise ValueError("hour must be in 0..23")
    if not (0 <= minute <= 59):
        raise ValueError("minute must be in 0..59")
    if not (0 <= second <= 59):
        raise ValueError("second must be in 0..59")
    if not (0 <= microsecond <= 999_999):
        raise ValueError("microsecond must be in 0..999999")
    if fold not in (0, 1):
        raise ValueError("fold must be either 0 or 1")


def _days_from_civil(year: int, month: int, day: int) -> int:
    y = year - (1 if month <= 2 else 0)
    era = y // 400 if y >= 0 else (y - 399) // 400
    yoe = y - era * 400
    mp = month - 3 if month > 2 else month + 9
    doy = (153 * mp + 2) // 5 + day - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return era * 146097 + doe - _EPOCH_ORDINAL_OFFSET


def _civil_from_days(days: int) -> tuple[int, int, int]:
    z = days + _EPOCH_ORDINAL_OFFSET
    era = z // 146097 if z >= 0 else (z - 146096) // 146097
    doe = z - era * 146097
    yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
    year = yoe + era * 400
    doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
    mp = (5 * doy + 2) // 153
    day = doy - (153 * mp + 2) // 5 + 1
    month = mp + 3 if mp < 10 else mp - 9
    year += 0 if month > 2 else 1
    return (year, month, day)


def _normalize_day_second_micro(
    days: int, seconds: int, microseconds: int
) -> tuple[int, int, int]:
    sec_carry, microseconds = divmod(microseconds, 1_000_000)
    seconds += sec_carry
    day_carry, seconds = divmod(seconds, _DAY_SECONDS)
    days += day_carry
    return int(days), int(seconds), int(microseconds)


def _format_time(
    hour: int, minute: int, second: int, microsecond: int, timespec: str
) -> str:
    base = f"{hour:02d}:{minute:02d}:{second:02d}"
    if timespec == "auto":
        return f"{base}.{microsecond:06d}" if microsecond else base
    if timespec == "seconds":
        return base
    if timespec == "milliseconds":
        return f"{base}.{microsecond // 1000:03d}"
    if timespec == "microseconds":
        return f"{base}.{microsecond:06d}"
    raise ValueError("Unknown timespec value")


class timedelta:
    __slots__ = ("days", "seconds", "microseconds")

    def __init__(
        self,
        *,
        days: int = 0,
        seconds: int = 0,
        microseconds: int = 0,
        milliseconds: int = 0,
        minutes: int = 0,
        hours: int = 0,
        weeks: int = 0,
    ) -> None:
        d = _as_int(days) + _as_int(weeks) * 7
        s = _as_int(seconds) + _as_int(minutes) * 60 + _as_int(hours) * 3600
        us = _as_int(microseconds) + _as_int(milliseconds) * 1000
        d, s, us = _normalize_day_second_micro(d, s, us)
        self.days = d
        self.seconds = s
        self.microseconds = us

    @classmethod
    def _from_parts(cls, days: int, seconds: int, microseconds: int) -> timedelta:
        d, s, us = _normalize_day_second_micro(days, seconds, microseconds)
        obj = cls.__new__(cls)
        obj.days = d
        obj.seconds = s
        obj.microseconds = us
        return obj

    def total_seconds(self) -> float:
        return self.days * _DAY_SECONDS + self.seconds + self.microseconds / 1_000_000.0

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

    def __eq__(self, other: object) -> bool:
        return (
            isinstance(other, timedelta)
            and self.days == other.days
            and self.seconds == other.seconds
            and self.microseconds == other.microseconds
        )

    def _format_positive(self) -> str:
        days = self.days
        hours, rem = divmod(self.seconds, 3600)
        minutes, seconds = divmod(rem, 60)
        if days:
            day_word = "day" if abs(days) == 1 else "days"
            prefix = f"{days} {day_word}, "
        else:
            prefix = ""
        if self.microseconds:
            return (
                f"{prefix}{hours}:{minutes:02d}:{seconds:02d}.{self.microseconds:06d}"
            )
        return f"{prefix}{hours}:{minutes:02d}:{seconds:02d}"

    def __str__(self) -> str:
        return self._format_positive()

    def __repr__(self) -> str:
        return (
            "timedelta("
            f"days={self.days}, seconds={self.seconds}, microseconds={self.microseconds})"
        )


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
        total = offset.days * _DAY_SECONDS + offset.seconds
        if not (-_DAY_SECONDS < total < _DAY_SECONDS):
            raise ValueError("offset must be strictly between -24h and +24h")
        self._offset = offset
        self._name = name

    def utcoffset(self, dt: datetime | None) -> timedelta:  # noqa: ARG002
        return self._offset

    def dst(self, dt: datetime | None) -> timedelta:  # noqa: ARG002
        return timedelta()

    def tzname(self, dt: datetime | None) -> str:  # noqa: ARG002
        if self._name is not None:
            return self._name
        total = self._offset.days * _DAY_SECONDS + self._offset.seconds
        sign = "+" if total >= 0 else "-"
        total = abs(total)
        hh, rem = divmod(total, 3600)
        mm, _ = divmod(rem, 60)
        if hh == 0 and mm == 0:
            return "UTC"
        return f"UTC{sign}{hh:02d}:{mm:02d}"


timezone.utc = timezone(timedelta())  # type: ignore[attr-defined]


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
        return f"{self.year:04d}-{self.month:02d}-{self.day:02d}"

    def __str__(self) -> str:
        return self.isoformat()


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
        if tzinfo is not None and not isinstance(tzinfo, globals()["tzinfo"]):
            raise TypeError("tzinfo argument must be None or of a tzinfo subclass")
        self.hour = hour
        self.minute = minute
        self.second = second
        self.microsecond = microsecond
        self.tzinfo = tzinfo
        self.fold = fold

    def isoformat(self, timespec: str = "auto") -> str:
        return _format_time(
            self.hour, self.minute, self.second, self.microsecond, timespec=timespec
        )

    def __str__(self) -> str:
        return self.isoformat()


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

    _ISO_RE = re.compile(
        r"^(\d{4})-(\d{2})-(\d{2})"
        r"(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,6}))?)?"
        r"(?:([Zz]|[+-]\d{2}:\d{2}))?)?$"
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
        if tzinfo is not None and not isinstance(tzinfo, globals()["tzinfo"]):
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
        out = (
            f"{self.year:04d}-{self.month:02d}-{self.day:02d}{sep}"
            f"{_format_time(self.hour, self.minute, self.second, self.microsecond, timespec)}"
        )
        offset = self.utcoffset()
        if offset is not None:
            total = offset.days * _DAY_SECONDS + offset.seconds
            sign = "+" if total >= 0 else "-"
            total = abs(total)
            hh, rem = divmod(total, 3600)
            mm, _ = divmod(rem, 60)
            out += f"{sign}{hh:02d}:{mm:02d}"
        return out

    def __str__(self) -> str:
        return self.isoformat(sep=" ")

    @classmethod
    def fromisoformat(cls, value: str) -> datetime:
        if not isinstance(value, str):
            raise TypeError("fromisoformat: argument must be str")
        match = cls._ISO_RE.fullmatch(value)
        if match is None:
            raise ValueError("Invalid isoformat string")
        y_s, mo_s, d_s, hh_s, mm_s, ss_s, us_s, tz_s = match.groups()
        year = int(y_s)
        month = int(mo_s)
        day = int(d_s)
        hour = int(hh_s or "0")
        minute = int(mm_s or "0")
        second = int(ss_s or "0")
        us_txt = us_s or "0"
        if len(us_txt) < 6:
            us_txt = us_txt + ("0" * (6 - len(us_txt)))
        microsecond = int(us_txt[:6])

        tz: tzinfo | None = None
        if tz_s:
            if tz_s in ("Z", "z"):
                tz = timezone.utc
            else:
                sign = 1 if tz_s[0] == "+" else -1
                off_h = int(tz_s[1:3])
                off_m = int(tz_s[4:6])
                tz = timezone(timedelta(hours=sign * off_h, minutes=sign * off_m))
        return cls(
            year,
            month,
            day,
            hour,
            minute,
            second,
            microsecond,
            tzinfo=tz,
        )

    def astimezone(self, tz: tzinfo | None = None) -> datetime:
        if tz is None:
            tz = timezone.utc
        if not isinstance(tz, globals()["tzinfo"]):
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
        offset = self.utcoffset()
        if offset is None:
            z_num = ""
        else:
            total = offset.days * _DAY_SECONDS + offset.seconds
            sign = "+" if total >= 0 else "-"
            total = abs(total)
            hh, rem = divmod(total, 3600)
            mm, _ = divmod(rem, 60)
            z_num = f"{sign}{hh:02d}{mm:02d}"
        replacements = {
            "%Y": f"{self.year:04d}",
            "%m": f"{self.month:02d}",
            "%d": f"{self.day:02d}",
            "%H": f"{self.hour:02d}",
            "%M": f"{self.minute:02d}",
            "%S": f"{self.second:02d}",
            "%f": f"{self.microsecond:06d}",
            "%z": z_num,
            "%Z": self.tzname() or "",
        }
        out = fmt
        for key, value in replacements.items():
            out = out.replace(key, value)
        return out

    @classmethod
    def strptime(cls, text: str, fmt: str) -> datetime:
        if fmt == "%Y-%m-%d %H:%M:%S":
            match = re.fullmatch(
                r"(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2}):(\d{2})", text
            )
            if match is None:
                raise ValueError("time data does not match format")
            y, mo, d, hh, mm, ss = (int(x) for x in match.groups())
            return cls(y, mo, d, hh, mm, ss)
        if fmt == "%m/%d/%Y %H:%M:%S":
            match = re.fullmatch(
                r"(\d{2})/(\d{2})/(\d{4}) (\d{2}):(\d{2}):(\d{2})", text
            )
            if match is None:
                raise ValueError("time data does not match format")
            mo, d, y, hh, mm, ss = (int(x) for x in match.groups())
            return cls(y, mo, d, hh, mm, ss)
        if fmt == "%Y-%j":
            match = re.fullmatch(r"(\d{4})-(\d{3})", text)
            if match is None:
                raise ValueError("time data does not match format")
            year = int(match.group(1))
            day_of_year = int(match.group(2))
            max_day = 366 if _is_leap(year) else 365
            if not (1 <= day_of_year <= max_day):
                raise ValueError("day of year out of range")
            month = 1
            day = day_of_year
            while True:
                dim = _days_in_month(year, month)
                if day <= dim:
                    break
                day -= dim
                month += 1
            return cls(year, month, day)
        if fmt == "%Y-%m-%d %H:%M:%S %z":
            match = re.fullmatch(
                r"(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2}):(\d{2}) ([+-]\d{4})", text
            )
            if match is None:
                raise ValueError("time data does not match format")
            y, mo, d, hh, mm, ss = (int(x) for x in match.groups()[:6])
            z = match.group(7)
            sign = 1 if z[0] == "+" else -1
            off_h = int(z[1:3]) * sign
            off_m = int(z[3:5]) * sign
            tz = timezone(timedelta(hours=off_h, minutes=off_m))
            return cls(y, mo, d, hh, mm, ss, tzinfo=tz)
        if fmt == "%Y-%m-%d %H:%M:%S %Z":
            raise ValueError("time data does not match format")
        raise ValueError("unsupported strptime format")


timedelta.resolution = timedelta(microseconds=1)  # type: ignore[attr-defined]
