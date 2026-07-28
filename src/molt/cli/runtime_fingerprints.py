from __future__ import annotations

import functools
import hashlib
import json
import os
from pathlib import Path
from typing import Any, cast

from molt.cli.cargo_source_closure import _cargo_crate_source_closure
from molt.cli.capability_spec import _dedupe_preserve_order
from molt.cli.compiler_metadata import _compiler_clean_source_state, _rustc_version
from molt.cli.file_hashing import (
    _hash_source_tree_metadata,
    _hash_source_tree_paths,
)
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli.runtime_artifact_selection import (
    RUNTIME_RLIB_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
)
from molt.wasm_artifact import is_valid_wasm_binary


def _runtime_artifact_identity(artifact: Path) -> dict[str, object] | None:
    try:
        resolved = artifact.resolve()
    except OSError:
        resolved = artifact
    try:
        stat_result = artifact.stat()
    except OSError:
        return None
    return {
        "path": os.fspath(resolved),
        "size": int(stat_result.st_size),
        "mtime_ns": int(stat_result.st_mtime_ns),
        "ctime_ns": int(getattr(stat_result, "st_ctime_ns", 0)),
        "dev": int(getattr(stat_result, "st_dev", 0)),
        "ino": int(getattr(stat_result, "st_ino", 0)),
    }


def _read_runtime_fingerprint(path: Path) -> dict[str, Any] | None:
    payload = _read_cached_json_object(path)
    if payload is not None:
        data = payload
    else:
        try:
            text = path.read_text().strip()
        except OSError:
            return None
        if not text:
            return None
        try:
            json.loads(text)
        except json.JSONDecodeError:
            return {"hash": text, "rustc": None, "inputs_digest": None}
        return None
    hash_value = data.get("hash")
    if not isinstance(hash_value, str) or not hash_value:
        return None
    rustc_value = data.get("rustc")
    inputs_digest = data.get("inputs_digest")
    meta_digest = data.get("meta_digest")
    source_state = data.get("source_state")
    if (
        (rustc_value is None or isinstance(rustc_value, str))
        and (inputs_digest is None or isinstance(inputs_digest, str))
        and (meta_digest is None or isinstance(meta_digest, str))
        and (source_state is None or isinstance(source_state, dict))
    ):
        return data
    if rustc_value is not None and not isinstance(rustc_value, str):
        rustc_value = None
    if inputs_digest is not None and not isinstance(inputs_digest, str):
        inputs_digest = None
    if meta_digest is not None and not isinstance(meta_digest, str):
        meta_digest = None
    if source_state is not None and not isinstance(source_state, dict):
        source_state = None
    return {
        "hash": hash_value,
        "rustc": rustc_value,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
        "source_state": source_state,
    }


def _write_runtime_fingerprint(
    path: Path,
    fingerprint: dict[str, Any],
    *,
    artifact: Path | None = None,
) -> None:
    payload = {
        "version": 3,
        "hash": fingerprint.get("hash"),
        "rustc": fingerprint.get("rustc"),
        "inputs_digest": fingerprint.get("inputs_digest"),
        "meta_digest": fingerprint.get("meta_digest"),
    }
    source_state = fingerprint.get("source_state")
    if isinstance(source_state, dict):
        payload["source_state"] = source_state
    if artifact is not None:
        payload["artifact_content_identity"] = artifact_content_identity(artifact)
        artifact_identity = _runtime_artifact_identity(artifact)
        if artifact_identity is not None:
            payload["artifact_identity"] = artifact_identity
    _write_cached_json_object(path, payload)


def _refresh_runtime_fingerprint_metadata(
    path: Path,
    fingerprint: dict[str, Any],
) -> None:
    payload = _read_cached_json_object(path) or {}
    payload.update(
        {
            "version": 2,
            "hash": fingerprint.get("hash"),
            "rustc": fingerprint.get("rustc"),
            "inputs_digest": fingerprint.get("inputs_digest"),
            "meta_digest": fingerprint.get("meta_digest"),
        }
    )
    source_state = fingerprint.get("source_state")
    if isinstance(source_state, dict):
        payload["source_state"] = source_state
    else:
        payload.pop("source_state", None)
    _write_cached_json_object(path, payload)


_RUNTIME_FACADE_CRATE = Path("runtime/molt-runtime")
_RUNTIME_SOURCE_FEATURE_MARKERS = frozenset({"default-features", "no-default-features"})


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
    for key in ("hash", "rustc", "inputs_digest", "meta_digest", "source_state"):
        if stored_fingerprint.get(key) != fingerprint.get(key):
            return True
    return False


def _runtime_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
    rustflags: str,
    runtime_features: tuple[str, ...] = (),
    artifact_selection: RuntimeArtifactSelection = RUNTIME_RLIB_ARTIFACTS,
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, Any] | None:
    feature_list = tuple(_dedupe_preserve_order(sorted(runtime_features)))
    meta = f"profile:{cargo_profile}\ntarget:{target_triple or 'native'}\n"
    meta += "build-schema:runtime-feature-profile-v4\n"
    meta += f"rustflags:{rustflags}\n"
    meta += f"features:{','.join(feature_list)}\n"
    meta += f"artifacts:{artifact_selection.source_identity}\n"
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    rustc_info = _rustc_version()
    source_state = _compiler_clean_source_state(project_root)
    if _stored_fingerprint_matches_clean_source_state(
        stored_fingerprint,
        source_state=source_state,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": stored_fingerprint.get("inputs_digest"),
            "meta_digest": meta_digest,
            "source_state": source_state,
        }
    source_paths = _runtime_source_paths(project_root, runtime_features=feature_list)
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
            "source_state": source_state,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        _hash_source_tree_paths(source_paths, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
        "source_state": source_state,
    }


def _runtime_manifest_cache_stamp(project_root: Path) -> str:
    runtime_root = project_root / "runtime"
    manifests = [
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
        runtime_root / "Cargo.toml",
        runtime_root / "Cargo.lock",
    ]
    manifests.extend(sorted(runtime_root.glob("*/Cargo.toml")))
    metadata = _hash_source_tree_metadata(manifests, project_root)
    return metadata[0] if metadata is not None else "metadata-unavailable"


def _runtime_source_features(runtime_features: tuple[str, ...]) -> tuple[str, ...]:
    return tuple(
        sorted(
            {
                feature
                for feature in runtime_features
                if feature and feature not in _RUNTIME_SOURCE_FEATURE_MARKERS
            }
        )
    )


@functools.lru_cache(maxsize=256)
def _runtime_source_paths_cached(
    project_root_str: str,
    runtime_features: tuple[str, ...],
    manifest_cache_stamp: str,
) -> tuple[Path, ...]:
    del manifest_cache_stamp
    project_root = Path(project_root_str)
    return tuple(
        _cargo_crate_source_closure(
            project_root=project_root,
            crate_root=project_root / _RUNTIME_FACADE_CRATE,
            crate_features=runtime_features,
            extra_source_paths=(
                project_root / "Cargo.toml",
                project_root / "Cargo.lock",
                project_root / "runtime/Cargo.toml",
                project_root / "runtime/Cargo.lock",
                project_root / "runtime/build_support",
                project_root / "runtime/molt-cpython-abi/shims",
            ),
        )
    )


def _runtime_source_paths(
    project_root: Path,
    runtime_features: tuple[str, ...] = (),
) -> list[Path]:
    normalized_features = _runtime_source_features(runtime_features)
    return list(
        _runtime_source_paths_cached(
            os.fspath(project_root),
            normalized_features,
            _runtime_manifest_cache_stamp(project_root),
        )
    )


def _artifact_needs_rebuild(
    artifact: Path,
    fingerprint: dict[str, str | None] | None,
    stored_fingerprint: dict[str, str | None] | None,
) -> bool:
    try:
        artifact.stat()
    except OSError:
        return True
    if not _artifact_content_looks_valid(artifact):
        return True
    if fingerprint is None or stored_fingerprint is None:
        return True
    if stored_fingerprint.get("hash") != fingerprint.get("hash"):
        return True
    meta_digest = fingerprint.get("meta_digest")
    if meta_digest:
        stored_meta_digest = stored_fingerprint.get("meta_digest")
        if stored_meta_digest is None or stored_meta_digest != meta_digest:
            return True
    rustc = fingerprint.get("rustc")
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        return stored_rustc is None or stored_rustc != rustc
    return False


def _runtime_artifact_fingerprint_matches(
    artifact: Path,
    fingerprint: dict[str, str | None] | None,
    fingerprint_path: Path,
    *,
    require_artifact_digest: bool,
) -> bool:
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    if _artifact_needs_rebuild(artifact, fingerprint, stored_fingerprint):
        return False
    if not require_artifact_digest:
        return True
    if stored_fingerprint is None:
        return False
    stored_content_identity = stored_fingerprint.get("artifact_content_identity")
    if not isinstance(stored_content_identity, dict):
        return False
    artifact_identity = stored_fingerprint.get("artifact_identity")
    if isinstance(artifact_identity, dict) and (
        artifact_identity == _runtime_artifact_identity(artifact)
    ):
        return True
    try:
        return artifact_content_identity(artifact) == stored_content_identity
    except (OSError, StaticArchiveIdentityError):
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
    if path.suffix in {".a", ".lib"}:
        return _is_valid_static_library_artifact(path)
    if path.suffix == ".wasm":
        return is_valid_wasm_binary(path)
    return True
