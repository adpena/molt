from __future__ import annotations

from molt.compiler_analysis import python_import_scan as _source_import_scan
from molt.compiler_analysis.python_source_demands import PythonSourceDemands
from molt.compiler_analysis.native_support_slice import (
    canonical_native_support_roots,
)

import ast
import contextlib
import re
from dataclasses import dataclass, field, replace
from collections.abc import Collection, Mapping, MutableMapping, Sequence
from pathlib import Path
from functools import cached_property
from typing import NamedTuple

from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
from molt.cli.config_resolution import STATIC_IMPORT_MODULES_ENV
from molt.cli import module_dependencies as _module_dependency_authority
from molt.cli import module_graph_cache as _module_graph_cache
from molt.cli import module_import_scanner as _module_import_scanner
from molt.cli import module_resolution as _module_resolution
from molt.cli import module_source as _module_source
from molt.cli.models import (
    ImportScanMode,
    _CompleteImportScan,
    _DiscoveredModuleGraph,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _PrecomputedModuleImportScan,
    _ImportAdmissionPolicy,
    _RuntimeImportScanCustody,
)
from molt.compiler_analysis.python_imports import _ImportScanRequests
from molt.compiler_analysis.python_namespace_graph import (
    PythonNamespacePremise,
    PythonGraphNamespaceIndex,
    UNKNOWN_GRAPH_NAMESPACE_WRITES,
    graph_python_namespace_writes,
)
from molt.compiler_analysis.python_namespace_writes import (
    PythonNamespaceWrite,
)
from molt.cli.python_import_resolution import python_package_child_names
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    _PythonAstDigestAdmission,
)
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)


# Submodule prefixes excluded from the module graph because they target
# platforms that Molt does not support (e.g. Emscripten/Pyodide).  The import
# scanner still discovers them but the graph walker skips any candidate whose
# dotted name starts with one of these prefixes.
PLATFORM_EXCLUDED_SUBMODULES = ("urllib3.contrib.emscripten",)


class _SourceScanJob(NamedTuple):
    """Project one admitted source context under its latest namespace premise."""

    module_name: str
    source: PythonSourceKey | None
    arguments: tuple[str, ...] = ()


@dataclass(frozen=True)
class _SourceDiscoveryContext:
    """Resolution and admission belong to the source, not the latest extension."""

    roots: tuple[Path, ...]
    module_roots: tuple[Path, ...]
    stdlib_root: Path
    project_root: Path | None
    stdlib_allowlist: frozenset[str]
    skip_modules: frozenset[str]
    stub_parents: frozenset[str]
    admission_policy: _ImportAdmissionPolicy
    allow_external_imports: bool
    runtime_import_custody: _RuntimeImportScanCustody | None
    seen_import_names: set[str] = field(default_factory=set, compare=False)

    @cached_property
    def dependency(self) -> "_SourceDiscoveryContext":
        return (
            replace(self, allow_external_imports=False, seen_import_names=set())
            if self.allow_external_imports
            else self
        )


class _SourceEntryJob(NamedTuple):
    path: Path
    module_name: str | None
    context: _SourceDiscoveryContext
    full_scan: bool = False
    precomputed: _PrecomputedModuleImportScan | None = None


class _ModuleGraphDiscovery:
    """One build's monotone namespace index and source-value worklist.

    Extensions keep original byte/span identities and each source's admission
    context. Only changed writer targets and newly demanded owners are revisited.
    """

    def __init__(
        self,
        resolver_cache: _module_resolution._ModuleResolutionCache,
        target_python: TargetPythonVersion,
    ) -> None:
        self.resolver_cache = resolver_cache
        self.target_python = target_python
        self.graph: dict[str, Path] = {}
        self.scan_sources: dict[str, _ModuleSourceScanAuthority] = {}
        self.contexts: dict[str, _SourceDiscoveryContext] = {}
        self.context_epochs: dict[str, int] = {}
        self.source_generations: dict[
            str, tuple[str, _module_source.PythonSourceSnapshot | None]
        ] = {}
        self.scanned_bodies: dict[
            str, set[tuple[PythonSourceKey, tuple[str, ...]]]
        ] = {}
        self.scan_receipts: dict[
            _SourceScanJob,
            tuple[PythonNamespacePremise | None, tuple[str, ...], ImportScanMode, int],
        ] = {}
        self.queue: list[_SourceEntryJob | _SourceScanJob] = []
        self.queued_scans: set[_SourceScanJob] = set()
        self.namespace_index = PythonGraphNamespaceIndex()
        self.namespace_writes: dict[str, set[PythonNamespaceWrite]] = {}
        self.writer_targets: dict[str, set[PythonNamespaceWrite]] = {}
        self.pending_namespace_writes: dict[
            str, dict[PythonSourceKey | None, list[PythonNamespaceWrite]]
        ] = {}
        self.active_owners: dict[str, set[PythonSourceKey | None]] = {}
        self.modules_by_source: dict[Path, set[str]] = {}
        self.package_children: dict[tuple[Path, ...], frozenset[str]] = {}
        self.demands: PythonSourceDemands | None = None

    def validate_sources(self, modules: Collection[str] | None = None) -> None:
        for module_name in self.source_generations if modules is None else modules:
            generation = self.source_generations.get(module_name)
            snapshot = generation[1] if generation is not None else None
            if snapshot is not None and snapshot.path.read_bytes() != snapshot.content:
                raise _module_source.PythonSourceChangedError(
                    f"source changed during deferred discovery: {snapshot.path}"
                )


class _LoadedModuleImportScan(NamedTuple):
    """One complete scan plus its operation-local source/parse outcome."""

    scan: _CompleteImportScan
    cache_hit: bool
    source_parsed: bool
    source: str | None
    tree: ast.AST | None
    snapshot: _module_source.PythonSourceSnapshot | None = None
    source_pruned: bool = False


def _bind_precomputed_module_import_scan(
    path: Path,
    *,
    module_name: str,
    import_scan_mode: ImportScanMode,
    scan: _CompleteImportScan,
    snapshot: _module_source.PythonSourceSnapshot,
    is_package: bool | None = None,
    package_paths: tuple[Path, ...] = (),
    native_support_function_roots: tuple[str, ...] = (),
    target_python: TargetPythonVersion,
    namespace_premise: PythonNamespacePremise | None = None,
) -> _PrecomputedModuleImportScan:
    if snapshot.path != path:
        raise ValueError(f"cannot bind source scan snapshot: {path}")
    return _PrecomputedModuleImportScan(
        _ModuleSourceScanAuthority(
            module_name,
            path,
            import_scan_mode,
            is_package,
            scan.requires_runtime_package_anchor,
            uses_runtime_import_protocol=(
                scan.source_requests.uses_runtime_import_protocol
            ),
            namespace_premise=namespace_premise,
            source_sha256=snapshot.sha256,
            package_paths=package_paths,
            native_support_function_roots=native_support_function_roots,
        ),
        target_python.tag,
        scan,
    )


def _validate_precomputed_module_import_scan(
    record: _PrecomputedModuleImportScan,
    *,
    path: Path,
    module_name: str,
    import_scan_mode: ImportScanMode,
    target_python: TargetPythonVersion,
) -> None:
    record.authority.validate(module_name, path)
    if (
        record.authority.mode != import_scan_mode
        or bool(record.authority.deferred_owners)
        or record.authority.requires_runtime_package_anchor
        != record.scan.requires_runtime_package_anchor
        or record.authority.uses_runtime_import_protocol
        != record.scan.source_requests.uses_runtime_import_protocol
        or record.target_python_tag != target_python.tag
    ):
        raise ValueError(f"precomputed source scan lost custody: {module_name!r}")
    if _module_source._source_content_sha256(path) != record.authority.source_sha256:
        raise _module_source.PythonSourceChangedError(
            f"precomputed source scan lost custody: {module_name!r}; "
            f"source changed or became unavailable: {path}"
        )


def _merge_discovered_module_graph(
    graph: MutableMapping[str, Path],
    scan_authorities: MutableMapping[str, _ModuleSourceScanAuthority],
    result: _DiscoveredModuleGraph,
) -> None:
    """Publish the closure and affected prior rows, without a graph-prefix copy."""
    result.scan_authority.validate_graph(result.graph)
    updates: dict[str, _ModuleSourceScanAuthority] = {}
    for source in (*result.revised_sources, *result.scan_authority.sources):
        name = source.module_name
        prior = updates.get(name, scan_authorities.get(name))
        prior_path = graph.get(name)
        if prior_path is None and name not in result.graph:
            raise ValueError("source revision lacks enclosing graph admission")
        if prior_path is not None:
            source.validate(name, prior_path)
        updates[name] = (
            prior
            if prior == source
            else _ModuleGraphScanAuthority((prior,))
            .merged(_ModuleGraphScanAuthority((source,)))
            .by_module[name]
            if prior is not None
            else source
        )
    # Validate the entire delta before either public projection changes.
    for name, path in result.graph.items():
        prior = graph.get(name)
        if prior is not None and prior.resolve() != path.resolve():
            raise ValueError(f"module {name!r} has conflicting source scan authorities")
    graph.update(result.graph)
    scan_authorities.update(updates)


def _parse_static_import_modules(raw: str) -> tuple[frozenset[str], str | None]:
    modules: set[str] = set()
    for part in re.split(r"[\s,]+", raw.strip()):
        if not part:
            continue
        if not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*",
            part,
        ):
            return frozenset(), (
                f"{STATIC_IMPORT_MODULES_ENV} must contain comma/space-separated "
                f"Python module names; invalid entry: {part!r}"
            )
        modules.add(part)
    return frozenset(modules), None


def _parse_static_import_modules_from_env(
    environ: Mapping[str, str],
) -> tuple[frozenset[str], str | None]:
    return _parse_static_import_modules(environ.get(STATIC_IMPORT_MODULES_ENV, ""))


def _record_module_reason(
    module_reasons: MutableMapping[str, set[str]],
    module_name: str,
    reason: str,
) -> None:
    module_reasons.setdefault(module_name, set()).add(reason)


@_source_tree_fingerprint_transaction()
def _extend_module_graph_with_closure(
    module_graph: MutableMapping[str, Path],
    *,
    scan_authorities: MutableMapping[str, _ModuleSourceScanAuthority],
    entry_paths: Sequence[Path],
    full_scan_roots: bool,
    discovery: _ModuleGraphDiscovery | None = None,
    roots: Sequence[Path],
    module_roots: Sequence[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
) -> _DiscoveredModuleGraph:
    if not entry_paths:
        return _DiscoveredModuleGraph({}, set(), _ModuleGraphScanAuthority())
    if runtime_import_custody is not None:
        # Only the explicitly supplied owner sources are newly admitted here.
        # Every other catalog row must already belong to the enclosing graph.
        owner_paths = set(runtime_import_custody.owners_by_module.values())
        if not owner_paths.issubset(path.resolve() for path in entry_paths):
            raise ValueError(
                "runtime import custody owners are not closure entry sources"
            )
        for name, path in runtime_import_custody.catalog:
            existing = module_graph.get(name)
            if existing is None and name in runtime_import_custody.owners_by_module:
                continue
            if existing is None or existing.resolve() != path:
                raise ValueError(
                    f"runtime import catalog lacks graph admission: {name!r}"
                )
    closure = _discover_module_graph_from_paths(
        entry_paths,
        list(roots),
        list(module_roots),
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        runtime_import_custody=runtime_import_custody,
        enclosing_scan_authority=(
            _ModuleGraphScanAuthority(tuple(scan_authorities.values()))
            if discovery is None
            else None
        ),
        full_scan_roots=full_scan_roots,
        discovery=discovery,
    )
    _merge_discovered_module_graph(module_graph, scan_authorities, closure)
    for name in closure.graph:
        _record_module_reason(module_reasons, name, reason)
    if runtime_import_custody is not None:
        runtime_import_custody.validate_graph(module_graph)
    return closure


def _resolve_static_import_module_paths(
    *,
    module_names: Collection[str],
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    import_admission_policy: _ImportAdmissionPolicy | None,
) -> tuple[dict[str, Path], list[str]]:
    resolved: dict[str, Path] = {}
    errors: list[str] = []
    for module_name in sorted(module_names):
        path = resolver_cache.resolve_module(
            module_name,
            list(roots),
            stdlib_root,
            stdlib_allowlist,
        )
        if path is None:
            errors.append(
                f"{STATIC_IMPORT_MODULES_ENV} module {module_name!r} was not found"
            )
            continue
        if import_admission_policy is not None and not (
            import_admission_policy.admits_import(
                module_name,
                path,
                from_entry_path=False,
            )
        ):
            errors.append(
                f"{STATIC_IMPORT_MODULES_ENV} module {module_name!r} resolves under "
                "an external root but is not within an admitted external static package"
            )
            continue
        resolved[module_name] = path
    return resolved, errors


@_source_tree_fingerprint_transaction()
def _extend_module_graph_with_static_import_modules(
    *,
    module_graph: MutableMapping[str, Path],
    scan_authorities: MutableMapping[str, _ModuleSourceScanAuthority],
    explicit_imports: set[str],
    module_names: Collection[str],
    roots: Sequence[Path],
    module_roots: Sequence[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    import_admission_policy: _ImportAdmissionPolicy | None,
    target_python: TargetPythonVersion,
    discovery: _ModuleGraphDiscovery | None = None,
) -> list[str]:
    if not module_names:
        return []
    resolved, errors = _resolve_static_import_module_paths(
        module_names=module_names,
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
    )
    if errors:
        return errors
    explicit_imports.update(module_names)
    closure = _extend_module_graph_with_closure(
        module_graph,
        scan_authorities=scan_authorities,
        entry_paths=tuple(resolved.values()),
        full_scan_roots=True,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolver_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        reason="explicit_static_import",
        discovery=discovery,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=False,
        target_python=target_python,
    )
    explicit_imports.update(closure.explicit_imports)
    return []


def _record_new_module_reasons(
    module_graph: Mapping[str, Path],
    before_names: set[str],
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
) -> None:
    for name in module_graph:
        if name in before_names:
            continue
        _record_module_reason(module_reasons, name, reason)


@_source_tree_fingerprint_transaction()
def _discover_module_graph_from_paths(
    entry_paths: Sequence[Path],
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_scans_by_path: Mapping[Path, _PrecomputedModuleImportScan]
    | None = None,
    enclosing_scan_authority: _ModuleGraphScanAuthority | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    *,
    full_scan_roots: bool,
    discovery: _ModuleGraphDiscovery | None = None,
    entry_imports: Collection[str] = (),
) -> _DiscoveredModuleGraph:
    resolution_cache = resolver_cache or (
        discovery.resolver_cache
        if discovery is not None
        else _module_resolution._ModuleResolutionCache()
    )
    state = discovery or _ModuleGraphDiscovery(resolution_cache, target_python)
    if (
        state.resolver_cache is not resolution_cache
        or state.target_python != target_python
    ):
        raise ValueError("module discovery continuation lost target/resolver custody")
    if state.queue:
        raise ValueError("module discovery continuation has unfinished source jobs")
    if runtime_import_custody is not None:
        if not full_scan_roots:
            raise ValueError("runtime import custody requires full-depth owner scans")
        for name, path in runtime_import_custody.catalog:
            if name not in runtime_import_custody.owners_by_module and (
                name not in state.graph or state.graph[name].resolve() != path
            ):
                raise ValueError(
                    f"runtime catalog lacks live source discovery: {name!r}"
                )
    precomputed_scans_by_path = {
        path.resolve(): scan for path, scan in (precomputed_scans_by_path or {}).items()
    }
    context = _SourceDiscoveryContext(
        tuple(roots),
        tuple(module_roots),
        stdlib_root,
        project_root,
        frozenset(stdlib_allowlist),
        frozenset(skip_modules or ()),
        frozenset(stub_parents or ()),
        import_admission_policy or _ImportAdmissionPolicy(),
        allow_entry_external_imports,
        runtime_import_custody,
    )
    graph, scan_sources = state.graph, state.scan_sources
    queue, queued_scans = state.queue, state.queued_scans
    source_generations, scanned_bodies = state.source_generations, state.scanned_bodies
    scan_receipts = state.scan_receipts
    namespace_index = state.namespace_index
    pending_namespace_writes = state.pending_namespace_writes
    modules_by_source, package_children = (
        state.modules_by_source,
        state.package_children,
    )
    explicit_imports: set[str] = set()
    touched: set[str] = set()
    projected: set[str] = set()
    revised: set[str] = set()
    queued_entries: dict[tuple[Path, str | None], _SourceEntryJob] = {}

    def queue_entry(path, name, source_context, *, full=False, precomputed=None):
        key = (resolution_cache.resolved_path(path), name)
        previous = queued_entries.get(key)
        if previous is not None:
            previous_raw = (
                previous.precomputed is None
                or not previous.precomputed.authority.native_support_function_roots
            )
            requested_raw = (
                precomputed is None
                or not precomputed.authority.native_support_function_roots
            )
            if previous_raw and (not requested_raw or previous.full_scan):
                source_context, precomputed = previous.context, previous.precomputed
            elif (
                previous.precomputed is not None
                and precomputed is not None
                and not previous_raw
                and not requested_raw
            ):
                if (
                    previous.precomputed.authority.native_support_function_roots
                    != precomputed.authority.native_support_function_roots
                ):
                    raise ValueError(
                        "queued source views disagree on native root custody"
                    )
            full = full or previous.full_scan
            if precomputed is not None and precomputed.authority.mode != (
                "full" if full else "module_init"
            ):
                precomputed = None
        job = _SourceEntryJob(path, name, source_context, full, precomputed)
        if previous == job:
            return
        queued_entries[key] = job
        queue.append(job)

    def queue_scan(module, owner=None, arguments=()):
        job = _SourceScanJob(module, owner, arguments)
        if job not in queued_scans:
            queued_scans.add(job)
            queue.append(job)

    def resolve_candidate(candidate, source_context):
        if source_context.runtime_import_custody is not None:
            catalog_path = source_context.runtime_import_custody.catalog_by_module.get(
                candidate
            )
            if catalog_path is not None:
                return catalog_path
        if candidate in graph:
            return graph[candidate]
        if enclosing_scan_authority is not None:
            admitted = enclosing_scan_authority.by_module.get(candidate)
            if admitted is not None:
                return admitted.source_path
        return resolution_cache.resolve_module(
            candidate,
            list(source_context.roots),
            source_context.stdlib_root,
            set(source_context.stdlib_allowlist),
        )

    def namespace_premise(path, package_paths):
        path = resolution_cache.resolved_path(path)
        bounded = namespace_index.premise(path, frozenset())
        if not package_paths or bounded.foreign_writes is None:
            return bounded
        if package_paths not in package_children:
            try:
                package_children[package_paths] = python_package_child_names(
                    package_paths, import_topology=resolution_cache.import_topology
                )
            except (OSError, ValueError):
                merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
                return namespace_index.premise(path, frozenset())
        return namespace_index.premise(path, package_children[package_paths])

    def merge_namespace_writes(batch):
        changed = namespace_index.merge(batch)
        affected = (
            set(scan_sources)
            if changed is None
            else set().union(*(modules_by_source.get(path, ()) for path in changed))
        )
        for module in sorted(affected):
            prior = scan_sources[module]
            premise = namespace_premise(prior.source_path, prior.package_paths)
            if prior.namespace_premise == premise:
                continue
            scan_sources[module] = replace(prior, namespace_premise=premise)
            revised.add(module)
            if _SourceScanJob(module, None) in scan_receipts:
                queue_scan(module)
            for owner, arguments in sorted(scanned_bodies.get(module, ())):
                queue_scan(module, owner, arguments)

    def admit_imports(names, source_context):
        for name in names:
            explicit_imports.add(name)
            if name in source_context.seen_import_names:
                continue
            source_context.seen_import_names.add(name)
            for candidate in _module_dependency_authority._expand_module_chain_cached(
                name
            ):
                if (
                    candidate in source_context.stub_parents
                    or candidate.split(".", 1)[0] in source_context.skip_modules
                ):
                    continue
                if any(
                    candidate == prefix or candidate.startswith(prefix + ".")
                    for prefix in PLATFORM_EXCLUDED_SUBMODULES
                ):
                    continue
                resolved = resolve_candidate(candidate, source_context)
                if resolved is None:
                    continue
                policy = source_context.admission_policy
                if policy.owns_source_closure_with_native_artifact_plan(
                    candidate, resolved
                ):
                    merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
                if not policy.admits_import(
                    candidate,
                    resolved,
                    from_entry_path=source_context.allow_external_imports,
                ):
                    continue
                touched.add(candidate)
                prior = scan_sources.get(candidate)
                if prior is None or prior.native_support_function_roots:
                    queue_entry(resolved, candidate, source_context.dependency)

    demands = state.demands
    if demands is None:
        demands = state.demands = PythonSourceDemands(
            admit_imports=lambda module, names: admit_imports(
                names, state.contexts[module]
            ),
            queue_body=queue_scan,
        )
    else:
        # The solver is retained; only its fully drained graph-owner callbacks
        # are rebound to the current extension's result accumulator.
        demands.admit_imports = lambda module, names: admit_imports(
            names, state.contexts[module]
        )
        demands.queue_body = queue_scan

    def activate_namespace_writes(module_name, source_owner):
        state.active_owners.setdefault(module_name, set()).add(source_owner)
        by_owner = pending_namespace_writes.get(module_name, {})
        if scan_sources[module_name].mode == "full":
            writes = [write for rows in by_owner.values() for write in rows]
            by_owner.clear()
        else:
            writes = by_owner.pop(source_owner, ())
        if writes:
            for write in writes:
                if write.module is not None and write.construct not in {
                    "import_binding",
                    "import_binding_disjoint",
                }:
                    state.writer_targets.setdefault(write.module, set()).add(write)
            source_context = state.contexts[module_name]
            merge_namespace_writes(
                graph_python_namespace_writes(
                    writes,
                    lambda name: (
                        resolution_cache.resolved_path(resolved)
                        if (resolved := resolve_candidate(name, source_context))
                        is not None
                        else None
                    ),
                )
            )

    def accept_source_scan(
        module_name, path, scan, generation=None, snapshot=None, *, source_owner=None
    ):
        projected.add(module_name)
        source_context = state.contexts[module_name]
        if generation is not None:
            prior = source_generations.setdefault(module_name, (generation, snapshot))
            if generation != prior[0]:
                raise _module_source.PythonSourceChangedError(
                    f"source changed during deferred discovery: {path}"
                )
            if prior[1] is None and snapshot is not None:
                source_generations[module_name] = (generation, snapshot)
            authority = scan_sources[module_name]
            authority.validate_generation(generation)
            if authority.source_sha256 != generation:
                scan_sources[module_name] = replace(authority, source_sha256=generation)
        if source_owner is None:
            summary = scan.source_requests.namespace_summary
            if summary is None:
                merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
            else:
                known = state.namespace_writes.setdefault(module_name, set())
                pending = pending_namespace_writes.setdefault(module_name, {})
                for write in summary.foreign_writes:
                    if write not in known:
                        known.add(write)
                        pending.setdefault(write.source_owner, []).append(write)
                # A widened stable view may expose new rows for an owner that
                # already ran. The value solver intentionally will not enqueue
                # that same activation twice, so publish these deltas now.
                for owner in tuple(state.active_owners.get(module_name, ())):
                    activate_namespace_writes(module_name, owner)
        activate_namespace_writes(module_name, source_owner)
        demands.accept(
            module_name,
            scan.source_requests,
            full=scan_sources[module_name].mode == "full",
        )
        demands.drain()
        for requested_name, execution_path in scan.source_executions:
            execution_name = requested_name or resolution_cache.module_name_from_path(
                execution_path,
                list(source_context.module_roots),
                source_context.stdlib_root,
            )
            if not re.fullmatch(
                r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*", execution_name
            ):
                raise ValueError(
                    f"statically executed source uses invalid module name {execution_name!r}"
                )
            policy = source_context.admission_policy
            if policy.owns_source_closure_with_native_artifact_plan(
                execution_name, execution_path
            ):
                merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
            if not policy.admits_import(
                execution_name,
                execution_path,
                from_entry_path=source_context.allow_external_imports,
            ):
                continue
            existing = graph.get(execution_name)
            if existing is not None and resolution_cache.resolved_path(
                existing
            ) != resolution_cache.resolved_path(execution_path):
                raise ValueError(
                    f"module {execution_name!r} has multiple statically executed source authorities: {existing} and {execution_path}"
                )
            explicit_imports.add(execution_name)
            touched.add(execution_name)
            if (
                existing is None
                or scan_sources[execution_name].native_support_function_roots
            ):
                execution_scan = precomputed_scans_by_path.get(
                    resolution_cache.resolved_path(execution_path)
                )
                if execution_scan is not None and (
                    execution_scan.authority.module_name != execution_name
                    or execution_scan.authority.mode != "module_init"
                    or execution_scan.authority.native_support_function_roots
                ):
                    execution_scan = None
                queue_entry(
                    execution_path,
                    execution_name,
                    source_context.dependency,
                    precomputed=execution_scan,
                )
        admit_imports(
            tuple(
                dict.fromkeys((*scan.imports, *scan.dynamic_relative_import_candidates))
            ),
            source_context,
        )

    for path in reversed(tuple(entry_paths)):
        resolved_path = resolution_cache.resolved_path(path)
        precomputed = precomputed_scans_by_path.get(resolved_path)
        names = (
            (precomputed.authority.module_name,)
            if precomputed is not None
            else tuple(
                name
                for name, owner_path in runtime_import_custody.owners
                if owner_path == resolved_path
            )
            if runtime_import_custody is not None
            else ()
        )
        for name in names or (None,):
            queue_entry(
                path, name, context, full=full_scan_roots, precomputed=precomputed
            )
    admit_imports(entry_imports, context)

    while queue:
        job = queue.pop()
        if isinstance(job, _SourceScanJob):
            queued_scans.discard(job)
            module_name = job.module_name
            path = graph[module_name]
            if job.source is not None and scan_sources[module_name].mode == "full":
                # Full inventory already covers every body. Retained demands
                # from an earlier initialization view cannot request a partial
                # runtime-custody scan after this source is promoted.
                continue
            source_context = state.contexts[module_name]
            activate_namespace_writes(module_name, job.source)
            authority = scan_sources[module_name]
            receipt = (
                authority.namespace_premise,
                authority.native_support_function_roots,
                authority.mode,
                state.context_epochs[module_name],
            )
            if scan_receipts.get(job) == receipt:
                continue
            loaded = _load_module_import_scan(
                path,
                module_name=module_name,
                is_package=bool(authority.is_package),
                import_scan_mode=authority.mode
                if job.source is None
                else "module_init",
                source_owner=job.source,
                source_arguments=job.arguments,
                source_snapshot=source_generations.get(module_name, (None, None))[1],
                resolution_cache=resolution_cache,
                project_root=source_context.project_root,
                roots=source_context.roots,
                stdlib_root=source_context.stdlib_root,
                stdlib_allowlist=set(source_context.stdlib_allowlist),
                target_python=target_python,
                runtime_import_custody=source_context.runtime_import_custody,
                namespace_premise=authority.namespace_premise,
                native_support_function_roots=authority.native_support_function_roots,
            )
            scan_receipts[job] = receipt
            if job.source is not None:
                scanned_bodies.setdefault(module_name, set()).add(
                    (job.source, job.arguments)
                )
            scan_sources[module_name] = replace(
                authority,
                requires_runtime_package_anchor=authority.requires_runtime_package_anchor
                or loaded.scan.requires_runtime_package_anchor,
                uses_runtime_import_protocol=authority.uses_runtime_import_protocol
                or loaded.scan.source_requests.uses_runtime_import_protocol,
            )
            accept_source_scan(
                module_name,
                path,
                loaded.scan,
                loaded.snapshot.sha256 if loaded.snapshot is not None else None,
                loaded.snapshot,
                source_owner=job.source,
            )
            continue
        path, forced_name, requested_context, full, precomputed = job
        entry_key = (resolution_cache.resolved_path(path), forced_name)
        if queued_entries.get(entry_key) is not job:
            continue
        del queued_entries[entry_key]
        module_name = forced_name or resolution_cache.module_name_from_path(
            path, list(requested_context.module_roots), requested_context.stdlib_root
        )
        touched.add(module_name)
        previous = scan_sources.get(module_name)
        if previous is not None:
            previous.validate(module_name, path)
        admitted = previous or (
            enclosing_scan_authority.by_module.get(module_name)
            if enclosing_scan_authority is not None
            else None
        )
        if admitted is not None:
            admitted.validate(module_name, path)
        incoming = (
            precomputed.authority
            if precomputed is not None
            else _ModuleSourceScanAuthority(
                module_name,
                path,
                _source_import_scan._module_import_scan_mode(
                    module_name, full_scan=full
                ),
                admitted.is_package if admitted is not None else None,
                package_paths=admitted.package_paths if admitted is not None else (),
                # An ordinary Python import requests the original initializer.
                source_sha256=previous.source_sha256 if previous is not None else None,
            )
        )
        if precomputed is not None and incoming.mode != (
            "full" if full else "module_init"
        ):
            raise ValueError(
                f"precomputed source scan lost requested depth: {module_name!r}"
            )
        authority = (
            _ModuleGraphScanAuthority((previous,))
            .merged(_ModuleGraphScanAuthority((incoming,)))
            .by_module[module_name]
            if previous is not None
            else incoming
        )
        previous_context = state.contexts.get(module_name)
        source_context = requested_context
        if (
            previous_context is not None
            and previous is not None
            and previous.source_sha256 is not None
            and not previous.native_support_function_roots
        ):
            # Full-depth runtime custody can widen an existing source without
            # changing its original import roots or application admission rules.
            source_context = (
                replace(
                    previous_context,
                    runtime_import_custody=requested_context.runtime_import_custody,
                    seen_import_names=set(),
                )
                if requested_context.runtime_import_custody is not None
                else previous_context
            )
        if previous_context == source_context:
            source_context = previous_context
        else:
            state.context_epochs[module_name] = (
                state.context_epochs.get(module_name, 0) + 1
            )
        state.contexts[module_name] = source_context
        graph[module_name] = resolution_cache.resolved_path(path)
        modules_by_source.setdefault(graph[module_name], set()).add(module_name)
        authority = replace(
            authority,
            namespace_premise=namespace_premise(path, authority.package_paths),
        )
        scan_sources[module_name] = authority
        # A newly admitted name can resolve an earlier writer's topology lease.
        # Index by receiver identity so this visits only that name's active rows.
        if previous is None and (writes := state.writer_targets.get(module_name)):
            merge_namespace_writes(
                graph_python_namespace_writes(writes, lambda name: graph.get(name))
            )
            authority = scan_sources[module_name]
        receipt = (
            authority.namespace_premise,
            authority.native_support_function_roots,
            authority.mode,
            state.context_epochs[module_name],
        )
        if scan_receipts.get(_SourceScanJob(module_name, None)) == receipt:
            continue
        policy = source_context.admission_policy
        if policy.owns_source_closure_with_native_artifact_plan(module_name, path):
            merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
            if precomputed is not None:
                _validate_precomputed_module_import_scan(
                    precomputed,
                    path=path,
                    module_name=module_name,
                    import_scan_mode=authority.mode,
                    target_python=target_python,
                )
            accepted_scan = (
                precomputed.scan
                if precomputed is not None
                else _CompleteImportScan(_ImportScanRequests((), ()), (), ())
            )
            # No scan proves what an unscanned source imports.
            scan_sources[module_name] = replace(
                scan_sources[module_name],
                uses_runtime_import_protocol=scan_sources[
                    module_name
                ].uses_runtime_import_protocol
                or precomputed is None
                or accepted_scan.source_requests.uses_runtime_import_protocol,
            )
            accept_source_scan(
                module_name,
                path,
                accepted_scan,
                precomputed.authority.source_sha256
                if precomputed is not None
                else None,
            )
            continue
        # A retained full/raw authority may dominate a narrower supplied view.
        # Its cached scan cannot be rebound to different selection or depth.
        if precomputed is not None and (
            precomputed.authority.mode != authority.mode
            or precomputed.authority.native_support_function_roots
            != authority.native_support_function_roots
        ):
            precomputed = None
        try:
            loaded = _load_module_import_scan(
                path,
                module_name=module_name,
                is_package=bool(authority.is_package),
                import_scan_mode=authority.mode,
                resolution_cache=resolution_cache,
                project_root=source_context.project_root,
                roots=source_context.roots,
                stdlib_root=source_context.stdlib_root,
                stdlib_allowlist=set(source_context.stdlib_allowlist),
                target_python=target_python,
                runtime_import_custody=source_context.runtime_import_custody,
                precomputed_scan=precomputed,
                namespace_premise=authority.namespace_premise,
                native_support_function_roots=authority.native_support_function_roots,
                source_snapshot=source_generations.get(module_name, (None, None))[1],
            )
        except (OSError, SyntaxError, UnicodeDecodeError):
            merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
            # A source that cannot be analyzed may import at run time.
            scan_sources[module_name] = replace(
                scan_sources[module_name], uses_runtime_import_protocol=True
            )
            continue
        scan_receipts[_SourceScanJob(module_name, None)] = receipt
        scan_sources[module_name] = replace(
            scan_sources[module_name],
            requires_runtime_package_anchor=authority.requires_runtime_package_anchor
            or loaded.scan.requires_runtime_package_anchor,
            uses_runtime_import_protocol=authority.uses_runtime_import_protocol
            or loaded.scan.source_requests.uses_runtime_import_protocol,
        )
        accept_source_scan(
            module_name,
            path,
            loaded.scan,
            loaded.snapshot.sha256 if loaded.snapshot is not None else None,
            loaded.snapshot,
        )
        if (
            previous is not None
            and previous.native_support_function_roots
            != authority.native_support_function_roots
        ):
            for owner, arguments in sorted(scanned_bodies.get(module_name, ())):
                queue_scan(module_name, owner, arguments)
    state.validate_sources(touched | projected)
    for module_name in projected:
        authority = scan_sources[module_name]
        scan_sources[module_name] = replace(
            authority,
            deferred_owners=authority.deferred_owners
            | scanned_bodies.get(module_name, set()),
        )
    result_graph = {name: graph[name] for name in sorted(touched) if name in graph}
    return _DiscoveredModuleGraph(
        result_graph,
        explicit_imports,
        _ModuleGraphScanAuthority(tuple(scan_sources[name] for name in result_graph)),
        state,
        tuple(
            scan_sources[name]
            for name in sorted((revised | projected) - result_graph.keys())
        ),
    )


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_scan: _PrecomputedModuleImportScan | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    discovery: _ModuleGraphDiscovery | None = None,
) -> _DiscoveredModuleGraph:
    return _discover_module_graph_from_paths(
        (entry_path,),
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        resolver_cache=resolver_cache,
        precomputed_scans_by_path=(
            {entry_path.resolve(): precomputed_scan}
            if precomputed_scan is not None
            else None
        ),
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        full_scan_roots=True,
        discovery=discovery,
    )


@_source_tree_fingerprint_transaction()
def _load_module_imports(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    tree: ast.AST,
    resolution_cache: _module_resolution._ModuleResolutionCache,
    project_root: Path | None,
    roots: Sequence[Path] | None = None,
    stdlib_root: Path | None = None,
    stdlib_allowlist: set[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    namespace_premise: PythonNamespacePremise | None = None,
) -> tuple[str, ...]:
    return _load_module_import_scan(
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        tree=tree,
        resolution_cache=resolution_cache,
        project_root=project_root,
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        target_python=target_python,
        runtime_import_custody=runtime_import_custody,
        namespace_premise=namespace_premise,
    ).scan.imports


@_source_tree_fingerprint_transaction()
def _load_module_import_scan(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    resolution_cache: _module_resolution._ModuleResolutionCache,
    project_root: Path | None,
    tree: ast.AST | None = None,
    source: str | None = None,
    source_snapshot: _module_source.PythonSourceSnapshot | None = None,
    source_filename: str | None = None,
    retain_source: bool = True,
    retain_tree: bool = True,
    roots: Sequence[Path] | None = None,
    stdlib_root: Path | None = None,
    stdlib_allowlist: set[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    precomputed_scan: _PrecomputedModuleImportScan | None = None,
    namespace_premise: PythonNamespacePremise | None = None,
    native_support_function_roots: tuple[str, ...] = (),
) -> _LoadedModuleImportScan:
    """Admit source-only requests, then resolve filesystem edges in this operation."""
    if native_support_function_roots:
        native_support_function_roots = canonical_native_support_roots(
            native_support_function_roots
        )
    if runtime_import_custody is not None and not runtime_import_custody.owns(
        module_name, path.resolve()
    ):
        runtime_import_custody = None
    runtime_catalog_owner = runtime_import_custody is not None
    if runtime_import_custody is not None:
        if source_owner is not None:
            raise ValueError("runtime import custody requires full-depth owner scans")
        runtime_import_custody.validate_scan_mode(
            module_name, path.resolve(), import_scan_mode
        )
    if source_owner is not None and precomputed_scan is not None:
        raise ValueError(
            "precomputed module scan cannot substitute for a deferred body"
        )
    if precomputed_scan is not None:
        if runtime_import_custody is not None:
            raise ValueError(
                "strict precomputed scans cannot certify runtime owner custody"
            )
        if precomputed_scan.authority.is_package != is_package:
            raise ValueError(
                f"precomputed package scan authority changed: {module_name!r}"
            )
        if (
            precomputed_scan.authority.native_support_function_roots
            != native_support_function_roots
        ):
            raise ValueError(f"precomputed source selection changed: {module_name!r}")
        _validate_precomputed_module_import_scan(
            precomputed_scan,
            path=path,
            module_name=module_name,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
        )
        if (
            source is None
            and tree is None
            and precomputed_scan.authority.namespace_premise == namespace_premise
        ):
            snapshot = source_snapshot or _module_source.PythonSourceSnapshot.capture(
                path
            )
            if snapshot.path != path:
                raise ValueError("source scan snapshot does not match supplied source")
            if snapshot.sha256 != precomputed_scan.authority.source_sha256:
                raise _module_source.PythonSourceChangedError(
                    f"precomputed source scan lost custody: {module_name!r}; "
                    f"source changed or became unavailable: {path}"
                )
            completed = _module_import_scanner._complete_import_scan(
                precomputed_scan.scan.source_requests,
                source_path=path,
                roots=roots,
                stdlib_root=stdlib_root,
                stdlib_allowlist=stdlib_allowlist,
                resolution_cache=resolution_cache,
                target_python=target_python,
            )
            return _LoadedModuleImportScan(
                completed,
                False,
                False,
                snapshot.text if retain_source else None,
                None,
                snapshot,
            )
    # Explicit source or AST is operation-local and must not be overridden by disk.
    if tree is not None or source is not None:
        project_root = None
    snapshot = source_snapshot
    if snapshot is not None:
        if snapshot.path != path:
            raise ValueError("source scan snapshot does not match supplied source")
        captured_source = snapshot.text_for_filename(
            str(path) if source_filename is None else source_filename
        )
        if source is not None and source != captured_source:
            raise ValueError("source scan snapshot does not match supplied source")
        source = captured_source
    elif source is None and tree is None:
        snapshot = _module_source.PythonSourceSnapshot.capture(path)
        source = snapshot.text_for_filename(
            str(path) if source_filename is None else source_filename
        )

    if snapshot is not None and source is not None and retain_source:
        resolution_cache.source_cache[resolution_cache.resolved_path(path)] = source

    def complete(requests: _ImportScanRequests) -> _CompleteImportScan:
        if runtime_import_custody is not None:
            requests = runtime_import_custody.complete_owner_scan(
                module_name, path.resolve(), requests
            )
        elif requests.runtime_catalog is not None:
            raise ValueError(
                f"a runtime catalog deferral requires runtime import custody: {module_name!r}"
            )
        return _module_import_scanner._complete_import_scan(
            requests,
            source_path=path,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=resolution_cache,
            target_python=target_python,
        )

    if project_root is not None:
        persisted = _module_graph_cache._read_persisted_import_scan_record(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            source_owner=source_owner,
            source_arguments=source_arguments,
            snapshot=snapshot,
            target_python=target_python,
            namespace_premise=namespace_premise,
            native_support_function_roots=native_support_function_roots,
            runtime_catalog_owner=runtime_catalog_owner,
        )
        if persisted is not None:
            return _LoadedModuleImportScan(
                complete(persisted), True, False, source, None, snapshot
            )

    source_parsed = False
    if tree is None:
        assert source is not None
        previous_parse_count = resolution_cache.source_parse_count
        parsed = resolution_cache.parse_module_ast(
            path,
            source,
            filename=str(path) if source_filename is None else source_filename,
            retain=retain_tree or bool(native_support_function_roots),
            target_python=target_python,
        )
        tree = parsed.tree
        ast_digest_admission = parsed.admission
        source_parsed = resolution_cache.source_parse_count != previous_parse_count
    else:
        ast_digest_admission = _PythonAstDigestAdmission(tree)
    assert tree is not None
    source_pruned = False
    if native_support_function_roots:
        if not isinstance(tree, ast.Module):
            raise ValueError("native support selection requires a module source")
        selected, source_pruned = resolution_cache.native_support_view(
            path,
            tree,
            native_support_function_roots,
            ast_digest_admission=ast_digest_admission,
            target_python=target_python,
        )
        tree, ast_digest_admission = selected.tree, selected.admission
    import_projection = resolution_cache.collect_graph_imports(
        path,
        tree,
        collector=_source_import_scan._collect_imports_for_graph,
        target_python=target_python,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        runtime_import_custody=runtime_import_custody,
        ast_digest_admission=ast_digest_admission,
        namespace_premise=namespace_premise,
    )
    requests = _source_import_scan._collect_import_scan_requests(
        import_projection,
        tree,
        source_path=path,
        module_name=module_name,
        import_scan_mode=import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
        source=source,
        namespace_premise=namespace_premise,
    )
    if project_root is not None and snapshot is not None:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                source_owner=source_owner,
                source_arguments=source_arguments,
                scan=requests,
                snapshot=snapshot,
                target_python=target_python,
                namespace_premise=namespace_premise,
                native_support_function_roots=native_support_function_roots,
                runtime_catalog_owner=runtime_catalog_owner,
            )
    return _LoadedModuleImportScan(
        complete(requests), False, source_parsed, source, tree, snapshot, source_pruned
    )
