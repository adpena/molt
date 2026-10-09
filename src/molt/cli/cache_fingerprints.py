from __future__ import annotations

import functools
import hashlib
import json
import os
import pathlib
from collections.abc import Iterable, Generator, Mapping
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import TYPE_CHECKING, Sequence

from molt.cli.compiler_metadata import (
    _compiler_clean_pathspec_source_state,
    _compiler_root,
    _compiler_python_source_root,
)
from molt.file_hashing import (
    _sha256_file,
    _source_fingerprint_files,
)
from molt.cli.python_import_resolution import (
    PythonImportPolicy,
)
from molt.cli.python_source_closure import (
    LocalPythonSourceClosure,
    local_python_import_closure,
    local_python_import_graph_transaction,
)


if TYPE_CHECKING:
    from molt.cli.module_source import PythonSourceSnapshot


_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION = "source-tree-v3"
_BACKEND_FACADE_CRATE = Path("runtime/molt-backend")


def _backend_crate_source_closure(
    project_root: Path,
    backend_features: tuple[str, ...],
) -> list[Path]:
    from molt.cli.cargo_source_closure import _cargo_crate_source_closure

    return _cargo_crate_source_closure(
        project_root=project_root,
        crate_root=project_root / _BACKEND_FACADE_CRATE,
        crate_features=backend_features,
        extra_source_paths=(
            project_root / "Cargo.toml",
            # molt-ir embeds this catalog from outside its Cargo crate root.
            project_root / "src" / "molt" / "backend_environment.json",
        ),
    )


def _backend_source_paths(
    project_root: Path,
    backend_features: tuple[str, ...] = (),
) -> list[Path]:
    # Reachable manifests own topology. Re-read their admitted content instead
    # of caching a graph behind a hand-enumerated manifest/stat stamp.
    return _backend_crate_source_closure(
        project_root, tuple(sorted(set(backend_features)))
    )


def _backend_source_identity_inputs(
    project_root: Path, source_paths: Sequence[Path]
) -> tuple[list[Path], str]:
    """Replace the workspace lockfile with its compiler dependency identity."""
    from molt.cli.cargo_source_closure import _cargo_locked_dependency_digest

    lock_path = (project_root / "Cargo.lock").resolve()
    paths = [path for path in source_paths if path.resolve() != lock_path]
    return paths, _cargo_locked_dependency_digest(
        project_root, project_root / _BACKEND_FACADE_CRATE
    )


@functools.lru_cache(maxsize=128)
def _frontend_tooling_source_paths_cached(project_root_str: str) -> tuple[Path, ...]:
    project_root = pathlib.Path(project_root_str)
    molt_root = _compiler_python_source_root(project_root) / "molt"
    return (
        molt_root / "cli",
        molt_root / "frontend",
        molt_root / "backend_environment.py",
        molt_root / "backend_environment.json",
        molt_root / "type_facts.py",
        molt_root / "capabilities.py",
        molt_root / "capability_manifest.py",
        molt_root / "capability_policy.py",
        molt_root / "_host_capabilities_generated.py",
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
    "capability_policy.py",
    "_host_capabilities_generated.py",
    "compat.py",
    "_wasm_runtime_exports.py",
    "_intrinsic_symbols.py",
    "_runtime_feature_gates.py",
    # Hardware descriptor origin is compared with these compiler-owned bodies.
    # Installed releases already bind them in the source-wide identity.
    "gpu/__init__.py",
    "stdlib/struct.py",
)


# Roots of the frontend lowering computation. Reachability starts here and
# follows module-level ``molt`` imports. If a literal-relative anchor becomes
# unknown, every admitted local owner can execute before a suffix fails; the
# shared closure then captures the complete ``molt`` source domain.
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
# backend/link/cargo file is excluded when no reachable edge or unknown-relative
# coverage obligation includes it. Coverage deliberately broadens invalidation.
_LOWERING_SCOPE_SEED_CLI_PREFIXES: tuple[str, ...] = ("frontend_", "module_")


_FRONTEND_LOWERING_IMPORT_POLICY = PythonImportPolicy(
    module_level_only=True,
    include_parent_packages=False,
    # Persisted lowering results require complete eager dependency identity.
    # Deferred bodies stay outside this projection. Unknown literal-relative
    # anchors require full local byte coverage; nonliteral names still require
    # the existing exact dynamic contract rather than partial candidates.
    fail_on_nonliteral_dynamic_import=True,
    allowed_prefix="molt",
    purpose="source_dependency",
    unknown_relative_sources="local_inventory",
)


def _lowering_scope_seed_paths(project_root: Path) -> tuple[Path, ...]:
    molt_root = _compiler_python_source_root(project_root) / "molt"
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
                paths.append(source)
    paths.extend(molt_root / relpath for relpath in _FRONTEND_AUX_SOURCE_RELPATHS)
    return tuple(dict.fromkeys(path for path in paths if path.exists()))


def _lowering_scope_source_closure(project_root: Path) -> LocalPythonSourceClosure:
    """Lowering's policy projection of the shared byte-keyed dependency graph.

    Whole frontend/analysis packages, frontend/module drivers and shared semantic
    inputs remain seeds. Grouped fromlist requests prefer actual named submodules
    without importing package aggregates. New edges/topology are discovered on
    every build; only the explicit build transaction may reuse a whole closure.
    Unknown literal-relative anchors widen source coverage to all admitted local
    owners, including sources that can execute before the import fails.
    """
    return local_python_import_closure(
        project_root,
        _lowering_scope_seed_paths(project_root),
        policy=_FRONTEND_LOWERING_IMPORT_POLICY,
        search_roots=(_compiler_python_source_root(project_root),),
    )


@dataclass(frozen=True, slots=True)
class _SourceFingerprintInputs:
    """Operation-owned canonical paths, with hashes already captured by discovery."""

    paths: tuple[Path, ...]
    source_sha256: Mapping[Path, str] = field(default_factory=dict)
    topology_digest: str = ""

    def __post_init__(self) -> None:
        object.__setattr__(
            self, "source_sha256", MappingProxyType(dict(self.source_sha256))
        )

    @classmethod
    def from_paths(cls, paths: Iterable[Path]) -> _SourceFingerprintInputs:
        return cls(tuple(sorted({path.resolve() for path in paths})))


def _frontend_semantic_tooling_sources(project_root: Path) -> _SourceFingerprintInputs:
    """Source paths the persisted per-module *frontend* caches must key on.

    Derived structurally by import reachability (see
    ``_lowering_scope_source_closure``): the whole ``frontend/`` tree plus
    every ``molt``-owned file reachable from the frontend/module drivers by
    module-level import, plus the shared aux semantic files. No hand-maintained
    denylist: known requests remain reachability-scoped. An unknown literal
    relative anchor requires complete local-owner coverage and consequently
    includes backend/link/cargo, guest stdlib and GPU Python sources as bytes.
    This is an invalidation cost, not a claim that these sources execute on the
    host or a grant of guest import metadata. New files and namespace topology
    are recaptured on the next operation.

    The bias is strictly toward inclusion -- every file on the frontend import
    path is hashed, so a spurious cold-start is the worst outcome; a stale
    lowering (miscompile) cannot arise from under-scoping here. ``frontend/`` is
    returned as a directory (its subpackages import one another and are kept
    whole); reachable files under it are therefore skipped from the per-file list
    to avoid hashing them twice.
    """
    molt_root = _compiler_python_source_root(project_root) / "molt"
    frontend_root = (molt_root / "frontend").resolve()
    closure = _lowering_scope_source_closure(project_root)
    paths: list[Path] = [frontend_root]
    for source in closure.paths:
        if frontend_root == source or frontend_root in source.parents:
            continue
        paths.append(source)
    # Force-include the shared aux semantic files even if a future refactor drops
    # them from the import closure -- they are load-bearing frontend inputs.
    source_sha256 = dict(closure.source_sha256)
    for relpath in _FRONTEND_AUX_SOURCE_RELPATHS:
        source = molt_root / relpath
        # Existing aux sources have already been admitted and captured by the
        # closure. Only absent or aliased lexical paths still need resolving.
        selected = source if source in closure.source_sha256 else source.resolve()
        paths.append(selected)
        if selected not in source_sha256:
            source_sha256[selected] = _file_content_signature(selected)
    return _SourceFingerprintInputs(
        tuple(sorted(set(paths))), source_sha256, closure.topology_digest
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


@dataclass(frozen=True)
class _FrontendSemanticSourceSnapshot:
    """Resolved tooling inputs and their identity for one immutable operation."""

    root: Path
    source_paths: tuple[Path, ...]
    fingerprint: str
    reference_sha256: Mapping[str, str] = field(default_factory=dict, compare=False)
    # Captured only when a semantic consumer actually needs source contents.
    # This lives for the existing immutable operation, never process-wide.
    reference_sources: dict[str, PythonSourceSnapshot] = field(
        default_factory=dict, compare=False
    )

    def reference_source(self, relative_path: str) -> PythonSourceSnapshot:
        from molt.cli.module_source import PythonSourceSnapshot

        if relative_path not in _FRONTEND_AUX_SOURCE_RELPATHS:
            raise ValueError("reference source is outside frontend semantic inputs")
        captured = self.reference_sources.get(relative_path)
        if captured is None:
            captured = PythonSourceSnapshot.capture(
                _compiler_python_source_root(self.root) / "molt" / relative_path
            )
            if captured.sha256 != self.reference_sha256.get(relative_path):
                raise ValueError(
                    "frontend reference source changed after semantic generation admission"
                )
            self.reference_sources[relative_path] = captured
        assert isinstance(captured, PythonSourceSnapshot)
        return captured


@dataclass
class _SourceTreeFingerprintTransaction:
    fingerprints: dict[tuple[str, ...], str] = field(default_factory=dict)
    compiler_plans: dict[tuple[str, ...], object] = field(default_factory=dict)
    installed_sources: set[tuple[str, str]] = field(default_factory=set)
    cargo_documents: dict[str, object] = field(default_factory=dict)
    frontend_semantic_sources: dict[Path, _FrontendSemanticSourceSnapshot] = field(
        default_factory=dict
    )


_SOURCE_TREE_FINGERPRINT_TRANSACTION: ContextVar[
    _SourceTreeFingerprintTransaction | None
] = ContextVar("_SOURCE_TREE_FINGERPRINT_TRANSACTION", default=None)


@contextmanager
def _source_tree_fingerprint_transaction() -> Generator[None]:
    """Share immutable tooling snapshots within one frontend operation.

    Build and shared run/deploy wrappers own the outer transaction. Graph,
    analysis and cache operations enter the same reentrant authority, so their
    cache keys, validation and publication share one identity even without the
    CLI wrapper. A new operation recaptures source bytes, import topology and
    resolved ownership; no process-wide path/stat-only memo may replace that.
    Tooling must remain immutable during an operation. Application source
    payloads are not frozen here and retain their content-validation gates.
    """

    current = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    if current is not None:
        yield
        return
    transaction_token = _SOURCE_TREE_FINGERPRINT_TRANSACTION.set(
        _SourceTreeFingerprintTransaction()
    )
    try:
        with local_python_import_graph_transaction():
            yield
    finally:
        _SOURCE_TREE_FINGERPRINT_TRANSACTION.reset(transaction_token)


@contextmanager
def _fresh_compiler_identity_inputs() -> Generator[None]:
    """Force live publication checks without reusing operation snapshots."""
    transaction_token = _SOURCE_TREE_FINGERPRINT_TRANSACTION.set(
        _SourceTreeFingerprintTransaction()
    )
    try:
        with local_python_import_graph_transaction(fresh=True):
            yield
    finally:
        _SOURCE_TREE_FINGERPRINT_TRANSACTION.reset(transaction_token)


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
    source_sha256: Mapping[Path, str],
) -> tuple[str, ...]:
    signature: list[str] = []
    for path_key in path_keys:
        path = pathlib.Path(path_key)
        for item in _source_fingerprint_files(path):
            try:
                rel_text = str(item.relative_to(root))
            except ValueError:
                rel_text = str(item)
            digest = source_sha256.get(item)
            if digest is None:
                # A directory may also contain non-Python inputs or symlink
                # aliases. Reuse captured identity for the latter; retain all
                # other frontend assets as ordinary byte-keyed inputs.
                digest = source_sha256.get(item.resolve()) if source_sha256 else None
            if digest is None:
                digest = _file_content_signature(item)
            signature.append(f"{rel_text}={digest}")
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
    inputs: _SourceFingerprintInputs,
    scope: str,
    extra_fingerprint_inputs: str,
) -> str:
    path_keys = tuple(str(path) for path in inputs.paths)
    if inputs.topology_digest:
        extra_fingerprint_inputs += (
            f"\nlocal-python-topology:{inputs.topology_digest}\n"
        )
    root = root.resolve()
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    transaction_key = (
        str(root),
        scope,
        extra_fingerprint_inputs,
        *path_keys,
    )
    if transaction is not None:
        cached = transaction.fingerprints.get(transaction_key)
        if cached is not None:
            return cached
    # Captured Python identity is already paid for by dependency discovery.
    # Going through Git would add subprocesses and discard that authority; hash
    # the receipt plus any remaining assets directly. Uncaptured backend/runtime
    # trees still benefit from the clean-pathspec shortcut.
    clean_signature = (
        None
        if inputs.source_sha256
        else _source_tree_clean_pathspec_signature(root, path_keys)
    )
    if clean_signature is None:
        content_signature = _source_tree_content_signature(
            root, path_keys, inputs.source_sha256
        )
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
        transaction.fingerprints[transaction_key] = digest
    return digest


def _selected_source_features(features: Sequence[str]) -> tuple[str, ...]:
    return tuple(sorted({feature for feature in features if feature}))


def _cache_fingerprint(
    *,
    backend_features: Sequence[str] | None = None,
    runtime_features: Sequence[str] | None = None,
    include_runtime_sources: bool = True,
    env: Mapping[str, str] | None = None,
    cargo_profile: str | None = None,
) -> str:
    from molt.cli.compiler_identity import (
        CompilerIdentityError,
        backend_build_admission,
        compiler_cargo_profile,
        installed_compiler_admission,
    )
    from molt.backend_executable_names import backend_features_for_target

    try:
        root = _compiler_root()
        source = os.environ if env is None else env
        profile = cargo_profile or compiler_cargo_profile(source)
        selected_features = (
            backend_features_for_target(
                is_wasm=False,
                is_luau_transpile=False,
                is_rust_transpile=False,
                env=source,
            )
            if backend_features is None
            else _selected_source_features(backend_features)
        )
        installed = installed_compiler_admission(root, selected_features, profile)
        if installed is not None:
            return installed.fingerprint
        admission = backend_build_admission(root, selected_features, profile, source)
        source_paths, lock_digest = _backend_source_identity_inputs(
            root, _backend_source_paths(root, selected_features)
        )
        if include_runtime_sources:
            from molt.cli.runtime_source_closure import runtime_source_paths

            source_paths += runtime_source_paths(
                root,
                **(
                    {}
                    if runtime_features is None
                    else {
                        "runtime_features": _selected_source_features(runtime_features)
                    }
                ),
            )
        runtime_contract = (
            "all-source-features"
            if runtime_features is None
            else ",".join(_selected_source_features(runtime_features))
        )
        return _source_tree_cache_fingerprint(
            root=root,
            inputs=_SourceFingerprintInputs.from_paths(source_paths),
            scope="compiler-runtime-backend",
            extra_fingerprint_inputs=(
                f"build:{admission.fingerprint}\n"
                f"backend_locked_dependencies:{lock_digest}\n"
                f"runtime_features:{runtime_contract}\n"
            ),
        )
    except CompilerIdentityError:
        raise
    except (OSError, ValueError) as exc:
        raise CompilerIdentityError(f"Compiler cache identity: {exc}") from exc


def _cache_tooling_fingerprint() -> str:
    from molt.cli.compiler_identity import installed_compiler_admission

    root = _compiler_root()
    installed = installed_compiler_admission(root)
    if installed is not None:
        return installed.fingerprint
    return _source_tree_cache_fingerprint(
        root=root,
        inputs=_SourceFingerprintInputs.from_paths(
            _frontend_tooling_source_paths(root)
        ),
        scope="frontend-tooling",
        extra_fingerprint_inputs="",
    )


def _frontend_semantic_tooling_fingerprint() -> str:
    """Tooling fingerprint for the per-module frontend caches.

    Identical in construction to ``_cache_tooling_fingerprint`` but over the
    lowering-relevant scope only (see ``_frontend_semantic_tooling_sources``),
    so known import graphs exclude unrelated backend/link/daemon/cargo/toolchain
    edits. Unknown relative anchors require broader local coverage. The distinct
    ``scope`` tag keeps this digest namespace-separated from the broad
    ``frontend-tooling`` fingerprint.
    """
    return _frontend_semantic_tooling_snapshot().fingerprint


def _frontend_semantic_tooling_snapshot() -> _FrontendSemanticSourceSnapshot:
    # Look up the operation snapshot BEFORE walking seeds, resolving the import
    # closure or canonicalizing every fingerprint path. The lower-level digest
    # memo is too late to eliminate those repeated filesystem operations.
    root = _compiler_root()
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    if transaction is not None:
        snapshot = transaction.frontend_semantic_sources.get(root)
        if snapshot is not None:
            return snapshot
    from molt.cli.compiler_identity import installed_compiler_admission

    installed = installed_compiler_admission(root)
    if installed is not None:
        # The admitted release already commits to every shipped input. Reuse
        # that generation authority without walking its mutable-source graph.
        # Domain separation is for this cache consumer, not runtime metadata.
        snapshot = _FrontendSemanticSourceSnapshot(
            root=root,
            source_paths=(),
            reference_sha256=MappingProxyType(
                {
                    entry["path"][len("src/molt/") :]: entry["sha256"]
                    for entry in installed.compiler.files
                    if entry["path"].startswith("src/molt/")
                    and entry["path"][len("src/molt/") :]
                    in _FRONTEND_AUX_SOURCE_RELPATHS
                }
            ),
            fingerprint=_compute_source_tree_content_digest(
                (), "frontend-semantic-tooling", f"installed:{installed.fingerprint}\n"
            ),
        )
    else:
        inputs = _frontend_semantic_tooling_sources(root)
        snapshot = _FrontendSemanticSourceSnapshot(
            root=root,
            source_paths=inputs.paths,
            reference_sha256=MappingProxyType(
                {
                    relative: inputs.source_sha256.get(
                        _compiler_python_source_root(root) / "molt" / relative
                    )
                    or inputs.source_sha256.get(
                        (
                            _compiler_python_source_root(root) / "molt" / relative
                        ).resolve(),
                        "unreadable",
                    )
                    for relative in _FRONTEND_AUX_SOURCE_RELPATHS
                }
            ),
            fingerprint=_source_tree_cache_fingerprint(
                root=root,
                inputs=inputs,
                scope="frontend-semantic-tooling",
                extra_fingerprint_inputs="",
            ),
        )
    if transaction is not None:
        transaction.frontend_semantic_sources[root] = snapshot
    return snapshot
