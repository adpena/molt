"""Intrinsic-backed `_datetime` compatibility wrapper."""

from _intrinsics import require_intrinsic as _require_intrinsic
from datetime import MAXYEAR, MINYEAR, date, datetime, time, timedelta, timezone, tzinfo

_MOLT_DATETIME_RUNTIME_READY = _require_intrinsic(
    "molt_datetime_runtime_ready"
)
_MOLT_DATETIME_RUNTIME_READY()

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
