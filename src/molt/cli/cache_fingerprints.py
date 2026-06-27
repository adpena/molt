from __future__ import annotations

import functools
import hashlib
import os
import pathlib
import tomllib
from pathlib import Path
from typing import Any, Sequence

from molt.cli.compiler_metadata import _compiler_root, _rustc_version
from molt.cli.runtime_fingerprints import (
    _hash_runtime_file,
    _hash_source_tree_metadata,
    _runtime_source_paths,
    _source_fingerprint_files,
)


_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION = "source-tree-v2"
_BACKEND_FACADE_CRATE = Path("runtime/molt-backend")
_BACKEND_CACHE_ALL_FEATURES = (
    "cbor",
    "egraphs",
    "jemalloc",
    "llvm",
    "luau-backend",
    "mlx",
    "native-backend",
    "polly",
    "rust-backend",
    "wasm-backend",
)


def _dedupe_source_paths(paths: Sequence[Path]) -> list[Path]:
    deduped: list[Path] = []
    seen: set[Path] = set()
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        deduped.append(path)
    return deduped


def _crate_source_paths(crate_root: Path) -> tuple[Path, Path, Path]:
    return (
        crate_root / "src",
        crate_root / "Cargo.toml",
        crate_root / "build.rs",
    )


def _cargo_manifest_stamp(manifest: Path) -> str:
    try:
        stat = manifest.stat()
    except OSError:
        return "missing"
    return f"{stat.st_size}:{stat.st_mtime_ns}:{stat.st_ctime_ns}"


@functools.lru_cache(maxsize=512)
def _read_cargo_manifest_cached(
    manifest_str: str,
    manifest_stamp: str,
) -> dict[str, Any]:
    manifest = Path(manifest_str)
    try:
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return {}
    return data if isinstance(data, dict) else {}


def _read_cargo_manifest(manifest: Path) -> dict[str, Any]:
    return _read_cargo_manifest_cached(
        os.fspath(manifest),
        _cargo_manifest_stamp(manifest),
    )


def _manifest_dependency_tables(data: dict[str, Any]) -> list[dict[str, Any]]:
    tables: list[dict[str, Any]] = []
    for key in ("dependencies", "build-dependencies"):
        table = data.get(key)
        if isinstance(table, dict):
            tables.append(table)
    target = data.get("target")
    if isinstance(target, dict):
        for target_table in target.values():
            if not isinstance(target_table, dict):
                continue
            for key in ("dependencies", "build-dependencies"):
                table = target_table.get(key)
                if isinstance(table, dict):
                    tables.append(table)
    return tables


def _local_path_dependencies(
    *,
    crate_root: Path,
    data: dict[str, Any],
    selected_optional_deps: set[str],
    child_features: dict[str, set[str]],
) -> list[tuple[str, Path, tuple[str, ...]]]:
    deps: list[tuple[str, Path, tuple[str, ...]]] = []
    for table in _manifest_dependency_tables(data):
        for dep_name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            dep_path = spec.get("path")
            if not isinstance(dep_path, str) or not dep_path:
                continue
            optional = bool(spec.get("optional", False))
            if optional and dep_name not in selected_optional_deps:
                continue
            features = set(child_features.get(dep_name, set()))
            spec_features = spec.get("features", [])
            if isinstance(spec_features, list):
                features.update(
                    feature for feature in spec_features if isinstance(feature, str)
                )
            dep_root = (crate_root / dep_path).resolve()
            deps.append((dep_name, dep_root, tuple(sorted(features))))
    return deps


def _feature_dependency_selection(
    data: dict[str, Any],
    requested_features: tuple[str, ...],
) -> tuple[set[str], dict[str, set[str]]]:
    features = data.get("features")
    if not isinstance(features, dict):
        return set(), {}
    if requested_features:
        pending = list(requested_features)
    else:
        default_features = features.get("default", [])
        pending = [item for item in default_features if isinstance(item, str)]
    seen_features: set[str] = set()
    selected_optional_deps: set[str] = set()
    child_features: dict[str, set[str]] = {}
    while pending:
        feature = pending.pop()
        if feature in seen_features:
            continue
        seen_features.add(feature)
        entries = features.get(feature, [])
        if not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, str) or not entry:
                continue
            if entry.startswith("dep:"):
                selected_optional_deps.add(entry[4:])
                continue
            if "/" in entry:
                dep_name, child_feature = entry.split("/", 1)
                child_feature = child_feature.removesuffix("?")
                if dep_name and child_feature:
                    selected_optional_deps.add(dep_name)
                    child_features.setdefault(dep_name, set()).add(child_feature)
                continue
            pending.append(entry)
    return selected_optional_deps, child_features


def _backend_source_feature_names(project_root: Path) -> tuple[str, ...]:
    data = _read_cargo_manifest(project_root / _BACKEND_FACADE_CRATE / "Cargo.toml")
    features = data.get("features")
    if not isinstance(features, dict):
        return _BACKEND_CACHE_ALL_FEATURES
    names = tuple(sorted(name for name in features if name != "default"))
    return names or _BACKEND_CACHE_ALL_FEATURES


def _backend_manifest_cache_stamp(project_root: Path) -> str:
    runtime_root = project_root / "runtime"
    manifests = {
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
        runtime_root / "molt-backend" / "Cargo.toml",
        runtime_root / "molt-ir" / "Cargo.toml",
        runtime_root / "molt-tir" / "Cargo.toml",
        runtime_root / "molt-codegen-abi" / "Cargo.toml",
    }
    manifests.update(runtime_root.glob("molt-backend*/Cargo.toml"))
    metadata = _hash_source_tree_metadata(sorted(manifests), project_root)
    return metadata[0] if metadata is not None else "metadata-unavailable"


def _backend_crate_source_closure(
    project_root: Path,
    backend_features: tuple[str, ...],
) -> list[Path]:
    source_paths: list[Path] = []
    project_root_resolved = project_root.resolve()
    pending: list[tuple[Path, tuple[str, ...]]] = [
        (project_root / _BACKEND_FACADE_CRATE, backend_features)
    ]
    seen: set[tuple[Path, tuple[str, ...]]] = set()
    while pending:
        crate_root, crate_features = pending.pop()
        key = (crate_root, crate_features)
        if key in seen:
            continue
        seen.add(key)
        source_paths.extend(_crate_source_paths(crate_root))
        data = _read_cargo_manifest(crate_root / "Cargo.toml")
        selected_optional_deps, child_features = _feature_dependency_selection(
            data, crate_features
        )
        for _dep_name, dep_root, dep_features in _local_path_dependencies(
            crate_root=crate_root,
            data=data,
            selected_optional_deps=selected_optional_deps,
            child_features=child_features,
        ):
            if (
                project_root_resolved in dep_root.parents
                or dep_root == project_root_resolved
            ):
                pending.append((dep_root, dep_features))
    source_paths.extend((project_root / "Cargo.toml", project_root / "Cargo.lock"))
    return _dedupe_source_paths(source_paths)


@functools.lru_cache(maxsize=256)
def _backend_source_paths_cached(
    project_root_str: str,
    backend_features: tuple[str, ...],
    manifest_cache_stamp: str,
) -> tuple[Path, ...]:
    project_root = Path(project_root_str)
    source_paths = _backend_crate_source_closure(project_root, backend_features)
    return tuple(source_paths)


def _backend_source_paths(
    project_root: Path,
    backend_features: tuple[str, ...] = (),
) -> list[Path]:
    normalized_features = tuple(sorted(set(backend_features)))
    return list(
        _backend_source_paths_cached(
            os.fspath(project_root),
            normalized_features,
            _backend_manifest_cache_stamp(project_root),
        )
    )


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
        root, _backend_source_feature_names(root)
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
