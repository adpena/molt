"""One policy-keyed local Python dependency graph for compiler and tool inputs."""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Iterable, Generator, Mapping
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import asdict, dataclass, field
from pathlib import Path, PurePosixPath, PureWindowsPath
from threading import Lock
from types import MappingProxyType
import tomllib
from typing import Literal, cast

from molt.cli.atomic_io import _atomic_write_text
from molt.file_publication import PublicationDurability
from molt.default_paths import _default_molt_cache
from molt.cli.module_source import PythonSourceSnapshot
from molt.compiler_analysis.python_namespace_graph import (
    NO_GRAPH_NAMESPACE_WRITES,
    PythonGraphNamespaceWrites,
    PythonNamespacePremise,
    graph_python_namespace_writes,
)
from molt.compiler_analysis.python_namespace_writes import (
    PythonNamespaceWrite,
    decode_python_namespace_write,
    python_namespace_write_payload,
    _resolved_python_namespace_summary,
    python_package_name,
)
from molt.cli.python_import_resolution import (
    python_package_child_names,
    _local_import_binding_policy,
    LocalPythonModuleResolver,
    LocalPythonModuleSource,
    LocalPythonImportAnalysis,
    LocalPythonImportDiagnostic,
    LocalPythonImportRequest,
    LocalPythonRelativeImportObligation,
    PythonImportPolicy,
    analyze_local_imports,
    local_import_analysis_identity,
    resolve_local_import_requests,
    relative_python_module_name,
)


_EXECUTABLE_TOOL_IMPORT_POLICY = PythonImportPolicy(
    module_level_only=False,
    include_parent_packages=True,
    fail_on_nonliteral_dynamic_import=True,
    purpose="source_dependency",
    unknown_relative_sources="local_inventory",
)
_DYNAMIC_IMPORT_MANIFEST = Path("src/molt/cli/python_source_closure.toml")
_GRAPH_CACHE_SCHEMA_VERSION = 21
_GraphDomainKey = tuple[
    Path,
    tuple[Path, ...],
    tuple[Path, ...],
    Path,
    tuple[Path, bytes] | None,
]
_GraphQuery = tuple[tuple[Path, ...], PythonImportPolicy]
_ImportManifest = tuple[Path, bytes, dict[str, object]]
_GRAPH_TRANSACTION: ContextVar[_GraphTransaction | None] = ContextVar(
    "_GRAPH_TRANSACTION", default=None
)


@dataclass(frozen=True, slots=True)
class LocalPythonSourceClosure:
    """Canonical inputs and identities of the exact bytes used for discovery.

    A receipt retains neither source bytes nor ASTs. Consumers reuse its hashes,
    not a second read of paths that may already name a different generation.
    """

    paths: tuple[Path, ...]
    source_sha256: Mapping[Path, str]
    content_digest: str
    source_bytes: int
    # Nonempty only when complete local-domain coverage was required. The
    # digest commits to root order, module aliases and namespace locations.
    topology_digest: str = ""

    def __post_init__(self) -> None:
        object.__setattr__(
            self, "source_sha256", MappingProxyType(dict(self.source_sha256))
        )


@dataclass(frozen=True)
class _CoveredSourceDomain:
    paths: frozenset[Path]
    topology_digest: str


@dataclass
class _GraphDomain:
    """Physical inputs shared by query projections in one immutable operation.

    The resolver owns canonical paths and filesystem facts. Relative keys here
    are lexical projections of those paths, not a second resolution cache.
    Raw captures retain no parsed ASTs; each analysis miss uses a temporary view.
    """

    root: Path
    resolver: LocalPythonModuleResolver
    persisted_entries: dict[str, dict[str, object]]
    entries: dict[str, dict[str, object]]
    cache_pruned: bool
    snapshots: dict[Path, PythonSourceSnapshot] = field(default_factory=dict)
    relative_keys: dict[Path, str] = field(default_factory=dict)
    package_children: dict[Path, frozenset[str]] = field(default_factory=dict)
    inventories: dict[tuple[str | None, bool], _CoveredSourceDomain] = field(
        default_factory=dict
    )
    receipts: dict[_GraphQuery, LocalPythonSourceClosure] = field(default_factory=dict)

    def capture(self, path: Path) -> PythonSourceSnapshot:
        if path not in self.snapshots:
            self.snapshots[path] = self.resolver.capture_source(path)
        return self.snapshots[path]

    def relative_key(self, path: Path) -> str:
        if path not in self.relative_keys:
            self.relative_keys[path] = _relative_cache_key(self.root, path)
        return self.relative_keys[path]

    def inventory(self, policy: PythonImportPolicy) -> _CoveredSourceDomain:
        key = (policy.allowed_prefix, policy.include_parent_packages)
        covered = self.inventories.get(key)
        if covered is not None:
            return covered
        inventory = self.resolver.source_inventory(
            allowed_prefix=policy.allowed_prefix,
            include_parent_packages=policy.include_parent_packages,
        )
        paths = tuple(dict.fromkeys(item.path for item in inventory.sources))
        for path in paths:
            self.capture(path)
        topology = {
            "roots": [self.relative_key(path) for path in self.resolver.search_roots],
            "sources": [
                [item.name, self.relative_key(item.path)] for item in inventory.sources
            ],
            "packages": [
                [name, [self.relative_key(path) for path in locations]]
                for name, locations in inventory.packages
            ],
        }
        covered = _CoveredSourceDomain(
            frozenset(paths),
            hashlib.sha256(
                json.dumps(topology, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
        )
        self.inventories[key] = covered
        return covered


@dataclass
class _GraphTransaction:
    domains: dict[_GraphDomainKey, _GraphDomain] = field(default_factory=dict)


@contextmanager
def local_python_import_graph_transaction(*, fresh: bool = False) -> Generator[None]:
    """Share immutable tooling inputs and separate queries within one command."""
    if not fresh and _GRAPH_TRANSACTION.get() is not None:
        yield
        return
    previous_context = _GRAPH_TRANSACTION.set(_GraphTransaction())
    try:
        yield
    finally:
        _GRAPH_TRANSACTION.reset(previous_context)


def python_source_closure_cache_path(project_root: Path) -> Path:
    """Keep project-scoped analysis hints under the shared mutable-cache authority.

    Callers supply the resolved source root, which namespaces relative entry
    names but never becomes a write destination. Source bytes, import policy
    and analysis identity still decide reuse; cache placement grants no source
    authority.
    """
    namespace = hashlib.sha256(os.fsencode(project_root)).hexdigest()
    return _default_molt_cache() / "python_source_closure" / f"{namespace}.json"


def _read_graph_cache(
    project_root: Path,
) -> tuple[dict[str, dict[str, object]], bool]:
    cache_path = python_source_closure_cache_path(project_root)
    try:
        source = cache_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return {}, True
    try:
        payload = json.loads(source)
    except (ValueError, RecursionError):
        return {}, True
    if not isinstance(payload, dict):
        return {}, True
    if payload.get("schema_version") != _GRAPH_CACHE_SCHEMA_VERSION:
        return {}, True
    entries = payload.get("entries")
    if not isinstance(entries, dict):
        return {}, True
    retained = _retained_graph_entries(project_root, entries)
    return retained, retained != entries


def _retained_graph_entries(
    project_root: Path, entries: Mapping[str, object]
) -> dict[str, dict[str, object]]:
    retained: dict[str, dict[str, object]] = {}
    for key, value in entries.items():
        if not isinstance(key, str) or not isinstance(value, dict):
            continue
        # Cached keys never grant source authority: only fresh canonical
        # discovery can look one up. Validate spelling without resolving every
        # sibling policy's paths, then prune vanished sources with one stat.
        relative = PurePosixPath(key)
        if (
            not key
            or relative.is_absolute()
            or PureWindowsPath(key).drive
            or "\\" in key
            or ".." in relative.parts
            or relative.as_posix() != key
        ):
            continue
        try:
            if not (project_root / key).is_file():
                continue
        except OSError:
            # Cache hints are disposable; discovery owns errors for reached sources.
            continue
        retained[key] = value
    return retained


# Content-keyed analysis rows this process has derived, per project root. A
# fresh MOLT_CACHE reuses them; the persisted cache still decides every variant
# it holds, so a tampered or stale persisted row is re-analyzed, never masked.
_PROCESS_GRAPH_ENTRIES: dict[Path, dict[str, dict[str, object]]] = {}
_PROCESS_GRAPH_ENTRIES_LOCK = Lock()


def _with_process_graph_entries(
    project_root: Path, persisted: dict[str, dict[str, object]]
) -> dict[str, dict[str, object]]:
    with _PROCESS_GRAPH_ENTRIES_LOCK:
        remembered = dict(_PROCESS_GRAPH_ENTRIES.get(project_root, {}))
    merged = {
        key: dict(variants)
        for key, variants in _retained_graph_entries(project_root, remembered).items()
    }
    for key, variants in persisted.items():
        merged.setdefault(key, {}).update(variants)
    return merged


def _remember_process_graph_entries(
    project_root: Path, entries: dict[str, dict[str, object]]
) -> None:
    with _PROCESS_GRAPH_ENTRIES_LOCK:
        remembered = _PROCESS_GRAPH_ENTRIES.setdefault(project_root, {})
        for key, variants in entries.items():
            remembered.setdefault(key, {}).update(variants)


def _write_graph_cache(
    project_root: Path,
    entries: dict[str, dict[str, object]],
) -> None:
    cache_path = python_source_closure_cache_path(project_root)
    payload = {
        "schema_version": _GRAPH_CACHE_SCHEMA_VERSION,
        "entries": entries,
    }
    try:
        _atomic_write_text(
            cache_path,
            json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n",
            durability=PublicationDurability.RECONSTRUCTIBLE,
        )
    except OSError:
        return


def _relative_cache_key(project_root: Path, source: Path) -> str:
    try:
        return source.relative_to(project_root).as_posix()
    except ValueError as exc:
        raise ValueError(
            f"Python tooling source is outside project root: {source}"
        ) from exc


def _analysis_policy_digest(
    module: str,
    is_package: bool,
    policy: PythonImportPolicy,
    premise: PythonNamespacePremise,
) -> str:
    payload = json.dumps(
        {
            "module": module,
            "is_package": is_package,
            "policy": asdict(policy),
            "namespace_premise": asdict(premise),
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _dynamic_contract_digest(expected: int | None, targets: tuple[str, ...]) -> str:
    payload = json.dumps({"expected": expected, "targets": targets}, sort_keys=True)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _analysis_payload(
    analysis: LocalPythonImportAnalysis, writes: tuple[PythonNamespaceWrite, ...]
) -> dict[str, object]:
    return {
        "namespace_writes": [python_namespace_write_payload(write) for write in writes],
        "requests": [
            {**asdict(request), "candidates": list(request.candidates)}
            for request in analysis.requests
        ],
        "unresolved_dynamic_imports": [
            asdict(diagnostic) for diagnostic in analysis.unresolved_dynamic_imports
        ],
        "discovery_requests": [
            {**asdict(request), "candidates": list(request.candidates)}
            for request in analysis.discovery_requests
        ],
        "relative_source_obligations": [
            {**asdict(request), "fromlist": list(request.fromlist)}
            for request in analysis.relative_source_obligations
        ],
    }


def _cached_namespace_writes(rows: object) -> tuple[PythonNamespaceWrite, ...] | None:
    if not isinstance(rows, list):
        return None
    try:
        writes = tuple(decode_python_namespace_write(row) for row in rows)
    except ValueError:
        return None
    return writes if len(set(writes)) == len(writes) else None


def _cached_analysis(
    value: object, source_digest: str, contract_digest: str, analysis_digest: str
) -> tuple[LocalPythonImportAnalysis, tuple[PythonNamespaceWrite, ...]] | None:
    if (
        not isinstance(value, dict)
        or value.get("source_sha256") != source_digest
        or value.get("dynamic_contract_sha256") != contract_digest
        or value.get("analysis_authority_sha256") != analysis_digest
    ):
        return None
    writes = _cached_namespace_writes(value.get("namespace_writes"))
    if writes is None:
        return None
    rows = value.get("requests")
    discovery_rows = value.get("discovery_requests")
    diagnostics = value.get("unresolved_dynamic_imports")
    relative_rows = value.get("relative_source_obligations")
    if (
        not isinstance(rows, list)
        or not isinstance(discovery_rows, list)
        or not isinstance(diagnostics, list)
        or not isinstance(relative_rows, list)
    ):
        return None
    requests: list[LocalPythonImportRequest] = []
    unresolved: list[LocalPythonImportDiagnostic] = []
    relative_obligations: list[LocalPythonRelativeImportObligation] = []
    for row in (*rows, *discovery_rows):
        if not isinstance(row, dict):
            return None
        kind, candidates = row.get("kind"), row.get("candidates")
        line, column = row.get("line"), row.get("column")
        if (
            kind not in ("direct", "from", "dynamic", "manifest")
            or not isinstance(candidates, list)
            or not candidates
            or not all(isinstance(target, str) and target for target in candidates)
            or type(line) is not int
            or line < 0
            or type(column) is not int
            or column < 0
        ):
            return None
        requests.append(
            LocalPythonImportRequest(
                cast(Literal["direct", "from", "dynamic", "manifest"], kind),
                tuple(cast(list[str], candidates)),
                line,
                column,
            )
        )
    for row in diagnostics:
        if not isinstance(row, dict):
            return None
        line, column, message = row.get("line"), row.get("column"), row.get("message")
        if (
            type(line) is not int
            or line < 0
            or type(column) is not int
            or column < 0
            or not isinstance(message, str)
            or not message
        ):
            return None
        unresolved.append(LocalPythonImportDiagnostic(line, column, message))
    for row in relative_rows:
        if not isinstance(row, dict):
            return None
        kind, name, level = row.get("kind"), row.get("name"), row.get("level")
        fromlist, line, column = row.get("fromlist"), row.get("line"), row.get("column")
        if (
            kind not in ("statement", "import_module", "dunder_import")
            or not isinstance(name, str)
            or type(level) is not int
            or level <= 0
            or not isinstance(fromlist, list)
            or not all(isinstance(item, str) for item in fromlist)
            or type(line) is not int
            or line < 0
            or type(column) is not int
            or column < 0
        ):
            return None
        relative_obligations.append(
            LocalPythonRelativeImportObligation(
                cast(Literal["statement", "import_module", "dunder_import"], kind),
                name,
                level,
                tuple(fromlist),
                line,
                column,
            )
        )
    return (
        LocalPythonImportAnalysis(
            tuple(requests[: len(rows)]),
            tuple(unresolved),
            tuple(requests[len(rows) :]),
            tuple(relative_obligations),
        ),
        writes,
    )


def _read_python_import_manifest(
    project_root: Path,
) -> _ImportManifest | None:
    manifest = project_root / _DYNAMIC_IMPORT_MANIFEST
    if not manifest.is_file():
        return None
    manifest = manifest.resolve()
    _relative_cache_key(project_root, manifest)
    try:
        content = manifest.read_bytes()
        payload = tomllib.loads(content.decode("utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(
            f"cannot read Python tooling import manifest {manifest}: {exc}"
        ) from exc
    if payload.get("schema_version") != 1:
        raise ValueError(
            f"unsupported Python tooling import manifest schema: {manifest}"
        )
    return manifest, content, payload


def _manifest_python_roots(
    project_root: Path, manifest: _ImportManifest | None, field: str
) -> tuple[Path, ...] | None:
    if manifest is None or field not in manifest[2]:
        return None
    values = manifest[2][field]
    if (
        not isinstance(values, list)
        or not values
        or not all(isinstance(value, str) and value for value in values)
    ):
        raise ValueError(f"invalid Python tooling {field}: {manifest[0]}")
    roots: list[Path] = []
    for value in values:
        if (
            PurePosixPath(value).is_absolute()
            or PureWindowsPath(value).drive
            or "\\" in value
            or ".." in PurePosixPath(value).parts
        ):
            raise ValueError(f"invalid Python tooling {field}: {value!r}")
        candidate = (project_root / value).resolve()
        _relative_cache_key(project_root, candidate)
        if not candidate.is_dir():
            raise ValueError(f"missing Python tooling {field} directory: {candidate}")
        if candidate in roots:
            raise ValueError(f"duplicate Python tooling {field} directory: {candidate}")
        roots.append(candidate)
    return tuple(roots)


def _read_dynamic_import_manifest(
    manifest_record: _ImportManifest | None,
    project_root: Path,
    resolver: LocalPythonModuleResolver,
) -> tuple[tuple[Path, bytes] | None, dict[Path, tuple[int, tuple[str, ...]]]]:
    if manifest_record is None:
        return None, {}
    manifest, content, payload = manifest_record
    rows = payload.get("source")
    if not isinstance(rows, list):
        raise ValueError(
            f"Python tooling import manifest has no source rows: {manifest}"
        )
    overrides: dict[Path, tuple[int, tuple[str, ...]]] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError(f"invalid Python tooling import manifest row: {manifest}")
        source_value = row.get("path")
        expected = row.get("nonliteral_calls")
        modules = row.get("modules", [])
        module_trees = row.get("module_trees", [])
        if (
            not isinstance(source_value, str)
            or not isinstance(expected, int)
            or expected < 0
            or not isinstance(modules, list)
            or not all(isinstance(module, str) for module in modules)
            or not isinstance(module_trees, list)
            or not all(isinstance(module, str) for module in module_trees)
        ):
            raise ValueError(f"invalid Python tooling import manifest row: {manifest}")
        source = (project_root / source_value).resolve()
        resolver.module_identity(source)
        if source in overrides:
            raise ValueError(f"duplicate Python tooling import manifest row: {source}")
        targets = set(modules)
        for module_tree in module_trees:
            tree_source = resolver.source_for_module(module_tree)
            if tree_source is None or tree_source.name != "__init__.py":
                raise ValueError(
                    f"dynamic import module tree is not a local package: {module_tree}"
                )
            package_dir = tree_source.parent
            targets.add(module_tree)
            for child in package_dir.rglob("*.py"):
                member = relative_python_module_name(child, package_dir)
                targets.add(f"{module_tree}.{member}" if member else module_tree)
        overrides[source] = (expected, tuple(sorted(targets)))
    return (manifest, content), overrides


def local_python_import_closure(
    project_root: Path,
    seeds: Iterable[Path],
    *,
    policy: PythonImportPolicy = _EXECUTABLE_TOOL_IMPORT_POLICY,
    search_roots: tuple[Path, ...] | None = None,
    source_roots: tuple[Path, ...] | None = None,
) -> LocalPythonSourceClosure:
    """Return the policy projection of one source-byte-keyed dependency graph.

    Seeds may name files or whole Python source directories. A project's import
    manifest declares ordered search roots and admitted source roots separately;
    a namespace search location does not grant ownership of all its descendants.
    Consumers may explicitly supply their admitted source domain independently
    of search order; omitted source roots retain the manifest/default scope.
    Unconfigured projects use ``tools``/``src``/repository search order. Tools use
    full lexical imports, parent-package execution and checked dynamic manifests.
    Lowering supplies its existing module-level-only policy and ``src`` root.
    Unknown literal-relative anchors
    require the complete local source domain, resolved and captured once per
    traversal. That byte inventory does not promote speculative owners to AST
    analysis; ordinary graph edges keep their error and manifest validation.
    No failed analysis becomes an empty closure. Cached grouped requests and
    symbolic coverage obligations always resolve against fresh topology outside
    the explicit build transaction, including previously missing members.
    """

    root = project_root.resolve()
    manifest_record = _read_python_import_manifest(root)
    declared_search_roots = _manifest_python_roots(
        root, manifest_record, "search_roots"
    )
    roots = tuple(
        candidate.resolve()
        for candidate in (
            search_roots
            if search_roots is not None
            else (
                declared_search_roots
                if declared_search_roots is not None
                else (root / "tools", root / "src", root)
            )
        )
        if candidate.is_dir()
    )
    if not roots:
        raise ValueError(f"project has no local Python source roots: {root}")
    declared_source_roots = _manifest_python_roots(
        root, manifest_record, "source_roots"
    )
    if source_roots is None:
        admitted_source_roots = (
            roots if declared_source_roots is None else declared_source_roots
        )
    else:
        admitted_source_roots = tuple(candidate.resolve() for candidate in source_roots)
    seed_paths = tuple(sorted({seed.resolve() for seed in seeds}))
    if any(
        not path.is_relative_to(root)
        for path in (*roots, *admitted_source_roots, *seed_paths)
    ):
        raise ValueError(f"Python tooling source is outside project root: {root}")
    if source_roots is not None:
        if not admitted_source_roots:
            raise ValueError("local Python source domain requires a source root")
        if len(set(admitted_source_roots)) != len(admitted_source_roots):
            raise ValueError("duplicate local Python source root")
        for source_root in admitted_source_roots:
            if not source_root.is_dir():
                raise ValueError(
                    f"missing local Python source root directory: {source_root}"
                )
    query = (seed_paths, policy)
    # Cache placement and manifest contracts cannot share an operation receipt
    # accidentally. Source-byte and topology reuse ends with this transaction.
    domain_key = (
        root,
        roots,
        admitted_source_roots,
        python_source_closure_cache_path(root),
        (manifest_record[0], manifest_record[1])
        if manifest_record is not None
        else None,
    )
    transaction = _GRAPH_TRANSACTION.get()
    domain = transaction.domains.get(domain_key) if transaction is not None else None
    if domain is None:
        resolver = LocalPythonModuleResolver(roots, source_roots=admitted_source_roots)
        persisted_entries, cache_pruned = _read_graph_cache(root)
        domain = _GraphDomain(
            root,
            resolver,
            persisted_entries,
            _with_process_graph_entries(root, persisted_entries),
            cache_pruned,
        )
        if transaction is not None:
            transaction.domains[domain_key] = domain
    if query in domain.receipts:
        return domain.receipts[query]
    resolver = domain.resolver
    snapshots = domain.snapshots
    capture = domain.capture

    manifest, dynamic_import_overrides = (
        _read_dynamic_import_manifest(manifest_record, root, resolver)
        if not policy.module_level_only
        else (None, {})
    )
    analysis_digest = hashlib.sha256(
        json.dumps(local_import_analysis_identity(), sort_keys=True).encode("utf-8")
    ).hexdigest()
    # Keep records from sibling policies/seeds. Each source/module/policy variant
    # replaces its byte generation; aliases share a snapshot, never an execution
    # context. Merging next_entries below retains every alias seen in this walk.
    # Failed queries never publish their tentative rows through a later query.
    next_entries = dict(domain.entries)
    seed_sources: list[LocalPythonModuleSource] = []
    for seed in seed_paths:
        for path in seed.rglob("*.py") if seed.is_dir() else (seed,):
            if path != seed:
                path = path.resolve()
            seed_sources.append(
                LocalPythonModuleSource(resolver.module_identity(path)[0], path)
            )
    package_children = domain.package_children

    # Content-keyed, so one summary serves every premise round.
    writes_by_source: dict[
        LocalPythonModuleSource, tuple[PythonNamespaceWrite, ...]
    ] = {}

    def walk(
        graph_writes: PythonGraphNamespaceWrites,
    ) -> tuple[set[LocalPythonModuleSource], set[Path], str]:
        """Reach the graph under one namespace premise."""
        covered_sources: set[Path] = set()
        topology_digest = ""
        pending = list(seed_sources)
        visited: set[LocalPythonModuleSource] = set()
        while pending:
            module_source = pending.pop()
            if module_source in visited:
                continue
            source = module_source.path
            if not source.is_file():
                raise FileNotFoundError(f"missing Python tooling source: {source}")
            visited.add(module_source)
            module = module_source.name
            is_package = source.name == "__init__.py"
            try:
                expected_dynamic_imports, dynamic_targets = (
                    dynamic_import_overrides.get(source, (None, ()))
                )
                if is_package and source not in package_children:
                    package_children[source] = python_package_child_names(
                        (source.parent,), import_topology=resolver.import_topology
                    )
                premise = graph_writes.premise(
                    source, package_children.get(source, frozenset())
                )
                cache_key = domain.relative_key(source)
                snapshot = capture(source)
                policy_digest = _analysis_policy_digest(
                    module, is_package, policy, premise
                )
                contract_digest = _dynamic_contract_digest(
                    expected_dynamic_imports, dynamic_targets
                )
                variants = next_entries.get(cache_key)
                cached = _cached_analysis(
                    variants.get(policy_digest) if variants is not None else None,
                    snapshot.sha256,
                    contract_digest,
                    analysis_digest,
                )
                analysis_missed = cached is None
                if cached is not None:
                    analysis, writes = cached
                else:
                    # Share captured bytes without retaining the analysis AST
                    # (or decoded text) in the walk's byte-identity owner.
                    parsed = PythonSourceSnapshot(snapshot.path, snapshot.content)
                    analysis = analyze_local_imports(
                        parsed,
                        module_source,
                        policy,
                        expected_nonliteral_dynamic_imports=expected_dynamic_imports,
                        nonliteral_dynamic_import_targets=dynamic_targets,
                        namespace_premise=premise,
                    )
                    # The analysis parsed this snapshot; summarize the same AST.
                    writes = writes_by_source.get(module_source)
                    if writes is None:
                        writes = _resolved_python_namespace_summary(
                            parsed.tree,
                            package=python_package_name(module, is_package=is_package),
                            target_python=_local_import_binding_policy().target_python,
                        ).foreign_writes
                    del parsed
                writes_by_source[module_source] = writes
                # Reapply the contract even on a cache hit; accepted unresolved
                # sites are retained, never silently converted into a complete
                # analysis.
                analysis.validate_dynamic_contract(
                    source, policy, expected_dynamic_imports
                )
                if analysis_missed:
                    # A validated hit already owns its serialized storage.
                    # Rebuild only misses, merging aliases without mutating
                    # cached_entries.
                    variants = dict(variants or {})
                    variants[policy_digest] = {
                        "source_sha256": snapshot.sha256,
                        "dynamic_contract_sha256": contract_digest,
                        "analysis_authority_sha256": analysis_digest,
                        **_analysis_payload(analysis, writes),
                    }
                    next_entries[cache_key] = variants
                dependencies = resolve_local_import_requests(
                    analysis,
                    resolver,
                    policy,
                )
                if analysis.relative_source_obligations and not topology_digest:
                    # Inventory members are byte dependencies, not executable
                    # graph requests. Complete coverage requires no recursive
                    # AST analysis of these speculative owners. Ordinary reached
                    # sources still follow their graph and validate all exact
                    # manifest obligations.
                    covered = domain.inventory(policy)
                    covered_sources.update(covered.paths)
                    topology_digest = covered.topology_digest
            except ValueError as exc:
                raise ValueError(
                    f"cannot derive Python tooling import closure for {source}: {exc}"
                ) from exc
            for dependency in dependencies:
                if dependency not in visited:
                    pending.append(dependency)
        return visited, covered_sources, topology_digest

    # Spec 0216 admits only named namespace writes, so the reached graph's
    # summaries bound what a callback can rebind. Start from no named write and
    # walk again until the premise the walk assumed is the one it found; writes
    # only add names, so the premise grows monotonically to its fixpoint.
    graph_writes = NO_GRAPH_NAMESPACE_WRITES
    while True:
        visited, covered_sources, topology_digest = walk(graph_writes)
        found = graph_python_namespace_writes(
            (
                write
                for module_source in sorted(
                    visited, key=lambda item: (item.name, item.path.as_posix())
                )
                for write in writes_by_source[module_source]
            ),
            resolver.source_for_module,
        )
        if found == graph_writes:
            break
        graph_writes = found
    resolver.import_topology.validate()
    reached = {module_source.path for module_source in visited}
    if domain.cache_pruned or next_entries != domain.persisted_entries:
        _write_graph_cache(root, next_entries)
    _remember_process_graph_entries(root, next_entries)
    domain.entries = next_entries
    domain.persisted_entries = next_entries
    domain.cache_pruned = False
    content_by_path = {
        path: snapshots[path].content for path in reached | covered_sources
    }
    if manifest is not None:
        content_by_path[manifest[0]] = manifest[1]
    paths = tuple(sorted(content_by_path, key=lambda path: path.as_posix()))
    digest = hashlib.sha256()
    if topology_digest:
        digest.update(b"local-python-source-domain-v1\0")
        digest.update(topology_digest.encode("ascii"))
        digest.update(b"\0")
    hashes: dict[Path, str] = {}
    source_bytes = 0
    for path in paths:
        content = content_by_path[path]
        digest.update(domain.relative_key(path).encode("utf-8"))
        digest.update(b"\0")
        digest.update(content)
        digest.update(b"\0")
        hashes[path] = (
            snapshots[path].sha256
            if path in snapshots
            else hashlib.sha256(content).hexdigest()
        )
        source_bytes += len(content)
    result = LocalPythonSourceClosure(
        paths, hashes, digest.hexdigest(), source_bytes, topology_digest
    )
    if transaction is not None:
        domain.receipts[query] = result
    return result
