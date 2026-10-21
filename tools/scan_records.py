"""Content-keyed per-file results for pure repository scanners.

Repository gates derive the same per-file facts on every run, although almost
every file is unchanged between runs. A scanner names one pure per-file
operation. Its result may depend only on the file's bytes, the identity text
the caller passes (the repository-relative path, or nothing when the result
ignores the path) and the scanner's code. A record stores that result under a
key over exactly those inputs: an unchanged file replays its result, and a
changed file, identity or code misses. This is the per-file cache of ruff and
the content-keyed action cache of Go and Bazel, applied to Molt's own gates.

The code generation digests the interpreter identity, the scanner's own source
file and the source bytes of every loaded first-party module (``tools``,
``molt``, ``sitecustomize`` and ``__main__``). The scanner names its own file
because a wrapper such as ``cProfile`` or ``runpy`` may run it outside
``sys.modules``. Publication recomputes the generation: a module that was
imported or edited during the run changes it, and the run publishes nothing. An
unreadable, corrupt or foreign record file is a miss. A failed publication
never changes the gate's result.

Only command entry points open records. Library callers and tests compute
directly, so an in-process monkeypatch cannot publish a result for the real
code. Records hold compact JSON facts, never parse trees.

A run that computed something publishes once. A fully replayed run writes
nothing, unless one of its records was last renewed a day ago. Records unused
for a week expire, so a narrow run (one selected file, a fixture checkout)
cannot evict the whole tree's records. Each scanner keeps its newest
generations only. These checks add no work to emitted programs.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator
from contextlib import contextmanager
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import time
from typing import Any, TypeVar

from molt.default_paths import _default_molt_cache
from molt.file_deletion import unlink_file
from molt.file_publication import (
    PublicationDurability,
    atomic_write_bytes,
    is_staged_file_path,
)

_T = TypeVar("_T")

SCHEMA = 1
# Scan records own this cache namespace and bound it themselves; the cache
# budget's pruner never treats an unknown namespace as an eviction unit.
DIRECTORY = "repository-scan-records"
DAY_SECONDS = 24 * 3600
# A record unused this long expires at the next publication.
RECORD_LIFETIME_SECONDS = 7 * DAY_SECONDS
# A replayed record older than this is renewed by a publication.
RECORD_RENEWAL_SECONDS = DAY_SECONDS
# A hard bound on one generation file; the least recently used records go first.
MAX_RECORDS = 1 << 17
# Concurrent checkouts may run different scanner code.
RETAINED_GENERATIONS = 4
# A stage older than this belongs to an interrupted publisher.
STALE_STAGE_SECONDS = 3600
_FIRST_PARTY_PACKAGES = frozenset({"tools", "molt"})
_FIRST_PARTY_MODULES = frozenset({"__main__", "sitecustomize"})


def _wall_clock() -> int:
    return int(time.time())


def _first_party_sources(owner: Path) -> list[tuple[str, Path]]:
    sources: list[tuple[str, Path]] = [("", owner)]
    for name, module in list(sys.modules.items()):
        if (
            name not in _FIRST_PARTY_MODULES
            and name.partition(".")[0] not in _FIRST_PARTY_PACKAGES
        ):
            continue
        source = getattr(module, "__file__", None)
        if isinstance(source, str):
            sources.append((name, Path(source)))
    return sorted(sources)


def code_generation(scanner: str, owner: Path) -> str | None:
    """Digest the interpreter, the scanner file and loaded first-party sources.

    None means the code cannot be identified, so nothing may be replayed.
    """
    digest = hashlib.sha256()
    for part in (
        f"molt.scan-records.v{SCHEMA}",
        scanner,
        sys.version,
        sys.implementation.cache_tag or "",
        sys.platform,
    ):
        digest.update(part.encode("utf-8", "surrogatepass"))
        digest.update(b"\0")
    for name, source in _first_party_sources(owner):
        try:
            data = source.read_bytes()
        except OSError:
            return None
        encoded = name.encode("utf-8", "surrogatepass")
        digest.update(f"{len(encoded)}:{len(data)}:".encode("ascii"))
        digest.update(encoded)
        digest.update(data)
    return digest.hexdigest()


def record_key(identity: str, data: bytes) -> str:
    """The key over exactly a result's inputs: its identity text and bytes."""
    encoded = identity.encode("utf-8", "surrogatepass")
    digest = hashlib.sha256(f"{len(encoded)}:".encode("ascii"))
    digest.update(encoded)
    digest.update(data)
    return digest.hexdigest()


def decoded_text(data: bytes, *, encoding: str, errors: str = "strict") -> str:
    """Decode exactly as ``Path.read_text`` does, from bytes already read.

    The result is a function of the hashed bytes, so a concurrent write cannot
    pair one file version's key with another version's result.
    """
    return io.TextIOWrapper(io.BytesIO(data), encoding=encoding, errors=errors).read()


class ScanRecords:
    """One scanner generation's records, loaded once and published once."""

    def __init__(
        self, scanner: str, *, owner: str | Path, cache_root: Path | None = None
    ) -> None:
        if not scanner or not scanner.replace("_", "").isalnum():
            raise ValueError(f"invalid scanner name: {scanner!r}")
        self.scanner = scanner
        self.owner = Path(owner).resolve()
        self.generation = code_generation(scanner, self.owner)
        root = _default_molt_cache() if cache_root is None else cache_root
        self.directory = root / DIRECTORY / scanner
        self.hits = 0
        self.misses = 0
        self._now = _wall_clock()
        self._renew = False
        self._stored = self._load()
        self._used: dict[str, list[Any]] = {}

    @property
    def path(self) -> Path | None:
        if self.generation is None:
            return None
        return self.directory / f"{self.generation}.json"

    def _load(self) -> dict[str, list[Any]]:
        path = self.path
        if path is None:
            return {}
        try:
            document = json.loads(path.read_bytes())
        except (OSError, ValueError):
            return {}
        if (
            not isinstance(document, dict)
            or document.get("schema") != SCHEMA
            or document.get("scanner") != self.scanner
            or document.get("generation") != self.generation
            or not isinstance(document.get("records"), dict)
        ):
            return {}
        return {
            key: record
            for key, record in document["records"].items()
            if isinstance(record, list) and len(record) == 2 and type(record[0]) is int
        }

    def result(
        self,
        identity: str,
        data: bytes,
        compute: Callable[[], _T],
        *,
        encode: Callable[[_T], Any],
        decode: Callable[[Any], _T],
    ) -> _T:
        """Replay or compute one pure per-file result.

        ``compute`` must derive its result from ``data`` and ``identity`` only.
        An exception propagates and records nothing.
        """
        if self.generation is None:
            return compute()
        key = record_key(identity, data)
        found, value = self.replay(key)
        if found:
            return decode(value)
        result = compute()
        self.record(key, encode(result))
        return result

    def replay(self, key: str) -> tuple[bool, Any]:
        """The stored JSON value for one ``record_key``, if any."""
        if self.generation is None:
            return False, None
        record = self._used.get(key) or self._stored.get(key)
        if record is None:
            return False, None
        self._used[key] = record
        self._renew |= self._now - record[0] >= RECORD_RENEWAL_SECONDS
        self.hits += 1
        return True, record[1]

    def record(self, key: str, value: Any) -> None:
        """Record one computed JSON value under the key of its exact inputs.

        A worker that read the file itself computes ``key`` from those bytes,
        so the key and the value always describe one file version.
        """
        if self.generation is None:
            return
        self._used[key] = [self._now, value]
        self.misses += 1

    def publish(self) -> bool:
        """Publish computed and renewed records; True when a file was written."""
        path = self.path
        if path is None or not (self.misses or self._renew):
            return False
        if code_generation(self.scanner, self.owner) != self.generation:
            return False
        records = {key: [self._now, value] for key, (_, value) in self._used.items()}
        retained = sorted(
            (
                (stamp, key, value)
                for key, (stamp, value) in self._stored.items()
                if key not in records and self._now - stamp < RECORD_LIFETIME_SECONDS
            ),
            reverse=True,
        )
        for stamp, key, value in retained[: max(0, MAX_RECORDS - len(records))]:
            records[key] = [stamp, value]
        document = {
            "schema": SCHEMA,
            "scanner": self.scanner,
            "generation": self.generation,
            "records": records,
        }
        payload = json.dumps(document, separators=(",", ":")).encode("utf-8")
        try:
            atomic_write_bytes(
                path, payload, durability=PublicationDurability.RECONSTRUCTIBLE
            )
        except (OSError, ValueError):
            return False
        self._retire(path)
        return True

    def _retire(self, current: Path) -> None:
        """Keep the newest generations; remove interrupted publishers' stages."""
        try:
            with os.scandir(self.directory) as listing:
                files = [
                    (entry.stat(follow_symlinks=False).st_mtime, Path(entry.path))
                    for entry in listing
                    if entry.is_file(follow_symlinks=False)
                ]
        except OSError:
            return
        generations = sorted(
            (item for item in files if item[1].suffix == ".json"),
            key=lambda item: item[0],
            reverse=True,
        )
        retired = [path for _, path in generations if path != current][
            RETAINED_GENERATIONS - 1 :
        ]
        retired.extend(
            path
            for mtime, path in files
            if is_staged_file_path(path) and self._now - mtime > STALE_STAGE_SECONDS
        )
        for path in retired:
            try:
                unlink_file(path)
            except OSError:
                continue


def scan_result(
    records: ScanRecords | None,
    identity: str,
    data: bytes,
    compute: Callable[[], _T],
    *,
    encode: Callable[[_T], Any],
    decode: Callable[[Any], _T],
) -> _T:
    """Compute directly without records; otherwise replay or record."""
    if records is None:
        return compute()
    return records.result(identity, data, compute, encode=encode, decode=decode)


@contextmanager
def open_scan_records(
    scanner: str,
    *,
    owner: str | Path,
    enabled: bool,
    cache_root: Path | None = None,
) -> Iterator[ScanRecords | None]:
    """Open one scanner's records for a command; publish after success only.

    ``owner`` is the scanner module's own ``__file__``.
    """
    if not enabled:
        yield None
        return
    records = ScanRecords(scanner, owner=owner, cache_root=cache_root)
    yield records
    records.publish()
