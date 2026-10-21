from __future__ import annotations

import functools
import hashlib
import os
from pathlib import Path

from molt.cli.runtime_paths import _build_state_root


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
