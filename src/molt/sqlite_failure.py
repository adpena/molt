"""The one classification of SQLite failures that Molt's stores act on.

SQLite reports an extended result code; its low eight bits are the primary
category. The file digest cache, source-record maintenance and the proof queue
read the class here, never message text, and each keeps its own policy per
class.
"""

from __future__ import annotations

import enum
import sqlite3


class SqliteFailure(enum.Enum):
    # Another connection holds the database past the busy timeout
    # (SQLITE_BUSY), or WAL-index recovery lost a race between processes
    # (SQLITE_PROTOCOL, "locking protocol"). Waiting can cure it.
    CONTENTION = enum.auto()
    # A read transaction cannot be promoted to a write because another writer
    # committed after it began (SQLITE_BUSY_SNAPSHOT). Retrying the same
    # transaction never cures it; only a new transaction can.
    STALE_SNAPSHOT = enum.auto()
    # A same-connection or shared-cache ownership failure (SQLITE_LOCKED).
    OWNERSHIP = enum.auto()
    # Anything else, including SQLITE_ERROR from a statement defect. Stores keep
    # these loud.
    OTHER = enum.auto()


def classify_sqlite_failure(error: BaseException) -> SqliteFailure:
    """The class of ``error``; an error without a result code is OTHER."""
    code = getattr(error, "sqlite_errorcode", None)
    if not isinstance(error, sqlite3.Error) or type(code) is not int:
        return SqliteFailure.OTHER
    if code == sqlite3.SQLITE_BUSY_SNAPSHOT:
        return SqliteFailure.STALE_SNAPSHOT
    primary = code & 0xFF
    if primary in (sqlite3.SQLITE_BUSY, sqlite3.SQLITE_PROTOCOL):
        return SqliteFailure.CONTENTION
    if primary == sqlite3.SQLITE_LOCKED:
        return SqliteFailure.OWNERSHIP
    return SqliteFailure.OTHER
