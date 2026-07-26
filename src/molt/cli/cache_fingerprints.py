from __future__ import annotations

import functools
import hashlib
import json
import os
import pathlib
from collections.abc import Iterator
from contextlib import contextmanager, suppress
from contextvars import ContextVar
from pathlib import Path
from typing import Sequence

from molt.cli.compiler_metadata import (
    _compiler_clean_pathspec_source_state,
    _compiler_root,
    _rustc_version,
)
from molt.cli.default_paths import _default_molt_cache
from molt.cli.file_hashing import (
    _hash_source_tree_metadata,
    _sha256_file,
    _source_fingerprint_files,
)
from molt.cli.python_import_resolution import (
    LocalPythonModuleResolver,
    PythonImportPolicy,
    local_import_dependencies,
)
from molt.cli.runtime_source_closure import runtime_source_paths


_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION = "source-tree-v3"
_BACKEND_FACADE_CRATE = Path("runtime/molt-backend")
_BACKEND_CACHE_ALL_FEATURES = (
    "cbor",
    "egraphs",
    "free-threaded",
    "jemalloc",
    "llvm",
    "luau-backend",
    "mlx",
    "native-backend",
    "polly",
    "rust-backend",
    "wasm-backend",
)


def _backend_source_feature_names(project_root: Path) -> tuple[str, ...]:
    from molt.cli.cargo_source_closure import _read_cargo_manifest

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
    from molt.cli.cargo_source_closure import _cargo_crate_source_closure

    return _cargo_crate_source_closure(
        project_root=project_root,
        crate_root=project_root / _BACKEND_FACADE_CRATE,
        crate_features=backend_features,
        extra_source_paths=(project_root / "Cargo.toml", project_root / "Cargo.lock"),
    )


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
#
#   * ``_intrinsic_symbols.py`` -- generated intrinsic symbol table consumed by
#     ``_wasm_runtime_exports`` (itself a load-bearing frontend input); editing it
#     changes what the exports helper returns, so it is a transitive lowering
#     input and must be hashed even though nothing under ``frontend/`` imports it
#     directly.
#   * ``_runtime_feature_gates.py`` -- generated feature-gate table. It is
#     consumed on the link/required-features path rather than proven on the
#     ast->TIR path, but it is a ``feature-gate`` authority and is retained
#     conservatively (a rarely-edited generated table; keeping it costs no
#     realistic cold-starts while removing any miscompile ambiguity).
_FRONTEND_AUX_SOURCE_RELPATHS: tuple[str, ...] = (
    "type_facts.py",
    "capabilities.py",
    "capability_manifest.py",
    "compat.py",
    "_wasm_runtime_exports.py",
    "_intrinsic_symbols.py",
    "_runtime_feature_gates.py",
)


# Roots of the frontend lowering computation. Reachability starts here and
# follows module-level ``molt`` imports; anything not reached provably never runs
# while a module is lowered and so cannot change the lowering result.
#
#   * ``frontend/`` -- the whole Python->TIR frontend (visitors, sema, lowering),
#     kept wholesale because its subpackages import one another.
#   * ``compiler_analysis/`` -- the compiler analysis package (static-truth,
#     native-support slicing, backend-IR / TIR fact-graph helpers). ``frontend/``
#     imports its lowering-facing submodules directly and it is a self-contained
#     analysis package (its only external ``molt`` import is *into* ``frontend/``),
#     so it is kept wholesale as a lowering root -- keeping the IR/TIR-adjacent
#     helpers in scope rather than relying on a package-``__init__`` re-export
#     edge that the refined resolver (below) no longer follows.
#   * ``cli/frontend_*.py`` and ``cli/module_*.py`` -- the CLI-level frontend /
#     module-graph drivers that invoke and cache the frontend (frontend pipeline,
#     workers, module resolution / graph / source / cache authorities).
#
# These are a structural naming rule, not a hand-maintained membership list: a new
# ``frontend_*`` / ``module_*`` driver is picked up automatically, and a new
# backend/link/cargo file is excluded automatically because it is not a seed and
# is not reachable from one.
_LOWERING_SCOPE_SEED_CLI_PREFIXES: tuple[str, ...] = ("frontend_", "module_")
_LOWERING_SCOPE_SOURCE_FILES_CACHE_SCHEMA_VERSION = 1


def _lowering_scope_cache_key(
    project_root: Path,
    seed_clean_state: dict[str, str | int],
) -> str:
    payload = {
        "version": _LOWERING_SCOPE_SOURCE_FILES_CACHE_SCHEMA_VERSION,
        "project_root": str(project_root.resolve()),
        "seed_clean_state": seed_clean_state,
    }
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def _lowering_scope_cache_path(
    project_root: Path,
    seed_clean_state: dict[str, str | int],
) -> Path:
    digest = _lowering_scope_cache_key(project_root, seed_clean_state)
    return (
        _default_molt_cache()
        / "frontend_lowering_scope"
        / digest[:2]
        / f"{digest}.json"
    )


def _read_lowering_scope_source_files_cache(
    project_root: Path,
    seed_clean_state: dict[str, str | int],
) -> tuple[str, ...] | None:
    cache_path = _lowering_scope_cache_path(project_root, seed_clean_state)
    try:
        payload = json.loads(cache_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if (
        not isinstance(payload, dict)
        or payload.get("version") != _LOWERING_SCOPE_SOURCE_FILES_CACHE_SCHEMA_VERSION
        or payload.get("seed_clean_state") != seed_clean_state
    ):
        return None
    files = payload.get("files")
    full_clean_state = payload.get("full_clean_state")
    if (
        not isinstance(files, list)
        or not all(isinstance(item, str) for item in files)
        or not isinstance(full_clean_state, dict)
    ):
        return None
    current_full_state = _compiler_clean_pathspec_source_state(
        project_root,
        _lowering_scope_clean_path_keys(project_root, tuple(files)),
    )
    if current_full_state != full_clean_state:
        return None
    return tuple(files)


def _write_lowering_scope_source_files_cache(
    project_root: Path,
    seed_clean_state: dict[str, str | int],
    full_clean_state: dict[str, str | int],
    files: tuple[str, ...],
) -> None:
    cache_path = _lowering_scope_cache_path(project_root, seed_clean_state)
    payload = {
        "version": _LOWERING_SCOPE_SOURCE_FILES_CACHE_SCHEMA_VERSION,
        "seed_clean_state": seed_clean_state,
        "full_clean_state": full_clean_state,
        "files": list(files),
    }
    tmp_path = cache_path.with_suffix(cache_path.suffix + f".{os.getpid()}.tmp")
    try:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        tmp_path.write_text(
            json.dumps(payload, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        tmp_path.replace(cache_path)
    except OSError:
        return
    finally:
        with suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


_FRONTEND_LOWERING_IMPORT_POLICY = PythonImportPolicy(
    module_level_only=True,
    include_parent_packages=False,
    fail_on_nonliteral_dynamic_import=False,
    allowed_prefix="molt",
)


def _lowering_scope_seed_paths(project_root: Path) -> tuple[Path, ...]:
    molt_root = project_root / "src" / "molt"
    frontend_root = molt_root / "frontend"
    compiler_analysis_root = molt_root / "compiler_analysis"
    cli_root = molt_root / "cli"
    paths: list[Path] = []
    if frontend_root.exists():
        paths.append(frontend_root)
    if compiler_analysis_root.exists():
        paths.append(compiler_analysis_root)
    if cli_root.exists():
        for source in cli_root.glob("*.py"):
            if source.name.startswith(_LOWERING_SCOPE_SEED_CLI_PREFIXES):
                paths.append(source.resolve())
    paths.extend(molt_root / relpath for relpath in _FRONTEND_AUX_SOURCE_RELPATHS)
    return tuple(dict.fromkeys(paths))


def _lowering_scope_clean_path_keys(
    project_root: Path,
    reached_files: tuple[str, ...],
) -> tuple[str, ...]:
    molt_root = project_root / "src" / "molt"
    frontend_root = (molt_root / "frontend").resolve()
    keys: list[str] = [str(frontend_root)]
    for path_str in reached_files:
        source = pathlib.Path(path_str)
        if frontend_root == source or frontend_root in source.parents:
            continue
        keys.append(path_str)
    keys.extend(str(molt_root / relpath) for relpath in _FRONTEND_AUX_SOURCE_RELPATHS)
    return tuple(dict.fromkeys(keys))


@functools.lru_cache(maxsize=64)
def _lowering_scope_source_files_cached(project_root_str: str) -> tuple[str, ...]:
    """Module-level import closure of the frontend lowering seeds.

    Returns every ``molt``-owned source file reachable by following module-level
    imports from the frontend / ``compiler_analysis`` / frontend-driver seeds.
    Because a ``from PKG import submodule`` edge is attributed to the submodule it
    names rather than to ``PKG/__init__`` (see
    ``_module_level_molt_import_targets``), importing a frontend driver no longer
    drags ``cli/__init__``'s command / extension-seal / daemon / package-registry
    layer into the closure: those command-orchestration files are neither seeds
    nor reachable from one, so editing them leaves the persisted analysis /
    lowering / import-graph fingerprints unchanged (fixing the chronic
    cross-session cold-start where every landing invalidated the whole cache).

    The bias remains strictly toward inclusion: the whole ``frontend/`` and
    ``compiler_analysis/`` packages are seeded wholesale and the shared aux
    semantic files (incl. the generated intrinsic / feature-gate tables) are
    force-included, so a lowering-relevant file is never dropped -- the worst case
    is a spurious cold-start, never a stale lowering. The set is cached per
    project root (mirroring ``_frontend_tooling_source_paths``); the per-module
    cache's own source-stat + ``context_digest`` gate remains the correctness
    authority for individual module edits.
    """
    project_root = pathlib.Path(project_root_str)
    src_root = (project_root / "src").resolve()
    resolver = LocalPythonModuleResolver((src_root,))
    molt_root = src_root / "molt"
    cli_root = molt_root / "cli"
    seed_clean_state = _compiler_clean_pathspec_source_state(
        project_root,
        tuple(str(path) for path in _lowering_scope_seed_paths(project_root)),
    )
    if seed_clean_state is not None:
        cached = _read_lowering_scope_source_files_cache(
            project_root,
            seed_clean_state,
        )
        if cached is not None:
            return cached

    seeds: set[Path] = set()
    frontend_root = molt_root / "frontend"
    if frontend_root.exists():
        for source in frontend_root.rglob("*.py"):
            seeds.add(source.resolve())
    compiler_analysis_root = molt_root / "compiler_analysis"
    if compiler_analysis_root.exists():
        for source in compiler_analysis_root.rglob("*.py"):
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
        try:
            dependencies = local_import_dependencies(
                current,
                resolver,
                _FRONTEND_LOWERING_IMPORT_POLICY,
            )
        except ValueError:
            dependencies = set()
        for source in dependencies:
            if source not in reached:
                pending.append(source)
    result = tuple(sorted(str(path) for path in reached))
    if seed_clean_state is not None:
        full_clean_state = _compiler_clean_pathspec_source_state(
            project_root,
            _lowering_scope_clean_path_keys(project_root, result),
        )
        if full_clean_state is not None:
            _write_lowering_scope_source_files_cache(
                project_root,
                seed_clean_state,
                full_clean_state,
                result,
            )
    return result


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
_SOURCE_TREE_FINGERPRINT_TRANSACTION: ContextVar[dict[tuple[str, ...], str] | None] = (
    ContextVar("_SOURCE_TREE_FINGERPRINT_TRANSACTION", default=None)
)


@contextmanager
def _source_tree_fingerprint_transaction() -> Iterator[None]:
    """Share content-complete source-tree fingerprints within one build command.

    The transaction cache is deliberately scoped: long-lived processes still
    recompute fingerprints between build requests, so in-process edits are
    observed. Within a single ``molt build`` invocation, every frontend cache key
    asks the same immutable tooling question dozens of times; computing it once
    removes that duplicate authority without weakening byte-level invalidation
    outside the command boundary.
    """

    current = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    if current is not None:
        yield
        return
    token = _SOURCE_TREE_FINGERPRINT_TRANSACTION.set({})
    try:
        yield
    finally:
        _SOURCE_TREE_FINGERPRINT_TRANSACTION.reset(token)


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
    if (
        len(_SOURCE_TREE_CONTENT_DIGEST_CACHE)
        >= _SOURCE_TREE_CONTENT_DIGEST_CACHE_LIMIT
    ):
        _SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    _SOURCE_TREE_CONTENT_DIGEST_CACHE[cache_key] = digest
    return digest


def _source_tree_clean_pathspec_signature(
    root: Path,
    path_keys: tuple[str, ...],
) -> tuple[str, ...] | None:
    clean_state = _compiler_clean_pathspec_source_state(root, path_keys)
    if clean_state is None:
        return None
    return (
        "git_clean_pathspec="
        + json.dumps(clean_state, sort_keys=True, separators=(",", ":")),
    )


def _source_tree_cache_fingerprint(
    *,
    root: Path,
    source_paths: Sequence[Path],
    scope: str,
    extra_fingerprint_inputs: str,
) -> str:
    path_keys = _source_fingerprint_path_keys(source_paths)
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    transaction_key = (
        str(root.resolve()),
        scope,
        extra_fingerprint_inputs,
        *path_keys,
    )
    if transaction is not None:
        cached = transaction.get(transaction_key)
        if cached is not None:
            return cached
    clean_signature = _source_tree_clean_pathspec_signature(root, path_keys)
    if clean_signature is None:
        content_signature = _source_tree_content_signature(root, path_keys)
    else:
        content_signature = clean_signature
    content_digest = _source_tree_content_digest(
        root,
        path_keys,
        content_signature,
        scope,
        extra_fingerprint_inputs,
    )
    # The content digest already commits to schema, scope, root-relative paths,
    # extra inputs, and every tracked byte. Installation roots and stat metadata
    # are provenance, not compiler semantics.
    digest = content_digest
    if transaction is not None:
        transaction[transaction_key] = digest
    return digest


def _selected_source_features(features: Sequence[str]) -> tuple[str, ...]:
    return tuple(sorted({feature for feature in features if feature}))


def _cache_fingerprint(
    *,
    backend_features: Sequence[str] | None = None,
    runtime_features: Sequence[str] | None = None,
    include_runtime_sources: bool = True,
) -> str:
    root = _compiler_root()
    rustc_info = _rustc_version() or ""
    rustflags = os.environ.get("RUSTFLAGS", "")
    # Hash source trees, not backend binaries: binary fingerprints over-invalidate
    # on incremental rebuilds even when source semantics are unchanged.
    selected_backend_features = (
        _backend_source_feature_names(root)
        if backend_features is None
        else _selected_source_features(backend_features)
    )
    source_paths = _backend_source_paths(root, selected_backend_features)
    if include_runtime_sources:
        if runtime_features is None:
            source_paths += runtime_source_paths(root)
        else:
            source_paths += runtime_source_paths(
                root,
                runtime_features=_selected_source_features(runtime_features),
            )
    runtime_feature_contract = (
        "all-source-features"
        if runtime_features is None
        else ",".join(_selected_source_features(runtime_features))
    )
    return _source_tree_cache_fingerprint(
        root=root,
        source_paths=source_paths,
        scope="compiler-runtime-backend",
        extra_fingerprint_inputs=(
            f"rustc:{rustc_info}\n"
            f"rustflags:{rustflags}\n"
            f"backend_features:{','.join(selected_backend_features)}\n"
            f"runtime_features:{runtime_feature_contract}\n"
        ),
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
