from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

from molt.cli.artifact_state import _artifact_state_path
from molt.cli.atomic_io import _atomic_write_json

# Low-level artifact-sync state primitives.
#
# The per-module frontend caches (``module_cache``, ``module_graph_cache``) read
# and write this artifact-sync state to decide whether a previously staged output
# is still current. That is a lowering-context concern, but the primitives
# themselves are pure filesystem state — they must not pull in the backend/codegen
# layer. Historically they lived in ``backend_cache`` (which transitively imports
# the whole native/wasm backend), so any lowering-context module that needed them
# dragged the backend onto the frontend import path and cold-started the lowering
# cache on unrelated backend edits. They live here instead: a leaf module that
# imports only ``artifact_state`` and ``atomic_io``. ``backend_cache`` re-exports
# them for its own callers, so no public import path changes.

_ARTIFACT_SYNC_STATE_CACHE: dict[Path, tuple[int, int, dict[str, Any] | None]] = {}


def _artifact_sync_state_path(project_root: Path, artifact: Path) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="artifact_sync",
        stem_suffix="",
        extension="json",
    )


def _read_artifact_sync_state(path: Path) -> dict[str, Any] | None:
    try:
        stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    cached = _ARTIFACT_SYNC_STATE_CACHE.get(path)
    if cached is not None:
        cached_size, cached_mtime_ns, cached_payload = cached
        if cached_size == stat.st_size and cached_mtime_ns == stat.st_mtime_ns:
            return cached_payload
    try:
        text = path.read_text().strip()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    if not text:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    payload = data if isinstance(data, dict) else None
    _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, payload)
    return payload


def _write_artifact_sync_state(
    path: Path,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
) -> None:
    stat = artifact.stat()
    payload = {
        "version": 1,
        "source_key": source_key,
        "tier": tier,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }
    _atomic_write_json(path, payload, indent=2)
    try:
        written_stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
    else:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            dict(payload),
        )


def _write_artifact_sync_payload(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    _atomic_write_json(path, payload, indent=2, default=default)
    try:
        written_stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
    else:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            dict(payload),
        )


def _artifact_sync_state_matches(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
) -> bool:
    try:
        stat = artifact.stat()
    except OSError:
        return False
    return _artifact_sync_state_matches_stat(
        state,
        source_key=source_key,
        tier=tier,
        stat=stat,
    )


def _artifact_sync_state_matches_stat(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    stat: os.stat_result,
) -> bool:
    if state is None:
        return False
    if state.get("source_key") != source_key or state.get("tier") != tier:
        return False
    return (
        state.get("size") == stat.st_size and state.get("mtime_ns") == stat.st_mtime_ns
    )
