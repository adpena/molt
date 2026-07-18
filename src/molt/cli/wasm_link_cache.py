from __future__ import annotations

import contextlib
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import time
from typing import Iterator, Mapping

from molt.cli.atomic_io import _atomic_write_bytes, _atomic_write_json
from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.default_paths import _default_molt_cache


WASM_LINK_CACHE_DIRECTORY = "wasm_link"
WASM_LINK_CACHE_FAMILIES = frozenset({"runtime_tree_shake", "split_app_optimize"})


@dataclass(frozen=True)
class WasmLinkCacheEntry:
    root: Path
    artifact: Path
    metadata: Path
    lock: Path
    schema: str
    key: str


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
    if not schema or not key:
        raise ValueError("wasm linker cache schema and key must be non-empty")
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
        metadata = json.loads(entry.metadata.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return WasmLinkCacheRead(None, None, "corrupt", 0)
    if not isinstance(metadata, dict):
        return WasmLinkCacheRead(None, None, "corrupt", len(data))
    cache = metadata.get("cache")
    if not isinstance(cache, dict):
        return WasmLinkCacheRead(None, None, "corrupt", len(data))
    expected = {
        "schema": entry.schema,
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
    payload = metadata.get("payload")
    return WasmLinkCacheRead(
        data,
        payload if isinstance(payload, dict) else {},
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
        "cache": {
            "schema": entry.schema,
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
