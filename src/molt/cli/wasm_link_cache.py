from __future__ import annotations

import contextlib
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import time
from typing import Collection, Iterator, Mapping

from molt.cli.atomic_io import _atomic_write_bytes, _atomic_write_json
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.cli.default_paths import _default_molt_cache
from molt.exact_json import loads_exact


WASM_LINK_CACHE_DIRECTORY = "wasm_link"
WASM_LINK_CACHE_FAMILIES = frozenset(
    {"runtime_tree_shake", "split_app_optimize", "final_link"}
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
_CACHE_SCHEMA_RE = re.compile(r"[a-z0-9][a-z0-9-]*")
_SHA256_RE = re.compile(r"[0-9a-f]{64}")


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
    files: dict[str, bytes] | None
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
    if family not in WASM_LINK_CACHE_FAMILIES:
        raise ValueError(f"unknown wasm linker cache family: {family}")
    if not isinstance(schema, str) or _CACHE_SCHEMA_RE.fullmatch(schema) is None:
        raise ValueError("wasm linker cache schema must be a lowercase identifier")
    if not isinstance(key, str) or _SHA256_RE.fullmatch(key) is None:
        raise ValueError("wasm linker cache key must be a lowercase SHA-256")
    family_root = (cache_root or _default_wasm_link_cache()) / family
    root = family_root / schema / key
    # A fixed 256-stripe lock set bounds filesystem metadata for the lifetime of
    # the cache while still single-flighting identical content keys. Deleting a
    # per-key lock after release is racy on POSIX and invalid on Windows when a
    # waiter already owns an open handle; stable stripes avoid both hazards.
    lock = family_root / ".locks" / f"{key[:2]}.lock"
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
) -> Iterator[float]:
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
    now = time.time()
    for path in (entry.root, entry.artifact, entry.metadata):
        with contextlib.suppress(OSError):
            os.utime(path, (now, now))
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
    files: dict[str, bytes] = {}
    bytes_read = 0
    for role, record in metadata["roles"].items():
        if not isinstance(record, dict) or set(record) != {"bytes", "sha256"}:
            return WasmLinkCacheBundleRead(None, "corrupt", bytes_read)
        try:
            data = _bundle_role_path(entry, role).read_bytes()
        except (OSError, ValueError):
            return WasmLinkCacheBundleRead(None, "corrupt", bytes_read)
        bytes_read += len(data)
        if (
            record["bytes"] != len(data)
            or record["sha256"] != hashlib.sha256(data).hexdigest()
        ):
            return WasmLinkCacheBundleRead(None, "corrupt", bytes_read)
        files[role] = data
    now = time.time()
    with contextlib.suppress(OSError):
        os.utime(entry.root, (now, now))
    return WasmLinkCacheBundleRead(files, "hit", bytes_read)


def _publish_wasm_link_cache_bundle(
    entry: WasmLinkCacheEntry, files: Mapping[str, bytes]
) -> None:
    """Publish role files first and the metadata last, which commits the entry."""

    if not files:
        raise ValueError("refusing to cache an empty wasm linker bundle")
    roles = {}
    for role, data in files.items():
        path = _bundle_role_path(entry, role)
        path.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_bytes(path, data)
        roles[role] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
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
