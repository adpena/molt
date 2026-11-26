"""Immutable content-addressed frontend records (cache frontier step 2).

An import scan or a module analysis is a pure function of its key. An index
maps a key to its result digest and, for batched results, an extent in an
immutable content-addressed pack. A producer publishes the pack before its
index. Readers seek only the requested row and verify the returned extent
against its result digest. Directly addressed products use whole blobs.

Retention: an admitted read renews the blob's mtime at most hourly (a record
its consumer refuses is not renewed); the compile cache's
byte and age budget (``molt.cache_eviction``) unlinks old blobs and index
files and compacts the index. An entry whose blob is gone is a miss.
``docs/design/storage_authority.md`` ("Compile cache") owns the
design and its measurements.
"""

from __future__ import annotations

import hashlib
import os
import re
import stat
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Final, Literal, get_args

from molt.default_paths import (
    FRONTEND_RECORD_INDEX_SUFFIX as LEGACY_INDEX_SUFFIX,
    FRONTEND_RECORD_PACK_INDEX_SUFFIX as INDEX_SUFFIX,
    _default_molt_cache,
    frontend_record_files_root,
)
from molt.file_publication import PublicationDurability, atomic_write_bytes

FrontendRecordKind = Literal["import-scan", "module-analysis", "import-graph"]
_KINDS: Final = frozenset(get_args(FrontendRecordKind))
_INDEX_HEADER: Final = b"molt-frontend-record-index v1\n"
_DIGEST: Final = re.compile(r"[0-9a-f]{64}")
_KEY: Final = re.compile(r"[0-9a-f]{16,64}")
# A used blob's mtime moves at most this often (Go GOCACHE renews hourly).
_RENEW_NS: Final = 3600 * 10**9
# Retention merges index files past this count.
_MERGE_INDEX_FILES: Final = 64
# Longest line including its newline: kind, key, result digest and optional
# pack digest with its two fixed-width extents.
_INDEX_ROW_LIMIT: Final = max(map(len, _KINDS)) + 2 + 64 + 64 + 3 + 64 + 16 + 1
# An operation saves its queued index entries at most this many at a time.
_PENDING_INDEX_ENTRIES: Final = 4096
_PACK_BYTES: Final = 4 * 1024 * 1024
# Small builds seek directly. Repeated lookups can retain raw rows without
# decoding every key, keeping the aggregate below a fixed memory budget.
_RESIDENT_AFTER_LOOKUPS: Final = 16
_RESIDENT_INDEX_BYTES: Final = 8 * 1024 * 1024
_EXTENT: Final = re.compile(r"[0-9a-f]{8}")
_READ_FLAGS: Final = (
    os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
)


@dataclass(frozen=True, slots=True)
class FrontendRecord:
    """One read record; its consumer refreshes it only after admitting it."""

    payload: bytes
    path: Path | None
    mtime_ns: int
    offset: int = 0

    def refresh(self) -> None:
        """Renew the blob's mtime when an hour stale; advisory, never raises."""
        if self.path is not None and time.time_ns() - self.mtime_ns >= _RENEW_NS:
            try:
                os.utime(self.path)
            except OSError:
                pass


def record_digest(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def is_record_digest(value: str) -> bool:
    return _DIGEST.fullmatch(value) is not None


@dataclass(frozen=True, slots=True)
class _PackedExtent:
    digest: str
    offset: int
    size: int


@dataclass(frozen=True, slots=True)
class _RecordLocation:
    digest: str
    extent: _PackedExtent | None = None

    def render(self) -> str:
        extent = self.extent
        if extent is None:
            return self.digest
        return f"{self.digest}:{extent.digest}:{extent.offset:08x}:{extent.size:08x}"

    @classmethod
    def parse(cls, text: str) -> _RecordLocation | None:
        fields = text.split(":")
        if not is_record_digest(fields[0]):
            return None
        if len(fields) == 1:
            return cls(fields[0])
        if (
            len(fields) != 4
            or not is_record_digest(fields[1])
            or _EXTENT.fullmatch(fields[2]) is None
            or _EXTENT.fullmatch(fields[3]) is None
        ):
            return None
        offset, size = int(fields[2], 16), int(fields[3], 16)
        if offset + size > _PACK_BYTES:
            return None
        return cls(fields[0], _PackedExtent(fields[1], offset, size))


type _RecordIndex = dict[tuple[str, str], _RecordLocation]


def parse_index(data: bytes) -> _RecordIndex | None:
    """The entries of one index file, or None when it is not one."""
    if not data.startswith(_INDEX_HEADER):
        return None
    entries: _RecordIndex = {}
    try:
        text = data[len(_INDEX_HEADER) :].decode("ascii")
    except UnicodeDecodeError:
        return None
    for line in text.splitlines():
        fields = line.split(" ")
        if (
            len(fields) != 3
            or fields[0] not in _KINDS
            or _KEY.fullmatch(fields[1]) is None
            or (location := _RecordLocation.parse(fields[2])) is None
        ):
            return None
        entries[(fields[0], fields[1])] = location
    return entries


def render_index(entries: _RecordIndex) -> bytes:
    return _INDEX_HEADER + "".join(
        f"{kind} {key} {location.render()}\n"
        for (kind, key), location in sorted(entries.items())
    ).encode("ascii")


def _index_paths(directory: Path) -> tuple[Path, ...]:
    try:
        with os.scandir(directory) as entries:
            files = []
            for entry in entries:
                if not entry.name.endswith((LEGACY_INDEX_SUFFIX, INDEX_SUFFIX)):
                    continue
                try:
                    metadata = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                if stat.S_ISREG(metadata.st_mode):
                    files.append((metadata.st_mtime_ns, entry.name))
    except OSError:
        return ()
    return tuple(directory / name for _, name in sorted(files, reverse=True))


def _index_location(
    read_at: Callable[[int, int], bytes], size: int, prefix: bytes
) -> _RecordLocation | None:
    """Read at most two rows per step, even for a damaged index."""
    lower, upper = len(_INDEX_HEADER), size
    while lower < upper:
        middle = (lower + upper) // 2
        base = max(lower, middle - _INDEX_ROW_LIMIT)
        window = read_at(base, min(upper - base, 2 * _INDEX_ROW_LIMIT))
        offset = middle - base
        prior = window.rfind(b"\n", 0, offset)
        end = window.find(b"\n", offset)
        if end < 0 or (prior < 0 and base != lower):
            return None
        row = window[prior + 1 : end]
        if len(row) >= _INDEX_ROW_LIMIT:
            return None
        if row.startswith(prefix):
            try:
                return _RecordLocation.parse(row[len(prefix) :].decode("ascii"))
            except UnicodeDecodeError:
                return None
        if row < prefix:
            lower = base + end + 1
        else:
            upper = base + prior + 1
    return None


def _seek_index(path: Path, prefix: bytes) -> _RecordLocation | None:
    try:
        with open(
            path, "rb", opener=lambda name, _: os.open(name, _READ_FLAGS)
        ) as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode):
                return None
            if stream.read(len(_INDEX_HEADER)) != _INDEX_HEADER:
                return None

            def read_at(offset: int, length: int) -> bytes:
                stream.seek(offset)
                return stream.read(length)

            return _index_location(read_at, metadata.st_size, prefix)
    except OSError:
        return None


def _verified_index_snapshot(path: Path, budget: int) -> bytes | None:
    """Retain only complete bytes sealed by the immutable filename."""
    try:
        with open(
            path, "rb", opener=lambda name, _: os.open(name, _READ_FLAGS)
        ) as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > budget:
                return None
            data = stream.read(metadata.st_size + 1)
    except OSError:
        return None
    if (
        len(data) != metadata.st_size
        or not data.startswith(_INDEX_HEADER)
        or record_digest(data) != path.stem
    ):
        return None
    return data


# After repeated exhausted searches in a fragmented store, remember which
# 24-bit key prefixes occur in its immutable indices. A set bit still needs
# the ordinary lookup (including its kind, priority and extent validation).
_PREFIX_BYTES: Final = 1 << 21
_PREFIX_AFTER_MISSES: Final = 16
_PREFIX_INPUT_LIMIT: Final = 256 << 20


def _index_key_prefixes(indices: tuple[Path, ...]) -> bytearray | None:
    prefixes = bytearray(_PREFIX_BYTES)
    remaining = _PREFIX_INPUT_LIMIT
    for path in indices:
        try:
            with open(
                path, "rb", opener=lambda name, _: os.open(name, _READ_FLAGS)
            ) as stream:
                info = os.fstat(stream.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_size > remaining:
                    return None
                remaining -= info.st_size
                header = stream.readline(len(_INDEX_HEADER))
                if header != _INDEX_HEADER:
                    return None
                digest = hashlib.sha256(header)
                size = len(header)
                while line := stream.readline(_INDEX_ROW_LIMIT + 1):
                    size += len(line)
                    if len(line) > _INDEX_ROW_LIMIT or size > info.st_size:
                        return None
                    digest.update(line)
                    kind_end = line.find(b" ")
                    if kind_end < 0:
                        return None
                    prefix = int(line[kind_end + 1 : kind_end + 7], 16)
                    prefixes[prefix >> 3] |= 1 << (prefix & 7)
                # Only content-addressed, complete snapshots prove absence.
                if size != info.st_size or digest.hexdigest() != path.stem:
                    return None
        except (OSError, ValueError):
            return None
    return prefixes


def _read_indices(indices: tuple[Path, ...]) -> _RecordIndex:
    result: _RecordIndex = {}
    for index in reversed(indices):
        try:
            with open(
                index, "rb", opener=lambda name, _: os.open(name, _READ_FLAGS)
            ) as stream:
                if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                    continue
                entries = parse_index(stream.read())
        except OSError:
            continue
        if entries is not None:
            result.update(entries)
    return result


class FrontendRecordFiles:
    """One process's view of one cache root's record files."""

    def __init__(self, cache_root: Path) -> None:
        self.cache_root = cache_root
        self.root = frontend_record_files_root(cache_root)
        self._blobs = self.root / "blobs"
        self._index = self.root / "index"
        self._pid = os.getpid()
        self._lock = threading.RLock()
        self._lookups: dict[tuple[str, str], _RecordLocation | None] = {}
        self._indices: tuple[Path, ...] = ()
        self._index_generation: tuple[int, int, int] | None = None
        self._index_snapshots: dict[Path, bytes | None] = {}
        self._index_snapshot_budget = _RESIDENT_INDEX_BYTES
        self._key_prefixes: bytearray | None = None
        self._exhausted_searches = 0
        self._pending: _RecordIndex = {}
        self._payloads: dict[str, bytes] = {}
        self._payload_bytes = 0
        # Addresses this process recorded: newer than any index file it loads.
        self._written: _RecordIndex = {}

    def blob_path(self, digest: str) -> Path:
        return self._blobs / digest[:2] / digest

    # Reading.

    def get(
        self, kind: FrontendRecordKind, key: str, *, max_bytes: int
    ) -> FrontendRecord | None:
        """Read the record from this view or an immutable published generation."""
        if self._pid != os.getpid():
            return None
        with self._lock:
            location = self._location(kind, key)
            if location is None:
                return None
            if (
                location.extent is None
                and (payload := self._payloads.get(location.digest)) is not None
            ):
                return (
                    FrontendRecord(payload, None, 0)
                    if len(payload) <= max_bytes
                    else None
                )
        record = self._read_blob(location, max_bytes)
        if record is not None:
            return record
        # The same result may have moved to another pack. A long-lived view
        # must let the latest published index answer after retention evicts
        # the physical extent it first cached.
        address = (kind, key)
        with self._lock:
            if address in self._pending:
                return None
            if self._written.get(address) == location:
                del self._written[address]
            if self._lookups.get(address) == location:
                del self._lookups[address]
            current = self._location(kind, key)
        if current is None or current == location:
            return None
        return self._read_blob(current, max_bytes)

    def digest(self, kind: FrontendRecordKind, key: str) -> str | None:
        with self._lock:
            location = self._location(kind, key)
            return None if location is None else location.digest

    def _location(self, kind: FrontendRecordKind, key: str) -> _RecordLocation | None:
        if kind not in _KINDS or _KEY.fullmatch(key) is None:
            return None
        address = (kind, key)
        if address in self._written:
            return self._written[address]
        location = self._lookups.get(address)
        if location is not None:
            return location
        generation = self._directory_generation()
        if generation != self._index_generation:
            self._indices = _index_paths(self._index)
            self._lookups.clear()
            self._index_snapshots.clear()
            self._index_snapshot_budget = _RESIDENT_INDEX_BYTES
            self._key_prefixes = None
            self._exhausted_searches = 0
            self._index_generation = generation
        if address in self._lookups:
            return self._lookups[address]
        if (
            self._exhausted_searches == _PREFIX_AFTER_MISSES
            and len(self._indices) > _MERGE_INDEX_FILES
        ):
            self._exhausted_searches += 1
            self._key_prefixes = _index_key_prefixes(self._indices)
            if self._directory_generation() != generation:
                # Publication or compaction raced the scan. Observe the new
                # generation before using any negative result.
                return self._location(kind, key)
        if self._key_prefixes is not None:
            prefix_bits = int(key[:6], 16)
            if not self._key_prefixes[prefix_bits >> 3] & (1 << (prefix_bits & 7)):
                return None
        prefix = f"{kind} {key} ".encode("ascii")
        reader = (
            self._seek_resident_index
            if len(self._lookups) >= _RESIDENT_AFTER_LOOKUPS
            else _seek_index
        )
        location = next(
            (
                found
                for index in self._indices
                if (found := reader(index, prefix)) is not None
            ),
            None,
        )
        self._lookups[address] = location
        if location is None:
            self._exhausted_searches += 1
        return location

    def _seek_resident_index(self, path: Path, prefix: bytes) -> _RecordLocation | None:
        if path not in self._index_snapshots:
            snapshot = (
                _verified_index_snapshot(path, self._index_snapshot_budget)
                if self._index_snapshot_budget
                else None
            )
            self._index_snapshots[path] = snapshot
            if snapshot is not None:
                self._index_snapshot_budget -= len(snapshot)
        snapshot = self._index_snapshots[path]
        if snapshot is None:
            return _seek_index(path, prefix)
        return _index_location(
            lambda offset, length: snapshot[offset : offset + length],
            len(snapshot),
            prefix,
        )

    def addresses(self) -> dict[tuple[str, str], str]:
        """Explicit inventory of every address, including this view's writes."""
        with self._lock:
            return {
                address: location.digest
                for address, location in (
                    _read_indices(_index_paths(self._index)) | self._written
                ).items()
            }

    def _directory_generation(self) -> tuple[int, int, int] | None:
        try:
            metadata = self._index.stat()
        except OSError:
            return None
        return metadata.st_ino, metadata.st_mtime_ns, metadata.st_ctime_ns

    def compact_index(self) -> None:
        """Merge immutable index files during an existing retention sweep."""
        generation = self._directory_generation()
        indices = _index_paths(self._index)
        if len(indices) <= _MERGE_INDEX_FILES:
            return
        entries = _read_indices(indices)
        if not entries or self._directory_generation() != generation:
            return
        merged = render_index(entries)
        destination = self._index / (record_digest(merged) + INDEX_SUFFIX)
        if self._directory_generation() != generation or not self._publish(
            destination, merged
        ):
            return
        for old in indices:
            if old == destination:
                continue
            try:
                old.unlink()
            except OSError:
                # A peer merged it first, or (Windows) still reads it.
                continue

    def _read_blob(
        self, location: _RecordLocation, max_bytes: int
    ) -> FrontendRecord | None:
        extent = location.extent
        path = self.blob_path(location.digest if extent is None else extent.digest)
        try:
            descriptor = os.open(path, _READ_FLAGS)
        except OSError:
            return None
        try:
            info = os.fstat(descriptor)
            offset, size = (
                (0, info.st_size) if extent is None else (extent.offset, extent.size)
            )
            if (
                not stat.S_ISREG(info.st_mode)
                or size > max_bytes
                or offset + size > info.st_size
            ):
                return None
            if offset:
                os.lseek(descriptor, offset, os.SEEK_SET)
            chunks = []
            remaining = size
            while remaining:
                chunk = os.read(descriptor, remaining)
                if not chunk:
                    return None
                chunks.append(chunk)
                remaining -= len(chunk)
        except OSError:
            return None
        finally:
            os.close(descriptor)
        data = b"".join(chunks)
        if record_digest(data) != location.digest:
            return None
        return FrontendRecord(data, path, info.st_mtime_ns, offset)

    # Publication.

    def put(self, kind: FrontendRecordKind, key: str, payload: bytes) -> bool:
        """Publish ``payload`` as the record for ``key``; False when unwritable."""
        if not isinstance(payload, bytes):
            raise TypeError("frontend record payload must be bytes")
        if self._pid != os.getpid():
            return False
        digest = record_digest(payload)
        location = _RecordLocation(digest)
        if len(payload) <= _PACK_BYTES and self._pinned_by_operation():
            with self._lock:
                if digest not in self._payloads:
                    if (
                        self._payload_bytes + len(payload) > _PACK_BYTES
                        and not self.save()
                    ):
                        return False
                    self._payloads[digest] = payload
                    self._payload_bytes += len(payload)
                return self._record(kind, key, location)
        path = self.blob_path(digest)
        if not os.path.exists(path) and not self._publish(path, payload):
            return False
        return self._record(kind, key, location)

    def put_blob(self, payload: bytes) -> str | None:
        """Publish immutable bytes without inventing a second address index."""
        if self._pid != os.getpid():
            return None
        digest = record_digest(payload)
        path = self.blob_path(digest)
        if not os.path.exists(path) and not self._publish(path, payload):
            return None
        return digest

    def link(self, kind: FrontendRecordKind, key: str, same_as_key: str) -> bool:
        """Record that ``key`` has the result ``same_as_key`` has (HF-926)."""
        with self._lock:
            location = self._location(kind, same_as_key)
            return location is not None and self._record(kind, key, location)

    def _record(
        self, kind: FrontendRecordKind, key: str, location: _RecordLocation
    ) -> bool:
        if kind not in _KINDS or _KEY.fullmatch(key) is None:
            raise ValueError(f"invalid frontend record address: {kind} {key}")
        with self._lock:
            self._written[(kind, key)] = location
            self._pending[(kind, key)] = location
            due = len(self._pending) >= _PENDING_INDEX_ENTRIES
        if due or not self._pinned_by_operation():
            return self.save()
        return True

    def save(self) -> bool:
        """Publish the operation's bounded pack, then its index."""
        with self._lock:
            if not self._pending:
                return True
            published = self._pending.copy()
            if self._payloads:
                # A shared digest has one extent, even when several premises
                # alias that result. Sort by content to share equal packs.
                rows = sorted(self._payloads.items())
                pack = self.put_blob(b"".join(payload for _, payload in rows))
                if pack is None:
                    return False
                offset = 0
                locations = {}
                for digest, payload in rows:
                    locations[digest] = _RecordLocation(
                        digest, _PackedExtent(pack, offset, len(payload))
                    )
                    offset += len(payload)
                for address, location in published.items():
                    if location.extent is None:
                        published[address] = locations.get(location.digest, location)
            content = render_index(published)
            name = record_digest(content) + INDEX_SUFFIX
            if not self._publish(self._index / name, content):
                return False
            self._written.update(published)
            self._pending.clear()
            self._payloads.clear()
            self._payload_bytes = 0
            return True

    def pending_count(self) -> int:
        return len(self._pending)

    def _publish(self, path: Path, data: bytes) -> bool:
        try:
            atomic_write_bytes(
                path,
                data,
                exclusive=True,
                durability=PublicationDurability.RECONSTRUCTIBLE,
                purpose="record",
            )
        except FileExistsError:
            pass
        except OSError:
            return False
        return True

    def _pinned_by_operation(self) -> bool:
        """Whether a frontend operation owns this view and will save it."""
        from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION

        transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
        return transaction is not None and transaction.frontend_records is self


_VIEWS: dict[tuple[str, int], FrontendRecordFiles] = {}
_VIEWS_LOCK = threading.Lock()


def _after_fork() -> None:
    global _VIEWS_LOCK
    _VIEWS_LOCK = threading.Lock()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork)


def frontend_record_files_view(cache_root: Path) -> FrontendRecordFiles:
    key = (os.fspath(cache_root), os.getpid())
    with _VIEWS_LOCK:
        view = _VIEWS.get(key)
        if view is None:
            view = _VIEWS[key] = FrontendRecordFiles(cache_root)
    return view


def frontend_record_files() -> FrontendRecordFiles:
    """The current operation's view, pinned so it saves once at operation exit."""
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION

    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    view = transaction.frontend_records if transaction is not None else None
    if view is None or view._pid != os.getpid():
        root = view.cache_root if view is not None else _default_molt_cache().resolve()
        view = frontend_record_files_view(root)
        if transaction is not None:
            transaction.frontend_records = view
    return view
