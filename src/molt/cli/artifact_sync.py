from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from molt.durable_file_identity import DurableFileIdentity, upgrade_legacy
from molt.backend_protocol import BackendEmissionOrigin
from molt.cli.artifact_state import _artifact_state_path
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    stable_regular_file_identity,
)

# Backend sync receipts use durable publication and freshly captured bytes.
# Only parsing is memoized; receipt matching separately re-admits the actual
# artifact. Reconstructible source-analysis candidates belong to json_cache.

_ARTIFACT_SYNC_STATE_CACHE: dict[Path, tuple[str, dict[str, Any] | None]] = {}
_ARTIFACT_SYNC_STATE_VERSION = 4


def _artifact_sync_state_path(project_root: Path, artifact: Path) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="artifact_sync/v4",
        stem_suffix="",
        extension="json",
    )


def _artifact_sync_payload_shape(value: object) -> bool:
    if (
        not isinstance(value, dict)
        or set(value)
        != {
            "version",
            "source_key",
            "tier",
            "size",
            "sha256",
            "generation",
            "emission_origin",
        }
        or value["version"] != _ARTIFACT_SYNC_STATE_VERSION
    ):
        return False
    generation = value["generation"]
    if isinstance(generation, list) and len(generation) == 2:
        try:
            DurableFileIdentity.from_stat_prefix(*generation)
        except ValueError:
            return False
    return (
        isinstance(generation, list)
        and len(generation) == 2
        and all(isinstance(part, str) for part in generation)
    )


def _migrate_artifact_sync_state(path: Path, artifact: Path) -> None:
    if path.exists() or path.parent.name != "v4":
        return
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    legacy = path.parent.parent / path.name
    if not legacy.exists():
        return
    lock = _acquire_file_lock(
        path.with_suffix(".identity-migration.lock"),
        timeout_s=10,
        timeout_message="sync receipt migration is busy",
    )
    try:
        if path.exists():
            return
        _, data = capture_stable_regular_file(
            legacy,
            label="legacy artifact sync payload",
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
        )
        decoded = json.loads(data)
        if not isinstance(decoded, dict) or decoded.get("version") != 3:
            return
        pair = decoded.get("generation")
        if not isinstance(pair, list) or len(pair) != 2:
            raise ValueError("invalid legacy sync generation")
        generation = upgrade_legacy(artifact, {"device": pair[0], "inode": pair[1]})
        current = stable_regular_file_identity(artifact, label="legacy synced artifact")
        if (decoded.get("sha256"), decoded.get("size")) != (
            current.sha256,
            current.size,
        ):
            raise ValueError("legacy sync artifact changed")
        decoded = {
            **decoded,
            "version": _ARTIFACT_SYNC_STATE_VERSION,
            "generation": list(generation.stat_prefix()),
        }
        if not _artifact_sync_payload_shape(decoded):
            raise ValueError("invalid legacy sync receipt")
        _atomic_write_json(path, decoded, indent=2)
    finally:
        _release_file_lock(lock)


def _read_artifact_sync_state(
    path: Path, *, artifact: Path | None = None
) -> dict[str, Any] | None:
    try:
        if artifact is not None:
            _migrate_artifact_sync_state(path, artifact)
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
    except (ValueError, RecursionError):
        payload = None
    else:
        payload = decoded if _artifact_sync_payload_shape(decoded) else None
    _ARTIFACT_SYNC_STATE_CACHE[path] = (identity.sha256, payload)
    return payload


def _write_artifact_sync_state(
    path: Path,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
    identity: StableRegularFileIdentity | None = None,
    emission_origin: BackendEmissionOrigin | None = None,
) -> None:
    try:
        identity = _artifact_sync_identity(artifact, identity=identity)
    except (OSError, ValueError) as error:
        raise OSError(
            f"Cannot attest backend artifact sync receipt: {error}"
        ) from error
    if emission_origin is not None and (
        emission_origin.output_sha256 != identity.sha256
        or emission_origin.output_size != identity.size
    ):
        raise OSError("Backend emission origin names different sync artifact bytes")
    payload = {
        "version": _ARTIFACT_SYNC_STATE_VERSION,
        "source_key": source_key,
        "tier": tier,
        "size": identity.size,
        "sha256": identity.sha256,
        "generation": list(identity.stat_token()[:2]),
        "emission_origin": (
            emission_origin.payload() if emission_origin is not None else None
        ),
    }
    _atomic_write_json(path, payload, indent=2)
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
    if state is None or not _artifact_sync_payload_shape(state):
        return False
    if state.get("source_key") != source_key or state.get("tier") != tier:
        return False
    try:
        identity = _artifact_sync_identity(artifact, identity=identity)
    except (OSError, ValueError):
        return False
    return (
        state["size"] == identity.size
        and state["sha256"] == identity.sha256
        and state["generation"] == list(identity.stat_token()[:2])
    )


def _artifact_sync_emission_origin(
    state: dict[str, Any] | None,
    *,
    identity: StableRegularFileIdentity,
) -> BackendEmissionOrigin | None:
    """Read an observation bound to the admitted generation, never an attestation.

    Recency updates change timestamps, so generation custody uses file identity
    and freshly read content. The closed supervisor journal remains the origin
    admission authority, independent of these untrusted retained observations.
    """
    if (
        state is None
        or not _artifact_sync_payload_shape(state)
        or state.get("sha256") != identity.sha256
        or state.get("size") != identity.size
        or state.get("generation") != list(identity.stat_token()[:2])
        or state.get("emission_origin") is None
    ):
        return None
    try:
        origin = BackendEmissionOrigin.from_payload(state["emission_origin"])
    except ValueError:
        return None
    if (origin.output_sha256, origin.output_size) != (identity.sha256, identity.size):
        return None
    return origin
