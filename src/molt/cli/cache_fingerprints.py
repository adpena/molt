from __future__ import annotations

import enum
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
from molt.host_tool_products import HostToolProduct
from molt.cli.python_source_closure import local_python_import_graph_transaction


if TYPE_CHECKING:
    from molt.cli.module_source import PythonSourceSnapshot
    from molt.cli.rust_cpu_policy import RustCpuPolicy
    from molt.cli.source_record_store import SourceRecordStore


_CACHE_SOURCE_FINGERPRINT_SCHEMA_VERSION = "source-tree-v3"


def _backend_crate_source_closure(
    project_root: Path,
    backend_features: tuple[str, ...],
    *,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> list[Path]:
    from molt.cli.cargo_source_closure import _cargo_crate_source_closure

    return _cargo_crate_source_closure(
        project_root=project_root,
        crate_root=project_root / "runtime" / product.value,
        crate_features=backend_features,
        # Each crate in the closure adds the files outside it that its build
        # reads (package.metadata.molt.build-inputs): molt-ir's catalog reaches
        # every product built on molt-ir, the backend's and the runner's alike.
        extra_source_paths=(project_root / "Cargo.toml",),
    )


def _backend_source_paths(
    project_root: Path,
    backend_features: tuple[str, ...] = (),
    *,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> list[Path]:
    # Reachable manifests own topology. Re-read their admitted content instead
    # of caching a graph behind a hand-enumerated manifest/stat stamp.
    return _backend_crate_source_closure(
        project_root, tuple(sorted(set(backend_features))), product=product
    )


def _backend_source_identity_inputs(
    project_root: Path,
    source_paths: Sequence[Path],
    *,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> tuple[list[Path], str]:
    """Replace the workspace lockfile with its compiler dependency identity."""
    from molt.cli.cargo_source_closure import _cargo_locked_dependency_digest

    lock_path = (project_root / "Cargo.lock").resolve()
    paths = [path for path in source_paths if path.resolve() != lock_path]
    return paths, _cargo_locked_dependency_digest(
        project_root, project_root / "runtime" / product.value
    )


@functools.lru_cache(maxsize=128)
def _frontend_tooling_source_paths_cached(project_root_str: str) -> tuple[Path, ...]:
    project_root = pathlib.Path(project_root_str)
    molt_root = _compiler_python_source_root(project_root) / "molt"
    return (
        molt_root / "cli",
        molt_root / "frontend",
        molt_root / "builtin_callables.py",
        molt_root / "backend_environment.py",
        molt_root / "backend_environment.json",
        molt_root / "backend_protocol.py",
        molt_root / "backend_daemon_protocol_version.txt",
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


class _FrontendInputRole(enum.Enum):
    """Why a compiler source that frontend code imports is in or out of the key.

    The frontend semantic fingerprint keys every persisted frontend record:
    module analysis, module lowering, import scans and intrinsic facts. It
    commits to the code that maps a record's keyed inputs to its content, and
    to nothing else (docs/design/frontend_semantic_fingerprint.md).
    """

    # In the key. Its code computes, encodes or decodes the content of a
    # frontend record, or computes an input to one that the record key does
    # not carry as a value.
    PRODUCER = "producer"
    # In the key. A compiler-owned guest source whose bytes the GPU
    # descriptor-origin check compares with lowered metadata.
    REFERENCE = "reference"
    # Out. A package ``__init__`` that runs because a submodule is imported.
    # It imports no Molt module and binds no value that a producer reads.
    PACKAGE = "package"
    # Out. Computes which record to read: fingerprints, digests and input
    # custody. Never what a record holds.
    IDENTITY = "identity"
    # Out. Moves opaque record bytes: atomic publication, the record store and
    # its size bound. A record format change bumps that record's schema.
    STORAGE = "storage"
    # Out. Diagnostics, progress, timings, input reports and failure text.
    # None of it is stored in a record.
    REPORTING = "reporting"
    # Out. Request types, admission and refusal, and host, process, path and
    # toolchain facts around the frontend. A value from it that reaches a
    # frontend computation is a field of that record's key: target Python,
    # parse codec, type-hint policy, stdlib allowlist, known and direct-call
    # modules, native exports or the namespace premise.
    BUILD_ENVIRONMENT = "build_environment"

    @property
    def keyed(self) -> bool:
        return self in (_FrontendInputRole.PRODUCER, _FrontendInputRole.REFERENCE)


_P = _FrontendInputRole.PRODUCER
_BUILD = _FrontendInputRole.BUILD_ENVIRONMENT

# Every compiler source that a frontend producer imports, with its role. Paths
# are relative to ``src/molt``; a directory entry covers its whole tree, data
# files included. tests/cli/test_cli_lowering_cache_tooling_scope.py fails when
# a producer imports an undeclared module, a declared path is missing, or a
# non-producer entry has no producer importer. A new dependency is classified
# when it is written, as rustc classifies every option [TRACKED] or
# [UNTRACKED]. Keep a module in the key when its role is in doubt: a key that
# is too narrow reuses a stale lowering.
_FRONTEND_COMPILER_INPUTS: Mapping[str, _FrontendInputRole] = MappingProxyType(
    {
        # The Python-to-TIR frontend and the analyses it runs, kept whole.
        "frontend": _P,
        "compiler_analysis": _P,
        # Drivers that compute, scan, encode and validate frontend records.
        "cli/frontend_execution.py": _P,
        "cli/frontend_integration.py": _P,
        "cli/frontend_parallel.py": _P,
        "cli/frontend_pipeline.py": _P,
        "cli/frontend_worker.py": _P,
        "cli/module_cache.py": _P,
        "cli/module_dependencies.py": _P,
        "cli/module_graph.py": _P,
        "cli/module_graph_cache.py": _P,
        "cli/module_graph_discovery.py": _P,
        "cli/module_import_scanner.py": _P,
        "cli/module_registry.py": _P,
        "cli/module_resolution.py": _P,
        "cli/module_source.py": _P,
        "cli/module_stdlib_policy.py": _P,
        "cli/python_import_resolution.py": _P,
        "cli/python_import_topology.py": _P,
        "cli/models.py": _P,
        # Defaults (stdlib profile names) that drivers read as constants.
        "cli/config_resolution.py": _P,
        # Record codecs: JSON values, class identities, lowering records.
        "cli/cache_keys.py": _P,
        "cli/function_records.py": _P,
        "cli/function_references.py": _P,
        # Generated tables and semantic helpers that lowering reads.
        "_intrinsic_symbols.py": _P,
        "_wasm_abi_generated.py": _P,
        "builtin_callables.py": _P,
        "compat.py": _P,
        "exact_json.py": _P,
        # The path every emitted module location reports (HF-372).
        "logical_source_paths.py": _P,
        "native_callable_abi.py": _P,
        "native_callable_exports.py": _P,
        "opcode_literal_payloads.py": _P,
        "python_module_names.py": _P,
        "python_private_names.py": _P,
        "stdlib_intrinsic_policy.py": _P,
        "target_python.py": _P,
        "type_facts.py": _P,
        # Target facts behind graph decisions such as is_wasm_target.
        "compilation_target.py": _P,
        "llvm_linker_roles.py": _P,
        "native_target.py": _P,
        "native_target_shape.py": _P,
        # GPU_PYTHON_BODY_REFERENCES in frontend/lowering/gpu_kernel_descriptor.py.
        "gpu/__init__.py": _FrontendInputRole.REFERENCE,
        "stdlib/struct.py": _FrontendInputRole.REFERENCE,
        "__init__.py": _FrontendInputRole.PACKAGE,
        "cli/__init__.py": _FrontendInputRole.PACKAGE,
        "cli/cache_fingerprints.py": _FrontendInputRole.IDENTITY,
        "cli/source_extension_input_custody.py": _FrontendInputRole.IDENTITY,
        "file_hashing.py": _FrontendInputRole.IDENTITY,
        "toolchain_identity.py": _FrontendInputRole.IDENTITY,
        "cli/atomic_io.py": _FrontendInputRole.STORAGE,
        "cli/json_cache.py": _FrontendInputRole.STORAGE,
        "cli/source_record_store.py": _FrontendInputRole.STORAGE,
        "file_publication.py": _FrontendInputRole.STORAGE,
        "cli/binary_image_analysis.py": _FrontendInputRole.REPORTING,
        "cli/build_diagnostics.py": _FrontendInputRole.REPORTING,
        "cli/output.py": _FrontendInputRole.REPORTING,
        "cli/progress.py": _FrontendInputRole.REPORTING,
        "dx.py": _FrontendInputRole.REPORTING,
        "input_report.py": _FrontendInputRole.REPORTING,
        "backend_environment.py": _BUILD,
        "backend_executable_names.py": _BUILD,
        "browser_asset_closure.py": _BUILD,
        "capability_policy.py": _BUILD,
        "cli/build_output_layout.py": _BUILD,
        "cli/build_request.py": _BUILD,
        # Builds the import admission policy from the environment. Discovery
        # applies it live (its rules live in cli/models.py); records see its
        # effects only as the keyed premise and context module sets.
        "cli/external_native.py": _BUILD,
        "cli/installed_runtime.py": _BUILD,
        "cli/project_roots.py": _BUILD,
        "cli/rust_cpu_policy.py": _BUILD,
        "cli/typecheck.py": _BUILD,
        "cli/wasm_toolchain.py": _BUILD,
        "compiler_process.py": _BUILD,
        "default_paths.py": _BUILD,
        "host_capacity.py": _BUILD,
        "source_root.py": _BUILD,
        "verified_subset.py": _BUILD,
    }
)
del _P, _BUILD

_FRONTEND_REFERENCE_SOURCES: tuple[str, ...] = tuple(
    relative
    for relative, role in _FRONTEND_COMPILER_INPUTS.items()
    if role is _FrontendInputRole.REFERENCE
)


@dataclass(frozen=True, slots=True)
class _SourceFingerprintInputs:
    """Operation-owned canonical paths, with hashes already captured by discovery."""

    paths: tuple[Path, ...]
    source_sha256: Mapping[Path, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(
            self, "source_sha256", MappingProxyType(dict(self.source_sha256))
        )

    @classmethod
    def from_paths(cls, paths: Iterable[Path]) -> _SourceFingerprintInputs:
        return cls(tuple(sorted({path.resolve() for path in paths})))


def _frontend_semantic_tooling_sources(project_root: Path) -> _SourceFingerprintInputs:
    """The keyed entries of ``_FRONTEND_COMPILER_INPUTS`` and their digests.

    Each keyed file is read once. Nothing is parsed or analyzed: a fresh
    process pays one read and one SHA-256 per file. A keyed path that is absent
    is left out, so a source that appears later changes the key.
    """
    molt_root = (_compiler_python_source_root(project_root) / "molt").resolve()
    paths: list[Path] = []
    source_sha256: dict[Path, str] = {}
    for relative, role in _FRONTEND_COMPILER_INPUTS.items():
        if not role.keyed:
            continue
        path = molt_root / relative
        files = _source_fingerprint_files(path)
        if not files:
            continue
        paths.append(path)
        for item in files:
            source_sha256[item] = _file_content_signature(item)
    return _SourceFingerprintInputs(tuple(sorted(paths)), source_sha256)


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

        if relative_path not in _FRONTEND_REFERENCE_SOURCES:
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
    # Each path resolves once per operation; a graph keys one module under
    # many import contexts and scans it under many premises.
    resolved_paths: dict[str, Path] = field(default_factory=dict)
    # Pin one connection owner through the operation, including nested calls.
    source_records: SourceRecordStore | None = None


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


def _operation_resolved(path: str | os.PathLike[str]) -> Path:
    """Resolve ``path`` once per frontend operation, never across them.

    ``realpath`` stats every component of the path; one edit-rebuild resolves
    each scanned module under every premise it is scanned for.
    """
    key = os.fspath(path)
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    if transaction is None:
        return Path(key).resolve()
    resolved = transaction.resolved_paths.get(key)
    if resolved is None:
        resolved = transaction.resolved_paths[key] = Path(key).resolve()
    return resolved


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
    rust_cpu: RustCpuPolicy,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> str:
    """The identity of one compiler product's sources, Cargo build and runtime.

    ``product`` selects the host tool whose crate closure and locked
    dependencies are hashed: a JIT run executes ``HostToolProduct.JIT``, so its
    cached launch must change when the runner's sources do.
    """
    from molt.cli.compiler_identity import (
        CompilerIdentityError,
        backend_build_admission,
        compiler_cargo_profile,
        installed_compiler_admission,
    )
    from molt.backend_executable_names import DEFAULT_BACKEND_FEATURES

    try:
        root = _compiler_root()
        source = os.environ if env is None else env
        profile = cargo_profile or compiler_cargo_profile(source)
        # Without explicit features this is the default native (cranelift)
        # compiler; the process environment never selects the backend.
        selected_features = _selected_source_features(
            DEFAULT_BACKEND_FEATURES if backend_features is None else backend_features
        )
        installed = installed_compiler_admission(root, selected_features, profile)
        if installed is not None:
            return installed.fingerprint
        admission = backend_build_admission(
            root, selected_features, profile, source, rust_cpu=rust_cpu, product=product
        )
        source_paths, lock_digest = _backend_source_identity_inputs(
            root,
            _backend_source_paths(root, selected_features, product=product),
            product=product,
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
    """Tooling fingerprint for the persisted frontend records.

    It digests the keyed entries of ``_FRONTEND_COMPILER_INPUTS``, so an edit
    to toolchain, publication, diagnostics or backend code leaves every record
    valid. The distinct ``scope`` tag keeps this digest namespace-separated
    from the broad ``frontend-tooling`` fingerprint.
    """
    return _frontend_semantic_tooling_snapshot().fingerprint


def _frontend_semantic_tooling_snapshot() -> _FrontendSemanticSourceSnapshot:
    # Look up the operation snapshot before reading any source: one operation
    # reads each keyed file once.
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
        # that generation authority without reading its sources again.
        # Domain separation is for this cache consumer, not runtime metadata.
        snapshot = _FrontendSemanticSourceSnapshot(
            root=root,
            source_paths=(),
            reference_sha256=MappingProxyType(
                {
                    entry["path"][len("src/molt/") :]: entry["sha256"]
                    for entry in installed.compiler.files
                    if entry["path"].startswith("src/molt/")
                    and entry["path"][len("src/molt/") :] in _FRONTEND_REFERENCE_SOURCES
                }
            ),
            fingerprint=_compute_source_tree_content_digest(
                (), "frontend-semantic-tooling", f"installed:{installed.fingerprint}\n"
            ),
        )
    else:
        inputs = _frontend_semantic_tooling_sources(root)
        molt_root = (_compiler_python_source_root(root) / "molt").resolve()
        # The captured digests are the identity; Git state never enters it.
        snapshot = _FrontendSemanticSourceSnapshot(
            root=root,
            source_paths=inputs.paths,
            reference_sha256=MappingProxyType(
                {
                    relative: inputs.source_sha256.get(
                        molt_root / relative, "unreadable"
                    )
                    for relative in _FRONTEND_REFERENCE_SOURCES
                }
            ),
            fingerprint=_compute_source_tree_content_digest(
                _source_tree_content_signature(
                    root.resolve(),
                    tuple(str(path) for path in inputs.paths),
                    inputs.source_sha256,
                ),
                "frontend-semantic-tooling",
                "",
            ),
        )
    if transaction is not None:
        transaction.frontend_semantic_sources[root] = snapshot
    return snapshot
