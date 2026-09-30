"""Artifact sidecars; runtime build keys are exact family projections only."""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
import stat
from typing import Any

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.runtime_identity_schema import (
    RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
    RuntimeBuildIdentity,
    runtime_build_fingerprint,
)
from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
    validate_artifact_content_identity,
)
from molt.exact_json import read_exact
from molt.python_identity_common import _valid_sha256
from molt.wasm_artifact import is_valid_wasm_binary

_RUNTIME_FINGERPRINT_SCHEMA_VERSION = 3
_FIELDS = frozenset(
    {
        "version",
        "hash",
        "rustc",
        "inputs_digest",
        "meta_digest",
        "source_state",
        "artifact_content_identity",
        "build_identity",
        "build_identity_scope",
    }
)
# The identity an artifact is admitted under. Everything else in a sidecar is
# refreshable metadata: source-tree timestamp digests and clean-head states are
# fast-path cache keys, and build identities are pinned to these keys by
# payload validation (where ``inputs_digest`` is their compile digest).
_ARTIFACT_SEMANTIC_IDENTITY_KEYS = (
    "hash",
    "rustc",
    "meta_digest",
    "build_identity_scope",
)
_ARTIFACT_REFRESHABLE_METADATA_KEYS = (
    "inputs_digest",
    "source_state",
    "build_identity",
)


def _artifact_semantic_identity(fingerprint: Mapping[str, Any]) -> dict[str, Any]:
    return {key: fingerprint.get(key) for key in _ARTIFACT_SEMANTIC_IDENTITY_KEYS}


def _runtime_fingerprint_payload_is_valid(payload: object) -> bool:
    if not isinstance(payload, dict) or set(payload) - _FIELDS:
        return False
    if type(payload.get("version")) is not int or payload.get("version") != 3:
        return False
    if not _valid_sha256(payload.get("hash")):
        return False
    if any(
        payload.get(key) is not None and not isinstance(payload.get(key), str)
        for key in ("rustc", "inputs_digest", "meta_digest")
    ):
        return False
    if payload.get("source_state") is not None and not isinstance(
        payload.get("source_state"), dict
    ):
        return False
    if "build_identity" in payload or "build_identity_scope" in payload:
        scope = payload.get("build_identity_scope")
        if not isinstance(scope, str) or scope not in {
            "compile",
            "member",
            "member-output",
        }:
            return False
        try:
            identity = RuntimeBuildIdentity.from_dict(payload.get("build_identity"))
            projection = runtime_build_fingerprint(identity, scope=scope)
        except (ValueError, TypeError):
            return False
        if any(payload.get(key) != value for key, value in projection.items()):
            return False
    if "artifact_content_identity" in payload:
        try:
            validate_artifact_content_identity(payload.get("artifact_content_identity"))
        except StaticArchiveIdentityError:
            return False
    return True


def _read_runtime_fingerprint(path: Path) -> dict[str, Any] | None:
    try:
        payload = read_exact(
            path,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="artifact fingerprint",
        )
    except (OSError, ValueError, UnicodeError):
        return None
    return payload if _runtime_fingerprint_payload_is_valid(payload) else None


def _fingerprint_payload(fingerprint: dict[str, Any]) -> dict[str, Any]:
    payload = {"version": _RUNTIME_FINGERPRINT_SCHEMA_VERSION, **fingerprint}
    if not _runtime_fingerprint_payload_is_valid(payload):
        raise ValueError(
            "runtime fingerprint is not a valid artifact identity projection"
        )
    return payload


def _write_runtime_fingerprint(
    path: Path, fingerprint: dict[str, Any], *, artifact: Path | None = None
) -> None:
    payload = _fingerprint_payload(fingerprint)
    if artifact is not None:
        payload["artifact_content_identity"] = artifact_content_identity(artifact)
    _atomic_write_json(path, payload, indent=2)


def _admitted_runtime_fingerprint(
    fingerprint: dict[str, Any], stored: dict[str, Any] | None
) -> dict[str, Any]:
    """Retain known receipt coordinates after source/artifact admission matched.

    Missing current tool metadata never erases an admitted identity. Known
    coordinates still have to match exactly; callers first verify the artifact
    bytes using the canonical admission predicate.
    """
    if stored is None or not _runtime_fingerprint_payload_is_valid(stored):
        raise ValueError("artifact receipt lost custody during admission")
    identity = _artifact_semantic_identity(stored)
    if any(
        fingerprint.get(key) is not None and fingerprint[key] != value
        for key, value in identity.items()
    ):
        raise ValueError("artifact semantic identity changed during admission")
    admitted = {
        **fingerprint,
        **{
            key: value
            for key, value in identity.items()
            if fingerprint.get(key) is None and value is not None
        },
    }

    if (
        admitted.get("build_identity_scope") is not None
        and admitted.get("build_identity") is None
    ):
        admitted["build_identity"] = stored["build_identity"]
    _fingerprint_payload(admitted)
    return admitted


def _refresh_runtime_fingerprint_metadata(
    path: Path, fingerprint: dict[str, Any]
) -> None:
    existing = _read_runtime_fingerprint(path)
    if existing is None:
        raise ValueError("artifact receipt lost custody before metadata refresh")
    payload = _fingerprint_payload(fingerprint)
    if _artifact_semantic_identity(existing) != _artifact_semantic_identity(payload):
        raise ValueError(
            "artifact metadata refresh cannot change its admitted semantic identity"
        )
    if "artifact_content_identity" in existing:
        payload["artifact_content_identity"] = existing["artifact_content_identity"]
    # The admitted identity remains usable if only its metadata cannot be
    # written. Every backend/native/WASM caller shares this policy; malformed
    # or changed semantic identity above is never suppressed.
    try:
        _atomic_write_json(path, payload, indent=2)
    except OSError:
        pass


def _stored_fingerprint_matches_source_metadata(
    stored_fingerprint: dict[str, Any] | None,
    *,
    inputs_digest: str | None,
    rustc: str | None,
    meta_digest: str | None,
) -> bool:
    if stored_fingerprint is None or not inputs_digest:
        return False
    if stored_fingerprint.get("inputs_digest") != inputs_digest:
        return False
    if meta_digest is not None:
        stored_meta = stored_fingerprint.get("meta_digest")
        if stored_meta is None or stored_meta != meta_digest:
            return False
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        if stored_rustc is None or stored_rustc != rustc:
            return False
    return isinstance(stored_fingerprint.get("hash"), str) and bool(
        stored_fingerprint.get("hash")
    )


def _stored_fingerprint_matches_clean_source_state(
    stored_fingerprint: dict[str, Any] | None,
    *,
    source_state: dict[str, str | int] | None,
    rustc: str | None,
    meta_digest: str | None,
) -> bool:
    if stored_fingerprint is None or source_state is None:
        return False
    if stored_fingerprint.get("source_state") != source_state:
        return False
    if meta_digest is not None:
        stored_meta = stored_fingerprint.get("meta_digest")
        if stored_meta is None or stored_meta != meta_digest:
            return False
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        if stored_rustc is None or stored_rustc != rustc:
            return False
    return isinstance(stored_fingerprint.get("hash"), str) and bool(
        stored_fingerprint.get("hash")
    )


def _runtime_fingerprint_metadata_needs_refresh(
    stored_fingerprint: dict[str, Any] | None,
    fingerprint: dict[str, Any],
) -> bool:
    if stored_fingerprint is None:
        return False
    # Only metadata is refreshable. A fingerprint that names another identity,
    # or knows less of it (an unavailable rustc), must not rewrite the receipt.
    if _artifact_semantic_identity(stored_fingerprint) != _artifact_semantic_identity(
        fingerprint
    ):
        return False
    return any(
        stored_fingerprint.get(key) != fingerprint.get(key)
        for key in _ARTIFACT_REFRESHABLE_METADATA_KEYS
    )


def _artifact_needs_rebuild(
    artifact: Path,
    fingerprint: dict[str, Any] | None,
    stored_fingerprint: dict[str, Any] | None,
) -> bool:
    if (
        fingerprint is None
        or stored_fingerprint is None
        or not _artifact_content_looks_valid(artifact)
    ):
        return True
    if not _runtime_fingerprint_payload_is_valid({"version": 3, **fingerprint}):
        return True
    if not _runtime_fingerprint_payload_is_valid(stored_fingerprint):
        return True
    return any(
        stored_fingerprint.get(key) != fingerprint.get(key)
        for key in _ARTIFACT_SEMANTIC_IDENTITY_KEYS
        if fingerprint.get(key) is not None
    )


def _runtime_artifact_fingerprint_matches(
    artifact: Path,
    fingerprint: dict[str, Any] | None,
    fingerprint_path: Path,
    *,
    require_artifact_digest: bool,
) -> bool:
    stored = _read_runtime_fingerprint(fingerprint_path)
    if _artifact_needs_rebuild(artifact, fingerprint, stored):
        return False
    if not require_artifact_digest:
        return True
    if stored is None or "artifact_content_identity" not in stored:
        return False
    try:
        return (
            artifact_content_identity(artifact) == stored["artifact_content_identity"]
        )
    except (OSError, StaticArchiveIdentityError, ValueError):
        return False


def _is_valid_static_library_artifact(path: Path) -> bool:
    if path.suffix not in {".a", ".lib"}:
        return True
    try:
        with path.open("rb") as handle:
            return handle.read(8) == b"!<arch>\n"
    except OSError:
        return False


def _artifact_content_looks_valid(path: Path) -> bool:
    try:
        metadata = path.stat(follow_symlinks=False)
    except OSError:
        return False
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
        return False
    if path.suffix in {".a", ".lib"}:
        return _is_valid_static_library_artifact(path)
    if path.suffix == ".wasm":
        return is_valid_wasm_binary(path)
    return True
