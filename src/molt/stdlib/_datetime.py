"""Intrinsic-backed `_datetime` compatibility wrapper."""

from datetime import MAXYEAR, MINYEAR, date, datetime, time, timedelta, timezone, tzinfo


_PyCapsule = type("PyCapsule", (), {"__slots__": ()})


UTC = timezone.utc
datetime_CAPI = _PyCapsule()

__all__ = [
    "MAXYEAR",
    "MINYEAR",
    "UTC",
    "date",
    "datetime",
    "datetime_CAPI",
    "time",
    "timedelta",
    "timezone",
    "tzinfo",
]
