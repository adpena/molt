"""Opaque, process-owned source records and their SQLite lifecycle.

All four source record families share the existing WAL database. Raw reads
never renew retention: consumers admit the returned bytes, then refresh the
immutable observation. Its random revision and observed use fence replacement
and concurrent reuse. Generation retirement remains independent of the global
cache byte/age policy, whose metadata-only maintenance uses bounded pages.

Corrupt stores are unavailable to this process. Their files are never unlinked:
SQLite peers may still own the database and WAL. Busy/full/read-only stores
remain optional cache misses or failed writes, with no alternate cache lane.
"""

from __future__ import annotations

import os
import sqlite3
import threading
import time
from collections.abc import Generator, Iterator, Sequence
from contextlib import closing, contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal
from weakref import WeakValueDictionary, finalize

from molt.file_locks import (
    _acquire_file_lock,
    _release_file_lock,
    _try_acquire_file_lock,
)
from molt.default_paths import (
    CACHE_MAINTENANCE_INTERVAL_SECONDS,
    SHARED_SOURCE_RECORDS_DIRECTORY,
    _default_molt_cache,
)

RecordKind = Literal[
    "import-scan", "intrinsic-facts", "module-analysis", "module-lowering"
]
STORE_NAME = "source_records.sqlite"
SOURCE_RECORD_BATCH_MAX_RECORDS = 256
SOURCE_RECORD_BATCH_MAX_BYTES = 1 << 20

_SCHEMA_VERSION = 1
_SCHEMA = (
    """CREATE TABLE IF NOT EXISTS records (
        kind TEXT NOT NULL,
        key TEXT NOT NULL,
        generation TEXT NOT NULL,
        payload BLOB NOT NULL,
        revision BLOB NOT NULL,
        used_ns INTEGER NOT NULL,
        PRIMARY KEY (kind, key)
    ) WITHOUT ROWID""",
    "CREATE INDEX IF NOT EXISTS records_generation ON records (generation)",
    """CREATE TABLE IF NOT EXISTS generations (
        generation TEXT PRIMARY KEY,
        used_ns INTEGER NOT NULL
    ) WITHOUT ROWID""",
    """CREATE TABLE IF NOT EXISTS trims (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 0),
        trimmed_ns INTEGER NOT NULL
    )""",
)
_HOUR_NS = 3600 * 1_000_000_000
_UNUSED_LIMIT_NS = 7 * 24 * _HOUR_NS
_REFRESH_INTERVAL_NS = _HOUR_NS
_RECORD_REFRESH_NS = int(CACHE_MAINTENANCE_INTERVAL_SECONDS * 1_000_000_000)
_RETAINED_GENERATIONS = 4
_BUSY_TIMEOUT_SECONDS = 60.0
_PAGE_SIZE = 256

# Foreign fork children keep inherited handles inert until process exit. Even a
# PID-fenced callback must retain its argument: dropping it could invoke the
# SQLite connection's C finalizer in that child.
_INHERITED_CONNECTIONS: list[sqlite3.Connection] = []


def _close_connection(connection: sqlite3.Connection, pid: int) -> None:
    if pid != os.getpid():
        _INHERITED_CONNECTIONS.append(connection)
    else:
        connection.close()


@dataclass(frozen=True, slots=True)
class SourceRecord:
    payload: bytes
    kind: RecordKind
    key: str
    generation: str
    revision: bytes
    used_ns: int
    _store: SourceRecordStore = field(repr=False, compare=False)

    def refresh(self) -> bool:
        """Renew an admitted observation if due and still the same record."""
        return self._store._refresh_record(self)


@dataclass(frozen=True, slots=True)
class SourceRecordMetadata:
    """An eviction observation, containing no payload allocation."""

    kind: str
    key: str
    revision: bytes
    used_ns: int
    size_bytes: int


@dataclass(frozen=True, slots=True)
class SourceRecordInventory:
    entries: int
    size_bytes: int


class SourceRecordUnavailable(Exception):
    """Optional source-record maintenance could not qualify its database."""


class SourceRecordBusy(SourceRecordUnavailable):
    """Another SQLite connection owns a required maintenance lock."""


class SourceRecordUnmigrated(SourceRecordUnavailable):
    """A dry run met a legacy schema that only a writing sweep may migrate."""


def _maintenance_error(error: sqlite3.Error) -> SourceRecordUnavailable:
    # Extended SQLite result codes retain their primary category in low8 bits.
    # LOCKED is a different same-connection/shared-cache ownership failure.
    code = getattr(error, "sqlite_errorcode", None)
    if type(code) is int and code & 0xFF == sqlite3.SQLITE_BUSY:
        return SourceRecordBusy("source record maintenance is busy")
    return SourceRecordUnavailable("source record maintenance is unavailable")


class SourceRecordMaintenance:
    """One nonwaiting connection for the existing global prune owner.

    Bounded metadata pages share one read snapshot; a separate connection does
    current-state CAS deletes. No cursor is live while deleting. The snapshot
    pins WAL pages through this cold sweep, so it closes before reclamation.
    """

    def __init__(
        self,
        connection: sqlite3.Connection,
        current: sqlite3.Connection,
        *,
        dry_run: bool,
    ) -> None:
        self._pid = os.getpid()
        self._connection = connection
        self._current = current
        self._dry_run = dry_run
        self._snapshot_closed = False
        # True also covers a checkpoint deferred by busy preceding vacuum.
        self.checkpoint_busy: bool | None = None

    @contextmanager
    def _cursor(
        self, query: str, parameters: tuple[Any, ...] = (), *, current: bool = False
    ) -> Generator[sqlite3.Cursor]:
        if self._pid != os.getpid():
            raise SourceRecordUnavailable(
                "source record maintenance belongs to another process"
            )
        cursor = None
        try:
            cursor = (self._current if current else self._connection).execute(
                query, parameters
            )
            yield cursor
        except sqlite3.Error as exc:
            raise _maintenance_error(exc) from exc
        finally:
            if cursor is not None:
                cursor.close()

    def inventory(self) -> SourceRecordInventory:
        with self._cursor(
            "SELECT count(*), coalesce(sum(length(payload)), 0) FROM records"
        ) as cursor:
            count, size = cursor.fetchone()
        return SourceRecordInventory(count, size)

    def iter_lru(self) -> Iterator[SourceRecordMetadata]:
        after: tuple[int, str, str] | None = None
        while True:
            parameters: tuple[Any, ...] = ()
            query = "SELECT kind, key, revision, used_ns, length(payload) FROM records"
            if after is not None:
                query += " WHERE (used_ns, kind, key) > (?, ?, ?)"
                parameters += after
            with self._cursor(
                query + " ORDER BY used_ns, kind, key LIMIT ?",
                (*parameters, _PAGE_SIZE),
            ) as cursor:
                rows = cursor.fetchall()
            if not rows:
                return
            for kind, key, revision, used_ns, size in rows:
                if (
                    not isinstance(kind, str)
                    or not isinstance(key, str)
                    or not isinstance(revision, bytes)
                    or len(revision) != 16
                    or type(used_ns) is not int
                    or type(size) is not int
                    or size < 0
                ):
                    raise SourceRecordUnavailable("invalid source record metadata")
                yield SourceRecordMetadata(kind, key, revision, used_ns, size)
            last = rows[-1]
            after = (last[3], last[0], last[1])

    def remove(
        self, observed: Sequence[SourceRecordMetadata]
    ) -> tuple[Literal["removed", "busy"], ...]:
        """Commit one bounded batch of observation-fenced evictions.

        A single oversized record remains evictable. No transaction or result
        escapes before commit; contention refuses the whole batch, without a
        retry. Dry-run forecasts use one read transaction and never write.
        """
        if len(observed) > SOURCE_RECORD_BATCH_MAX_RECORDS:
            raise ValueError("source record eviction batch exceeds record bound")
        batch = tuple(observed)
        if len(batch) > SOURCE_RECORD_BATCH_MAX_RECORDS:
            raise ValueError("source record eviction batch exceeds record bound")
        if not batch:
            return ()
        if any(row.size_bytes < 0 for row in batch):
            raise ValueError("source record eviction size must be nonnegative")
        if (
            len(batch) > 1
            and sum(row.size_bytes for row in batch) > SOURCE_RECORD_BATCH_MAX_BYTES
        ):
            raise ValueError("source record eviction batch exceeds payload bound")
        predicate = "kind = ? AND key = ? AND revision = ? AND used_ns = ?"
        query = (
            "SELECT 1 FROM records WHERE "
            if self._dry_run
            else "DELETE FROM records WHERE "
        ) + predicate
        outcomes: list[Literal["removed", "busy"]] = []
        owns_transaction = False
        try:
            with self._cursor(
                "BEGIN" if self._dry_run else "BEGIN IMMEDIATE", current=True
            ):
                pass
            owns_transaction = True
            for row in batch:
                with self._cursor(
                    query,
                    (row.kind, row.key, row.revision, row.used_ns),
                    current=True,
                ) as cursor:
                    present = (
                        cursor.fetchone() is not None
                        if self._dry_run
                        else cursor.rowcount == 1
                    )
                    outcomes.append("removed" if present else "busy")
            with self._cursor("COMMIT", current=True):
                pass
            owns_transaction = False
            return tuple(outcomes)
        except BaseException as error:
            if owns_transaction and self._current.in_transaction:
                try:
                    with self._cursor("ROLLBACK", current=True):
                        pass
                except BaseException:
                    # A failed rollback cannot leave an admitted write owner.
                    _close_connection(self._current, self._pid)
                    raise
            if isinstance(error, SourceRecordBusy):
                return ("busy",) * len(batch)
            raise

    def close_snapshot(self) -> None:
        if not self._snapshot_closed:
            _close_connection(self._connection, self._pid)
            self._snapshot_closed = True

    def reclaim(self) -> None:
        self.close_snapshot()
        if not self._dry_run:
            try:
                # Bound maintenance; peers keep their pages without a retry.
                with self._cursor(
                    f"PRAGMA incremental_vacuum({_PAGE_SIZE})", current=True
                ) as cursor:
                    # SQLite emits one result per step; closing early abandons
                    # the remaining bounded page work.
                    for _ in cursor:
                        pass
                with self._cursor(
                    "PRAGMA wal_checkpoint(TRUNCATE)", current=True
                ) as cursor:
                    row = cursor.fetchone()
                    self.checkpoint_busy = bool(row[0])
            except SourceRecordBusy:
                self.checkpoint_busy = True


class SourceRecordStore:
    """This process's connection to one shared record table."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._pid = os.getpid()
        self._lock = threading.Lock()
        self._connection: sqlite3.Connection | None = None
        self._connection_finalizer: finalize | None = None
        self._refreshed: dict[str, int] = {}
        self._unavailable = False

    def close(self) -> None:
        """Release this process's connection; later operations may reopen it."""
        if self._pid != os.getpid():
            return
        with self._lock:
            self._release_connection()
            self._refreshed.clear()

    def _release_connection(self) -> None:
        finalizer, self._connection_finalizer = self._connection_finalizer, None
        self._connection = None
        if finalizer is not None:
            finalizer()

    def get(
        self, kind: RecordKind, key: str, *, generation: str, max_bytes: int | None
    ) -> SourceRecord | None:
        """Observe bytes, bounded in SQL before payload materialization.

        None explicitly selects uncapped lowering; other consumers retain their
        admitted allocation bounds. No generation or row recency changes.
        """
        if max_bytes is not None and (type(max_bytes) is not int or max_bytes < 0):
            raise ValueError("source record size bound must be nonnegative or None")
        if self._pid != os.getpid():
            return None
        query = (
            "SELECT payload, revision, used_ns FROM records"
            " WHERE kind = ? AND key = ? AND generation = ?"
            " AND typeof(payload) = 'blob' AND typeof(revision) = 'blob'"
            " AND length(revision) = 16 AND typeof(used_ns) = 'integer'"
        )
        parameters: tuple[Any, ...] = (kind, key, generation)
        if max_bytes is not None:
            query += " AND length(payload) <= ?"
            parameters += (max_bytes,)

        def read(connection: sqlite3.Connection):
            cursor = connection.execute(query, parameters)
            try:
                return cursor.fetchone()
            finally:
                cursor.close()

        with self._lock:
            _ok, row = self._run(read)
        if row is None:
            return None
        return SourceRecord(row[0], kind, key, generation, row[1], row[2], self)

    def put(
        self, kind: RecordKind, key: str, payload: bytes, *, generation: str
    ) -> bool:
        if not isinstance(payload, bytes):
            raise TypeError("source record payload must be bytes")
        if self._pid != os.getpid():
            return False
        now = time.time_ns()
        with self._lock:
            stored, _result = self._run(
                lambda connection: connection.execute(
                    "INSERT OR REPLACE INTO records"
                    " (kind, key, generation, payload, revision, used_ns)"
                    " VALUES (?, ?, ?, ?, randomblob(16), ?)",
                    (kind, key, generation, payload, now),
                ),
                generation=generation,
                now=now,
            )
        return stored

    def _refresh_record(self, record: SourceRecord) -> bool:
        if self._pid != os.getpid():
            return False
        now = time.time_ns()
        if now - record.used_ns < _RECORD_REFRESH_NS:
            return False
        with self._lock:
            ok, changed = self._run(
                lambda connection: (
                    connection.execute(
                        "UPDATE records SET used_ns = ? WHERE kind = ? AND key = ?"
                        " AND generation = ? AND revision = ? AND used_ns = ?",
                        (
                            now,
                            record.kind,
                            record.key,
                            record.generation,
                            record.revision,
                            record.used_ns,
                        ),
                    ).rowcount
                )
            )
            if ok and changed == 1:
                self._run(
                    lambda _connection: None, generation=record.generation, now=now
                )
                return True
        return False

    def _run(
        self, operation: Any, *, generation: str | None = None, now: int | None = None
    ) -> tuple[bool, Any]:
        connection = self._connect()
        if connection is None:
            return False, None
        try:
            if generation is not None:
                self._refresh_generation(
                    connection, generation, time.time_ns() if now is None else now
                )
            return True, operation(connection)
        except sqlite3.OperationalError:
            return False, None
        except sqlite3.DatabaseError:
            self._disable()
            return False, None

    def _connect(self) -> sqlite3.Connection | None:
        if self._pid != os.getpid() or self._unavailable:
            return None
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
            self._initialize(connection)
        except (OSError, SourceRecordUnavailable, sqlite3.OperationalError):
            connection.close()
            return None
        except sqlite3.DatabaseError:
            connection.close()
            self._disable()
            return None
        except BaseException:
            connection.close()
            raise
        self._connection = connection
        # The callback owns no reference to this store. Weak references have
        # already cleared when it runs, so a concurrent factory lookup cannot
        # resurrect a store while SQLite releases the GIL during close.
        self._connection_finalizer = finalize(
            self, _close_connection, connection, self._pid
        )
        return connection

    def _initialize(self, connection: sqlite3.Connection, *, wait: bool = True) -> None:
        connection.execute("PRAGMA synchronous = NORMAL").close()
        if self._schema_ready(connection):
            return
        # WAL conversion needs an exclusive database lock outside a transaction.
        # Competing conversions can return BUSY without invoking SQLite's busy
        # handler. Serialize only this cold transition with the existing portable
        # lock authority; normal opens and all record operations use SQLite alone.
        lock_path = self.path.with_name(self.path.name + ".initialize.lock")
        if wait:
            try:
                handle = _acquire_file_lock(
                    lock_path,
                    timeout_s=_BUSY_TIMEOUT_SECONDS,
                    timeout_message="source record initialization is busy",
                )
            except RuntimeError as exc:
                if str(exc) != "source record initialization is busy":
                    raise
                raise SourceRecordBusy(str(exc)) from exc
        else:
            handle = _try_acquire_file_lock(lock_path)
            if handle is None:
                raise SourceRecordBusy("source record initialization is busy")
        try:
            if self._schema_ready(connection):
                return
            connection.execute("PRAGMA auto_vacuum = INCREMENTAL").close()
            with closing(connection.execute("PRAGMA journal_mode = WAL")) as cursor:
                if cursor.fetchone() != ("wal",):
                    raise SourceRecordUnavailable("source record WAL is unavailable")
            connection.execute("BEGIN IMMEDIATE").close()
            try:
                for statement in _SCHEMA:
                    connection.execute(statement).close()
                self._migrate(connection)
                connection.execute(f"PRAGMA user_version = {_SCHEMA_VERSION}").close()
                connection.execute("COMMIT").close()
            except BaseException:
                if connection.in_transaction:
                    connection.execute("ROLLBACK").close()
                raise
        finally:
            _release_file_lock(handle)

    @staticmethod
    def _schema_ready(connection: sqlite3.Connection) -> bool:
        with closing(connection.execute("PRAGMA user_version")) as cursor:
            version = cursor.fetchone()[0]
        if version not in (0, _SCHEMA_VERSION):
            raise SourceRecordUnavailable("source record schema version is unsupported")
        return version == _SCHEMA_VERSION

    def _migrate(self, connection: sqlite3.Connection) -> None:
        # The initialization owner holds one write transaction for schema,
        # legacy row migration, indexes and the version's publication.
        with closing(connection.execute("PRAGMA table_info(records)")) as cursor:
            columns = {row[1] for row in cursor}
        if "revision" not in columns:
            connection.execute(
                "ALTER TABLE records ADD COLUMN revision BLOB NOT NULL DEFAULT X''"
            ).close()
            connection.execute("UPDATE records SET revision = randomblob(16)").close()
        if "used_ns" not in columns:
            connection.execute(
                "ALTER TABLE records ADD COLUMN used_ns INTEGER NOT NULL DEFAULT 0"
            ).close()
            connection.execute(
                "UPDATE records SET used_ns = coalesce((SELECT used_ns FROM generations"
                " WHERE generations.generation = records.generation), ?)",
                (time.time_ns(),),
            ).close()
        connection.execute(
            "CREATE INDEX IF NOT EXISTS records_recency ON records (used_ns, kind, key)"
        ).close()

    def _refresh_generation(
        self, connection: sqlite3.Connection, generation: str, now: int
    ) -> None:
        checked = self._refreshed.get(generation)
        if checked is not None and 0 <= now - checked < _REFRESH_INTERVAL_NS:
            return
        row = connection.execute(
            "SELECT used_ns FROM generations WHERE generation = ?", (generation,)
        ).fetchone()
        if row is None or now - row[0] >= _REFRESH_INTERVAL_NS:
            connection.execute(
                "INSERT OR REPLACE INTO generations VALUES (?, ?)", (generation, now)
            )
            self._trim_if_due(connection, now)
        self._refreshed[generation] = now

    def _trim_if_due(self, connection: sqlite3.Connection, now: int) -> None:
        row = connection.execute("SELECT trimmed_ns FROM trims").fetchone()
        if row is not None and now - row[0] < _REFRESH_INTERVAL_NS:
            return
        connection.execute("BEGIN IMMEDIATE")
        try:
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
                "DELETE FROM records WHERE generation NOT IN (SELECT generation FROM generations)"
            )
            connection.execute("INSERT OR REPLACE INTO trims VALUES (0, ?)", (now,))
            connection.execute("COMMIT")
        except BaseException:
            connection.execute("ROLLBACK")
            raise
        with closing(
            connection.execute(f"PRAGMA incremental_vacuum({_PAGE_SIZE})")
        ) as cursor:
            for _ in cursor:
                pass

    def _disable(self) -> None:
        self._unavailable = True
        self._refreshed.clear()
        try:
            self._release_connection()
        except sqlite3.Error:
            pass

    @contextmanager
    def maintenance(self, *, dry_run: bool) -> Generator[SourceRecordMaintenance]:
        if self._pid != os.getpid() or self._unavailable:
            raise SourceRecordUnavailable("source record store is unavailable")
        snapshot: sqlite3.Connection | None = None
        current: sqlite3.Connection | None = None
        maintenance: SourceRecordMaintenance | None = None
        try:
            current = sqlite3.connect(
                self.path.absolute().as_uri() + ("?mode=ro" if dry_run else "?mode=rw"),
                uri=True,
                timeout=0,
                isolation_level=None,
            )
            if dry_run:
                if not self._schema_ready(current):
                    raise SourceRecordUnmigrated(
                        "source record schema is not initialized"
                    )
            else:
                self._initialize(current, wait=False)
            snapshot = sqlite3.connect(
                self.path.absolute().as_uri() + "?mode=ro",
                uri=True,
                timeout=0,
                isolation_level=None,
            )
            snapshot.execute("BEGIN").close()
            if not self._schema_ready(snapshot):
                raise SourceRecordUnavailable("source record schema is not initialized")
            maintenance = SourceRecordMaintenance(snapshot, current, dry_run=dry_run)
            yield maintenance
        except SourceRecordUnavailable as exc:
            cause = exc.__cause__
            if isinstance(cause, sqlite3.DatabaseError) and not isinstance(
                cause, sqlite3.OperationalError
            ):
                with self._lock:
                    self._disable()
            raise
        except OSError as exc:
            raise SourceRecordUnavailable(
                "source record initialization is unavailable"
            ) from exc
        except sqlite3.OperationalError as exc:
            raise _maintenance_error(exc) from exc
        except sqlite3.DatabaseError as exc:
            with self._lock:
                self._disable()
            raise SourceRecordUnavailable("source record store is corrupt") from exc
        finally:
            if maintenance is not None:
                maintenance.close_snapshot()
            elif snapshot is not None:
                _close_connection(snapshot, self._pid)
            if current is not None:
                _close_connection(current, self._pid)

    def physical_bytes(self) -> dict[str, int | None]:
        sizes: dict[str, int | None] = {}
        allocated: int | None = 0
        for label, suffix in (("database", ""), ("wal", "-wal"), ("shm", "-shm")):
            try:
                metadata = Path(f"{self.path}{suffix}").lstat()
            except FileNotFoundError:
                sizes[label] = 0
                continue
            except OSError:
                sizes[label] = None
                allocated = None
                continue
            sizes[label] = metadata.st_size
            if allocated is not None:
                blocks = getattr(metadata, "st_blocks", None)
                allocated = None if blocks is None else allocated + blocks * 512
        sizes["allocated"] = allocated
        return sizes


_STORES: WeakValueDictionary[tuple[str, int], SourceRecordStore] = WeakValueDictionary()
_STORES_LOCK = threading.Lock()


def _after_fork() -> None:
    global _STORES_LOCK
    _STORES_LOCK = threading.Lock()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork)


def source_record_store(state_root: Path) -> SourceRecordStore:
    """Share a process-owned store only while a caller still owns it."""
    key = (os.fspath(state_root), os.getpid())
    with _STORES_LOCK:
        store = _STORES.get(key)
        if store is None:
            store = _STORES[key] = SourceRecordStore(state_root / STORE_NAME)
    return store


def shared_source_records() -> SourceRecordStore:
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION

    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    store = transaction.source_records if transaction is not None else None
    if store is None or store._pid != os.getpid():
        root = (
            store.path.parent
            if store is not None
            else (_default_molt_cache() / SHARED_SOURCE_RECORDS_DIRECTORY).resolve()
        )
        store = source_record_store(root)
        if transaction is not None:
            transaction.source_records = store
    return store
