from __future__ import annotations

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


# ``cli/*.py`` modules that run strictly AFTER the frontend has produced its
# import-scan / analysis / lowering result -- backend codegen, native/wasm
# linking and artifact staging -- or are orthogonal build machinery (the backend
# daemon, cargo/toolchain provisioning, packaging, SBOM/signing, the dx/queue CLI
# surface, and terminal output formatting). None of them can change *what* a
# module lowers to; they only consume a finished lowering or manage the build
# environment. An edit to any of them must therefore NOT invalidate the persisted
# per-module frontend caches (analysis, lowering, import graph), which is exactly
# the needless cold-start the ranked witness modules pay today when unrelated
# CLI/runtime/link code is touched.
#
# Membership is deliberately CONSERVATIVE: a file belongs here only when it
# provably cannot alter a frontend result. Anything that participates in module
# resolution, import admission, capability/type facts, native-export discovery,
# or frontend compute -- or whose role is uncertain -- is intentionally *omitted*
# so it keeps invalidating the caches. Over-scoping (a spurious invalidation) is a
# harmless perf cost; under-scoping (reusing a stale lowering) is a miscompile, so
# the bias is strictly toward inclusion.
_POST_LOWERING_ORTHOGONAL_CLI_BASENAMES: frozenset[str] = frozenset(
    {
        "backend_binary.py",
        "backend_cache.py",
        "backend_cache_setup.py",
        "backend_compile.py",
        "backend_daemon_config.py",
        "backend_daemon_logs.py",
        "backend_daemon_paths.py",
        "backend_daemon_startup.py",
        "backend_diagnostics.py",
        "backend_execution.py",
        "backend_ir.py",
        "backend_output_pipeline.py",
        "backend_pipeline.py",
        "binary_image_analysis.py",
        "cargo_execution.py",
        "cargo_profiles.py",
        "completion.py",
        "dx_cli.py",
        # NOTE: external_native.py is NOT orthogonal — frontend_pipeline imports its
        # `_resolve_import_admission_policy`, which feeds `direct_call_modules`, a
        # lowering-affecting input (native-direct-call vs Python-call lowering).
        # Excluding it would risk STALE lowering (miscompile). Kept in scope.
        "link_pipeline.py",
        "maintenance.py",
        "mlir_backend.py",
        "native_binary.py",
        "native_link_command.py",
        "native_link_deps.py",
        "native_main_stub.py",
        "native_toolchain.py",
        "non_native_output.py",
        "queue_cli.py",
        "runtime_build.py",
        "runtime_wasm_cache.py",
        "runtime_wasm_validation.py",
        "sbom.py",
        "signing.py",
        "wasm.py",
        "wasm_toolchain.py",
    }
)


def _frontend_semantic_tooling_source_paths(project_root: Path) -> list[Path]:
    """Source paths the persisted per-module *frontend* caches must key on.

    This is the ``_frontend_tooling_source_paths`` closure MINUS the post-lowering
    / orthogonal ``cli/*.py`` modules named in
    ``_POST_LOWERING_ORTHOGONAL_CLI_BASENAMES``. The whole ``frontend/`` tree is
    kept intact: its subpackages (``visitors``, ``sema``, ``lowering``) import one
    another, so no subtree can be soundly excluded from one phase without risking
    a stale result. The single sound reduction is dropping the CLI files that
    provably run after (or beside) lowering.

    The import-scan, analysis, and lowering caches all key on this one set: each
    phase depends on the frontend semantic tooling and is independent of the
    orthogonal backend/link/toolchain tooling, so they share the identical sound
    scope. Keeping this as one authority (rather than three coincidentally-equal
    lists) preserves a single source of truth for the cache identity.

    ``cli/*.py`` is enumerated on every call so that a newly-added CLI module is
    included by default (and therefore keeps invalidating the caches) unless it is
    explicitly classified as orthogonal above -- the fail-safe direction.
    """
    molt_root = project_root / "src" / "molt"
    cli_root = molt_root / "cli"
    paths: list[Path] = [molt_root / "frontend"]
    for cli_file in sorted(cli_root.glob("*.py"), key=lambda candidate: str(candidate)):
        if cli_file.name in _POST_LOWERING_ORTHOGONAL_CLI_BASENAMES:
            continue
        paths.append(cli_file)
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
