"""Immutable content-addressed frontend records (cache frontier step 2).

The oracle is the filesystem itself: blob names are SHA-256 digests of their
bytes, index files are parsed independently of the reader under test, and a
second view stands in for another process.
"""

from __future__ import annotations

import hashlib
import os
import time
from pathlib import Path

import pytest

from molt.cli import frontend_record_files as records
from molt.cli.frontend_record_files import FrontendRecordFiles, parse_index

_LIMIT = 1 << 20
_KEY = "a" * 24
_OTHER = "b" * 24


def _read(
    view: FrontendRecordFiles,
    kind: records.FrontendRecordKind,
    key: str,
    *,
    max_bytes: int,
) -> bytes | None:
    record = view.get(kind, key, max_bytes=max_bytes)
    return None if record is None else record.payload


def _blobs(root: Path) -> dict[str, bytes]:
    found = {}
    for path in (records.frontend_record_files_root(root) / "blobs").rglob("*"):
        if path.is_file() and not path.name.startswith("."):
            found[path.name] = path.read_bytes()
    return found


def _index_files(root: Path) -> list[Path]:
    index = records.frontend_record_files_root(root) / "index"
    return sorted(index.glob(f"*{records.INDEX_SUFFIX}")) if index.is_dir() else []


def test_records_are_blobs_named_by_their_bytes_and_shared_when_equal(
    tmp_path: Path,
) -> None:
    files = FrontendRecordFiles(tmp_path)
    assert files.put("import-scan", _KEY, b'{"scan": 1}')
    assert files.put("import-scan", _OTHER, b'{"scan": 1}')
    assert files.put("module-analysis", _KEY, b'{"analysis": 2}')
    blobs = _blobs(tmp_path)
    assert blobs == {
        hashlib.sha256(data).hexdigest(): data
        for data in (b'{"scan": 1}', b'{"analysis": 2}')
    }
    # Outside an operation every put publishes its index entry at once.
    entries: dict[tuple[str, str], str] = {}
    for path in _index_files(tmp_path):
        parsed = parse_index(path.read_bytes())
        assert parsed is not None
        entries.update(parsed)
    assert entries == {
        ("import-scan", _KEY): hashlib.sha256(b'{"scan": 1}').hexdigest(),
        ("import-scan", _OTHER): hashlib.sha256(b'{"scan": 1}').hexdigest(),
        ("module-analysis", _KEY): hashlib.sha256(b'{"analysis": 2}').hexdigest(),
    }
    peer = FrontendRecordFiles(tmp_path)
    assert _read(peer, "import-scan", _OTHER, max_bytes=_LIMIT) == b'{"scan": 1}'
    assert _read(peer, "module-analysis", _KEY, max_bytes=_LIMIT) == b'{"analysis": 2}'
    assert _read(peer, "module-analysis", _OTHER, max_bytes=_LIMIT) is None


@pytest.mark.parametrize("damage", ["flipped", "truncated", "missing", "oversized"])
def test_a_damaged_missing_or_oversized_blob_is_a_miss(
    tmp_path: Path, damage: str
) -> None:
    payload = b'{"scan": "' + b"x" * 64 + b'"}'
    files = FrontendRecordFiles(tmp_path)
    assert files.put("import-scan", _KEY, payload)
    blob = files.blob_path(hashlib.sha256(payload).hexdigest())
    limit = _LIMIT
    if damage == "flipped":
        blob.write_bytes(payload.replace(b"x", b"y", 1))
    elif damage == "truncated":
        blob.write_bytes(payload[:-3])
    elif damage == "missing":
        blob.unlink()
    else:
        limit = len(payload) - 1
    assert (
        _read(FrontendRecordFiles(tmp_path), "import-scan", _KEY, max_bytes=limit)
        is None
    )


def test_an_operation_publishes_one_index_and_a_peer_sees_it_after_a_miss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = FrontendRecordFiles(tmp_path)
    monkeypatch.setattr(files, "_pinned_by_operation", lambda: True)
    peer = FrontendRecordFiles(tmp_path)
    assert _read(peer, "import-scan", _KEY, max_bytes=_LIMIT) is None
    for index in range(5):
        assert files.put("import-scan", f"{index:024x}", f"[{index}]".encode())
    # Queued, not indexed: the owner reads its own records, a peer cannot.
    assert _index_files(tmp_path) == []
    assert _read(files, "import-scan", f"{3:024x}", max_bytes=_LIMIT) == b"[3]"
    assert files.save()
    assert len(_index_files(tmp_path)) == 1
    # The peer loaded the index before it existed; its next miss rescans.
    assert _read(peer, "import-scan", f"{4:024x}", max_bytes=_LIMIT) == b"[4]"


def test_the_newest_index_entry_wins_and_own_writes_beat_loaded_files(
    tmp_path: Path,
) -> None:
    """A key names its source by path; a changed source publishes a newer
    entry for the same key, and a reader must not keep the stale one."""
    first = FrontendRecordFiles(tmp_path)
    assert first.put("import-scan", _KEY, b'{"generation": 1}')
    old_index = _index_files(tmp_path)[0]
    past = time.time() - 60
    os.utime(old_index, (past, past))
    second = FrontendRecordFiles(tmp_path)
    assert second.put("import-scan", _KEY, b'{"generation": 2}')
    assert (
        _read(FrontendRecordFiles(tmp_path), "import-scan", _KEY, max_bytes=_LIMIT)
        == b'{"generation": 2}'
    )
    # The first writer keeps its own newer-for-it record over loaded files.
    assert _read(first, "import-scan", _KEY, max_bytes=_LIMIT) == b'{"generation": 1}'


def test_link_names_the_equal_scans_blob_and_needs_a_known_target(
    tmp_path: Path,
) -> None:
    files = FrontendRecordFiles(tmp_path)
    assert not files.link("import-scan", _OTHER, _KEY)
    assert files.put("import-scan", _KEY, b'{"scan": 1}')
    assert files.link("import-scan", _OTHER, _KEY)
    assert len(_blobs(tmp_path)) == 1
    assert (
        _read(FrontendRecordFiles(tmp_path), "import-scan", _OTHER, max_bytes=_LIMIT)
        == b'{"scan": 1}'
    )


def test_a_reader_merges_many_index_files_into_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(records, "_MERGE_INDEX_FILES", 4)
    writer = FrontendRecordFiles(tmp_path)
    for index in range(6):
        assert writer.put("module-analysis", f"{index:024x}", f"[{index}]".encode())
    assert len(_index_files(tmp_path)) == 6
    reader = FrontendRecordFiles(tmp_path)
    assert _read(reader, "module-analysis", f"{5:024x}", max_bytes=_LIMIT) == b"[5]"
    (merged,) = _index_files(tmp_path)
    parsed = parse_index(merged.read_bytes())
    assert parsed is not None and len(parsed) == 6
    assert merged.name == hashlib.sha256(merged.read_bytes()).hexdigest() + ".idx"
    assert all(
        _read(
            FrontendRecordFiles(tmp_path),
            "module-analysis",
            f"{index:024x}",
            max_bytes=_LIMIT,
        )
        == f"[{index}]".encode()
        for index in range(6)
    )


def test_a_malformed_index_file_answers_nothing(tmp_path: Path) -> None:
    files = FrontendRecordFiles(tmp_path)
    assert files.put("import-scan", _KEY, b"[1]")
    index = records.frontend_record_files_root(tmp_path) / "index"
    digest = hashlib.sha256(b"[2]").hexdigest()
    (index / ("f" * 64 + ".idx")).write_bytes(
        b"not an index\nimport-scan " + _OTHER.encode() + b" " + digest.encode()
    )
    peer = FrontendRecordFiles(tmp_path)
    assert _read(peer, "import-scan", _KEY, max_bytes=_LIMIT) == b"[1]"
    assert _read(peer, "import-scan", _OTHER, max_bytes=_LIMIT) is None


def test_only_an_admitted_read_renews_a_stale_blob_at_most_hourly(
    tmp_path: Path,
) -> None:
    files = FrontendRecordFiles(tmp_path)
    assert files.put("import-scan", _KEY, b"[1]")
    blob = files.blob_path(hashlib.sha256(b"[1]").hexdigest())
    stale = time.time() - 2 * 3600
    os.utime(blob, (stale, stale))
    record = files.get("import-scan", _KEY, max_bytes=_LIMIT)
    assert record is not None and record.payload == b"[1]"
    # Reading is not admission: a refused record keeps its age.
    assert blob.stat().st_mtime == pytest.approx(stale, abs=1)
    record.refresh()
    assert blob.stat().st_mtime > stale + 3600
    recent = time.time() - 60
    os.utime(blob, (recent, recent))
    record = files.get("import-scan", _KEY, max_bytes=_LIMIT)
    assert record is not None
    record.refresh()
    assert blob.stat().st_mtime == pytest.approx(recent, abs=1)


def test_cache_eviction_sees_each_blob_and_index_file(tmp_path: Path) -> None:
    from molt.cache_eviction import _collect_entries

    files = FrontendRecordFiles(tmp_path)
    assert files.put("import-scan", _KEY, b"[1]")
    assert files.put("module-analysis", _KEY, b"[2]")
    expected = {
        files.blob_path(hashlib.sha256(data).hexdigest()) for data in (b"[1]", b"[2]")
    } | set(_index_files(tmp_path))
    collected = {entry.path for entry in _collect_entries(tmp_path)}
    assert expected <= collected
    root = records.frontend_record_files_root(tmp_path)
    assert root not in collected and root.parent not in collected
