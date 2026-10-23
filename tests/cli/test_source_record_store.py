"""Source records retain opaque values and fail without unlinking peer state."""

from __future__ import annotations

import json
import multiprocessing
import os
import sqlite3
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from pathlib import Path

import pytest

from molt.cli import source_record_store as records
from tests.process_guard_common import install_module_view

_HOUR = 3600 * 1_000_000_000
_DAY = 24 * _HOUR
_LIMIT = 1 << 20


def _store(tmp_path: Path) -> records.SourceRecordStore:
    return records.SourceRecordStore(tmp_path)


def _fixture_path(cache_root: Path) -> Path:
    path = cache_root / records.STORE_RELATIVE_PATH
    path.parent.mkdir(parents=True, exist_ok=True)
    _, lock = records.source_record_namespace_paths(cache_root, path.parent.name)
    lease = records._try_acquire_file_lock(lock, shared=True)
    assert lease is not None
    records._release_file_lock(lease)
    return path


def test_records_round_trip_per_kind_and_key(tmp_path: Path) -> None:
    store = _store(tmp_path)
    assert store.put("import-scan", "k1", b"\x00\xffopaque\n", generation="g")
    assert store.put("intrinsic-facts", "k1", b"[1,2]", generation="g")

    assert (
        store.get("import-scan", "k1", generation="g", max_bytes=_LIMIT).payload
        == b"\x00\xffopaque\n"
    )
    assert (
        store.get("intrinsic-facts", "k1", generation="g", max_bytes=_LIMIT).payload
        == b"[1,2]"
    )
    assert store.get("import-scan", "k2", generation="g", max_bytes=_LIMIT) is None


def test_a_later_record_replaces_the_earlier_one(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.put("import-scan", "k", b"old", generation="g")
    store.put("import-scan", "k", b"new", generation="g")

    assert (
        store.get("import-scan", "k", generation="g", max_bytes=_LIMIT).payload
        == b"new"
    )
    with sqlite3.connect(tmp_path / records.STORE_RELATIVE_PATH) as connection:
        assert connection.execute("SELECT count(*) FROM records").fetchone() == (1,)


def test_size_limit_rejects_before_materializing_payload(tmp_path: Path) -> None:
    store = _store(tmp_path)
    payload = b"x" * 64
    assert store.put("import-scan", "big", payload, generation="g")
    assert store.put("import-scan", "empty", b"", generation="g")
    connection = store._connect()
    assert connection is not None
    materialized = []

    def observe_payload(cursor, row):
        if cursor.description[0][0] == "payload":
            materialized.append(row[0])
        return row

    connection.row_factory = observe_payload
    assert store.get("import-scan", "big", generation="g", max_bytes=63) is None
    assert materialized == []
    assert (
        store.get("import-scan", "big", generation="g", max_bytes=64).payload == payload
    )
    assert materialized == [payload]
    assert store.get("import-scan", "empty", generation="g", max_bytes=0).payload == b""


def test_non_blob_rows_are_misses_and_non_bytes_writes_refused(tmp_path: Path) -> None:
    store = _store(tmp_path)
    with pytest.raises(TypeError, match="payload must be bytes"):
        store.put("import-scan", "text", "not bytes", generation="g")
    assert store.put("import-scan", "text", b"original", generation="g")
    with sqlite3.connect(store.path) as connection:
        connection.execute("UPDATE records SET payload = ?", ("not a BLOB",))
    assert store.get("import-scan", "text", generation="g", max_bytes=_LIMIT) is None


@pytest.mark.parametrize(
    "method",
    [
        name
        for name in ("spawn", "fork")
        if name in multiprocessing.get_all_start_methods()
    ],
)
def test_another_process_reads_what_this_process_wrote(
    tmp_path: Path, method: str
) -> None:
    parent = records.source_record_store(tmp_path)
    assert parent.put("import-scan", "k", b"\x00child\xff", generation="g")
    connection = parent._connect()
    assert connection is not None
    # An inherited parent connection would incorrectly retain this transaction.
    connection.execute("BEGIN").close()
    try:
        with multiprocessing.get_context(method).Pool(1) as pool:
            seen, in_transaction, pid = pool.apply(_read_in_child, (str(tmp_path),))
        assert seen == b"\x00child\xff"
        assert not in_transaction and pid != os.getpid()
    finally:
        connection.execute("ROLLBACK").close()


def _read_in_child(root: str) -> object:
    store = records.source_record_store(Path(root))
    record = store.get("import-scan", "k", generation="g", max_bytes=_LIMIT)
    connection = store._connect()
    assert connection is not None
    return (
        record.payload if record is not None else None,
        connection.in_transaction,
        os.getpid(),
    )


def test_generation_is_provenance_and_global_policy_owns_retirement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    store = _store(tmp_path)
    for index in range(6):
        clock[0] += 2 * _HOUR
        assert store.put(
            "import-scan", f"k{index}", str(index).encode(), generation=f"g{index}"
        )
    clock[0] += 8 * _DAY
    assert store.put("import-scan", "fresh", b"9", generation="g9")
    for index in range(6):
        observed = store.get(
            "import-scan", f"k{index}", generation=f"g{index}", max_bytes=None
        )
        assert observed is not None and observed.payload == str(index).encode()
        assert (
            store.get("import-scan", f"k{index}", generation="wrong", max_bytes=None)
            is None
        )
    with closing(sqlite3.connect(store.path)) as peer:
        assert peer.execute("SELECT count(*) FROM records").fetchone() == (7,)
        assert (
            peer.execute(
                "SELECT name FROM sqlite_schema WHERE name IN ('generations','trims','records_generation')"
            ).fetchall()
            == []
        )


def test_corrupt_store_is_unavailable_without_unlinking_peer_files(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = _fixture_path(tmp_path)
    original = b"not a database" * 512
    path.write_bytes(original)
    before = (path.stat().st_ino, path.read_bytes())
    # SQLite owns its WAL/SHM lifecycle and may remove malformed sidecars on
    # close. Trap application-level unlinking; a real live-peer test below
    # independently proves that a database error preserves admitted peer data.
    original_unlink = Path.unlink
    protected = {
        path,
        path.with_name(path.name + "-wal"),
        path.with_name(path.name + "-shm"),
    }

    def refuse_application_unlink(candidate, *args, **kwargs):
        assert candidate not in protected, "Molt must not unlink a peer-owned store"
        return original_unlink(candidate, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", refuse_application_unlink)
    store = _store(tmp_path)
    with path.open("rb") as peer:
        assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) is None
        assert peer.read() == original

        def no_reconnect(*args, **kwargs):
            pytest.fail("known corrupt optional store must not repeatedly reconnect")

        monkeypatch.setattr(records.sqlite3, "connect", no_reconnect)
        assert store.put("import-scan", "k", b"recovered", generation="g") is False
        assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) is None
    assert (path.stat().st_ino, path.read_bytes()) == before


@pytest.mark.skipif(
    not hasattr(os, "geteuid") or os.geteuid() == 0,
    reason="needs a POSIX user that file modes restrict",
)
def test_an_unwritable_state_root_refuses_records(tmp_path: Path) -> None:
    root = tmp_path / "state"
    root.mkdir()
    root.chmod(0o500)
    try:
        store = _store(root)
        assert store.put("import-scan", "k", b"recovered", generation="g") is False
        assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) is None
    finally:
        root.chmod(0o700)


@pytest.mark.parametrize(
    "kind", ["import-scan", "intrinsic-facts", "module-analysis", "module-lowering"]
)
def test_all_kinds_return_immutable_observations(
    tmp_path: Path, kind: records.RecordKind
) -> None:
    from dataclasses import FrozenInstanceError

    store = _store(tmp_path)
    assert store.put(kind, "key", b"opaque", generation="g")
    observed = store.get(kind, "key", generation="g", max_bytes=None)
    assert observed is not None
    assert (observed.kind, observed.key, observed.generation, observed.payload) == (
        kind,
        "key",
        "g",
        b"opaque",
    )
    assert len(observed.revision) == 16 and observed.used_ns > 0
    with pytest.raises(FrozenInstanceError):
        setattr(observed, "payload", b"changed")
    assert store.get(kind, "key", generation="other", max_bytes=None) is None


def test_uncapped_lowering_exceeds_analysis_allocation_bound(tmp_path: Path) -> None:
    store = _store(tmp_path)
    payload = b"x" * ((16 << 20) + 1)
    assert store.put("module-lowering", "large", payload, generation="g")
    assert (
        store.get("module-lowering", "large", generation="g", max_bytes=16 << 20)
        is None
    )
    admitted = store.get("module-lowering", "large", generation="g", max_bytes=None)
    assert admitted is not None and admitted.payload == payload
    with closing(sqlite3.connect(store.path)) as peer:
        # SQLite's rowid-table contract keeps payload out of search-tree
        # interior keys. WITHOUT ROWID tables cannot satisfy either oracle.
        assert peer.execute("SELECT rowid FROM records").fetchone()[0] > 0
        assert peer.execute("PRAGMA index_info(records)").fetchall() == []
        index = next(
            row[1]
            for row in peer.execute("PRAGMA index_list(records)")
            if row[2] == 1 and row[3] == "pk"
        )
        assert [row[2] for row in peer.execute(f'PRAGMA index_info("{index}")')] == [
            "kind",
            "key",
        ]


def test_only_admitted_use_refreshes_and_stale_observations_cannot_renew(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    store = _store(tmp_path)
    assert store.put("module-lowering", "k", b'{"valid": true}', generation="g")
    assert store.put("module-lowering", "invalid", b"{invalid", generation="g")
    original = store.get("module-lowering", "k", generation="g", max_bytes=None)
    assert original is not None
    clock[0] += 301 * 1_000_000_000
    invalid = store.get("module-lowering", "invalid", generation="g", max_bytes=None)
    assert invalid is not None
    with pytest.raises(json.JSONDecodeError):
        json.loads(invalid.payload)
    connection = store._connect()
    assert connection is not None
    assert connection.execute(
        "SELECT used_ns FROM records WHERE key = 'invalid'"
    ).fetchone() == (original.used_ns,)
    stale = store.get("module-lowering", "k", generation="g", max_bytes=None)
    assert stale is not None
    # Successful independent semantic admission precedes either refresh.
    assert json.loads(original.payload) == json.loads(stale.payload) == {"valid": True}
    assert original.refresh()
    assert not stale.refresh()
    refreshed = store.get("module-lowering", "k", generation="g", max_bytes=None)
    assert refreshed is not None and refreshed.used_ns == clock[0]
    assert not refreshed.refresh()
    assert store.put("module-lowering", "k", b"replacement", generation="g")
    replacement = store.get("module-lowering", "k", generation="g", max_bytes=None)
    assert replacement is not None and replacement.revision != refreshed.revision
    clock[0] += 301 * 1_000_000_000
    assert not refreshed.refresh()
    latest = store.get("module-lowering", "k", generation="g", max_bytes=None)
    assert latest is not None and latest.used_ns == replacement.used_ns
    assert connection.execute(
        "SELECT used_ns FROM records WHERE key = 'invalid'"
    ).fetchone() == (original.used_ns,)


def test_advisory_refresh_declines_busy_owners_without_reopening(tmp_path, monkeypatch):
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    kinds = ("import-scan", "intrinsic-facts", "module-analysis", "module-lowering")
    store = _store(tmp_path)
    for kind in kinds:
        assert store.put(kind, "k", kind.encode(), generation="g")
    old = [store.get(kind, "k", generation="g", max_bytes=None) for kind in kinds]
    assert all(record is not None for record in old)
    clock[0] += 301 * 1_000_000_000
    store.close()

    # This first boundary makes the old implementation fail immediately,
    # without running its 60s writer wait or deadlocking on the local lock.
    def no_reopen():
        pytest.fail("advisory refresh reopened a closed store")

    with monkeypatch.context() as closed:
        closed.setattr(store, "_connect", no_reopen)
        assert all(record.refresh() is False for record in old)
    assert store._connection_resources is None
    observed = [store.get(kind, "k", generation="g", max_bytes=None) for kind in kinds]
    connection = store._connect()
    assert connection is not None
    # A nondefault value proves exact restoration, not hardcoding 60000.
    connection.execute("PRAGMA busy_timeout = 12345").close()
    try:
        assert store._lock.acquire(blocking=False)
        try:
            assert all(record.refresh() is False for record in observed)
        finally:
            store._lock.release()
        with closing(
            sqlite3.connect(store.path, isolation_level=None, timeout=0)
        ) as peer:
            expected = peer.execute(
                "SELECT * FROM records ORDER BY kind,key"
            ).fetchall()
            peer.execute("BEGIN IMMEDIATE").close()
            try:
                assert all(record.refresh() is False for record in observed)
                assert connection.execute("PRAGMA busy_timeout").fetchone() == (12345,)
                assert (
                    peer.execute("SELECT * FROM records ORDER BY kind,key").fetchall()
                    == expected
                )
                assert not store._unavailable
            finally:
                peer.execute("ROLLBACK").close()
            assert all(record.refresh() is True for record in observed)
            assert connection.execute("PRAGMA busy_timeout").fetchone() == (12345,)
            # A now-stale admitted observation cannot renew the newer recency.
            assert all(record.refresh() is False for record in observed)
            current = [
                store.get(kind, "k", generation="g", max_bytes=None) for kind in kinds
            ]
            assert all(
                record.used_ns == clock[0] and record.refresh() is False
                for record in current
            )
            for kind in kinds:
                assert store.put(kind, "k", b"replacement", generation="g")
            replacements = peer.execute(
                "SELECT * FROM records ORDER BY kind,key"
            ).fetchall()
            clock[0] += 301 * 1_000_000_000
            assert all(record.refresh() is False for record in current)
            assert (
                peer.execute("SELECT * FROM records ORDER BY kind,key").fetchall()
                == replacements
            )
            assert connection.execute("PRAGMA busy_timeout").fetchone() == (12345,)
    finally:
        store.close()


def test_metadata_pages_do_not_select_payload_and_deletion_is_observation_fenced(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    monkeypatch.setattr(records, "_PAGE_SIZE", 2)
    store = _store(tmp_path)
    for index in range(5):
        assert store.put(
            "module-analysis", str(index), bytes([index]) * 10000, generation="g"
        )
    with store.maintenance(dry_run=False) as maintenance:

        def metadata_only(cursor, row):
            assert "payload" not in {column[0] for column in cursor.description}
            assert all(
                not isinstance(value, bytes) or len(value) == 16 for value in row
            )
            return row

        maintenance._connection.row_factory = metadata_only
        inventory = maintenance.inventory()
        observed = list(maintenance.iter_lru(maintenance.inventory()))
        assert [row.key for row in observed] == [str(i) for i in range(5)]
        assert inventory.entries == 5 and inventory.size_bytes == 50000
        candidate = store.get("module-analysis", "0", generation="g", max_bytes=None)
        assert candidate is not None
        clock[0] += 301 * 1_000_000_000
        assert candidate.refresh()
        assert store.put("module-analysis", "1", b"replacement", generation="g")
        assert maintenance.remove(observed[:3]) == ("busy", "busy", "removed")
    assert store.get("module-analysis", "0", generation="g", max_bytes=None) is not None
    assert (
        store.get("module-analysis", "1", generation="g", max_bytes=None).payload
        == b"replacement"
    )
    assert store.get("module-analysis", "2", generation="g", max_bytes=None) is None


def _write_concurrent_record(arguments):
    root, index = arguments
    kinds: tuple[records.RecordKind, ...] = (
        "import-scan",
        "intrinsic-facts",
        "module-analysis",
        "module-lowering",
    )
    store = _store(Path(root))
    return store.put(kinds[index % 4], str(index), str(index).encode(), generation="g")


@pytest.mark.parametrize("executor", ["threads", "spawn"])
@pytest.mark.parametrize("initial", ["virgin", "current"])
def test_concurrent_connections_keep_all_record_kinds(
    tmp_path: Path, executor: str, initial: str
) -> None:
    path = _fixture_path(tmp_path)
    if initial == "current":
        store = _store(tmp_path)
        assert store.put("import-scan", "old", b"\x00\xff", generation="g")
        store.close()
    arguments = [(str(tmp_path), index) for index in range(20)]
    if executor == "threads":
        with ThreadPoolExecutor(max_workers=4) as pool:
            outcomes = list(pool.map(_write_concurrent_record, arguments))
    else:
        with multiprocessing.get_context("spawn").Pool(4) as pool:
            outcomes = pool.map(_write_concurrent_record, arguments)
    assert all(outcomes), outcomes
    # Raw SQLite is the independent payload/schema oracle, including the old
    # opaque row. It does not use the store's read or maintenance projection.
    kinds = ("import-scan", "intrinsic-facts", "module-analysis", "module-lowering")
    expected = {(kinds[i % 4], str(i), str(i).encode()) for i in range(20)}
    if initial == "current":
        expected.add(("import-scan", "old", b"\x00\xff"))
    with closing(sqlite3.connect(path)) as connection:
        assert (
            set(connection.execute("SELECT kind,key,payload FROM records")) == expected
        )
        assert connection.execute("PRAGMA integrity_check").fetchone() == ("ok",)
        assert connection.execute("PRAGMA journal_mode").fetchone() == ("wal",)
        assert connection.execute(
            "SELECT count(*) FROM records WHERE length(revision)!=16 OR used_ns<=0"
        ).fetchone() == (0,)


def test_ready_opens_do_not_write_or_wait_for_a_writer(tmp_path, monkeypatch):
    store = _store(tmp_path)
    assert store.put("import-scan", "k", b"preserved", generation="g")
    store.close()
    monkeypatch.setattr(records, "_BUSY_TIMEOUT_SECONDS", 0)
    with closing(sqlite3.connect(store.path, isolation_level=None)) as peer:
        peer.execute("BEGIN IMMEDIATE").close()
        try:
            observed = store.get("import-scan", "k", generation="g", max_bytes=None)
            assert observed is not None and observed.payload == b"preserved"
            with store.maintenance(dry_run=False) as maintenance:
                assert maintenance.inventory().entries == 1
                assert maintenance.remove(
                    list(maintenance.iter_lru(maintenance.inventory()))
                ) == ("busy",)
            assert not store.put("import-scan", "blocked", b"no", generation="g")
        finally:
            peer.execute("ROLLBACK").close()
    assert not store._unavailable
    assert store.put("import-scan", "after", b"yes", generation="g")


def test_initialization_lock_is_nonwaiting_for_maintenance(tmp_path, monkeypatch):
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    store = _store(tmp_path)
    _fixture_path(tmp_path)
    with closing(sqlite3.connect(store.path)):
        pass
    handle = _try_acquire_file_lock(
        store.path.with_name(store.path.name + ".initialize.lock")
    )
    assert handle is not None
    monkeypatch.setattr(records, "_BUSY_TIMEOUT_SECONDS", 0)
    try:
        with pytest.raises(records.SourceRecordBusy):
            with store.maintenance(dry_run=False):
                pytest.fail(
                    "a cold maintenance owner cannot bypass bootstrap ownership"
                )
        assert not store.put("import-scan", "blocked", b"no", generation="g")
        assert not store._unavailable
    finally:
        _release_file_lock(handle)
    assert store.put("import-scan", "after", b"yes", generation="g")


def _probe_cold_bootstrap(root):
    records._BUSY_TIMEOUT_SECONDS = 0
    store = _store(Path(root))
    try:
        return store.put("import-scan", "child", b"opaque", generation="g")
    finally:
        store.close()


@pytest.mark.parametrize("executor", ["threads", "spawn"])
@pytest.mark.parametrize("initial", ["virgin"])
def test_cold_peer_cannot_bypass_initialization_owner(tmp_path, executor, initial):
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    path = _fixture_path(tmp_path)
    with closing(sqlite3.connect(path)) as peer:
        original = list(peer.iterdump())
    handle = _try_acquire_file_lock(path.with_name(path.name + ".initialize.lock"))
    assert handle is not None
    old_timeout = records._BUSY_TIMEOUT_SECONDS
    try:
        # The OS lock establishes the schedule across both ownership domains;
        # the contender uses real SQLite and cannot publish partial schema.
        if executor == "threads":
            with ThreadPoolExecutor(max_workers=1) as pool:
                outcome = pool.submit(_probe_cold_bootstrap, str(tmp_path)).result(10)
        else:
            with multiprocessing.get_context("spawn").Pool(1) as pool:
                outcome = pool.apply_async(_probe_cold_bootstrap, (str(tmp_path),)).get(
                    10
                )
        assert outcome is False
        with closing(sqlite3.connect(path)) as peer:
            assert list(peer.iterdump()) == original
            assert peer.execute("PRAGMA user_version").fetchone() == (0,)
    finally:
        records._BUSY_TIMEOUT_SECONDS = old_timeout
        _release_file_lock(handle)
    assert _write_concurrent_record((str(tmp_path), 0))


def test_ready_connection_trace_contains_no_persistent_setup(tmp_path, monkeypatch):
    store = _store(tmp_path)
    assert store.put("import-scan", "k", b"opaque", generation="g")
    store.close()
    statements = []
    original_connect = sqlite3.connect

    def connect(*args, **kwargs):
        connection = original_connect(*args, **kwargs)
        connection.set_trace_callback(statements.append)
        return connection

    def no_bootstrap_lock(*args, **kwargs):
        pytest.fail("a ready database must not acquire a bootstrap lock")

    install_module_view(monkeypatch, "sqlite3", sqlite3, records, connect=connect)
    monkeypatch.setattr(records, "_acquire_file_lock", no_bootstrap_lock)
    original_try_lock = records._try_acquire_file_lock

    def lease_only(path, **kwargs):
        assert kwargs.get("shared") is True, "ready connections never bootstrap"
        return original_try_lock(path, **kwargs)

    monkeypatch.setattr(records, "_try_acquire_file_lock", lease_only)
    assert (
        store.get("import-scan", "k", generation="g", max_bytes=None).payload
        == b"opaque"
    )
    with store.maintenance(dry_run=False) as maintenance:
        assert maintenance.inventory().entries == 1
    assert all(
        not sql.upper().startswith(("CREATE", "ALTER", "INSERT", "UPDATE", "DELETE"))
        and "auto_vacuum" not in sql
        and "journal_mode" not in sql
        and not ("user_version" in sql and "=" in sql)
        for sql in statements
    )


@pytest.mark.parametrize("initial", ["virgin"])
def test_schema_failure_rolls_back_the_whole_bootstrap(tmp_path, initial):
    store = _store(tmp_path)
    _fixture_path(tmp_path)
    with closing(sqlite3.connect(store.path, isolation_level=None)) as connection:
        connection.execute("PRAGMA auto_vacuum = INCREMENTAL").close()
        connection.execute("PRAGMA journal_mode = WAL").close()
        original = list(connection.iterdump())
        connection.set_authorizer(
            lambda action, name, *_args: (
                sqlite3.SQLITE_DENY
                if action == sqlite3.SQLITE_CREATE_INDEX and name == "records_recency"
                else sqlite3.SQLITE_OK
            )
        )
        with pytest.raises(sqlite3.DatabaseError):
            store._initialize(connection)
        connection.set_authorizer(None)
        assert not connection.in_transaction
        assert connection.execute("PRAGMA user_version").fetchone() == (0,)
        assert list(connection.iterdump()) == original
    assert store.put("import-scan", "after", b"yes", generation="g")


@pytest.mark.parametrize("version", [-1, 1, 2, records._SCHEMA_VERSION + 1])
@pytest.mark.parametrize("dry_run", [False, True])
def test_unknown_schema_is_unavailable_without_downgrade(
    tmp_path, version, dry_run, monkeypatch
):
    store = _store(tmp_path)
    _fixture_path(tmp_path)
    assert store.put("import-scan", "k", b"preserved", generation="g")
    store.close()
    with closing(sqlite3.connect(store.path, isolation_level=None)) as peer:
        peer.execute(f"PRAGMA user_version = {version}").close()
        original = list(peer.iterdump())
        with pytest.raises(records.SourceRecordUnavailable, match="unsupported"):
            with store.maintenance(dry_run=dry_run):
                pytest.fail("unsupported schema must not be migrated")
        original_initialize = store._initialize
        attempts = []

        def initialize(connection, **kwargs):
            attempts.append(1)
            return original_initialize(connection, **kwargs)

        monkeypatch.setattr(store, "_initialize", initialize)
        for _ in range(3):
            assert not store.put("import-scan", "new", b"no", generation="g")
            assert store.get("import-scan", "k", generation="g", max_bytes=32) is None
        assert attempts == [1]
        assert store._unavailable
        with pytest.raises(
            records.SourceRecordUnavailable, match="store is unavailable"
        ):
            with store.maintenance(dry_run=dry_run):
                pytest.fail("a disabled optional store cannot reconnect")
        assert list(peer.iterdump()) == original
        assert peer.execute("PRAGMA user_version").fetchone() == (version,)
        assert peer.execute("SELECT payload FROM records").fetchall() == [
            (b"preserved",)
        ]


@pytest.mark.parametrize("initial", ["virgin"])
def test_dry_run_refuses_uninitialized_schema_without_bootstrap(tmp_path, initial):
    store = _store(tmp_path)
    _fixture_path(tmp_path)
    with closing(sqlite3.connect(store.path, isolation_level=None)) as peer:
        peer.execute("CREATE TABLE sentinel(value BLOB)").close()
        original = list(peer.iterdump())
        with pytest.raises(records.SourceRecordUnavailable, match="not initialized"):
            with store.maintenance(dry_run=True):
                pytest.fail("dry-run cannot bootstrap a legacy database")
        assert peer.execute("PRAGMA user_version").fetchone() == (0,)
        assert list(peer.iterdump()) == original
    assert not store.path.with_name(store.path.name + ".initialize.lock").exists()


@pytest.mark.parametrize("dry_run", [False, True])
def test_maintenance_admits_its_independently_opened_snapshot(
    tmp_path, monkeypatch, dry_run
):
    store = _store(tmp_path)
    assert store.put("import-scan", "k", b"preserved", generation="g")
    store.close()
    original_connect = sqlite3.connect
    opens = 0

    def connect(*args, **kwargs):
        nonlocal opens
        opens += 1
        if opens == 2:
            # An independently opened handle must qualify the actual snapshot,
            # even after the first handle admitted the previous schema version.
            with closing(original_connect(store.path, isolation_level=None)) as peer:
                peer.execute(
                    f"PRAGMA user_version = {records._SCHEMA_VERSION + 1}"
                ).close()
        return original_connect(*args, **kwargs)

    install_module_view(monkeypatch, "sqlite3", sqlite3, records, connect=connect)
    with pytest.raises(records.SourceRecordUnavailable, match="unsupported"):
        with store.maintenance(dry_run=dry_run):
            pytest.fail("unadmitted snapshot cannot be exposed")
    with closing(original_connect(store.path)) as peer:
        assert peer.execute("SELECT payload FROM records").fetchall() == [
            (b"preserved",)
        ]
        assert peer.execute("PRAGMA user_version").fetchone() == (
            records._SCHEMA_VERSION + 1,
        )


def test_database_error_closes_only_owned_connection_and_preserves_live_peer(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    assert store.put("module-analysis", "k", b"peer payload", generation="g")
    identity = store.path.stat().st_ino
    with sqlite3.connect(store.path) as peer:
        own = store._connect()
        assert own is not None
        own.set_authorizer(
            lambda action, *_args: (
                sqlite3.SQLITE_DENY
                if action == sqlite3.SQLITE_SELECT
                else sqlite3.SQLITE_OK
            )
        )
        assert store.get("module-analysis", "k", generation="g", max_bytes=None) is None
        assert (
            store.put("module-analysis", "k", b"must not replace", generation="g")
            is False
        )
        assert peer.execute("SELECT payload FROM records").fetchone() == (
            b"peer payload",
        )
        assert store.path.stat().st_ino == identity


@pytest.mark.parametrize("failure", ["readonly", "full"])
def test_sqlite_write_refusal_preserves_existing_records(
    tmp_path: Path, failure: str
) -> None:
    store = _store(tmp_path)
    assert store.put("module-lowering", "old", b"preserved", generation="g")
    connection = store._connect()
    assert connection is not None
    if failure == "readonly":
        connection.execute("PRAGMA query_only = ON").close()
    else:
        pages = connection.execute("PRAGMA page_count").fetchone()[0]
        connection.execute(f"PRAGMA max_page_count = {pages}").close()
    assert not store.put("module-lowering", "new", b"x" * 100000, generation="g")
    old = store.get("module-lowering", "old", generation="g", max_bytes=None)
    assert old is not None and old.payload == b"preserved"
    assert connection.execute("SELECT count(*) FROM records").fetchone() == (1,)


def test_busy_writer_remains_optional_and_does_not_block_wal_reads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    assert _store(tmp_path).put("module-analysis", "k", b"preserved", generation="g")
    monkeypatch.setattr(records, "_BUSY_TIMEOUT_SECONDS", 0)
    store = _store(tmp_path)
    assert store._connect() is not None
    with sqlite3.connect(store.path, isolation_level=None) as peer:
        peer.execute("BEGIN IMMEDIATE").close()
        try:
            assert not store.put(
                "module-analysis", "new", b"not stored", generation="other"
            )
            observed = store.get("module-analysis", "k", generation="g", max_bytes=None)
            assert observed is not None and observed.payload == b"preserved"
        finally:
            peer.execute("ROLLBACK").close()


@pytest.mark.parametrize("operation", ["remove", "reclaim"])
def test_maintenance_refuses_a_live_writer_without_failure_or_retry(
    tmp_path: Path, operation: str
) -> None:
    store = _store(tmp_path)
    assert store.put("module-analysis", "k", b"preserved", generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        observed = next(maintenance.iter_lru(maintenance.inventory()))
        assert maintenance._current.execute("PRAGMA busy_timeout").fetchone() == (0,)
        statements = []
        maintenance._current.set_trace_callback(statements.append)
        with closing(sqlite3.connect(store.path, isolation_level=None)) as peer:
            peer.execute("BEGIN IMMEDIATE").close()
            try:
                # Independent engine oracle: a real conflicting writer owns the
                # lock, not a mocked operational-error spelling.
                with pytest.raises(sqlite3.OperationalError) as engine:
                    maintenance._current.execute("DELETE FROM records")
                assert engine.value.sqlite_errorcode & 0xFF == sqlite3.SQLITE_BUSY
                statements.clear()
                if operation == "remove":
                    assert maintenance.remove([observed]) == ("busy",)
                    assert sum(sql == "BEGIN IMMEDIATE" for sql in statements) == 1
                    assert not any(sql.startswith("DELETE") for sql in statements)
                else:
                    maintenance.reclaim()
                    assert maintenance._snapshot_closed
                    assert maintenance.checkpoint_busy is True
                    assert sum("incremental_vacuum" in sql for sql in statements) == 1
                    assert sum("wal_checkpoint" in sql for sql in statements) <= 1
                assert peer.execute("SELECT payload FROM records").fetchall() == [
                    (b"preserved",)
                ]
            finally:
                peer.execute("ROLLBACK").close()
    assert not store._unavailable
    current = store.get("module-analysis", "k", generation="g", max_bytes=None)
    assert current is not None and current.payload == b"preserved"


def test_extended_busy_snapshot_is_a_refusal_and_preserves_the_new_record(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    assert store.put("module-analysis", "k", b"old", generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        # Force a real stale read-to-write promotion on the current connection
        # to exercise SQLite's extended BUSY category, not an invented error.
        maintenance._current.execute("BEGIN").close()
        cursor = maintenance._current.execute("SELECT payload FROM records")
        assert cursor.fetchone() == (b"old",)
        cursor.close()
        assert store.put("module-analysis", "k", b"replacement", generation="g")
        with pytest.raises(sqlite3.OperationalError) as engine:
            maintenance._current.execute("DELETE FROM records")
        assert engine.value.sqlite_errorcode == sqlite3.SQLITE_BUSY_SNAPSHOT
        with pytest.raises(records.SourceRecordBusy) as refused:
            with maintenance._cursor("DELETE FROM records", current=True):
                pass
        assert refused.value.__cause__.sqlite_errorcode == sqlite3.SQLITE_BUSY_SNAPSHOT
    current = store.get("module-analysis", "k", generation="g", max_bytes=None)
    assert current is not None and current.payload == b"replacement"


@pytest.mark.parametrize("operation", ["remove", "reclaim"])
def test_other_operational_errors_remain_failures(
    tmp_path: Path, operation: str
) -> None:
    store = _store(tmp_path)
    assert store.put("module-analysis", "keep", b"preserved", generation="g")
    assert store.put("module-analysis", "victim", b"x" * 100000, generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        observed = {
            row.key: row for row in maintenance.iter_lru(maintenance.inventory())
        }
        if operation == "reclaim":
            assert maintenance.remove([observed["victim"]]) == ("removed",)
            assert maintenance._current.execute("PRAGMA freelist_count").fetchone()[0]
        maintenance._current.execute("PRAGMA query_only = ON").close()
        with pytest.raises(records.SourceRecordUnavailable) as refused:
            if operation == "remove":
                maintenance.remove([observed["keep"]])
            else:
                maintenance.reclaim()
        assert not isinstance(refused.value, records.SourceRecordBusy)
        cause = refused.value.__cause__
        assert isinstance(cause, sqlite3.OperationalError)
        assert cause.sqlite_errorcode & 0xFF == sqlite3.SQLITE_READONLY
        assert maintenance.checkpoint_busy is None
    current = store.get("module-analysis", "keep", generation="g", max_bytes=None)
    assert current is not None and current.payload == b"preserved"


def test_same_connection_locked_error_is_not_classified_as_writer_contention(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    for number in range(3):
        assert store.put("module-analysis", str(number), b"preserved", generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        cursor = maintenance._current.execute("SELECT key FROM records")
        assert cursor.fetchone() is not None
        try:
            with pytest.raises(records.SourceRecordUnavailable) as refused:
                with maintenance._cursor("DROP TABLE records", current=True):
                    pass
            assert not isinstance(refused.value, records.SourceRecordBusy)
            cause = refused.value.__cause__
            assert isinstance(cause, sqlite3.OperationalError)
            assert cause.sqlite_errorcode == sqlite3.SQLITE_LOCKED
        finally:
            cursor.close()
    with store.maintenance(dry_run=True) as maintenance:
        assert maintenance.inventory().entries == 3


def test_batch_admission_and_single_oversized_record(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    assert store.put("module-lowering", "large", b"x" * ((1 << 20) + 1), generation="g")
    assert store.put("module-analysis", "small", b"preserved", generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        observed = {
            row.key: row for row in maintenance.iter_lru(maintenance.inventory())
        }
        statements = []
        maintenance._current.set_trace_callback(statements.append)
        assert maintenance.remove([]) == ()
        with pytest.raises(ValueError, match="record bound"):
            maintenance.remove([observed["small"]] * 257)
        with pytest.raises(ValueError, match="payload bound"):
            maintenance.remove([observed["large"], observed["small"]])
        assert statements == [], "oversized batches must fail before any SQL"
        assert maintenance.remove([observed["large"]]) == ("removed",)
        assert statements.count("BEGIN IMMEDIATE") == 1
        assert statements.count("COMMIT") == 1
        assert not maintenance._current.in_transaction
    current = store.get("module-analysis", "small", generation="g", max_bytes=None)
    assert current is not None and current.payload == b"preserved"


def test_dry_run_batch_forecasts_in_order_without_writing(tmp_path: Path) -> None:
    store = _store(tmp_path)
    for key in ("first", "second", "third"):
        assert store.put("module-analysis", key, key.encode(), generation="g")
    with store.maintenance(dry_run=True) as maintenance:
        observed = {
            row.key: row for row in maintenance.iter_lru(maintenance.inventory())
        }
        assert store.put("module-analysis", "second", b"replacement", generation="g")
        statements = []
        maintenance._current.set_trace_callback(statements.append)
        assert maintenance.remove(
            [observed["third"], observed["second"], observed["first"]]
        ) == ("removed", "busy", "removed")
        assert statements.count("BEGIN") == 1
        assert statements.count("COMMIT") == 1
        assert not maintenance._current.in_transaction
        assert not any(
            sql.startswith(("INSERT", "UPDATE", "DELETE", "BEGIN IMMEDIATE"))
            for sql in statements
        )
    with closing(sqlite3.connect(store.path)) as connection:
        assert connection.execute(
            "SELECT key,payload FROM records ORDER BY key"
        ).fetchall() == [
            ("first", b"first"),
            ("second", b"replacement"),
            ("third", b"third"),
        ]


@pytest.mark.parametrize("phase", ["statement", "commit"])
def test_batch_failures_roll_back_every_staged_deletion(
    tmp_path: Path, phase: str
) -> None:
    store = _store(tmp_path)
    for key in ("first", "second", "third"):
        assert store.put("module-analysis", key, key.encode(), generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        observed = {
            row.key: row for row in maintenance.iter_lru(maintenance.inventory())
        }
        if phase == "statement":
            maintenance._current.execute(
                "CREATE TRIGGER refuse_second BEFORE DELETE ON records"
                " WHEN old.key='second' BEGIN SELECT RAISE(ABORT,'refused'); END"
            ).close()
            expected_code = sqlite3.SQLITE_CONSTRAINT_TRIGGER
        else:
            maintenance._current.execute("PRAGMA foreign_keys=ON").close()
            maintenance._current.executescript(
                "CREATE TABLE parent(id INTEGER PRIMARY KEY);"
                "CREATE TABLE child(id INTEGER REFERENCES parent(id)"
                " DEFERRABLE INITIALLY DEFERRED);"
                "CREATE TRIGGER refuse_commit AFTER DELETE ON records"
                " WHEN old.key='second' BEGIN INSERT INTO child VALUES(1); END;"
            )
            expected_code = sqlite3.SQLITE_CONSTRAINT_FOREIGNKEY
        statements = []
        maintenance._current.set_trace_callback(statements.append)
        with pytest.raises(records.SourceRecordUnavailable) as refused:
            maintenance.remove(
                [observed["first"], observed["second"], observed["third"]]
            )
        assert not isinstance(refused.value, records.SourceRecordBusy)
        assert refused.value.__cause__.sqlite_errorcode == expected_code
        assert statements.count("BEGIN IMMEDIATE") == 1
        assert statements.count("ROLLBACK") == 1
        assert not maintenance._current.in_transaction
        assert maintenance._current.execute(
            "SELECT key,payload FROM records ORDER BY key"
        ).fetchall() == [
            ("first", b"first"),
            ("second", b"second"),
            ("third", b"third"),
        ]
        if phase == "commit":
            assert maintenance._current.execute("SELECT * FROM child").fetchall() == []


def test_bounded_vacuum_drains_pages_and_later_passes_converge(tmp_path: Path) -> None:
    store = _store(tmp_path)
    assert store.put("module-lowering", "discard", b"x" * (3 << 20), generation="g")
    assert store.put("module-analysis", "keep", b"preserved", generation="g")
    connection = store._connect()
    assert connection is not None
    with closing(connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")) as cursor:
        assert cursor.fetchone()[0] == 0
    with store.maintenance(dry_run=False) as maintenance:
        observed = {
            row.key: row for row in maintenance.iter_lru(maintenance.inventory())
        }
        assert maintenance.remove([observed["discard"]]) == ("removed",)

    def page_state():
        values = []
        for name in ("page_count", "freelist_count", "page_size"):
            with closing(connection.execute(f"PRAGMA {name}")) as cursor:
                values.append(cursor.fetchone()[0])
        return tuple(values)

    initial_pages, free, page_size = page_state()
    assert 512 < free < 1024, "known3MiB fixture must span several bounded passes"
    initial_physical = store.path.stat().st_size
    previous_pages = initial_pages
    passes = (free + 255) // 256
    for _ in range(passes):
        expected = min(256, free)
        with store.maintenance(dry_run=False) as maintenance:
            # No new deletion here: later cold passes must finish backlog.
            maintenance.reclaim()
            assert maintenance.checkpoint_busy is False
        pages, remaining, observed_page_size = page_state()
        assert observed_page_size == page_size
        assert free - remaining == expected
        assert previous_pages - pages >= expected
        assert store.path.stat().st_size == pages * page_size
        free, previous_pages = remaining, pages
    assert free == 0
    assert initial_physical - store.path.stat().st_size >= 512 * page_size
    current = store.get("module-analysis", "keep", generation="g", max_bytes=None)
    assert current is not None and current.payload == b"preserved"


def test_malformed_metadata_refuses_maintenance_without_loading_payload(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    assert store.put("module-analysis", "k", b"opaque", generation="g")
    with sqlite3.connect(store.path) as connection:
        connection.execute("UPDATE records SET used_ns = 'not a timestamp'")
    assert store.get("module-analysis", "k", generation="g", max_bytes=None) is None
    with store.maintenance(dry_run=True) as maintenance:
        with pytest.raises(
            records.SourceRecordUnavailable, match="invalid source record metadata"
        ):
            list(maintenance.iter_lru(maintenance.inventory()))


def test_factory_releases_abandoned_roots_and_closes_their_connections(tmp_path):
    import gc
    import weakref

    refs, connections = [], []
    for index in range(24):
        store = records.source_record_store(tmp_path / str(index))
        assert store.put("module-lowering", "body", b"body", generation="g")
        refs.append(weakref.ref(store))
        # Keep the connection independently alive: collection alone cannot
        # make this oracle pass without the store closing its resource.
        connections.append(store._connect())
    del store
    gc.collect()
    assert all(ref() is None for ref in refs)
    for connection in connections:
        with pytest.raises(sqlite3.ProgrammingError, match="closed"):
            connection.execute("SELECT 1")


def test_record_observation_pins_its_owner_until_admission_ends(tmp_path, monkeypatch):
    import gc
    import weakref

    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: 10 * _DAY)
    store = records.source_record_store(tmp_path)
    assert store.put("module-analysis", "k", b"body", generation="g")
    observation = store.get("module-analysis", "k", generation="g", max_bytes=16)
    assert observation is not None
    connection = store._connect()
    owner = weakref.ref(store)
    del store
    gc.collect()
    assert owner() is not None
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: 11 * _DAY)
    assert observation.refresh()
    del observation
    gc.collect()
    assert owner() is None
    with pytest.raises(sqlite3.ProgrammingError, match="closed"):
        connection.execute("SELECT 1")


@pytest.mark.parametrize("fail", [False, True])
def test_operation_pins_one_store_and_releases_on_all_exits(
    tmp_path, monkeypatch, fail
):
    import gc
    import weakref
    from molt.cli.cache_fingerprints import (
        _fresh_compiler_identity_inputs,
        _source_tree_fingerprint_transaction,
    )

    monkeypatch.setattr(records, "_default_molt_cache", lambda: tmp_path)
    connections = []
    owner = None
    try:
        with _source_tree_fingerprint_transaction():
            store = records.shared_source_records()
            owner = weakref.ref(store)
            assert store.put("import-scan", "k", b"body", generation="g")
            connection = store._connect()
            connections.append(connection)
            del store
            assert records.shared_source_records() is owner()
            with _source_tree_fingerprint_transaction():
                assert records.shared_source_records()._connect() is connection
            # A fresh identity scope must not close a surviving outer owner.
            with _fresh_compiler_identity_inputs():
                assert records.shared_source_records() is owner()
                assert records.shared_source_records()._connect() is connection
            assert connection.execute("SELECT count(*) FROM records").fetchone() == (1,)
            if fail:
                raise RuntimeError("operation failed")
    except RuntimeError as exc:
        assert fail and str(exc) == "operation failed"
    gc.collect()
    assert owner is not None and owner() is None
    for connection in connections:
        with pytest.raises(sqlite3.ProgrammingError, match="closed"):
            connection.execute("SELECT 1")


def _exercise_inherited_owner(store, maintenance, pipe):
    observation = records.SourceRecord(
        b"body", "import-scan", "k", "g", b"x" * 16, 0, store
    )
    refused = (
        store.get("import-scan", "k", generation="g", max_bytes=16) is None
        and not store.put("import-scan", "k", b"wrong", generation="g")
        and not observation.refresh()
    )
    store.close()
    try:
        maintenance.inventory()
    except records.SourceRecordUnavailable:
        pass
    else:
        raise AssertionError("inherited maintenance performed SQL")
    maintenance.close_snapshot()
    fresh = records.source_record_store(store.cache_root)
    value = fresh.get("import-scan", "k", generation="g", max_bytes=16)
    pipe.send((refused, None if value is None else value.payload))
    pipe.close()


def _close_fork_observer(child):
    """Reap only the native child created by a bounded fork metadata oracle."""
    if child.is_alive():
        child.terminate()
    child.join(10)
    if child.is_alive():
        child.kill()
        child.join(10)
    assert not child.is_alive(), "owned fork observer did not stop"
    child.close()


@pytest.mark.skipif(
    "fork" not in multiprocessing.get_all_start_methods(), reason="requires fork"
)
def test_inherited_owner_refuses_before_a_parent_lock_and_fresh_owner_works(tmp_path):
    context = multiprocessing.get_context("fork")
    store = records.source_record_store(tmp_path)
    assert store.put("import-scan", "k", b"body", generation="g")
    receive, send = context.Pipe(duplex=False)
    with store.maintenance(dry_run=True) as maintenance:
        child = context.Process(
            target=_exercise_inherited_owner, args=(store, maintenance, send)
        )
        with store._lock:
            child.start()
            send.close()
            try:
                assert receive.poll(10), "child waited on an inherited owner lock"
                assert receive.recv() == (True, b"body")
                child.join(10)
                assert child.exitcode == 0
            finally:
                receive.close()
                _close_fork_observer(child)
        assert maintenance.inventory().entries == 1
    assert (
        store.get("import-scan", "k", generation="g", max_bytes=16).payload == b"body"
    )


def test_factory_cannot_resurrect_an_owner_while_sqlite_closes(tmp_path, monkeypatch):
    import threading
    import weakref

    entered, release = threading.Event(), threading.Event()
    original_connect = sqlite3.connect
    closing = []

    class ClosingConnection(sqlite3.Connection):
        def close(self):
            if closing and self is closing[0]:
                entered.set()
                assert release.wait(10)
            return super().close()

    install_module_view(
        monkeypatch,
        "sqlite3",
        sqlite3,
        records,
        connect=lambda *args, **kwargs: original_connect(
            *args, factory=ClosingConnection, **kwargs
        ),
    )
    holder = [records.source_record_store(tmp_path)]
    assert holder[0].put("import-scan", "k", b"body", generation="g")
    closing.append(holder[0]._connect())
    owner = weakref.ref(holder[0])
    thread = threading.Thread(target=holder.clear)
    thread.start()
    try:
        assert entered.wait(10), "owner did not close its SQLite connection"
        assert owner() is None, "factory could resurrect an owner being finalized"
        replacement = records.source_record_store(tmp_path)
        assert replacement.put("import-scan", "k", b"new", generation="g")
    finally:
        release.set()
        thread.join(10)
    assert not thread.is_alive()
    assert (
        replacement.get("import-scan", "k", generation="g", max_bytes=16).payload
        == b"new"
    )


def _drop_inherited_direct_owner(holder, connection_ref, root, pipe):
    import gc

    holder.clear()
    gc.collect()
    retained = connection_ref() is not None
    fresh = records.source_record_store(Path(root))
    record = fresh.get("import-scan", "k", generation="g", max_bytes=16)
    pipe.send((retained, None if record is None else record.payload))
    pipe.close()


@pytest.mark.skipif(
    "fork" not in multiprocessing.get_all_start_methods(), reason="requires fork"
)
def test_fork_finalization_keeps_a_directly_constructed_handle_inert(
    tmp_path, monkeypatch
):
    import weakref

    original_connect = sqlite3.connect

    class ObservableConnection(sqlite3.Connection):
        pass

    install_module_view(
        monkeypatch,
        "sqlite3",
        sqlite3,
        records,
        connect=lambda *args, **kwargs: original_connect(
            *args, factory=ObservableConnection, **kwargs
        ),
    )
    holder = [_store(tmp_path)]
    assert holder[0].put("import-scan", "k", b"body", generation="g")
    connection_ref = weakref.ref(holder[0]._connect())
    context = multiprocessing.get_context("fork")
    receive, send = context.Pipe(duplex=False)
    child = context.Process(
        target=_drop_inherited_direct_owner,
        args=(holder, connection_ref, str(tmp_path), send),
    )
    child.start()
    send.close()
    try:
        assert receive.poll(10)
        assert receive.recv() == (True, b"body")
        child.join(10)
        assert child.exitcode == 0
    finally:
        receive.close()
        _close_fork_observer(child)
    holder.clear()
    assert connection_ref() is None


@pytest.mark.parametrize("dry_run", [False, True])
def test_paused_metadata_page_releases_wal_and_fences_newer_rows(
    tmp_path, monkeypatch, dry_run
):
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    monkeypatch.setattr(records, "_PAGE_SIZE", 2)
    store = _store(tmp_path)
    for key in ("a", "b", "c", "d"):
        assert store.put("module-analysis", key, b"original", generation="g")
    with closing(sqlite3.connect(store.path, isolation_level=None, timeout=0)) as peer:
        peer.execute("PRAGMA wal_autocheckpoint=0").close()
        with store.maintenance(dry_run=dry_run) as maintenance:
            iterator = maintenance.iter_lru(maintenance.inventory())
            first = next(iterator)
            assert first.key == "a"
            # A real competing publication modifies pages that the paused sweep
            # observed. Checkpoint success is the oracle for released read locks.
            clock[0] += 301 * 1_000_000_000
            peer.execute(
                "UPDATE records SET payload=?, used_ns=? WHERE key='a'",
                (b"renewed", clock[0]),
            ).close()
            peer.execute(
                "INSERT INTO records VALUES('module-analysis','later','g',?,randomblob(16),?)",
                (b"x" * (2 << 20), clock[0]),
            ).close()
            with closing(peer.execute("PRAGMA wal_checkpoint(TRUNCATE)")) as cursor:
                assert cursor.fetchone() == (0, 0, 0)
            assert Path(str(store.path) + "-wal").stat().st_size == 0
            assert maintenance.remove([first]) == ("busy",)
            assert [row.key for row in iterator] == ["b", "c", "d"]
            assert peer.execute(
                "SELECT payload FROM records WHERE key='a'"
            ).fetchone() == (b"renewed",)


def test_fresh_schema_has_only_the_row_retention_authority(tmp_path):
    store = _store(tmp_path)
    assert store.put("module-lowering", "opaque", b"\x00body\xff", generation="g")
    with closing(sqlite3.connect(store.path)) as peer:
        assert peer.execute("PRAGMA integrity_check").fetchone() == ("ok",)
        assert peer.execute("PRAGMA user_version").fetchone() == (
            records._SCHEMA_VERSION,
        )
        assert (
            peer.execute(
                "SELECT name FROM sqlite_schema WHERE name IN ('generations','trims','records_generation')"
            ).fetchall()
            == []
        )
        assert peer.execute("PRAGMA index_info(records)").fetchall() == []
        assert "records_recency" in {
            r[1] for r in peer.execute("PRAGMA index_list(records)")
        }
        assert peer.execute("SELECT payload FROM records").fetchall() == [
            (b"\x00body\xff",)
        ]


def test_backward_clock_and_interleaved_publication_have_finite_observation_budget(
    tmp_path, monkeypatch
):
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    monkeypatch.setattr(records, "_PAGE_SIZE", 1)
    store = _store(tmp_path)
    for offset, key in enumerate(("a", "b", "c", "d")):
        clock[0] = (100 + offset) * _DAY
        assert store.put("module-analysis", key, b"original", generation="g")
    with store.maintenance(dry_run=False) as maintenance:
        statements = []
        maintenance._connection.set_trace_callback(statements.append)
        inventory = maintenance.inventory()
        iterator = maintenance.iter_lru(inventory)
        first = next(iterator)
        assert first.key == "a"
        # Real admitted reuse occurs after wall time moves backward from the
        # last publication. Its recency advances but remains below the fence.
        clock[0] = 100 * _DAY + 301 * 1_000_000_000
        admitted = store.get("module-analysis", "a", generation="g", max_bytes=None)
        assert admitted is not None and admitted.refresh()
        second = next(iterator)
        assert second.key == "a" and second.used_ns > first.used_ns
        assert maintenance.remove([first]) == ("busy",)
        # Keep publishing between the forward cursor and captured upper key.
        # The initial inventory, rather than publisher quiescence, ends work.
        for index in range(inventory.entries - 2):
            clock[0] += 1
            key = f"interleaved-{index}"
            assert store.put("module-analysis", key, b"new", generation="g")
            assert next(iterator).key == key
        clock[0] += 1
        assert store.put("module-analysis", "must-wait", b"new", generation="g")
        with pytest.raises(StopIteration):
            next(iterator)
        assert sum("SELECT count(*)" in statement for statement in statements) == 1
        current = store.get("module-analysis", "a", generation="g", max_bytes=None)
        assert current is not None and current.payload == b"original"


def test_version_namespaces_preserve_flat_legacy_and_unknown_future_state(tmp_path):
    legacy = (
        tmp_path / records.SHARED_SOURCE_RECORDS_DIRECTORY / "source_records.sqlite"
    )
    legacy.parent.mkdir(parents=True)
    with closing(sqlite3.connect(legacy, isolation_level=None)) as peer:
        peer.execute("PRAGMA journal_mode=WAL")
        peer.execute("CREATE TABLE witness (value TEXT)")
        peer.execute("INSERT INTO witness VALUES ('legacy')")
        identity = legacy.stat().st_ino
        store = _store(tmp_path)
        assert store.put("import-scan", "k", b"new", generation="g")
        assert store.path.parent.name == f"v{records._SCHEMA_VERSION}"
        assert legacy.stat().st_ino == identity
        assert peer.execute("SELECT value FROM witness").fetchall() == [("legacy",)]
        assert peer.execute("PRAGMA integrity_check").fetchone() == ("ok",)
    assert records.superseded_store_namespace(f"v{records._SCHEMA_VERSION - 1}")
    for name in [
        f"v{records._SCHEMA_VERSION}",
        f"v{records._SCHEMA_VERSION + 1}",
        "v01",
        "v-1",
        "notes",
    ]:
        assert not records.superseded_store_namespace(name)


def test_namespace_retirement_refusal_is_retryable_without_sqlite_open(
    tmp_path, monkeypatch
):
    store = _store(tmp_path)
    exclusive = records._try_acquire_file_lock(store.lifetime_lock)
    assert exclusive is not None
    original = sqlite3.connect
    attempts = []

    def observed(*args, **kwargs):
        attempts.append(args)
        return original(*args, **kwargs)

    install_module_view(monkeypatch, "sqlite3", sqlite3, records, connect=observed)
    try:
        assert store.get("import-scan", "k", generation="g", max_bytes=8) is None
        assert not store.put("import-scan", "k", b"value", generation="g")
        assert attempts == [] and not store._unavailable
    finally:
        records._release_file_lock(exclusive)
    assert store.put("import-scan", "k", b"value", generation="g")
    assert len(attempts) == 1
    for _ in range(3):
        assert (
            store.get("import-scan", "k", generation="g", max_bytes=8).payload
            == b"value"
        )
    assert len(attempts) == 1
    assert records._try_acquire_file_lock(store.lifetime_lock) is None
    store.close()
    exclusive = records._try_acquire_file_lock(store.lifetime_lock)
    assert exclusive is not None
    records._release_file_lock(exclusive)


def _gc_inside_file_lock_mutex(cache_text, mutex_name, owner_kind, channel):
    import gc
    from molt import file_locks

    gc.disable()
    cache = Path(cache_text)
    store = records.source_record_store(cache)
    assert store.put("import-scan", "k", b"body", generation="g")
    lifetime_lock = store.lifetime_lock
    if owner_kind == "store":
        connection, owner = store._connect(), store
    else:
        store.close()
        owner = store.maintenance(dry_run=False)
        maintenance = owner.__enter__()
        connection = maintenance._connection
        del maintenance
    cycle = [owner]
    cycle.append(cycle)
    del store, owner, cycle
    channel.send("ready")
    with getattr(file_locks, mutex_name):
        gc.collect()
    try:
        connection.execute("SELECT 1")
    except sqlite3.ProgrammingError:
        pass
    else:
        raise AssertionError("GC did not close the abandoned SQLite connection")
    # A normal later cold open releases parked leases without doing lock work
    # from GC. Its independent cache root must not reacquire the original lease.
    probe = records.source_record_store(cache / "next")
    assert probe.put("import-scan", "probe", b"body", generation="g")
    exclusive = file_locks._try_acquire_file_lock(lifetime_lock)
    assert exclusive is not None
    file_locks._release_file_lock(exclusive)
    probe.close()
    channel.send("closed-and-released")
    channel.close()


@pytest.mark.parametrize("owner_kind", ["store", "maintenance"])
@pytest.mark.parametrize(
    "mutex_name", ["_IN_PROCESS_LOCK_REGISTRY_GUARD", "_FILE_LOCK_LIFECYCLE_CONDITION"]
)
def test_gc_cannot_reenter_file_lock_mutexes(tmp_path, mutex_name, owner_kind):
    ctx = multiprocessing.get_context("spawn")
    parent, channel = ctx.Pipe()
    child = ctx.Process(
        target=_gc_inside_file_lock_mutex,
        args=(str(tmp_path), mutex_name, owner_kind, channel),
    )
    child.start()
    channel.close()
    try:
        assert parent.poll(30) and parent.recv() == "ready", "observer did not start"
        assert parent.poll(5), "GC reentered a non-reentrant file-lock mutex"
        assert parent.recv() == "closed-and-released"
        child.join(5)
        assert child.exitcode == 0
    finally:
        parent.close()
        _close_fork_observer(child)
