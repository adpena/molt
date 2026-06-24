from __future__ import annotations

import functools
import hashlib
import os
import pathlib
from pathlib import Path
from typing import Sequence

from molt.cli.compiler_metadata import _compiler_root, _rustc_version
from molt.cli.runtime_fingerprints import (
    _hash_runtime_file,
    _hash_source_tree_metadata,
    _runtime_source_paths,
    _source_fingerprint_files,
)


_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION = "source-tree-v2"


@functools.lru_cache(maxsize=256)
def _backend_source_paths_cached(
    project_root_str: str,
    backend_features: tuple[str, ...],
) -> tuple[Path, ...]:
    project_root = Path(project_root_str)
    backend_root = project_root / "runtime/molt-backend"
    # Track the full backend source tree so new files are covered mechanically.
    source_paths: list[Path] = [
        backend_root / "src",
        backend_root / "Cargo.toml",
        backend_root / "build.rs",
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
    ]
    return tuple(source_paths)


def _backend_source_paths(
    project_root: Path,
    backend_features: tuple[str, ...] = (),
) -> list[Path]:
    return list(_backend_source_paths_cached(os.fspath(project_root), backend_features))


@functools.lru_cache(maxsize=128)
def _frontend_tooling_source_paths_cached(project_root_str: str) -> tuple[Path, ...]:
    project_root = pathlib.Path(project_root_str)
    molt_root = project_root / "src" / "molt"
    return (
        molt_root / "cli",
        molt_root / "frontend",
        molt_root / "type_facts.py",
        molt_root / "capabilities.py",
        molt_root / "capability_manifest.py",
        molt_root / "compat.py",
        molt_root / "_wasm_runtime_exports.py",
    )


def _frontend_tooling_source_paths(project_root: Path) -> list[Path]:
    return list(_frontend_tooling_source_paths_cached(os.fspath(project_root)))


def _source_fingerprint_path_keys(paths: Sequence[Path]) -> tuple[str, ...]:
    return tuple(
        str(path.resolve())
        for path in sorted(set(paths), key=lambda candidate: str(candidate))
    )


@functools.lru_cache(maxsize=64)
def _source_tree_content_digest_cached(
    root_str: str,
    path_keys: tuple[str, ...],
    metadata_digest: str,
    scope: str,
    extra_fingerprint_inputs: str,
) -> str:
    root = pathlib.Path(root_str)
    hasher = hashlib.sha256()
    hasher.update(_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(scope.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(extra_fingerprint_inputs.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(metadata_digest.encode("utf-8"))
    hasher.update(b"\0")
    for path_key in path_keys:
        path = pathlib.Path(path_key)
        for item in _source_fingerprint_files(path):
            _hash_runtime_file(item, root, hasher)
    return hasher.hexdigest()


def _source_tree_cache_fingerprint(
    *,
    root: Path,
    source_paths: Sequence[Path],
    scope: str,
    extra_fingerprint_inputs: str,
) -> str:
    path_keys = _source_fingerprint_path_keys(source_paths)
    normalized_paths = [pathlib.Path(path_key) for path_key in path_keys]
    metadata = _hash_source_tree_metadata(normalized_paths, root)
    metadata_digest = metadata[0] if metadata is not None else "metadata-unavailable"
    file_count = metadata[1] if metadata is not None else -1
    content_digest = _source_tree_content_digest_cached(
        str(root),
        path_keys,
        metadata_digest,
        scope,
        extra_fingerprint_inputs,
    )
    hasher = hashlib.sha256()
    hasher.update(_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(scope.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(f"files:{file_count}".encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(metadata_digest.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(content_digest.encode("utf-8"))
    return hasher.hexdigest()


def _cache_fingerprint() -> str:
    root = _compiler_root()
    rustc_info = _rustc_version() or ""
    rustflags = os.environ.get("RUSTFLAGS", "")
    # Hash source trees, not backend binaries: binary fingerprints over-invalidate
    # on incremental rebuilds even when source semantics are unchanged.
    source_paths = _backend_source_paths(
        root, ("egraphs", "luau-backend", "rust-backend", "wasm-backend")
    ) + _runtime_source_paths(root)
    return _source_tree_cache_fingerprint(
        root=root,
        source_paths=source_paths,
        scope="compiler-runtime-backend",
        extra_fingerprint_inputs=(f"rustc:{rustc_info}\nrustflags:{rustflags}\n"),
    )


def _cache_tooling_fingerprint() -> str:
    root = _compiler_root()
    return _source_tree_cache_fingerprint(
        root=root,
        source_paths=_frontend_tooling_source_paths(root),
        scope="frontend-tooling",
        extra_fingerprint_inputs="",
    )
