from __future__ import annotations

import ast
import functools
import hashlib
import os
import pathlib
import tomllib
from pathlib import Path
from typing import Any, Sequence

from molt.cli.compiler_metadata import _compiler_root, _rustc_version
from molt.cli.file_hashing import _sha256_file
from molt.cli.runtime_fingerprints import (
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


# Molt-owned frontend source files that are shared by *every* frontend phase
# (import scan, analysis, lowering) yet live outside cli/ and frontend/.
_FRONTEND_AUX_SOURCE_RELPATHS: tuple[str, ...] = (
    "type_facts.py",
    "capabilities.py",
    "capability_manifest.py",
    "compat.py",
    "_wasm_runtime_exports.py",
)


# Roots of the frontend lowering computation. Reachability starts here and
# follows module-level ``molt`` imports; anything not reached provably never runs
# while a module is lowered and so cannot change the lowering result.
#
#   * ``frontend/`` -- the whole Python->TIR frontend (visitors, sema, lowering),
#     kept wholesale because its subpackages import one another.
#   * ``cli/frontend_*.py`` and ``cli/module_*.py`` -- the CLI-level frontend /
#     module-graph drivers that invoke and cache the frontend (frontend pipeline,
#     workers, module resolution / graph / source / cache authorities).
#
# These are a structural naming rule, not a hand-maintained membership list: a new
# ``frontend_*`` / ``module_*`` driver is picked up automatically, and a new
# backend/link/cargo file is excluded automatically because it is not a seed and
# is not reachable from one.
_LOWERING_SCOPE_SEED_CLI_PREFIXES: tuple[str, ...] = ("frontend_", "module_")


def _module_level_molt_import_targets(path: Path, src_root: Path) -> set[str]:
    """Fully-qualified ``molt`` modules imported at MODULE LEVEL from ``path``.

    Only top-level ``import`` / ``from ... import`` statements are considered:
    reachability models the closure that executes as a side effect of *importing*
    the frontend drivers, which is exactly the set of files whose code runs to
    produce a lowering. A dependency pulled in only inside a function body is
    deferred work (post-lowering command dispatch, native linking, diagnostics)
    that runs after the cached lowering is computed, so it is intentionally not
    followed -- following it would drag the backend back onto the frontend path.
    """
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"))
    except (OSError, SyntaxError, ValueError):
        return set()
    try:
        module_name = _module_name_for_source(path, src_root)
    except ValueError:
        module_name = None
    package = (
        module_name.rsplit(".", 1)[0]
        if module_name and "." in module_name
        else module_name
    )
    targets: set[str] = set()
    for node in tree.body:  # module level only
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name.startswith("molt"):
                    targets.add(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.level == 0:
                base = node.module or ""
                if not base.startswith("molt"):
                    continue
            else:
                if package is None:
                    continue
                base_parts = package.split(".")
                if node.level > 1:
                    trim = node.level - 1
                    base_parts = base_parts[:-trim] if trim < len(base_parts) else []
                base = ".".join(
                    base_parts + ([node.module] if node.module else [])
                )
                if not base.startswith("molt"):
                    continue
            targets.add(base)
            for alias in node.names:
                targets.add(f"{base}.{alias.name}")
    return targets


def _module_name_for_source(path: Path, src_root: Path) -> str:
    rel = path.resolve().relative_to(src_root)
    parts = list(rel.parts)
    if parts[-1] == "__init__.py":
        parts = parts[:-1]
    elif parts[-1].endswith(".py"):
        parts[-1] = parts[-1][: -len(".py")]
    return ".".join(parts)


def _molt_module_source_file(module: str, src_root: Path) -> Path | None:
    rel = Path(*module.split("."))
    candidate = src_root / f"{rel}.py"
    if candidate.exists():
        return candidate.resolve()
    package_init = src_root / rel / "__init__.py"
    if package_init.exists():
        return package_init.resolve()
    return None


def _resolve_import_targets_to_files(targets: set[str], src_root: Path) -> set[Path]:
    files: set[Path] = set()
    for target in targets:
        source = _molt_module_source_file(target, src_root)
        if source is not None:
            files.add(source)
            continue
        # ``from pkg.mod import name`` yields ``pkg.mod.name``; ``name`` may be an
        # attribute rather than a submodule -- fall back to the owning module.
        if "." in target:
            owner = _molt_module_source_file(target.rsplit(".", 1)[0], src_root)
            if owner is not None:
                files.add(owner)
    return files


@functools.lru_cache(maxsize=64)
def _lowering_scope_source_files_cached(project_root_str: str) -> tuple[str, ...]:
    """Module-level import closure of the frontend lowering seeds.

    Returns every ``molt``-owned source file reachable by following module-level
    imports from the frontend/module drivers. After the frontend<->backend import
    seams were cut, this closure provably excludes the backend / native-link /
    cargo / daemon / toolchain layer, so an edit to any of those files leaves the
    persisted analysis / lowering / import-graph fingerprints unchanged. The set
    is cached per project root (mirroring ``_frontend_tooling_source_paths``); the
    per-module cache's own source-stat + ``context_digest`` gate remains the
    correctness authority for individual module edits.
    """
    project_root = pathlib.Path(project_root_str)
    src_root = (project_root / "src").resolve()
    molt_root = src_root / "molt"
    cli_root = molt_root / "cli"

    seeds: set[Path] = set()
    frontend_root = molt_root / "frontend"
    if frontend_root.exists():
        for source in frontend_root.rglob("*.py"):
            seeds.add(source.resolve())
    if cli_root.exists():
        for source in cli_root.glob("*.py"):
            if source.name.startswith(_LOWERING_SCOPE_SEED_CLI_PREFIXES):
                seeds.add(source.resolve())

    reached: set[Path] = set()
    pending = list(seeds)
    while pending:
        current = pending.pop()
        if current in reached:
            continue
        reached.add(current)
        targets = _module_level_molt_import_targets(current, src_root)
        for source in _resolve_import_targets_to_files(targets, src_root):
            if source not in reached:
                pending.append(source)
    return tuple(sorted(str(path) for path in reached))


def _frontend_semantic_tooling_source_paths(project_root: Path) -> list[Path]:
    """Source paths the persisted per-module *frontend* caches must key on.

    Derived structurally by import reachability (see
    ``_lowering_scope_source_files_cached``): the whole ``frontend/`` tree plus
    every ``molt``-owned file reachable from the frontend/module drivers by
    module-level import, plus the shared aux semantic files. No hand-maintained
    denylist: adding a backend/link/cargo file never enters this scope (it is not
    reachable from a lowering seed), while a new lowering-relevant module that a
    driver imports is picked up automatically.

    The bias is strictly toward inclusion -- every file on the frontend import
    path is hashed, so a spurious cold-start is the worst outcome; a stale
    lowering (miscompile) cannot arise from under-scoping here. ``frontend/`` is
    returned as a directory (its subpackages import one another and are kept
    whole); reachable files under it are therefore skipped from the per-file list
    to avoid hashing them twice.
    """
    molt_root = project_root / "src" / "molt"
    frontend_root = (molt_root / "frontend").resolve()
    paths: list[Path] = [molt_root / "frontend"]
    for path_str in _lowering_scope_source_files_cached(os.fspath(project_root)):
        source = pathlib.Path(path_str)
        if frontend_root == source or frontend_root in source.parents:
            continue
        paths.append(source)
    # Force-include the shared aux semantic files even if a future refactor drops
    # them from the import closure -- they are load-bearing frontend inputs.
    paths.extend(molt_root / relpath for relpath in _FRONTEND_AUX_SOURCE_RELPATHS)
    return paths


def _source_fingerprint_path_keys(paths: Sequence[Path]) -> tuple[str, ...]:
    return tuple(
        str(path.resolve())
        for path in sorted(set(paths), key=lambda candidate: str(candidate))
    )


# Per-process cache of source-tree content digests. The key includes a
# *content-derived* signature (see `_file_content_signature`) rather than only
# stat metadata: same-length edits under coarse filesystem mtime resolution can
# leave (size, mtime_ns, ctime_ns) unchanged, so a metadata-only key would serve
# a stale content digest and cause the frontend cache to reuse a stale lowering
# (a miscompile). Reading file bytes is the only sound authority for content, so
# the digest is always computed from bytes; the cache merely dedupes repeated
# identical trees within one build process.
_SOURCE_TREE_CONTENT_DIGEST_CACHE: dict[tuple[str, ...], str] = {}
_SOURCE_TREE_CONTENT_DIGEST_CACHE_LIMIT = 64


def _file_content_signature(path: Path) -> str:
    """Content-sound per-file signature: sha256 of the file bytes.

    Using the content hash (not stat metadata) guarantees that any change to a
    tracked source file changes the signature even when size and mtime collide.
    """

    try:
        digest = _sha256_file(path)
    except OSError:
        return "unreadable"
    return digest


def _source_tree_content_signature(
    root: Path,
    path_keys: tuple[str, ...],
) -> tuple[str, ...]:
    signature: list[str] = []
    for path_key in path_keys:
        path = pathlib.Path(path_key)
        for item in _source_fingerprint_files(path):
            try:
                rel_text = str(item.relative_to(root))
            except ValueError:
                rel_text = str(item)
            signature.append(f"{rel_text}={_file_content_signature(item)}")
    return tuple(signature)


def _compute_source_tree_content_digest(
    content_signature: tuple[str, ...],
    scope: str,
    extra_fingerprint_inputs: str,
) -> str:
    # The signature already carries each tracked file's relative path and the
    # sha256 of its bytes, so hashing the signature is a sound, read-once content
    # digest: any byte-level change to any tracked file changes the signature and
    # therefore this digest, independent of stat metadata.
    hasher = hashlib.sha256()
    hasher.update(_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(scope.encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(extra_fingerprint_inputs.encode("utf-8"))
    hasher.update(b"\0")
    for entry in content_signature:
        hasher.update(entry.encode("utf-8"))
        hasher.update(b"\0")
    return hasher.hexdigest()


def _source_tree_content_digest(
    root: Path,
    path_keys: tuple[str, ...],
    content_signature: tuple[str, ...],
    scope: str,
    extra_fingerprint_inputs: str,
) -> str:
    cache_key = (
        str(root),
        scope,
        extra_fingerprint_inputs,
        *path_keys,
        "\0",
        *content_signature,
    )
    cached = _SOURCE_TREE_CONTENT_DIGEST_CACHE.get(cache_key)
    if cached is not None:
        return cached
    digest = _compute_source_tree_content_digest(
        content_signature,
        scope,
        extra_fingerprint_inputs,
    )
    if len(_SOURCE_TREE_CONTENT_DIGEST_CACHE) >= _SOURCE_TREE_CONTENT_DIGEST_CACHE_LIMIT:
        _SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    _SOURCE_TREE_CONTENT_DIGEST_CACHE[cache_key] = digest
    return digest


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
    content_signature = _source_tree_content_signature(root, path_keys)
    content_digest = _source_tree_content_digest(
        root,
        path_keys,
        content_signature,
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


def _frontend_semantic_tooling_fingerprint() -> str:
    """Tooling fingerprint for the per-module frontend caches.

    Identical in construction to ``_cache_tooling_fingerprint`` but over the
    lowering-relevant scope only (see ``_frontend_semantic_tooling_source_paths``),
    so an unrelated backend/link/daemon/cargo/toolchain edit does not cold-start a
    module's persisted analysis / lowering / import-graph entry. The distinct
    ``scope`` tag keeps this digest namespace-separated from the broad
    ``frontend-tooling`` fingerprint.
    """
    root = _compiler_root()
    return _source_tree_cache_fingerprint(
        root=root,
        source_paths=_frontend_semantic_tooling_source_paths(root),
        scope="frontend-semantic-tooling",
        extra_fingerprint_inputs="",
    )
