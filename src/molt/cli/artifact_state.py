from __future__ import annotations

import functools
import hashlib
import os
from pathlib import Path

from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.runtime_fingerprints import (
    _runtime_artifact_fingerprint_matches,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _build_state_root,
    _build_state_root_cached,
    _cargo_target_root_cached,
)


@functools.lru_cache(maxsize=512)
def _resolved_artifact_hash_key(path_str: str) -> str:
    return hashlib.sha256(str(Path(path_str).resolve()).encode("utf-8")).hexdigest()[
        :16
    ]


def _runtime_fingerprint_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
    target_triple: str | None,
) -> Path:
    target = (target_triple or "native").replace(os.sep, "_").replace(":", "_")
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="runtime_fingerprints",
        stem_suffix=f"{cargo_profile}.{target}",
        extension="fingerprint",
    )


def _runtime_target_fingerprint_path(
    build_state_root: Path,
    artifact: Path,
    *,
    cargo_profile: str,
    target_label: str,
) -> Path:
    return _artifact_state_path_for_build_state_root(
        build_state_root,
        artifact,
        subdir="runtime_fingerprints",
        stem_suffix=f"{cargo_profile}.{target_label}",
        extension="fingerprint",
    )


@functools.lru_cache(maxsize=4096)
def _artifact_state_path_cached(
    build_state_root_str: str,
    artifact_path_str: str,
    artifact_name: str,
    subdir: str,
    stem_suffix: str,
    extension: str,
) -> Path:
    artifact_key = _resolved_artifact_hash_key(artifact_path_str)
    stem = (
        f"{artifact_name}.{stem_suffix}.{artifact_key}"
        if stem_suffix
        else f"{artifact_name}.{artifact_key}"
    )
    return Path(build_state_root_str) / subdir / f"{stem}.{extension}"


@functools.lru_cache(maxsize=512)
def _build_state_subdir_cached(build_state_root_str: str, subdir: str) -> Path:
    return Path(build_state_root_str) / subdir


def _artifact_state_path(
    project_root: Path,
    artifact: Path,
    *,
    subdir: str,
    stem_suffix: str,
    extension: str,
) -> Path:
    build_state_root = os.fspath(_build_state_root(project_root))
    return _artifact_state_path_cached(
        build_state_root,
        os.fspath(artifact),
        artifact.name,
        subdir,
        stem_suffix,
        extension,
    )


def _artifact_state_path_for_build_state_root(
    build_state_root: Path,
    artifact: Path,
    *,
    subdir: str,
    stem_suffix: str,
    extension: str,
) -> Path:
    return _artifact_state_path_cached(
        os.fspath(build_state_root),
        os.fspath(artifact),
        artifact.name,
        subdir,
        stem_suffix,
        extension,
    )


def _canonical_target_root(project_root: Path) -> Path:
    return _cargo_target_root_cached(
        os.fspath(project_root),
        None,
        os.fspath(Path.cwd()),
    )


def _canonical_build_state_root(project_root: Path) -> Path:
    return _build_state_root_cached(
        os.fspath(project_root),
        os.environ.get("MOLT_BUILD_STATE_DIR"),
        None,
        os.fspath(Path.cwd()),
        None,
    )


def _maybe_hydrate_artifact_from_canonical_target(
    *,
    artifact: Path,
    fingerprint: dict[str, str | None] | None,
    fingerprint_path: Path,
    candidate_artifact: Path,
    candidate_fingerprint_path: Path,
    require_artifact_digest: bool = False,
) -> bool:
    if fingerprint is None:
        return False
    if artifact.resolve() == candidate_artifact.resolve():
        return False
    if not _runtime_artifact_fingerprint_matches(
        candidate_artifact,
        fingerprint,
        candidate_fingerprint_path,
        require_artifact_digest=require_artifact_digest,
    ):
        return False
    try:
        artifact.parent.mkdir(parents=True, exist_ok=True)
        _atomic_copy_file(candidate_artifact, artifact)
        fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            fingerprint_path,
            fingerprint,
            artifact=artifact if require_artifact_digest else None,
        )
    except OSError:
        return False
    return True
