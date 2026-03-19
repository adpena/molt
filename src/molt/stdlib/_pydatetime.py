"""Intrinsic-backed `_pydatetime` compatibility wrapper."""

import sys

from _intrinsics import require_intrinsic as _require_intrinsic
from datetime import MAXYEAR, MINYEAR, date, datetime, time, timedelta, timezone, tzinfo

_MOLT_DATETIME_RUNTIME_READY = _require_intrinsic(
    "molt_datetime_runtime_ready"
)
_MOLT_DATETIME_RUNTIME_READY()

UTC = timezone.utc

__all__ = [
    "MAXYEAR",
    "MINYEAR",
    "UTC",
    "date",
    "datetime",
    "sys",
    "time",
    "timedelta",
    "timezone",
    "tzinfo",
]

globals().pop("_require_intrinsic", None)
