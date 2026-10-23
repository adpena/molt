from __future__ import annotations

import contextlib
import hashlib
import re
import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Collection, Generator, Mapping

from molt.cache_eviction import refresh_cache_entry_recency
from molt.default_paths import (
    WASM_LINK_CACHE_DIRECTORY,
    _default_molt_cache,
    wasm_link_cache_paths,
)

from molt.cli.atomic_io import _atomic_write_bytes, _atomic_write_json
from molt.exact_json import loads_exact
from molt.file_hashing import _sha256_file_with_size
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.file_publication import (
    PublicationDurability,
    atomic_replace,
    clone_file,
    staged_file_path,
)

WASM_LINK_CACHE_ENTRY_SCHEMA = "molt.wasm-link-cache-entry.v4"
# A bundle entry holds every output role of one transform (for example the
# complete private output family of one final link) under one content key.
WASM_LINK_CACHE_BUNDLE_SCHEMA = "molt.wasm-link-cache-bundle.v1"
_ROLE_RE = re.compile(r"[a-z][a-z0-9_]*")
_WASM_LINK_CACHE_ROOT_KEYS = frozenset({"schema", "cache", "payload"})
_WASM_LINK_CACHE_RECORD_KEYS = frozenset(
    {"family", "transform_schema", "key", "artifact_bytes", "artifact_sha256"}
)


@dataclass(frozen=True)
class WasmLinkCacheEntry:
    root: Path
    artifact: Path
    metadata: Path
    lock: Path
    family: str
    schema: str
    key: str


@dataclass(frozen=True)
class WasmLinkCacheBundleRead:
    """Verified role files of one bundle; read and transfer them under its lock."""

    files: dict[str, Path] | None
    status: str
    bytes_read: int


@dataclass(frozen=True)
class WasmLinkCacheRead:
    data: bytes | None
    payload: dict[str, object] | None
    status: str
    bytes_read: int


def _default_wasm_link_cache() -> Path:
    return _default_molt_cache() / WASM_LINK_CACHE_DIRECTORY


def _wasm_link_cache_entry(
    family: str,
    schema: str,
    key: str,
    *,
    cache_root: Path | None = None,
) -> WasmLinkCacheEntry:
    root, lock = wasm_link_cache_paths(
        cache_root or _default_wasm_link_cache(), family, schema, key
    )
    return WasmLinkCacheEntry(
        root=root,
        artifact=root / "artifact.wasm",
        metadata=root / "metadata.json",
        lock=lock,
        family=family,
        schema=schema,
        key=key,
    )


@contextmanager
def _locked_wasm_link_cache_entry(
    entry: WasmLinkCacheEntry,
    *,
    timeout_s: float = 900.0,
) -> Generator[float]:
    started = time.perf_counter()
    handle = _acquire_file_lock(
        entry.lock,
        timeout_s=timeout_s,
        timeout_message=(
            "Timed out waiting for wasm linker cache producer lock "
            f"{entry.lock} after {timeout_s:.0f}s"
        ),
    )
    wait_ms = max(0.0, (time.perf_counter() - started) * 1000.0)
    try:
        yield wait_ms
    finally:
        _release_file_lock(handle)


def _read_wasm_link_cache_entry(entry: WasmLinkCacheEntry) -> WasmLinkCacheRead:
    artifact_exists = entry.artifact.is_file()
    metadata_exists = entry.metadata.is_file()
    if not artifact_exists and not metadata_exists:
        return WasmLinkCacheRead(None, None, "missing", 0)
    try:
        data = entry.artifact.read_bytes()
        metadata = loads_exact(entry.metadata.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError, TypeError):
        return WasmLinkCacheRead(None, None, "corrupt", 0)
    if not isinstance(metadata, dict) or set(metadata) != _WASM_LINK_CACHE_ROOT_KEYS:
        return WasmLinkCacheRead(None, None, "corrupt", len(data))
    cache = metadata.get("cache")
    payload = metadata.get("payload")
    if (
        metadata.get("schema") != WASM_LINK_CACHE_ENTRY_SCHEMA
        or not isinstance(cache, dict)
        or set(cache) != _WASM_LINK_CACHE_RECORD_KEYS
        or not isinstance(payload, dict)
    ):
        return WasmLinkCacheRead(None, None, "corrupt", len(data))
    expected = {
        "family": entry.family,
        "transform_schema": entry.schema,
        "key": entry.key,
        "artifact_bytes": len(data),
        "artifact_sha256": hashlib.sha256(data).hexdigest(),
    }
    if any(cache.get(name) != value for name, value in expected.items()):
        return WasmLinkCacheRead(None, None, "corrupt", len(data))
    if len(data) < 8 or data[:8] != b"\x00asm\x01\x00\x00\x00":
        return WasmLinkCacheRead(None, None, "corrupt", len(data))
    refresh_cache_entry_recency(entry.root)
    return WasmLinkCacheRead(
        data,
        payload,
        "hit",
        len(data),
    )


def _publish_wasm_link_cache_entry(
    entry: WasmLinkCacheEntry,
    data: bytes,
    *,
    payload: Mapping[str, object] | None = None,
) -> None:
    if len(data) < 8 or data[:8] != b"\x00asm\x01\x00\x00\x00":
        raise ValueError("refusing to cache a non-WASM linker artifact")
    metadata = {
        "schema": WASM_LINK_CACHE_ENTRY_SCHEMA,
        "cache": {
            "family": entry.family,
            "transform_schema": entry.schema,
            "key": entry.key,
            "artifact_bytes": len(data),
            "artifact_sha256": hashlib.sha256(data).hexdigest(),
        },
        "payload": dict(payload or {}),
    }
    entry.root.mkdir(parents=True, exist_ok=True)
    _atomic_write_bytes(entry.artifact, data)
    _atomic_write_json(entry.metadata, metadata, indent=2, sort_keys=True)


def _invalidate_wasm_link_cache_entry(entry: WasmLinkCacheEntry) -> None:
    for path in (entry.artifact, entry.metadata):
        with contextlib.suppress(OSError):
            path.unlink()
    with contextlib.suppress(OSError):
        entry.root.rmdir()


def _bundle_role_path(entry: WasmLinkCacheEntry, role: str) -> Path:
    if _ROLE_RE.fullmatch(role) is None:
        raise ValueError(f"invalid wasm linker cache bundle role: {role!r}")
    return entry.root / "roles" / role


def _read_wasm_link_cache_bundle(
    entry: WasmLinkCacheEntry, roles: Collection[str]
) -> WasmLinkCacheBundleRead:
    """Read one complete bundle; any missing, extra or changed role is corrupt."""

    if not entry.metadata.is_file():
        return WasmLinkCacheBundleRead(None, "missing", 0)
    try:
        metadata = loads_exact(entry.metadata.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError, TypeError):
        return WasmLinkCacheBundleRead(None, "corrupt", 0)
    expected_cache = {
        "family": entry.family,
        "transform_schema": entry.schema,
        "key": entry.key,
    }
    if (
        not isinstance(metadata, dict)
        or set(metadata) != {"schema", "cache", "roles"}
        or metadata.get("schema") != WASM_LINK_CACHE_BUNDLE_SCHEMA
        or metadata.get("cache") != expected_cache
        or not isinstance(metadata.get("roles"), dict)
        or set(metadata["roles"]) != set(roles)
    ):
        return WasmLinkCacheBundleRead(None, "corrupt", 0)
    files: dict[str, Path] = {}
    bytes_read = 0
    for role, record in metadata["roles"].items():
        if not isinstance(record, dict) or set(record) != {"bytes", "sha256"}:
            return WasmLinkCacheBundleRead(None, "corrupt", bytes_read)
        try:
            path = _bundle_role_path(entry, role)
            digest, size = _sha256_file_with_size(path)
        except (OSError, ValueError):
            return WasmLinkCacheBundleRead(None, "corrupt", bytes_read)
        bytes_read += size
        if record["bytes"] != size or record["sha256"] != digest:
            return WasmLinkCacheBundleRead(None, "corrupt", bytes_read)
        files[role] = path
    refresh_cache_entry_recency(entry.root)
    return WasmLinkCacheBundleRead(files, "hit", bytes_read)


def _transfer_file(source: Path, destination: Path) -> None:
    """Replace `destination` with a clone of `source` (see ``clone_file``).

    A clone shares the source's blocks, so publishing or restoring a 60 MB
    linked module writes no data. Cache entries and private link outputs are
    reconstructible: readers verify their recorded digests.
    """

    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = staged_file_path(destination, purpose="wasm-link-cache")
    try:
        clone_file(source, stage)
        atomic_replace(
            stage, destination, durability=PublicationDurability.RECONSTRUCTIBLE
        )
    finally:
        with contextlib.suppress(FileNotFoundError):
            stage.unlink()


def _publish_wasm_link_cache_bundle(
    entry: WasmLinkCacheEntry, files: Mapping[str, Path]
) -> None:
    """Publish role files first and the metadata last, which commits the entry."""

    if not files:
        raise ValueError("refusing to cache an empty wasm linker bundle")
    roles = {}
    for role, source in files.items():
        digest, size = _sha256_file_with_size(source)
        _transfer_file(source, _bundle_role_path(entry, role))
        roles[role] = {"bytes": size, "sha256": digest}
    _atomic_write_json(
        entry.metadata,
        {
            "schema": WASM_LINK_CACHE_BUNDLE_SCHEMA,
            "cache": {
                "family": entry.family,
                "transform_schema": entry.schema,
                "key": entry.key,
            },
            "roles": roles,
        },
        indent=2,
        sort_keys=True,
        durability=PublicationDurability.RECONSTRUCTIBLE,
    )


def _invalidate_wasm_link_cache_bundle(entry: WasmLinkCacheEntry) -> None:
    with contextlib.suppress(OSError):
        entry.metadata.unlink()
    roles_root = entry.root / "roles"
    if roles_root.is_dir():
        for child in roles_root.iterdir():
            with contextlib.suppress(OSError):
                child.unlink()
        with contextlib.suppress(OSError):
            roles_root.rmdir()
    with contextlib.suppress(OSError):
        entry.root.rmdir()
