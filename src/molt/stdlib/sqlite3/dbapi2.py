"""DB-API 2.0 surface for the Molt SQLite driver.

Mirrors CPython's :mod:`sqlite3.dbapi2`: re-exports the low-level driver
from :mod:`_sqlite3` and adds the DB-API type constructors (``Date``,
``Time``, ``Timestamp``, ``DateFromTicks``, ``TimeFromTicks``,
``TimestampFromTicks``, ``Binary``).
"""

# fmt: off
# pylint: disable=all
# ruff: noqa

from __future__ import annotations

import datetime
import time

# Keep the module inside the intrinsic-backed stdlib gate.  All the
# real intrinsics are required by `_sqlite3`; this probe keeps the
# stdlib enforcement check happy.
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY

_require_intrinsic("molt_stdlib_probe")
del _require_intrinsic

from _sqlite3 import *  # noqa: E402,F401,F403
from _sqlite3 import (
    _deprecated_version,
    register_adapter,
    register_converter,
)


_deprecated_names = frozenset({"version", "version_info"})

paramstyle = "qmark"
apilevel = "2.0"

# DB-API 2.0 §10 type constructors -------------------------------------------

Date = datetime.date
Time = datetime.time
Timestamp = datetime.datetime


def DateFromTicks(ticks):
    return Date(*time.localtime(ticks)[:3])


def TimeFromTicks(ticks):
    return Time(*time.localtime(ticks)[3:6])


def TimestampFromTicks(ticks):
    return Timestamp(*time.localtime(ticks)[:6])


_deprecated_version_info = tuple(int(part) for part in _deprecated_version.split("."))
sqlite_version_info = tuple(int(part) for part in sqlite_version.split("."))  # noqa: F405

Binary = memoryview


# ---------------------------------------------------------------------------
# CPython parity: register the legacy date/datetime adapters/converters.
# These emit DeprecationWarning when called (matching pysqlite 3.12+).
# ---------------------------------------------------------------------------


def _register_adapters_and_converters() -> None:
    from warnings import warn

    msg = (
        "The default {what} is deprecated as of Python 3.12; "
        "see the sqlite3 documentation for suggested replacement recipes"
    )

    def adapt_date(val):
        warn(msg.format(what="date adapter"), DeprecationWarning, stacklevel=2)
        return val.isoformat()

    def adapt_datetime(val):
        warn(msg.format(what="datetime adapter"), DeprecationWarning, stacklevel=2)
        return val.isoformat(" ")

    def convert_date(val):
        warn(msg.format(what="date converter"), DeprecationWarning, stacklevel=2)
        return datetime.date(*map(int, val.split(b"-")))

    def convert_timestamp(val):
        warn(msg.format(what="timestamp converter"), DeprecationWarning, stacklevel=2)
        datepart, timepart = val.split(b" ")
        year, month, day = map(int, datepart.split(b"-"))
        timepart_full = timepart.split(b".")
        hours, minutes, seconds = map(int, timepart_full[0].split(b":"))
        if len(timepart_full) == 2:
            microseconds = int("{:0<6.6}".format(timepart_full[1].decode()))
        else:
            microseconds = 0
        return datetime.datetime(
            year, month, day, hours, minutes, seconds, microseconds
        )

    register_adapter(datetime.date, adapt_date)
    register_adapter(datetime.datetime, adapt_datetime)
    register_converter("date", convert_date)
    register_converter("timestamp", convert_timestamp)


_register_adapters_and_converters()
del _register_adapters_and_converters


def __getattr__(name):
    if name in _deprecated_names:
        from warnings import warn

        warn(
            f"{name} is deprecated and will be removed in Python 3.14",
            DeprecationWarning,
            stacklevel=2,
        )
        return globals()[f"_deprecated_{name}"]
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
