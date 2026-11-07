"""The one SQLite failure classification that Molt's stores act on."""

from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from molt.sqlite_failure import SqliteFailure, classify_sqlite_failure


def _error(kind: type[sqlite3.Error], message: str, code: int) -> sqlite3.Error:
    # SQLite-raised errors carry their extended result code (CPython 3.11+).
    error = kind(message)
    error.sqlite_errorcode = code
    return error


@pytest.mark.parametrize(
    ("error", "expected"),
    [
        (
            _error(sqlite3.OperationalError, "database is locked", sqlite3.SQLITE_BUSY),
            SqliteFailure.CONTENTION,
        ),
        (
            _error(
                sqlite3.OperationalError,
                "database is locked",
                sqlite3.SQLITE_BUSY_RECOVERY,
            ),
            SqliteFailure.CONTENTION,
        ),
        (
            _error(
                sqlite3.OperationalError, "locking protocol", sqlite3.SQLITE_PROTOCOL
            ),
            SqliteFailure.CONTENTION,
        ),
        (
            _error(
                sqlite3.OperationalError,
                "database is locked",
                sqlite3.SQLITE_BUSY_SNAPSHOT,
            ),
            SqliteFailure.STALE_SNAPSHOT,
        ),
        (
            _error(
                sqlite3.OperationalError,
                "database table is locked",
                sqlite3.SQLITE_LOCKED,
            ),
            SqliteFailure.OWNERSHIP,
        ),
        (
            _error(sqlite3.OperationalError, "no such column: x", sqlite3.SQLITE_ERROR),
            SqliteFailure.OTHER,
        ),
        (
            _error(sqlite3.OperationalError, "disk I/O error", sqlite3.SQLITE_IOERR),
            SqliteFailure.OTHER,
        ),
        (
            _error(
                sqlite3.DatabaseError,
                "database disk image is malformed",
                sqlite3.SQLITE_CORRUPT,
            ),
            SqliteFailure.OTHER,
        ),
        # A message alone is not a class: errors the sqlite3 module raises
        # itself carry no result code.
        (sqlite3.OperationalError("database is locked"), SqliteFailure.OTHER),
        (ValueError("database is locked"), SqliteFailure.OTHER),
    ],
)
def test_classification_reads_the_result_code(
    error: BaseException, expected: SqliteFailure
) -> None:
    assert classify_sqlite_failure(error) is expected


def test_a_real_busy_writer_is_contention(tmp_path: Path) -> None:
    store = tmp_path / "store.sqlite"
    holder = sqlite3.connect(store, isolation_level=None)
    holder.execute("CREATE TABLE t (x)")
    holder.execute("BEGIN IMMEDIATE")
    waiter = sqlite3.connect(store, timeout=0, isolation_level=None)
    try:
        with pytest.raises(sqlite3.OperationalError) as busy:
            waiter.execute("BEGIN IMMEDIATE")
        assert classify_sqlite_failure(busy.value) is SqliteFailure.CONTENTION
    finally:
        waiter.close()
        holder.execute("ROLLBACK")
        holder.close()


def test_source_record_maintenance_treats_contention_and_stale_snapshots_as_busy() -> (
    None
):
    from molt.cli import source_record_store as records

    for code in (
        sqlite3.SQLITE_BUSY,
        sqlite3.SQLITE_PROTOCOL,
        sqlite3.SQLITE_BUSY_SNAPSHOT,
    ):
        refused = records._maintenance_error(
            _error(sqlite3.OperationalError, "contended", code)
        )
        assert isinstance(refused, records.SourceRecordBusy), code
    other = records._maintenance_error(
        _error(sqlite3.OperationalError, "no such column: x", sqlite3.SQLITE_ERROR)
    )
    assert type(other) is records.SourceRecordUnavailable
