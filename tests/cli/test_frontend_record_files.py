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
        entries.update(
            {address: location.digest for address, location in parsed.items()}
        )
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
    monkeypatch.setattr(records, "_PACK_BYTES", 8)
    peer = FrontendRecordFiles(tmp_path)
    assert _read(peer, "import-scan", _KEY, max_bytes=_LIMIT) is None
    for index in range(5):
        assert files.put("import-scan", f"{index:024x}", f"[{index}]".encode())
    # Equal records and aliases in later batches point to the first extent.
    assert files.put("import-scan", _KEY, b"[0]")
    assert files.link("import-scan", _OTHER, _KEY)
    # Queued, not indexed: the owner reads its own records, a peer cannot.
    assert _index_files(tmp_path) == []
    assert _read(files, "import-scan", f"{3:024x}", max_bytes=_LIMIT) == b"[3]"
    assert _read(files, "import-scan", _OTHER, max_bytes=_LIMIT) == b"[0]"
    assert files.save()
    assert len(_index_files(tmp_path)) == 1
    assert len(_blobs(tmp_path)) == 3
    # The peer loaded the index before it existed; its next miss rescans.
    assert _read(peer, "import-scan", f"{4:024x}", max_bytes=_LIMIT) == b"[4]"
    assert _read(peer, "import-scan", _OTHER, max_bytes=_LIMIT) == b"[0]"


def test_member_verification_is_independent_inside_a_pack(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = FrontendRecordFiles(tmp_path)
    monkeypatch.setattr(files, "_pinned_by_operation", lambda: True)
    assert files.put("import-scan", _KEY, b"[1]")
    assert files.put("import-scan", _OTHER, b"[2]")
    assert files.save()
    peer = FrontendRecordFiles(tmp_path)
    first = peer.get("import-scan", _KEY, max_bytes=_LIMIT)
    second = peer.get("import-scan", _OTHER, max_bytes=_LIMIT)
    assert first is not None and second is not None
    assert first.path is not None and first.path == second.path
    with first.path.open("r+b") as stream:
        stream.seek(first.offset)
        stream.write(b"!")
    assert peer.get("import-scan", _KEY, max_bytes=_LIMIT) is None
    assert _read(peer, "import-scan", _OTHER, max_bytes=_LIMIT) == b"[2]"


def test_a_long_lived_view_follows_republication_after_pack_eviction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    initial = FrontendRecordFiles(tmp_path)
    latest = FrontendRecordFiles(tmp_path)
    peer = FrontendRecordFiles(tmp_path)
    for files, other in [(initial, b"[2]"), (latest, b"[3]")]:
        monkeypatch.setattr(files, "_pinned_by_operation", lambda: True)
        assert files.put("import-scan", _KEY, b"[1]")
        assert files.put("import-scan", _OTHER, other)
        assert files.save()
        if files is initial:
            record = peer.get("import-scan", _KEY, max_bytes=_LIMIT)
            assert record is not None and record.path is not None
            first_pack = record.path
            old_index = _index_files(tmp_path)[0]
            past = time.time() - 60
            os.utime(old_index, (past, past))
    first_pack.unlink()
    for view in (initial, peer):
        assert _read(view, "import-scan", _KEY, max_bytes=_LIMIT) == b"[1]"


def test_failed_pack_index_leaves_legacy_record_visible_until_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = FrontendRecordFiles(tmp_path)
    assert files.put("import-scan", _KEY, b"[1]")
    (index,) = _index_files(tmp_path)
    index.rename(index.with_suffix(records.LEGACY_INDEX_SUFFIX))
    peer = FrontendRecordFiles(tmp_path)
    assert _read(peer, "import-scan", _KEY, max_bytes=_LIMIT) == b"[1]"

    monkeypatch.setattr(files, "_pinned_by_operation", lambda: True)
    publish = files._publish

    def refuse_index(path: Path, data: bytes) -> bool:
        return path.suffix != records.INDEX_SUFFIX and publish(path, data)

    monkeypatch.setattr(files, "_publish", refuse_index)
    assert files.put("import-scan", _OTHER, b"[2]")
    assert not files.save()
    assert _index_files(tmp_path) == []
    assert peer.get("import-scan", _OTHER, max_bytes=_LIMIT) is None
    assert _read(files, "import-scan", _OTHER, max_bytes=_LIMIT) == b"[2]"

    monkeypatch.setattr(files, "_publish", publish)
    assert files.save()
    assert _read(peer, "import-scan", _KEY, max_bytes=_LIMIT) == b"[1]"
    assert _read(peer, "import-scan", _OTHER, max_bytes=_LIMIT) == b"[2]"


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


def test_retention_merges_many_index_files_without_foreground_compaction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(records, "_MERGE_INDEX_FILES", 4)
    writer = FrontendRecordFiles(tmp_path)
    for index in range(6):
        assert writer.put("module-analysis", f"{index:024x}", f"[{index}]".encode())
    assert len(_index_files(tmp_path)) == 6
    reader = FrontendRecordFiles(tmp_path)
    assert _read(reader, "module-analysis", f"{5:024x}", max_bytes=_LIMIT) == b"[5]"
    assert len(_index_files(tmp_path)) == 6
    from molt.cache_eviction import _prune_locked

    _prune_locked(tmp_path, max_bytes=None, max_age_days=None, dry_run=True)
    assert len(_index_files(tmp_path)) == 6
    _prune_locked(tmp_path, max_bytes=None, max_age_days=None, dry_run=False)
    (merged,) = _index_files(tmp_path)
    parsed = parse_index(merged.read_bytes())
    assert parsed is not None and len(parsed) == 6
    assert (
        merged.name
        == hashlib.sha256(merged.read_bytes()).hexdigest() + records.INDEX_SUFFIX
    )
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


def test_seek_resolves_variable_width_rows_without_decoding_unasked_entries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    files = FrontendRecordFiles(tmp_path)
    monkeypatch.setattr(files, "_pinned_by_operation", lambda: True)
    keys = [f"{value:0{16 + value % 49}x}" for value in range(257)]
    for kind in ("import-scan", "module-analysis"):
        for key in keys:
            assert files.put(kind, key, f"{kind} {key}".encode())
    assert files.save()

    def unexpected_full_scan(data: bytes) -> None:
        pytest.fail(f"a lookup decoded an entire {len(data)}-byte index")

    monkeypatch.setattr(records, "parse_index", unexpected_full_scan)
    reader = FrontendRecordFiles(tmp_path)
    for kind in ("import-scan", "module-analysis"):
        for key in keys:
            assert (
                _read(reader, kind, key, max_bytes=_LIMIT) == f"{kind} {key}".encode()
            )
        assert _read(reader, kind, "f" * 64, max_bytes=_LIMIT) is None


def test_resident_index_reuses_verified_bytes_until_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(records, "_RESIDENT_AFTER_LOOKUPS", 0)
    writer = FrontendRecordFiles(tmp_path)
    assert writer.put("import-scan", _KEY, b"before")
    reader = FrontendRecordFiles(tmp_path)
    assert _read(reader, "import-scan", _KEY, max_bytes=_LIMIT) == b"before"

    opened = []
    original_snapshot = records._verified_index_snapshot

    def snapshot(path: Path, budget: int) -> bytes | None:
        opened.append(path)
        return original_snapshot(path, budget)

    monkeypatch.setattr(records, "_verified_index_snapshot", snapshot)
    assert reader.digest("import-scan", _OTHER) is None
    assert reader.digest("import-scan", "c" * 24) is None
    assert opened == []

    assert writer.put("import-scan", _OTHER, b"published")
    assert _read(reader, "import-scan", _OTHER, max_bytes=_LIMIT) == b"published"
    assert len(opened) == 1


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


def test_fragmented_index_skips_proven_absences_and_seeks_prefix_collisions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(records, "_MERGE_INDEX_FILES", 1)
    monkeypatch.setattr(records, "_PREFIX_AFTER_MISSES", 2)
    writer = FrontendRecordFiles(tmp_path)
    assert writer.put("import-scan", _KEY, b"a")
    assert writer.put("module-analysis", _OTHER, b"b")
    reader = FrontendRecordFiles(tmp_path)
    calls = 0
    seek = records._seek_index

    def counted(path: Path, prefix: bytes) -> records._RecordLocation | None:
        nonlocal calls
        calls += 1
        return seek(path, prefix)

    monkeypatch.setattr(records, "_seek_index", counted)
    assert _read(reader, "import-scan", _KEY, max_bytes=_LIMIT) == b"a"
    assert reader._key_prefixes is None
    for char in "cd":
        assert reader.digest("import-scan", char * 24) is None
    prior = calls
    assert reader.digest("import-scan", "e" * 24) is None
    assert calls == prior
    assert reader._key_prefixes is not None
    # Prefix collisions for another key or kind go through the usual search.
    assert reader.digest("import-scan", "aaaaaa" + "1" * 18) is None
    assert calls > prior
    assert _read(reader, "module-analysis", _OTHER, max_bytes=_LIMIT) == b"b"
    assert reader.digest("import-scan", _OTHER) is None
    # The peer can add precisely the key this generation ruled out.
    assert writer.put("import-scan", "e" * 24, b"new")
    assert _read(reader, "import-scan", "e" * 24, max_bytes=_LIMIT) == b"new"


def test_index_prefix_absence_observes_a_publication_racing_the_scan(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(records, "_MERGE_INDEX_FILES", 1)
    monkeypatch.setattr(records, "_PREFIX_AFTER_MISSES", 1)
    writer = FrontendRecordFiles(tmp_path)
    assert writer.put("import-scan", _KEY, b"a")
    assert writer.put("import-scan", _OTHER, b"b")
    reader = FrontendRecordFiles(tmp_path)
    assert reader.digest("import-scan", "c" * 24) is None
    new = "d" * 24
    scan = records._index_key_prefixes

    def publish_after_scan(indices: tuple[Path, ...]) -> bytearray | None:
        prefixes = scan(indices)
        assert prefixes is not None
        assert writer.put("import-scan", new, b"fresh")
        return prefixes

    monkeypatch.setattr(records, "_index_key_prefixes", publish_after_scan)
    assert _read(reader, "import-scan", new, max_bytes=_LIMIT) == b"fresh"


def test_unverified_index_or_input_limit_falls_back_to_ordinary_lookup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(records, "_MERGE_INDEX_FILES", 1)
    monkeypatch.setattr(records, "_PREFIX_AFTER_MISSES", 1)
    writer = FrontendRecordFiles(tmp_path)
    assert writer.put("import-scan", _KEY, b"a")
    assert writer.put("import-scan", _OTHER, b"b")
    indices = tuple(_index_files(tmp_path))
    # A concurrent repair can replace a damaged file. Its bytes cannot seal
    # a negative proof for that immutable name.
    damaged = indices[0].with_name("0" * 64 + records.INDEX_SUFFIX)
    damaged.write_bytes(indices[0].read_bytes())
    reader = FrontendRecordFiles(tmp_path)
    assert reader.digest("import-scan", "c" * 24) is None
    assert _read(reader, "import-scan", _KEY, max_bytes=_LIMIT) == b"a"
    assert reader._key_prefixes is None
    monkeypatch.setattr(records, "_PREFIX_INPUT_LIMIT", 1)
    assert records._index_key_prefixes(indices) is None
