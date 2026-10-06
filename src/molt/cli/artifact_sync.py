from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from molt.cli.artifact_state import _artifact_state_path
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    stable_regular_file_identity,
)

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
# uses the shared direct-file identity authority. The ``molt.cli`` facade points
# to this leaf authority, so public imports do not route through backend_cache.

_ARTIFACT_SYNC_STATE_CACHE: dict[Path, tuple[str, dict[str, Any] | None]] = {}
_ARTIFACT_SYNC_STATE_VERSION = 2


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
        identity, data = capture_stable_regular_file(
            path,
            label="artifact sync payload",
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
        )
    except (OSError, ValueError):
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    # Only parsing is cached. A detached metadata token cannot establish that
    # the receipt still contains the bytes from which its cached payload came.
    cached = _ARTIFACT_SYNC_STATE_CACHE.get(path)
    if cached is not None and cached[0] == identity.sha256:
        return cached[1]
    try:
        decoded = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError):
        payload = None
    else:
        payload = decoded if isinstance(decoded, dict) else None
    _ARTIFACT_SYNC_STATE_CACHE[path] = (identity.sha256, payload)
    return payload


def _write_artifact_sync_state(
    path: Path,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
    identity: StableRegularFileIdentity | None = None,
) -> None:
    try:
        identity = _artifact_sync_identity(artifact, identity=identity)
    except (OSError, ValueError) as error:
        raise OSError(
            f"Cannot attest backend artifact sync receipt: {error}"
        ) from error
    payload = {
        "version": _ARTIFACT_SYNC_STATE_VERSION,
        "source_key": source_key,
        "tier": tier,
        "size": identity.size,
        "sha256": identity.sha256,
    }
    _write_artifact_sync_payload(path, payload)


def _write_artifact_sync_payload(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    _atomic_write_json(path, payload, indent=2, default=default)
    # Only a read of the published generation may populate the process cache;
    # a peer can replace the path immediately after atomic publication.
    _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)


def _artifact_sync_identity(
    artifact: Path, *, identity: StableRegularFileIdentity | None
) -> StableRegularFileIdentity:
    if identity is not None and identity.path != artifact.expanduser().absolute():
        raise ValueError("Backend sync identity belongs to a different artifact path")
    current = stable_regular_file_identity(artifact, label="backend synced artifact")
    if identity is not None and current != identity:
        raise ValueError("Backend synced artifact changed since content admission")
    return current


def _artifact_sync_state_matches(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
    identity: StableRegularFileIdentity | None = None,
) -> bool:
    """Match one source receipt to current bytes and any supplied admission."""
    if state is None or state.get("version") != _ARTIFACT_SYNC_STATE_VERSION:
        return False
    if state.get("source_key") != source_key or state.get("tier") != tier:
        return False
    try:
        identity = _artifact_sync_identity(artifact, identity=identity)
    except (OSError, ValueError):
        return False
    return state.get("size") == identity.size and state.get("sha256") == identity.sha256
