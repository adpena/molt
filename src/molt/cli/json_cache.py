from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_write_json

_PERSISTED_JSON_OBJECT_CACHE: dict[Path, tuple[int, int, dict[str, Any] | None]] = {}


def _read_cached_json_object(path: Path) -> dict[str, Any] | None:
    try:
        stat = path.stat()
    except OSError:
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
        return None
    cached = _PERSISTED_JSON_OBJECT_CACHE.get(path)
    if cached is not None:
        cached_size, cached_mtime_ns, cached_payload = cached
        if cached_size == stat.st_size and cached_mtime_ns == stat.st_mtime_ns:
            return cached_payload
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        _PERSISTED_JSON_OBJECT_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    payload = data if isinstance(data, dict) else None
    _PERSISTED_JSON_OBJECT_CACHE[path] = (
        stat.st_size,
        stat.st_mtime_ns,
        payload,
    )
    return payload


def _write_cached_json_object(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    _atomic_write_json(path, payload, indent=2, default=default)
    try:
        written_stat = path.stat()
    except OSError:
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
    else:
        _PERSISTED_JSON_OBJECT_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            copy.deepcopy(payload),
        )
