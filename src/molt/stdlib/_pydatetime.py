"""Intrinsic-backed `_pydatetime` compatibility wrapper."""

import sys

from datetime import MAXYEAR, MINYEAR, date, datetime, time, timedelta, timezone, tzinfo


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
