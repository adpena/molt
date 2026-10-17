"""The per-source record store keeps, replaces, retires and recovers records."""

from __future__ import annotations

import json
import multiprocessing
import os
import sqlite3
import time
from pathlib import Path

import pytest

from molt.cli import source_record_store as records
from tests.process_guard_common import install_module_view

_HOUR = 3600 * 1_000_000_000
_DAY = 24 * _HOUR
_LIMIT = 1 << 20


def _store(tmp_path: Path) -> records.SourceRecordStore:
    return records.SourceRecordStore(tmp_path / records.STORE_NAME)


def test_records_round_trip_per_kind_and_key(tmp_path: Path) -> None:
    store = _store(tmp_path)
    assert store.put("import-scan", "k1", {"imports": ["math"]}, generation="g")
    assert store.put("intrinsic-facts", "k1", {"facts": 1}, generation="g")

    assert store.get("import-scan", "k1", generation="g", max_bytes=_LIMIT) == {
        "imports": ["math"]
    }
    assert store.get("intrinsic-facts", "k1", generation="g", max_bytes=_LIMIT) == {
        "facts": 1
    }
    assert store.get("import-scan", "k2", generation="g", max_bytes=_LIMIT) is None


def test_a_later_record_replaces_the_earlier_one(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.put("import-scan", "k", {"source_sha256": "old"}, generation="g")
    store.put("import-scan", "k", {"source_sha256": "new"}, generation="g")

    assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) == {
        "source_sha256": "new"
    }
    with sqlite3.connect(tmp_path / records.STORE_NAME) as connection:
        assert connection.execute("SELECT count(*) FROM records").fetchone() == (1,)


def test_oversized_and_non_object_records_are_misses(tmp_path: Path) -> None:
    store = _store(tmp_path)
    store.put("import-scan", "big", {"blob": "x" * 64}, generation="g")
    store.put("import-scan", "list", {"a": 1}, generation="g")
    with sqlite3.connect(tmp_path / records.STORE_NAME) as connection:
        connection.execute(
            "UPDATE records SET payload = ? WHERE key = 'list'", (b"[1, 2]",)
        )

    assert store.get("import-scan", "big", generation="g", max_bytes=32) is None
    assert store.get("import-scan", "list", generation="g", max_bytes=_LIMIT) is None


def test_another_process_reads_what_this_process_wrote(tmp_path: Path) -> None:
    _store(tmp_path).put("import-scan", "k", {"v": 7}, generation="g")
    with multiprocessing.get_context("spawn").Pool(1) as pool:
        seen = pool.apply(_read_in_child, (str(tmp_path),))
    assert seen == {"v": 7}


def _read_in_child(root: str) -> object:
    store = records.source_record_store(Path(root))
    return store.get("import-scan", "k", generation="g", max_bytes=_LIMIT)


def test_retired_generations_lose_their_records(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    clock = [100 * _DAY]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])

    # Six compiler generations, one an hour apart; each process uses one.
    for index in range(6):
        clock[0] += 2 * _HOUR
        _store(tmp_path).put(
            "import-scan", f"k{index}", {"i": index}, generation=f"g{index}"
        )

    kept = {
        index
        for index in range(6)
        if _store(tmp_path).get(
            "import-scan", f"k{index}", generation="probe", max_bytes=_LIMIT
        )
        is not None
    }
    # Only the four most recently used generations keep their records.
    assert kept == {2, 3, 4, 5}

    # A week without use retires even a recent generation.
    clock[0] += 8 * _DAY
    _store(tmp_path).put("import-scan", "fresh", {"i": 9}, generation="g9")
    survivors = {
        key
        for (key,) in sqlite3.connect(tmp_path / records.STORE_NAME).execute(
            "SELECT key FROM records"
        )
    }
    assert survivors == {"fresh"}


def test_a_corrupt_store_is_recreated(tmp_path: Path) -> None:
    path = tmp_path / records.STORE_NAME
    path.write_bytes(b"not a database" * 512)

    store = _store(tmp_path)
    assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) is None
    store.put("import-scan", "k", {"v": 1}, generation="g")
    assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) == {"v": 1}
    assert json.loads(
        sqlite3.connect(path).execute("SELECT payload FROM records").fetchone()[0]
    ) == {"v": 1}


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
        assert store.put("import-scan", "k", {"v": 1}, generation="g") is False
        assert store.get("import-scan", "k", generation="g", max_bytes=_LIMIT) is None
    finally:
        root.chmod(0o700)
