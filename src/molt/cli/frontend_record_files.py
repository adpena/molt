"""Immutable content-addressed frontend records (cache frontier step 2).

An import scan or a module analysis is a pure function of its key. Its result
is one immutable file named by the SHA-256 of its bytes (``blobs/aa/<digest>``),
and index files map keys to digests (``index/<digest>.idx``), as Go's GOCACHE
pairs an action ID with an output ID. One frontend operation publishes one
index file for the results it made. Publication is temp file plus rename and
idempotent: equal results share one blob, and a concurrent writer of the same
blob or index loses nothing. A reader takes no lock and opens no database: it
loads the index files once (again only when a miss finds the index directory
changed), then reads a blob with one open. A blob whose bytes do not hash to
its name is a miss, so a torn or damaged file never answers.

Retention: an admitted read renews the blob's mtime at most hourly (a record
its consumer refuses is not renewed); the compile cache's
byte and age budget (``molt.cache_eviction``) unlinks old blobs and index
files. A reader merges many index files into one; an entry whose blob is gone
is a miss. ``docs/design/storage_authority.md`` ("Compile cache") owns the
design and its measurements.
"""

from __future__ import annotations

import hashlib
import os
import re
import stat
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Final, Literal, get_args

from molt.default_paths import FRONTEND_RECORD_FILES_DIRECTORY, _default_molt_cache
from molt.file_publication import PublicationDurability, atomic_write_bytes

FrontendRecordKind = Literal["import-scan", "module-analysis"]
_KINDS: Final = frozenset(get_args(FrontendRecordKind))
LAYOUT: Final = "v1"
INDEX_SUFFIX: Final = ".idx"
_INDEX_HEADER: Final = b"molt-frontend-record-index v1\n"
_DIGEST: Final = re.compile(r"[0-9a-f]{64}")
_KEY: Final = re.compile(r"[0-9a-f]{16,64}")
# A used blob's mtime moves at most this often (Go GOCACHE renews hourly).
_RENEW_NS: Final = 3600 * 10**9
# A reader merges the index files it loaded into one past this count.
_MERGE_INDEX_FILES: Final = 64
# An operation saves its queued index entries at most this many at a time.
_PENDING_INDEX_ENTRIES: Final = 4096
_READ_FLAGS: Final = (
    os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
)


@dataclass(frozen=True, slots=True)
class FrontendRecord:
    """One read record; its consumer refreshes it only after admitting it."""

    payload: bytes
    path: Path
    mtime_ns: int

    def refresh(self) -> None:
        """Renew the blob's mtime when an hour stale; advisory, never raises."""
        if time.time_ns() - self.mtime_ns >= _RENEW_NS:
            try:
                os.utime(self.path)
            except OSError:
                pass


def record_digest(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def frontend_record_files_root(cache_root: Path) -> Path:
    return cache_root / FRONTEND_RECORD_FILES_DIRECTORY / LAYOUT


def parse_index(data: bytes) -> dict[tuple[str, str], str] | None:
    """The entries of one index file, or None when it is not one."""
    if not data.startswith(_INDEX_HEADER):
        return None
    entries: dict[tuple[str, str], str] = {}
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
            or _DIGEST.fullmatch(fields[2]) is None
        ):
            return None
        entries[(fields[0], fields[1])] = fields[2]
    return entries


def render_index(entries: dict[tuple[str, str], str]) -> bytes:
    return _INDEX_HEADER + "".join(
        f"{kind} {key} {digest}\n" for (kind, key), digest in sorted(entries.items())
    ).encode("ascii")


class FrontendRecordFiles:
    """One process's view of one cache root's record files."""

    def __init__(self, cache_root: Path) -> None:
        self.cache_root = cache_root
        self.root = frontend_record_files_root(cache_root)
        self._blobs = self.root / "blobs"
        self._index = self.root / "index"
        self._pid = os.getpid()
        self._lock = threading.Lock()
        self._table: dict[tuple[str, str], str] = {}
        self._loaded: set[str] = set()
        self._index_mtime_ns: int | None = None
        self._pending: dict[tuple[str, str], str] = {}
        # Addresses this process recorded: newer than any index file it loads.
        self._written: set[tuple[str, str]] = set()

    def blob_path(self, digest: str) -> Path:
        return self._blobs / digest[:2] / digest

    # Reading.

    def get(
        self, kind: FrontendRecordKind, key: str, *, max_bytes: int
    ) -> FrontendRecord | None:
        """The record, or None; never waits and never raises OSError."""
        if self._pid != os.getpid():
            return None
        digest = self.digest(kind, key)
        return None if digest is None else self._read_blob(digest, max_bytes)

    def digest(self, kind: FrontendRecordKind, key: str) -> str | None:
        address = (kind, key)
        with self._lock:
            digest = self._table.get(address)
            if digest is None and self._index_changed_locked():
                self._load_index_locked()
                digest = self._table.get(address)
        return digest

    def addresses(self) -> dict[tuple[str, str], str]:
        """Every address this view resolves, with the index files loaded."""
        with self._lock:
            if self._index_changed_locked():
                self._load_index_locked()
            return dict(self._table)

    def _index_changed_locked(self) -> bool:
        try:
            mtime = os.stat(self._index).st_mtime_ns
        except OSError:
            return False
        return mtime != self._index_mtime_ns

    def _load_index_locked(self) -> None:
        try:
            self._index_mtime_ns = os.stat(self._index).st_mtime_ns
            names = os.listdir(self._index)
        except OSError:
            return
        # A key names its source by path, and the payload checks the bytes, so
        # index files may map one key to several source generations: the most
        # recently published entry wins, and nothing loaded overrides an entry
        # this process wrote.
        fresh: list[tuple[int, str]] = []
        for name in names:
            if not name.endswith(INDEX_SUFFIX) or name in self._loaded:
                continue
            self._loaded.add(name)
            try:
                fresh.append((os.stat(self._index / name).st_mtime_ns, name))
            except OSError:
                continue
        for _mtime, name in sorted(fresh):
            try:
                with open(self._index / name, "rb") as stream:
                    entries = parse_index(stream.read())
            except OSError:
                continue
            if entries is not None:
                for address, digest in entries.items():
                    if address not in self._written:
                        self._table[address] = digest
        live = {name for name in names if name in self._loaded}
        if len(live) > _MERGE_INDEX_FILES:
            self._merge_locked(live)

    def _merge_locked(self, names: set[str]) -> None:
        merged = render_index(self._table)
        name = record_digest(merged) + INDEX_SUFFIX
        if not self._publish(self._index / name, merged):
            return
        self._loaded.add(name)
        for old in names - {name}:
            try:
                os.unlink(self._index / old)
            except OSError:
                # A peer merged it first, or (Windows) still reads it.
                continue

    def _read_blob(self, digest: str, max_bytes: int) -> FrontendRecord | None:
        path = self.blob_path(digest)
        try:
            descriptor = os.open(path, _READ_FLAGS)
        except OSError:
            return None
        try:
            info = os.fstat(descriptor)
            if not stat.S_ISREG(info.st_mode) or info.st_size > max_bytes:
                return None
            chunks = []
            remaining = info.st_size
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
        if record_digest(data) != digest:
            return None
        return FrontendRecord(data, path, info.st_mtime_ns)

    # Publication.

    def put(self, kind: FrontendRecordKind, key: str, payload: bytes) -> bool:
        """Publish ``payload`` as the record for ``key``; False when unwritable."""
        if not isinstance(payload, bytes):
            raise TypeError("frontend record payload must be bytes")
        if self._pid != os.getpid():
            return False
        digest = record_digest(payload)
        path = self.blob_path(digest)
        if not os.path.exists(path) and not self._publish(path, payload):
            return False
        return self._record(kind, key, digest)

    def link(self, kind: FrontendRecordKind, key: str, same_as_key: str) -> bool:
        """Record that ``key`` has the result ``same_as_key`` has (HF-926)."""
        digest = self.digest(kind, same_as_key)
        return digest is not None and self._record(kind, key, digest)

    def _record(self, kind: FrontendRecordKind, key: str, digest: str) -> bool:
        if kind not in _KINDS or _KEY.fullmatch(key) is None:
            raise ValueError(f"invalid frontend record address: {kind} {key}")
        with self._lock:
            self._table[(kind, key)] = digest
            self._written.add((kind, key))
            self._pending[(kind, key)] = digest
            due = len(self._pending) >= _PENDING_INDEX_ENTRIES
        if due or not self._pinned_by_operation():
            return self.save()
        return True

    def save(self) -> bool:
        """Publish one index file of the queued entries."""
        with self._lock:
            pending, self._pending = self._pending, {}
        if not pending:
            return True
        content = render_index(pending)
        name = record_digest(content) + INDEX_SUFFIX
        if not self._publish(self._index / name, content):
            with self._lock:
                for address, digest in pending.items():
                    self._pending.setdefault(address, digest)
            return False
        with self._lock:
            self._loaded.add(name)
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
