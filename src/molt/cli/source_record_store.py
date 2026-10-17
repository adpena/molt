"""One SQLite table for a build state root's per-source records (HF-267).

Import scans and intrinsic source facts were one JSON file per record: a
one-file edit-rebuild opened, hashed and parsed 10,940 of them, and one
checkout held 185,639 files (1.4 GB) in one directory because nothing retired
old compiler generations. They now share one WAL-mode table keyed by
``(kind, key)``, the layout mypy's SQLite cache and Cargo's global cache
tracker use for many small records. A read is one B-tree seek; a write is one
small transaction without fsync, since every record is reconstructible.

Retention is per compiler generation (the frontend tooling fingerprint each
key already includes): a process refreshes its generation's use at most once
an hour, and at most once an hour a process deletes the records of
generations unused for a week and of all but the newest four, as the build
record and Go's build cache bound their entries. An edited source overwrites
its record, whose key names the path, not the bytes.

A record is a candidate: its consumer still qualifies its semantic inputs. A
store that SQLite reports corrupt is deleted and recreated. A busy, full or
read-only database makes ``get`` a miss and ``put`` return False.
"""

from __future__ import annotations

import json
import os
import sqlite3
import threading
import time
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Literal

RecordKind = Literal["import-scan", "intrinsic-facts"]

STORE_NAME = "source_records.sqlite"

_SCHEMA = """
CREATE TABLE IF NOT EXISTS records (
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    generation TEXT NOT NULL,
    payload BLOB NOT NULL,
    PRIMARY KEY (kind, key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS records_generation ON records (generation);
CREATE TABLE IF NOT EXISTS generations (
    generation TEXT PRIMARY KEY,
    used_ns INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS trims (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 0),
    trimmed_ns INTEGER NOT NULL
);
"""

_HOUR_NS = 3600 * 1_000_000_000
_UNUSED_LIMIT_NS = 7 * 24 * _HOUR_NS
_REFRESH_INTERVAL_NS = _HOUR_NS
_RETAINED_GENERATIONS = 4
_BUSY_TIMEOUT_SECONDS = 60.0


class SourceRecordStore:
    """This process's connection to one state root's record table."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._lock = threading.Lock()
        self._connection: sqlite3.Connection | None = None
        self._refreshed: set[str] = set()

    def get(
        self, kind: RecordKind, key: str, *, generation: str, max_bytes: int
    ) -> Mapping[str, Any] | None:
        """The stored object for ``(kind, key)``, or None for any miss."""
        with self._lock:
            _stored, row = self._run(
                lambda connection: connection.execute(
                    "SELECT payload FROM records WHERE kind = ? AND key = ?",
                    (kind, key),
                ).fetchone(),
                generation,
            )
        if row is None or len(row[0]) > max_bytes:
            return None
        try:
            decoded = json.loads(row[0])
        except (ValueError, RecursionError):
            return None
        return decoded if isinstance(decoded, dict) else None

    def put(
        self,
        kind: RecordKind,
        key: str,
        payload: Mapping[str, Any],
        *,
        generation: str,
    ) -> bool:
        """Store ``payload`` for ``(kind, key)``; False when it was not stored."""
        data = json.dumps(payload, separators=(",", ":")).encode()
        with self._lock:
            stored, _result = self._run(
                lambda connection: connection.execute(
                    "INSERT OR REPLACE INTO records VALUES (?, ?, ?, ?)",
                    (kind, key, generation, data),
                ),
                generation,
            )
        return stored

    def _run(self, operation: Any, generation: str) -> tuple[bool, Any]:
        connection = self._connect()
        if connection is None:
            return False, None
        try:
            if generation not in self._refreshed:
                self._refresh(connection, generation)
            return True, operation(connection)
        except sqlite3.OperationalError:
            # Busy past the timeout, a full disk or a read-only state root.
            return False, None
        except sqlite3.DatabaseError:
            self._quarantine()
            return False, None

    def _connect(self) -> sqlite3.Connection | None:
        if self._connection is not None:
            return self._connection
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            connection = sqlite3.connect(
                self.path,
                timeout=_BUSY_TIMEOUT_SECONDS,
                isolation_level=None,
                check_same_thread=False,
            )
        except (OSError, sqlite3.Error):
            return None
        try:
            # auto_vacuum applies only before the first table exists.
            connection.execute("PRAGMA auto_vacuum = INCREMENTAL")
            connection.execute("PRAGMA journal_mode = WAL")
            connection.execute("PRAGMA synchronous = NORMAL")
            connection.executescript(_SCHEMA)
        except sqlite3.OperationalError:
            connection.close()
            return None
        except sqlite3.DatabaseError:
            connection.close()
            self._quarantine()
            return None
        self._connection = connection
        return connection

    def _refresh(self, connection: sqlite3.Connection, generation: str) -> None:
        now = time.time_ns()
        row = connection.execute(
            "SELECT used_ns FROM generations WHERE generation = ?", (generation,)
        ).fetchone()
        if row is None or now - row[0] >= _REFRESH_INTERVAL_NS:
            connection.execute(
                "INSERT OR REPLACE INTO generations VALUES (?, ?)", (generation, now)
            )
            self._trim_if_due(connection, now)
        self._refreshed.add(generation)

    def _trim_if_due(self, connection: sqlite3.Connection, now: int) -> None:
        row = connection.execute("SELECT trimmed_ns FROM trims").fetchone()
        if row is not None and now - row[0] < _REFRESH_INTERVAL_NS:
            return
        connection.execute("BEGIN IMMEDIATE")
        try:
            # A peer may have trimmed while this process waited for the lock.
            row = connection.execute("SELECT trimmed_ns FROM trims").fetchone()
            if row is not None and now - row[0] < _REFRESH_INTERVAL_NS:
                connection.execute("COMMIT")
                return
            connection.execute(
                "DELETE FROM generations WHERE used_ns < ? OR generation NOT IN"
                " (SELECT generation FROM generations ORDER BY used_ns DESC LIMIT ?)",
                (now - _UNUSED_LIMIT_NS, _RETAINED_GENERATIONS),
            )
            connection.execute(
                "DELETE FROM records WHERE generation NOT IN"
                " (SELECT generation FROM generations)"
            )
            connection.execute("INSERT OR REPLACE INTO trims VALUES (0, ?)", (now,))
            connection.execute("COMMIT")
        except BaseException:
            connection.execute("ROLLBACK")
            raise
        connection.execute("PRAGMA incremental_vacuum")

    def _quarantine(self) -> None:
        connection, self._connection = self._connection, None
        if connection is not None:
            try:
                connection.close()
            except sqlite3.Error:
                pass
        # The records are reconstructible; a peer that still holds the old
        # file keeps reading it until it reconnects.
        for suffix in ("", "-wal", "-shm"):
            try:
                os.unlink(f"{self.path}{suffix}")
            except OSError:
                continue


_STORES: dict[tuple[str, int], SourceRecordStore] = {}


def source_record_store(state_root: Path) -> SourceRecordStore:
    """This process's store for ``state_root``.

    A forked child opens its own connection; the parent's stays untouched.
    """
    key = (os.fspath(state_root), os.getpid())
    store = _STORES.get(key)
    if store is None:
        store = _STORES[key] = SourceRecordStore(state_root / STORE_NAME)
    return store
