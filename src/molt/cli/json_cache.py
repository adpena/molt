from __future__ import annotations

import json
import os
import stat
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from molt import file_publication
from molt.cli.atomic_io import _atomic_write_json
from molt.toolchain_identity import capture_stable_regular_file

# Source-analysis schemas retain their existing allocation bound. Lowering IR
# has a separate size domain and deliberately uses the uncapped default.
SOURCE_ANALYSIS_CACHE_MAX_BYTES = 16 * 1024 * 1024
_PERSISTED_JSON_OBJECT_CACHE: dict[
    Path, tuple[os.stat_result, int, dict[str, Any] | None]
] = {}


def _read_cached_json_object(
    path: Path, *, max_bytes: int | None = None
) -> Mapping[str, Any] | None:
    """Return a reusable candidate; its consumer must qualify semantic inputs.

    The metadata observation only avoids reparsing a remembered candidate. It
    does not prove current pathname bytes or an unchanged closed file. A stale
    candidate remains useful only when the caller's independent source/context
    identity admits it. Current-byte receipts use their own strong reader.
    Callers must not mutate this borrowed raw payload.
    """
    try:
        observed = path.lstat()
    except OSError:
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
        return None
    if (
        not stat.S_ISREG(observed.st_mode)
        or file_publication.metadata_is_link_like(observed)
        or max_bytes is not None
        and (
            type(max_bytes) is not int or max_bytes <= 0 or observed.st_size > max_bytes
        )
    ):
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
        return None
    cached = _PERSISTED_JSON_OBJECT_CACHE.get(path)
    if cached is not None:
        previous, captured_size, cached_payload = cached
        if max_bytes is not None and captured_size > max_bytes:
            _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
            return None
        if (
            os.path.samestat(previous, observed)
            and previous.st_size == observed.st_size
            and previous.st_mtime_ns == observed.st_mtime_ns
            and previous.st_ctime_ns == observed.st_ctime_ns
        ):
            return cached_payload
    try:
        _identity, data = capture_stable_regular_file(
            path, label="compiler cache candidate", max_bytes=max_bytes
        )
    except (OSError, ValueError):
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
        return None
    try:
        decoded = json.loads(data)
    except (ValueError, RecursionError):
        payload = None
    else:
        payload = decoded if isinstance(decoded, dict) else None
    _PERSISTED_JSON_OBJECT_CACHE[path] = (observed, len(data), payload)
    return payload


def _write_cached_json_object(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    try:
        _atomic_write_json(
            path,
            payload,
            indent=2,
            default=default,
            durability=file_publication.PublicationDurability.RECONSTRUCTIBLE,
        )
    finally:
        # Only a captured read can populate the candidate memo. A peer can
        # replace the published leaf before any post-write stat observation.
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
