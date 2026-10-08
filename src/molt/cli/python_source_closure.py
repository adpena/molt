"""One policy-keyed local Python dependency graph for compiler and tool inputs."""

from __future__ import annotations

import ast
import hashlib
import json
import os
from collections.abc import Callable, Iterable, Generator, Mapping
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from types import MappingProxyType
import tomllib
from typing import Literal, cast

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.default_paths import _default_molt_cache
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.python_import_resolution import (
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
_GRAPH_CACHE_SCHEMA_VERSION = 19
_GraphQuery = tuple[
    Path, tuple[Path, ...], tuple[Path, ...], tuple[Path, ...], PythonImportPolicy
]
_ImportManifest = tuple[Path, bytes, dict[str, object]]
_GRAPH_TRANSACTION: ContextVar[dict[_GraphQuery, LocalPythonSourceClosure] | None] = (
    ContextVar("_GRAPH_TRANSACTION", default=None)
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


@contextmanager
def local_python_import_graph_transaction(*, fresh: bool = False) -> Generator[None]:
    """Reuse immutable tooling closure queries only within one build command."""
    if not fresh and _GRAPH_TRANSACTION.get() is not None:
        yield
        return
    previous_context = _GRAPH_TRANSACTION.set({})
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
        payload = json.loads(cache_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}, True
    if not isinstance(payload, dict):
        return {}, True
    if payload.get("schema_version") != _GRAPH_CACHE_SCHEMA_VERSION:
        return {}, True
    entries = payload.get("entries")
    if not isinstance(entries, dict):
        return {}, True
    retained: dict[str, dict[str, object]] = {}
    for key, value in entries.items():
        if not isinstance(key, str) or not isinstance(value, dict):
            continue
        # Persisted keys never grant source authority: only fresh canonical
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
    return retained, retained != entries


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
) -> str:
    payload = json.dumps(
        {
            "module": module,
            "is_package": is_package,
            "policy": asdict(policy),
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _dynamic_contract_digest(expected: int | None, targets: tuple[str, ...]) -> str:
    payload = json.dumps({"expected": expected, "targets": targets}, sort_keys=True)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _analysis_payload(analysis: LocalPythonImportAnalysis) -> dict[str, object]:
    return {
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


def _cached_analysis(
    value: object, source_digest: str, contract_digest: str, analysis_digest: str
) -> LocalPythonImportAnalysis | None:
    if (
        not isinstance(value, dict)
        or value.get("source_sha256") != source_digest
        or value.get("dynamic_contract_sha256") != contract_digest
        or value.get("analysis_authority_sha256") != analysis_digest
    ):
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
    return LocalPythonImportAnalysis(
        tuple(requests[: len(rows)]),
        tuple(unresolved),
        tuple(requests[len(rows) :]),
        tuple(relative_obligations),
    )


def _molt_cli_lazy_targets(
    snapshot: PythonSourceSnapshot,
) -> set[str]:
    """Derive finite ``molt.cli`` lazy imports from their source authorities."""

    # The graph owns captured bytes for the whole walk, but this derivation owns
    # its AST only until the finite target projection is complete.
    source = snapshot.path
    tree = PythonSourceSnapshot(source, snapshot.content).tree
    targets: set[str] = set()
    registry_found = False
    for node in tree.body:
        value = None
        if (
            isinstance(node, ast.AnnAssign)
            and isinstance(node.target, ast.Name)
            and node.target.id == "_LAZY_REEXPORTS"
        ):
            value = node.value
        elif isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "_LAZY_REEXPORTS"
            for target in node.targets
        ):
            value = node.value
        if value is not None:
            if not isinstance(value, ast.Dict):
                raise ValueError(
                    f"molt.cli lazy reexport registry is not a literal dict: {source}"
                )
            registry_found = True
            for registry_value in value.values:
                if (
                    not isinstance(
                        registry_value,
                        (ast.Tuple, ast.List),
                    )
                    or not registry_value.elts
                ):
                    raise ValueError(f"invalid molt.cli lazy reexport row in {source}")
                module_node = registry_value.elts[0]
                if not (
                    isinstance(module_node, ast.Constant)
                    and isinstance(module_node.value, str)
                ):
                    raise ValueError(
                        f"non-literal molt.cli lazy reexport module in {source}"
                    )
                targets.add(f"molt.cli.{module_node.value}")
        assignment_value = None
        if isinstance(node, ast.Assign):
            assignment_value = node.value
        elif isinstance(node, ast.AnnAssign):
            assignment_value = node.value
        if (
            isinstance(assignment_value, ast.Call)
            and isinstance(assignment_value.func, ast.Name)
            and assignment_value.func.id == "_LazyPostLoweringModule"
            and assignment_value.args
        ):
            module_node = assignment_value.args[0]
            if not (
                isinstance(module_node, ast.Constant)
                and isinstance(module_node.value, str)
            ):
                raise ValueError(f"non-literal molt.cli lazy proxy module in {source}")
            targets.add(f"molt.cli.{module_node.value}")
    if not registry_found:
        raise ValueError(f"molt.cli lazy reexport registry is missing: {source}")
    return targets


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
    capture: Callable[[Path], PythonSourceSnapshot],
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
        derive_molt_cli_lazy_targets = row.get("derive_molt_cli_lazy_targets", False)
        if (
            not isinstance(source_value, str)
            or not isinstance(expected, int)
            or expected < 0
            or not isinstance(modules, list)
            or not all(isinstance(module, str) for module in modules)
            or not isinstance(module_trees, list)
            or not all(isinstance(module, str) for module in module_trees)
            or not isinstance(derive_molt_cli_lazy_targets, bool)
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
        if derive_molt_cli_lazy_targets:
            targets.update(_molt_cli_lazy_targets(capture(source)))
        overrides[source] = (expected, tuple(sorted(targets)))
    return (manifest, content), overrides


def local_python_import_closure(
    project_root: Path,
    seeds: Iterable[Path],
    *,
    policy: PythonImportPolicy = _EXECUTABLE_TOOL_IMPORT_POLICY,
    search_roots: tuple[Path, ...] | None = None,
) -> LocalPythonSourceClosure:
    """Return the policy projection of one source-byte-keyed dependency graph.

    Seeds may name files or whole Python source directories. A project's import
    manifest declares ordered search roots and admitted source roots separately;
    a namespace search location does not grant ownership of all its descendants.
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
    source_roots = roots if declared_source_roots is None else declared_source_roots
    seed_paths = tuple(sorted({seed.resolve() for seed in seeds}))
    if any(not path.is_relative_to(root) for path in (*roots, *seed_paths)):
        raise ValueError(f"Python tooling source is outside project root: {root}")
    query = (root, seed_paths, roots, source_roots, policy)
    transaction = _GRAPH_TRANSACTION.get()
    if transaction is not None and query in transaction:
        return transaction[query]
    resolver = LocalPythonModuleResolver(roots, source_roots=source_roots)
    snapshots: dict[Path, PythonSourceSnapshot] = {}
    covered_sources: set[Path] = set()
    topology_digest = ""

    def capture(path: Path) -> PythonSourceSnapshot:
        if path not in snapshots:
            snapshots[path] = resolver.capture_source(path)
        return snapshots[path]

    manifest, dynamic_import_overrides = (
        _read_dynamic_import_manifest(manifest_record, root, resolver, capture)
        if not policy.module_level_only
        else (None, {})
    )
    cached_entries, cache_pruned = _read_graph_cache(root)
    analysis_digest = hashlib.sha256(
        json.dumps(local_import_analysis_identity(), sort_keys=True).encode("utf-8")
    ).hexdigest()
    # Keep records from sibling policies/seeds. Each source/module/policy variant
    # replaces its byte generation; aliases share a snapshot, never an execution
    # context. Merging next_entries below retains every alias seen in this walk.
    next_entries = dict(cached_entries)
    pending: list[LocalPythonModuleSource] = []
    for seed in seed_paths:
        for path in seed.rglob("*.py") if seed.is_dir() else (seed,):
            if path != seed:
                path = path.resolve()
            pending.append(
                LocalPythonModuleSource(resolver.module_identity(path)[0], path)
            )
    visited: set[LocalPythonModuleSource] = set()
    reached: set[Path] = set()
    while pending:
        module_source = pending.pop()
        if module_source in visited:
            continue
        source = module_source.path
        if not source.is_file():
            raise FileNotFoundError(f"missing Python tooling source: {source}")
        visited.add(module_source)
        module = module_source.name
        reached.add(source)
        try:
            expected_dynamic_imports, dynamic_targets = dynamic_import_overrides.get(
                source,
                (None, ()),
            )
            cache_key = _relative_cache_key(root, source)
            snapshot = capture(source)
            policy_digest = _analysis_policy_digest(
                module,
                source.name == "__init__.py",
                policy,
            )
            contract_digest = _dynamic_contract_digest(
                expected_dynamic_imports, dynamic_targets
            )
            variants = next_entries.get(cache_key)
            analysis = _cached_analysis(
                variants.get(policy_digest) if variants is not None else None,
                snapshot.sha256,
                contract_digest,
                analysis_digest,
            )
            analysis_missed = analysis is None
            if analysis is None:
                analysis = analyze_local_imports(
                    # Share captured bytes without retaining the analysis AST
                    # (or decoded text) in the walk's byte-identity owner.
                    PythonSourceSnapshot(snapshot.path, snapshot.content),
                    module_source,
                    policy,
                    expected_nonliteral_dynamic_imports=expected_dynamic_imports,
                    nonliteral_dynamic_import_targets=dynamic_targets,
                )
            # Reapply the contract even on a cache hit; accepted unresolved sites
            # are retained, never silently converted into a complete analysis.
            analysis.validate_dynamic_contract(source, policy, expected_dynamic_imports)
            if analysis_missed:
                # A validated hit already owns its serialized storage. Rebuild
                # only misses, merging aliases without mutating cached_entries.
                variants = dict(variants or {})
                variants[policy_digest] = {
                    "source_sha256": snapshot.sha256,
                    "dynamic_contract_sha256": contract_digest,
                    "analysis_authority_sha256": analysis_digest,
                    **_analysis_payload(analysis),
                }
                next_entries[cache_key] = variants
            dependencies = resolve_local_import_requests(
                analysis,
                resolver,
                policy,
            )
            if analysis.relative_source_obligations and not topology_digest:
                inventory = resolver.source_inventory(
                    allowed_prefix=policy.allowed_prefix,
                    include_parent_packages=policy.include_parent_packages,
                )
                # Inventory members are byte dependencies, not executable graph
                # requests. Complete coverage requires no recursive AST analysis
                # of these speculative owners. Ordinary reached sources still
                # follow their graph and validate all exact manifest obligations.
                for item in inventory.sources:
                    capture(item.path)
                    covered_sources.add(item.path)
                topology = {
                    "roots": [_relative_cache_key(root, path) for path in roots],
                    "sources": [
                        [item.name, _relative_cache_key(root, item.path)]
                        for item in inventory.sources
                    ],
                    "packages": [
                        [name, [_relative_cache_key(root, path) for path in locations]]
                        for name, locations in inventory.packages
                    ],
                }
                topology_digest = hashlib.sha256(
                    json.dumps(topology, sort_keys=True, separators=(",", ":")).encode()
                ).hexdigest()
        except ValueError as exc:
            raise ValueError(
                f"cannot derive Python tooling import closure for {source}: {exc}"
            ) from exc
        for dependency in dependencies:
            if dependency not in visited:
                pending.append(dependency)
    if cache_pruned or next_entries != cached_entries:
        _write_graph_cache(root, next_entries)
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
        digest.update(_relative_cache_key(root, path).encode("utf-8"))
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
        transaction[query] = result
    return result
