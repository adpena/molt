from __future__ import annotations

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

from molt.compiler_analysis import python_import_scan as _source_import_scan
from molt.compiler_analysis.python_source_demands import (
    PythonSourceDemands,
    SourceCandidate,
    SourceMember,
)
from molt.compiler_analysis.native_support_slice import (
    canonical_native_support_roots,
)

import ast
import contextlib
import re
from dataclasses import dataclass, field, replace
from collections.abc import Callable, Collection, Mapping, MutableMapping, Sequence
from pathlib import Path
from functools import cached_property
from typing import Final, NamedTuple

from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
from molt.cli import module_dependencies as _module_dependency_authority
from molt.cli import module_graph_cache as _module_graph_cache
from molt.cli import module_import_scanner as _module_import_scanner
from molt.cli import module_resolution as _module_resolution
from molt.cli import module_source as _module_source
from molt.cli.models import (
    ModuleReason,
    UnresolvedRoot,
    _CompleteImportScan,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _PrecomputedModuleImportScan,
    _ImportAdmissionPolicy,
    _RuntimeImportScanCustody,
)
from molt.compiler_analysis.python_imports import (
    ImportScanMode,
    _ImportDiscoveryProjection,
    _ImportScanRequests,
)
from molt.compiler_analysis.python_import_scan import (
    SCAN_PREMISE_READS,
    ScanPremiseReads,
)
from molt.compiler_analysis.python_namespace_graph import (
    PythonNamespacePremise,
    PythonGraphNamespaceIndex,
    UNKNOWN_GRAPH_NAMESPACE_WRITES,
    graph_python_namespace_writes,
    join_python_namespace_premises,
    python_namespace_name_universe,
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


@dataclass(frozen=True)
class _DiscoveredModuleGraph:
    graph: dict[str, Path]
    explicit_imports: set[str]
    scan_authority: _ModuleGraphScanAuthority
    discovery: "_ModuleGraphDiscovery | None" = field(
        default=None, compare=False, repr=False
    )
    revised_sources: tuple[_ModuleSourceScanAuthority, ...] = ()

    def __post_init__(self) -> None:
        self.scan_authority.validate_graph(self.graph)


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


def _graph_import_scan_purpose(
    selection: ModuleBodySelection, source_owner: PythonSourceKey | None
) -> _source_import_scan.SourceImportPurpose:
    return (
        _source_import_scan.SourceImportPurpose.INVENTORY
        if selection.kind == "native_support" and source_owner is None
        else _source_import_scan.SourceImportPurpose.EXECUTION
    )


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
        self.target_sys_platform = resolver_cache.target_sys_platform
        self.graph: dict[str, Path] = {}
        self.scan_sources: dict[str, _ModuleSourceScanAuthority] = {}
        # Each scanned source's name universe: its premise view keys receipts.
        self.scan_universes: dict[str, frozenset[str] | None] = {}
        # The premise each module's last module-level scan read. Its body scans
        # read the same premise, so they share that scan's binding analysis; a
        # body scan never reads a premise behind its module's current one.
        self.scan_premises: dict[str, PythonNamespacePremise | None] = {}
        # Body scans whose module premise moved past its pin: they wait,
        # unscanned and without outputs, until that module's own scan settles.
        self.deferred_bodies: dict[str, set[_SourceScanJob]] = {}
        # Each unit's latest scan output. A rescan with a different output
        # supersedes the earlier one, whose accumulated facts only a replay
        # can drop; the flag records that for the last pass.
        self.scan_outputs: dict[_SourceScanJob, _CompleteImportScan] = {}
        # Each unit's latest scan with the premise answers it read: a unit
        # whose answers hold under a new premise keeps its output unscanned.
        self.scan_loads: dict[_SourceScanJob, _LoadedModuleImportScan] = {}
        self.superseded_scans = False
        # The premise each module's admitted outputs derive at the end of a
        # pass (``_discover_module_graph_from_paths``, replay convergence).
        self.derived_premises: dict[str, PythonNamespacePremise | None] = {}
        # Every call this operation made, and each unit's scan outputs by
        # premise view: a continuation replays the whole history.
        self.invocations: tuple[_DiscoveryInvocation, ...] = ()
        self.scan_memo: dict[tuple[object, ...], _LoadedModuleImportScan] = {}
        # Set once a call keeps its accumulated outputs. The caller has
        # published that graph, and no replay of the history reproduces it, so
        # every later continuation of the operation accumulates too.
        self.accumulated = False
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
        self.scan_revisions: set[_SourceScanJob] = set()
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
            if snapshot is not None:
                snapshot.report_input()
                current = _module_source.PythonSourceSnapshot.capture(snapshot.path)
                if current.content != snapshot.content:
                    raise _module_source.PythonSourceChangedError(
                        f"source changed during deferred discovery: {snapshot.path}"
                    )


def _ready_namespace_revisions(
    state: _ModuleGraphDiscovery,
) -> set[_SourceScanJob]:
    """Wake dependent revisions once their currently known writers settle."""
    dirty = {job.module_name for job in state.scan_revisions}
    if len(dirty) < 2:
        return state.scan_revisions.copy()

    writers: dict[str, set[str]] = {}
    for writer, writes in state.namespace_writes.items():
        active = state.active_owners.get(writer, ())
        for write in writes:
            target = state.graph.get(write.module) if write.module is not None else None
            if target is not None and write.source_owner in active:
                for name in state.modules_by_source[target]:
                    if writer != name:
                        writers.setdefault(name, set()).add(writer)
    if not writers:
        return state.scan_revisions.copy()

    ready: set[str] = set()
    pending: list[Collection[str]] = [state.graph.keys()]
    while pending:
        components = _module_dependency_authority._module_strongly_connected_components(
            pending.pop(), writers
        )
        blocked: set[str] = set()
        for component in components:
            if any(
                not blocked.isdisjoint(writers.get(member, ())) for member in component
            ):
                # A clean component can relay a pending writer's effects.
                blocked.update(component)
            elif not dirty.isdisjoint(component):
                # The admission pass already evaluated the head. During a
                # revision, settle the interior under that head's output.
                # A new head result can reawaken the interior on the next wave.
                head, *interior = component
                if dirty.isdisjoint(interior):
                    ready.add(head)
                else:
                    pending.append(interior)
                # No dependent runs until this entire component is clean,
                # even when only one nested component runs in this wave.
                blocked.update(component)
    return {job for job in state.scan_revisions if job.module_name in ready}


class _LoadedModuleImportScan(NamedTuple):
    """One complete scan plus its operation-local source/parse outcome."""

    scan: _CompleteImportScan
    cache_hit: bool
    source_parsed: bool
    source: str | None
    tree: ast.AST | None
    snapshot: _module_source.PythonSourceSnapshot | None = None
    source_pruned: bool = False
    # The early-cutoff witness of HF-926, when the caller asked for it
    # (``record_premise_reads``).
    premise_reads: "_ScanCutoff | None" = None


@dataclass(frozen=True, slots=True)
class _ScanCutoff:
    """What one scan read of its premise, and how to republish its result.

    ``view`` projects a premise as the loader projected the scanned one;
    ``publish`` records that the scan under another premise equals this one
    (a key row aliasing the stored result), when the scan was persisted.
    """

    reads: ScanPremiseReads
    view: Callable[[PythonNamespacePremise | None], PythonNamespacePremise | None]
    publish: Callable[[PythonNamespacePremise | None], None] | None

    def unchanged_under(self, premise: PythonNamespacePremise | None) -> bool:
        return self.reads.unchanged_under(self.view(premise))


def _bind_precomputed_module_import_scan(
    path: Path,
    *,
    module_name: str,
    import_scan_mode: ImportScanMode,
    purpose: _source_import_scan.SourceImportPurpose,
    scan: _CompleteImportScan,
    snapshot: _module_source.PythonSourceSnapshot,
    is_package: bool | None = None,
    package_paths: tuple[Path, ...] = (),
    native_support_function_roots: tuple[str, ...] = (),
    target_sys_platform: str | None = None,
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
            body_selection=ModuleBodySelection.from_requested_roots(
                native_support_function_roots
            ),
        ),
        target_python.tag,
        target_sys_platform,
        scan,
        purpose,
    )


def _validate_precomputed_module_import_purpose(
    record: _PrecomputedModuleImportScan,
    purpose: _source_import_scan.SourceImportPurpose,
) -> None:
    if record.purpose is not purpose:
        raise ValueError(
            "precomputed source scan lost purpose custody: "
            f"{record.authority.module_name!r}"
        )


def _validate_precomputed_module_import_scan(
    record: _PrecomputedModuleImportScan,
    *,
    path: Path,
    module_name: str,
    import_scan_mode: ImportScanMode,
    purpose: _source_import_scan.SourceImportPurpose,
    target_sys_platform: str | None = None,
    target_python: TargetPythonVersion,
) -> None:
    _validate_precomputed_module_import_purpose(record, purpose)
    record.authority.validate(module_name, path)
    if (
        record.authority.mode != import_scan_mode
        or bool(record.authority.deferred_owners)
        or record.authority.requires_runtime_package_anchor
        != record.scan.requires_runtime_package_anchor
        or record.authority.uses_runtime_import_protocol
        != record.scan.source_requests.uses_runtime_import_protocol
        or record.target_python_tag != target_python.tag
        or record.target_sys_platform != target_sys_platform
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


def _record_module_reason(
    module_reasons: MutableMapping[str, set[ModuleReason]],
    module_name: str,
    reason: ModuleReason,
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
    module_reasons: MutableMapping[str, set[ModuleReason]],
    root_reason: ModuleReason,
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
) -> _DiscoveredModuleGraph:
    """Admit the closure of ``entry_paths``. Their modules carry ``root_reason``;
    every other module the closure reaches carries ``root_reason.closure``."""
    if not root_reason.is_root:
        raise ValueError(f"closure producers stamp a root reason, not {root_reason}")
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
    root_paths = {path.resolve() for path in entry_paths}
    for name, path in closure.graph.items():
        _record_module_reason(
            module_reasons,
            name,
            root_reason if path.resolve() in root_paths else root_reason.closure,
        )
    if runtime_import_custody is not None:
        runtime_import_custody.validate_graph(module_graph)
    return closure


def _resolve_include_module_paths(
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
            errors.append(str(UnresolvedRoot(module_name, ModuleReason.EXPLICIT_ROOT)))
            continue
        if import_admission_policy is not None and not (
            import_admission_policy.admits_import(
                module_name,
                path,
                from_entry_path=False,
            )
        ):
            errors.append(
                f"include module {module_name!r} resolves under an external root "
                "but is not within an admitted external static package"
            )
            continue
        resolved[module_name] = path
    return resolved, errors


@_source_tree_fingerprint_transaction()
def _extend_module_graph_with_include_modules(
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
    module_reasons: MutableMapping[str, set[ModuleReason]],
    import_admission_policy: _ImportAdmissionPolicy | None,
    target_python: TargetPythonVersion,
    discovery: _ModuleGraphDiscovery | None = None,
) -> list[str]:
    if not module_names:
        return []
    resolved, errors = _resolve_include_module_paths(
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
        root_reason=ModuleReason.EXPLICIT_ROOT,
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
    module_reasons: MutableMapping[str, set[ModuleReason]],
    reason: ModuleReason,
) -> None:
    for name in module_graph:
        if name in before_names:
            continue
        _record_module_reason(module_reasons, name, reason)


@dataclass(frozen=True, slots=True)
class _DiscoveryInvocation:
    """One call of ``_discover_module_graph_from_paths``, kept so a replay can
    rebuild the operation's graph from its current scan outputs."""

    entry_paths: Sequence[Path]
    roots: list[Path]
    module_roots: list[Path]
    stdlib_root: Path
    project_root: Path | None
    stdlib_allowlist: set[str]
    skip_modules: set[str] | None
    stub_parents: set[str] | None
    precomputed_scans_by_path: Mapping[Path, _PrecomputedModuleImportScan] | None
    enclosing_scan_authority: "_ModuleGraphScanAuthority | None"
    import_admission_policy: _ImportAdmissionPolicy | None
    allow_entry_external_imports: bool
    target_python: TargetPythonVersion
    runtime_import_custody: _RuntimeImportScanCustody | None
    full_scan_roots: bool
    entry_imports: Collection[str]
    native_callable_exports: Mapping[str, tuple[str, ...]] | None

    def run(
        self,
        resolver: _module_resolution._ModuleResolutionCache | None,
        state: "_ModuleGraphDiscovery | None",
        settled: Mapping[str, PythonNamespacePremise | None] | None,
        memo: dict[tuple[object, ...], "_LoadedModuleImportScan"],
    ) -> _DiscoveredModuleGraph:
        return _discovery_pass(
            self.entry_paths,
            self.roots,
            self.module_roots,
            self.stdlib_root,
            self.project_root,
            self.stdlib_allowlist,
            self.skip_modules,
            self.stub_parents,
            resolver if state is None else state.resolver_cache,
            self.precomputed_scans_by_path,
            self.enclosing_scan_authority,
            self.import_admission_policy,
            self.allow_entry_external_imports,
            self.target_python,
            self.runtime_import_custody,
            full_scan_roots=self.full_scan_roots,
            discovery=state,
            entry_imports=self.entry_imports,
            native_callable_exports=self.native_callable_exports,
            settled_premises=settled,
            scan_memo=memo,
        )


# Replays a call tries before it keeps its first pass's accumulated outputs,
# which cover every premise that pass read.
_MAX_DISCOVERY_REPLAYS: Final = 8


# Rows that write a module namespace by naming or reaching it, as opposed to
# member stores the graph joins with escaped module values.
_DIRECT_NAMESPACE_WRITES: Final = frozenset(
    {"attribute", "setattr", "namespace", "reflection", "import_binding"}
)


def _written_names(premise: PythonNamespacePremise | None) -> object:
    """The names other code writes into a module, as its scan view reads them."""
    return None if premise is None else (premise.foreign_writes,)


def _joined(
    left: PythonNamespacePremise | None, right: PythonNamespacePremise | None
) -> PythonNamespacePremise | None:
    """The least premise above both (no premise is unbounded)."""
    if left == right:
        return left
    return join_python_namespace_premises(left, right)


def _settled(premise: PythonNamespacePremise | None) -> PythonNamespacePremise | None:
    """A premise as a scan reads it: exact receivers join after discovery."""
    return None if premise is None else replace(premise, exact_receivers=())


def _runtime_catalog_unadmitted(
    custody: _RuntimeImportScanCustody, graph: Mapping[str, Path]
) -> str | None:
    """The first runtime catalog row, other than an owner the call admits,
    that the enclosing graph does not hold at its path."""
    for name, path in custody.catalog:
        if name not in custody.owners_by_module and (
            name not in graph or graph[name].resolve() != path
        ):
            return name
    return None


def _replay_history(
    history: Sequence[_DiscoveryInvocation],
    resolver: _module_resolution._ModuleResolutionCache,
    settled: Mapping[str, PythonNamespacePremise | None],
    memo: dict[tuple[object, ...], _LoadedModuleImportScan],
) -> _DiscoveredModuleGraph | None:
    """Replay every invocation of an operation from a fresh state: the graph
    is what the current outputs reach, never an earlier scan's. None when a
    step's runtime catalog, built from the published graph, names a module
    the replay lost: the replay cannot reproduce what the caller published."""
    replay: _DiscoveredModuleGraph | None = None
    for step in history:
        state = None if replay is None else replay.discovery
        custody = step.runtime_import_custody
        if (
            custody is not None
            and _runtime_catalog_unadmitted(
                custody, {} if state is None else state.graph
            )
            is not None
        ):
            return None
        replay = step.run(resolver, state, settled, memo)
    return replay


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
    native_callable_exports: Mapping[str, tuple[str, ...]] | None = None,
) -> _DiscoveredModuleGraph:
    """Discover a graph whose edges, demands and premises are the union of
    each scan unit's current output, never the history of its scans (HF-926).

    The first pass finds the units and settles their premises; its scans may
    read a premise behind the latest one, and their outputs accumulate. A
    replay from a fresh state then admits only each reached unit's output
    under the settled premises (memoized: no unit scans twice for the same
    inputs) and derives the premises again. When the derived premises
    reproduce the settled ones, the replay's graph is the answer; when no
    premise reproduces itself, the first pass's accumulated graph is. A pass
    that scanned no unit twice needs no replay. A continuation
    (``discovery``) extends a replayed state in place. The caller publishes
    each call's graph additively, so a continuation never drops a module an
    earlier call published: when its replay would, it keeps its accumulated
    outputs. Once any call kept them, no later call replays: a replay of that
    call reaches only one premise's graph, not the published accumulation.
    """

    invocation = _DiscoveryInvocation(
        entry_paths,
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules,
        stub_parents,
        precomputed_scans_by_path,
        enclosing_scan_authority,
        import_admission_policy,
        allow_entry_external_imports,
        target_python,
        runtime_import_custody,
        full_scan_roots,
        entry_imports,
        native_callable_exports,
    )
    history = (*(() if discovery is None else discovery.invocations), invocation)
    memo = {} if discovery is None else discovery.scan_memo
    # The caller's published rows: a continuation reports each one it changes.
    prior_sources = {} if discovery is None else dict(discovery.scan_sources)
    prior_solver = None if discovery is None else discovery.demands
    # The caller's published modules: a replay must reach each of them.
    prior_graph = {} if discovery is None else dict(discovery.graph)
    accumulated = discovery is not None and discovery.accumulated
    result = invocation.run(resolver_cache, discovery, None, memo)
    assert result.discovery is not None
    if not result.discovery.superseded_scans or accumulated:
        # No rescan changed a unit's output, so the accumulated outputs are the
        # current ones; or an earlier call published its accumulated graph.
        result.discovery.invocations = history
        result.discovery.scan_memo = memo
        return result
    resolver = result.discovery.resolver_cache
    settled = {
        name: _settled(premise)
        for name, premise in result.discovery.derived_premises.items()
    }
    seen = {tuple(sorted(settled.items()))}
    for _ in range(_MAX_DISCOVERY_REPLAYS):
        replay = _replay_history(history, resolver, settled, memo)
        if replay is None:
            break
        replay_state = replay.discovery
        assert replay_state is not None
        derived = {
            name: _settled(premise)
            for name, premise in replay_state.derived_premises.items()
        }
        if all(settled.get(name) == premise for name, premise in derived.items()):
            # The current outputs derive exactly the premises they read: the
            # replay's graph is closed under them.
            if any(
                replay_state.graph.get(name) != path
                for name, path in prior_graph.items()
            ):
                # It drops a module the caller published.
                break
            replay_state.invocations = history
            replay_state.scan_memo = memo
            if discovery is None:
                return replay
            # The caller holds its continuation state: it becomes the replay.
            # A replay settles every step's premises at once, so the rows this
            # call changed are those that differ from the caller's own.
            vars(discovery).clear()
            vars(discovery).update(vars(replay_state))
            if prior_solver is not None and discovery.demands is not prior_solver:
                # So does the solver a caller already holds.
                vars(prior_solver).clear()
                vars(prior_solver).update(vars(discovery.demands))
                discovery.demands = prior_solver
            return replace(
                replay,
                discovery=discovery,
                revised_sources=tuple(
                    authority
                    for name, authority in sorted(discovery.scan_sources.items())
                    if name in prior_sources
                    and name not in replay.graph
                    and prior_sources[name] != authority
                ),
            )
        joined = {
            name: _joined(settled.get(name, premise), premise)
            for name, premise in derived.items()
        }
        # Below the settled premises, read the derived ones next: a scan's
        # output need not shrink with its premise, so only a premise its own
        # outputs reproduce is closed. Above them, widen to the join.
        settled = (
            derived
            if all(settled.get(name, p) == p for name, p in joined.items())
            else joined
        )
        key = tuple(sorted(settled.items()))
        if key in seen:
            break
        seen.add(key)
    # No premise reproduces itself, or a replay loses a module the caller
    # published: the first pass accumulated the outputs of every premise it
    # read, which covers each candidate's graph.
    result.discovery.invocations = history
    result.discovery.scan_memo = memo
    result.discovery.accumulated = True
    return result


def _discovery_pass(
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
    native_callable_exports: Mapping[str, tuple[str, ...]] | None = None,
    settled_premises: Mapping[str, PythonNamespacePremise | None] | None = None,
    scan_memo: dict[tuple[object, ...], _LoadedModuleImportScan] | None = None,
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
        or state.target_sys_platform != resolution_cache.target_sys_platform
    ):
        raise ValueError("module discovery continuation lost target/resolver custody")
    if state.queue or state.scan_revisions or state.deferred_bodies:
        raise ValueError("module discovery continuation has unfinished source jobs")
    state.superseded_scans = False
    if runtime_import_custody is not None:
        if not full_scan_roots:
            raise ValueError("runtime import custody requires full-depth owner scans")
        missing = _runtime_catalog_unadmitted(runtime_import_custody, state.graph)
        if missing is not None:
            raise ValueError(
                f"runtime catalog lacks live source discovery: {missing!r}"
            )
    precomputed_scans_by_path = {
        path.resolve(): scan for path, scan in (precomputed_scans_by_path or {}).items()
    }
    for record in precomputed_scans_by_path.values():
        # Check the incoming request before queue coalescing, receipt reuse or
        # raw-source dominance can discard a narrower precomputed view.
        _validate_precomputed_module_import_purpose(
            record, _graph_import_scan_purpose(record.authority.body_selection, None)
        )
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
    scan_universes, scan_premises = state.scan_universes, state.scan_premises
    queue, queued_scans = state.queue, state.queued_scans
    scan_revisions, deferred_bodies = state.scan_revisions, state.deferred_bodies
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
                or previous.precomputed.authority.body_selection.kind
                == "complete_source"
            )
            requested_raw = (
                precomputed is None
                or precomputed.authority.body_selection.kind == "complete_source"
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
                    previous.precomputed.authority.body_selection.roots
                    != precomputed.authority.body_selection.roots
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
        if job in deferred_bodies.get(module, ()):
            # Already waiting for its module's scan to settle.
            return
        if job not in queued_scans:
            if job in scan_receipts:
                # New admissions and first body demands may contribute more
                # namespace facts. Revisit completed projections only after
                # that work drains, using the entire latest premise once.
                scan_revisions.add(job)
            else:
                queued_scans.add(job)
                queue.append(job)

    def release_deferred_bodies(module: str, *, forced: bool = False) -> None:
        # A settled module premise: each waiting body scans once, now.
        pinned = scan_premises.get(module)
        if not forced and scan_view(module, pinned) != scan_view(
            module, scan_sources[module].namespace_premise
        ):
            return
        for job in sorted(
            deferred_bodies.pop(module, ()),
            key=lambda item: (item.source or (), item.arguments),
            reverse=True,
        ):
            scan_revisions.discard(job)
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

    def namespace_premise(
        path: Path,
        package_paths: tuple[Path, ...],
        source_context: _SourceDiscoveryContext,
        module_name: str,
        *,
        settled: bool = True,
    ) -> PythonNamespacePremise | None:
        if settled and settled_premises is not None and module_name in settled_premises:
            return settled_premises[module_name]
        path = resolution_cache.resolved_path(path)
        exclusions = resolution_cache.noncanonical_imports(
            source_context.roots,
            source_context.stdlib_root,
            source_context.stdlib_allowlist,
        )
        # Premises are computed only once the demand queue below exists.
        assert demands is not None
        opaque = demands.opaque_activation_sources(module_name)
        policy = source_context.admission_policy
        if policy.owns_source_closure_with_native_artifact_plan(module_name, path):
            opaque = None
        # A native extension's C code can read any function's code object.
        native_code = bool(policy.native_artifact_plan.artifacts)
        bounded = namespace_index.premise(
            path,
            frozenset(),
            noncanonical_imports=exclusions,
            opaque_activation_sources=opaque,
            native_code=native_code,
        )
        if not package_paths or bounded.foreign_writes is None:
            return bounded
        if package_paths not in package_children:
            try:
                package_children[package_paths] = python_package_child_names(
                    package_paths, import_topology=resolution_cache.import_topology
                )
            except (OSError, ValueError):
                merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
                return namespace_index.premise(
                    path,
                    frozenset(),
                    noncanonical_imports=exclusions,
                    opaque_activation_sources=opaque,
                    native_code=native_code,
                )
        return namespace_index.premise(
            path,
            package_children[package_paths],
            noncanonical_imports=exclusions,
            opaque_activation_sources=opaque,
            native_code=native_code,
        )

    def scan_view(
        module: str, premise: PythonNamespacePremise | None
    ) -> PythonNamespacePremise | None:
        """The premise a module's scans read: its receipt and revision key."""
        if premise is None:
            return None
        return premise.scan_view(scan_universes.get(module))

    def refresh_namespace_premises(affected):
        for module in sorted(set(affected) & set(scan_sources)):
            prior = scan_sources[module]
            premise = namespace_premise(
                prior.source_path, prior.package_paths, state.contexts[module], module
            )
            if prior.namespace_premise == premise:
                continue
            # Lowering reads the whole latest premise; a scan reads its view.
            scan_sources[module] = replace(prior, namespace_premise=premise)
            if scan_view(module, prior.namespace_premise) == scan_view(module, premise):
                continue
            revised.add(module)
            if _SourceScanJob(module, None) in scan_receipts:
                queue_scan(module)
            for owner, arguments in sorted(scanned_bodies.get(module, ())):
                queue_scan(module, owner, arguments)

    def merge_namespace_writes(batch):
        changed = namespace_index.merge(batch)
        affected = (
            set(scan_sources)
            if changed is None
            else set().union(*(modules_by_source.get(path, ()) for path in changed))
        )
        refresh_namespace_premises(affected)

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
                if prior is None or prior.body_selection.kind == "native_support":
                    queue_entry(resolved, candidate, source_context.dependency)

    def require_runtime_import_protocol(module_name: str) -> None:
        authority = scan_sources[module_name]
        if not authority.uses_runtime_import_protocol:
            scan_sources[module_name] = replace(
                authority, uses_runtime_import_protocol=True
            )

    demands = state.demands
    if demands is None:
        demands = state.demands = PythonSourceDemands(
            admit_imports=lambda module, names: admit_imports(
                names, state.contexts[module]
            ),
            queue_body=queue_scan,
            require_runtime_import_protocol=require_runtime_import_protocol,
        )
    else:
        # The solver is retained; only its fully drained graph-owner callbacks
        # are rebound to the current extension's result accumulator.
        demands.admit_imports = lambda module, names: admit_imports(
            names, state.contexts[module]
        )
        demands.queue_body = queue_scan
        demands.require_runtime_import_protocol = require_runtime_import_protocol

    def activate_namespace_writes(module_name, source_owner):
        state.active_owners.setdefault(module_name, set()).add(source_owner)
        by_owner = pending_namespace_writes.get(module_name, {})
        writes = by_owner.pop(source_owner, ())
        if writes:
            for write in writes:
                if write.module is not None and write.construct not in {
                    "import_binding",
                    "guarded_import",
                }:
                    state.writer_targets.setdefault(write.module, set()).add(write)
            source_context = state.contexts[module_name]
            writer_path = graph.get(module_name)
            merge_namespace_writes(
                graph_python_namespace_writes(
                    writes,
                    lambda name: (
                        resolution_cache.resolved_path(resolved)
                        if (resolved := resolve_candidate(name, source_context))
                        is not None
                        else None
                    ),
                    writer=None if writer_path is None else (module_name, writer_path),
                )
            )
        return writes

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
                if (
                    summary.own_names is not None
                    and scan_sources[module_name].namespace_own_names is None
                ):
                    scan_sources[module_name] = replace(
                        scan_sources[module_name],
                        namespace_own_names=summary.own_names,
                    )
                if scan_sources[module_name].namespace_own_line is None:
                    scan_sources[module_name] = replace(
                        scan_sources[module_name],
                        namespace_own_line=summary.own_namespace_line,
                    )
                known = state.namespace_writes.setdefault(module_name, set())
                pending = pending_namespace_writes.setdefault(module_name, {})
                for write in summary.foreign_writes:
                    if write not in known:
                        known.add(write)
                        pending.setdefault(write.source_owner, []).append(write)
                if scan.source_requests.runtime_catalog is not None:
                    # Runtime catalog custody makes every body available to
                    # dynamic execution. Ordinary full inventory is only
                    # availability: foreign writes join when their owner runs.
                    writer_path = graph.get(module_name)
                    merge_namespace_writes(
                        graph_python_namespace_writes(
                            summary.foreign_writes,
                            lambda name: graph.get(name),
                            entry=False,
                            writer=None
                            if writer_path is None
                            else (module_name, writer_path),
                        )
                    )
                # A widened stable view may expose new rows for an owner that
                # already ran. The value solver intentionally will not enqueue
                # that same activation twice, so publish these deltas now.
                for owner in tuple(state.active_owners.get(module_name, ())):
                    activate_namespace_writes(module_name, owner)
        activate_namespace_writes(module_name, source_owner)
        demands.accept(
            module_name,
            scan.source_requests,
        )
        demands.drain()
        refresh_namespace_premises(demands.drain_opaque_activation_changes())
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
                or scan_sources[execution_name].body_selection.kind == "native_support"
            ):
                execution_scan = precomputed_scans_by_path.get(
                    resolution_cache.resolved_path(execution_path)
                )
                if execution_scan is not None and (
                    execution_scan.authority.module_name != execution_name
                    or execution_scan.authority.mode != "module_init"
                    or execution_scan.authority.body_selection.kind == "native_support"
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
    for module_name, exports in (native_callable_exports or {}).items():
        for name in exports:
            # The native artifact consumes this callable, independently of the
            # source bindings retained for lowering. These canonical demand
            # edges survive later promotion to an ordinary complete source.
            demands.demand(
                (module_name, SourceMember(SourceCandidate(module_name, None), name)),
                "escape",
            )
    demands.drain()
    refresh_namespace_premises(demands.drain_opaque_activation_changes())

    while queue or scan_revisions or deferred_bodies:
        if not queue and not scan_revisions:
            # Nothing else can settle these modules: scan each waiting body
            # once against its module's final premise.
            for module in sorted(deferred_bodies):
                scan_premises[module] = scan_sources[module].namespace_premise
                release_deferred_bodies(module, forced=True)
            continue
        if not queue:
            # Recompute readiness after new demands and admissions drain:
            # those can introduce another active writer or close a cycle.
            ready_revisions = _ready_namespace_revisions(state)
            revisions: list[_SourceScanJob] = sorted(
                ready_revisions,
                key=lambda item: (
                    item.module_name,
                    item.source is not None,
                    item.source or (),
                    item.arguments,
                ),
            )
            queue.extend(reversed(revisions))
            queued_scans.update(ready_revisions)
            scan_revisions.difference_update(ready_revisions)
        job = queue.pop()
        if isinstance(job, _SourceScanJob):
            queued_scans.discard(job)
            module_name = job.module_name
            path = graph[module_name]
            before_activation = _written_names(
                scan_view(module_name, scan_sources[module_name].namespace_premise)
            )
            activated = activate_namespace_writes(module_name, job.source)
            # A scan always sees the names its own unit writes into a module
            # namespace directly (not through an escaped value's member store).
            own_writes_moved = any(
                write.construct in _DIRECT_NAMESPACE_WRITES for write in activated
            ) and before_activation != _written_names(
                scan_view(module_name, scan_sources[module_name].namespace_premise)
            )
            source_context = state.contexts[module_name]
            custody = source_context.runtime_import_custody
            if (
                job.source is not None
                and scan_sources[module_name].mode == "full"
                and custody is not None
                and custody.owns(module_name, path)
            ):
                # Only actual runtime catalog custody covers every body.
                # Ordinary full inventories still project demanded bodies;
                # their availability does not assert execution.
                continue
            authority = scan_sources[module_name]
            premise = authority.namespace_premise
            if (
                job.source is not None
                and module_name in scan_premises
                and not own_writes_moved
            ):
                premise = scan_premises[module_name]
                if scan_view(module_name, premise) != scan_view(
                    module_name, authority.namespace_premise
                ):
                    # Never scan a body against a stale premise: it waits for
                    # its module's scan to settle and adds nothing meanwhile.
                    deferred_bodies.setdefault(module_name, set()).add(job)
                    continue
            receipt = (
                scan_view(module_name, premise),
                authority.body_selection.roots,
                authority.mode,
                state.context_epochs[module_name],
            )
            if scan_receipts.get(job) == receipt:
                if job.source is None:
                    release_deferred_bodies(module_name)
                continue
            # The memo names the scan's inputs: the context itself, not this
            # state's epoch counter, so a replay reuses an equal scan; an
            # entry scan of the same inputs shares the key.
            memo_key = (
                job,
                path,
                receipt[0],
                authority.body_selection,
                authority.mode,
                source_context,
                None,
            )
            loaded = None if scan_memo is None else scan_memo.get(memo_key)
            previous_load = state.scan_loads.get(job)
            previous_receipt = scan_receipts.get(job)
            # Early cutoff: a unit whose every premise answer holds under the
            # new view reads the same things and keeps its output, so it is
            # neither scanned nor published again.
            unchanged = (
                loaded is None
                and previous_load is not None
                and previous_load.premise_reads is not None
                and previous_receipt is not None
                and previous_receipt[1:] == receipt[1:]
                and previous_load.premise_reads.unchanged_under(premise)
            )
            if unchanged:
                assert previous_load is not None
                assert previous_load.premise_reads is not None
                loaded = previous_load
                if previous_load.premise_reads.publish is not None:
                    previous_load.premise_reads.publish(premise)
            loaded = loaded or _load_module_import_scan(
                path,
                module_name=module_name,
                is_package=bool(authority.is_package),
                import_scan_mode=authority.mode
                if job.source is None
                else "module_init",
                purpose=_graph_import_scan_purpose(
                    authority.body_selection, job.source
                ),
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
                namespace_premise=premise,
                native_support_function_roots=authority.body_selection.roots,
                record_premise_reads=True,
            )
            state.scan_loads[job] = loaded
            if scan_memo is not None:
                scan_memo[memo_key] = loaded
            if loaded.snapshot is not None:
                scan_universes[module_name] = python_namespace_name_universe(
                    loaded.snapshot.text
                )
            if job.source is None:
                scan_premises[module_name] = premise
            state.superseded_scans |= state.scan_outputs.get(job, loaded.scan) != (
                loaded.scan
            )
            state.scan_outputs[job] = loaded.scan
            scan_receipts[job] = (scan_view(module_name, premise), *receipt[1:])
            if scan_view(module_name, premise) != scan_view(
                module_name, authority.namespace_premise
            ):
                # Read behind the latest premise: revise with the module.
                scan_revisions.add(job)
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
            if not unchanged:
                accept_source_scan(
                    module_name,
                    path,
                    loaded.scan,
                    loaded.snapshot.sha256 if loaded.snapshot is not None else None,
                    loaded.snapshot,
                    source_owner=job.source,
                )
            if job.source is None:
                release_deferred_bodies(module_name)
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
                body_selection=ModuleBodySelection("complete_source"),
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
            and previous.body_selection.kind == "complete_source"
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
            namespace_premise=namespace_premise(
                path, authority.package_paths, source_context, module_name
            ),
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
            scan_view(module_name, authority.namespace_premise),
            authority.body_selection.roots,
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
                    purpose=_graph_import_scan_purpose(authority.body_selection, None),
                    target_python=target_python,
                    target_sys_platform=resolution_cache.target_sys_platform,
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
            or precomputed.authority.body_selection != authority.body_selection
        ):
            precomputed = None
        entry_memo_key = (
            _SourceScanJob(module_name, None),
            path,
            receipt[0],
            authority.body_selection,
            authority.mode,
            source_context,
            precomputed,
        )
        try:
            loaded = (
                None if scan_memo is None else scan_memo.get(entry_memo_key)
            ) or _load_module_import_scan(
                path,
                module_name=module_name,
                is_package=bool(authority.is_package),
                import_scan_mode=authority.mode,
                purpose=_graph_import_scan_purpose(authority.body_selection, None),
                resolution_cache=resolution_cache,
                project_root=source_context.project_root,
                roots=source_context.roots,
                stdlib_root=source_context.stdlib_root,
                stdlib_allowlist=set(source_context.stdlib_allowlist),
                target_python=target_python,
                runtime_import_custody=source_context.runtime_import_custody,
                precomputed_scan=precomputed,
                namespace_premise=authority.namespace_premise,
                native_support_function_roots=authority.body_selection.roots,
                source_snapshot=source_generations.get(module_name, (None, None))[1],
                record_premise_reads=True,
            )
        except (OSError, SyntaxError, UnicodeDecodeError):
            merge_namespace_writes(UNKNOWN_GRAPH_NAMESPACE_WRITES)
            # A source that cannot be analyzed may import at run time.
            scan_sources[module_name] = replace(
                scan_sources[module_name], uses_runtime_import_protocol=True
            )
            continue
        state.scan_loads[_SourceScanJob(module_name, None)] = loaded
        if scan_memo is not None:
            scan_memo[entry_memo_key] = loaded
        if loaded.snapshot is not None:
            scan_universes[module_name] = python_namespace_name_universe(
                loaded.snapshot.text
            )
        scan_premises[module_name] = authority.namespace_premise
        module_job = _SourceScanJob(module_name, None)
        state.superseded_scans |= state.scan_outputs.get(module_job, loaded.scan) != (
            loaded.scan
        )
        state.scan_outputs[module_job] = loaded.scan
        scan_receipts[_SourceScanJob(module_name, None)] = (
            scan_view(module_name, authority.namespace_premise),
            *receipt[1:],
        )
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
        release_deferred_bodies(module_name)
        if (
            previous is not None
            and previous.body_selection.roots != authority.body_selection.roots
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
    # Exact receivers are a fact of the final graph that only lowering reads:
    # no scan consumes them, so they join each premise once the graph is
    # closed and never revise a scan or its cache key.
    receivers_by_module = demands.exact_receivers()
    for module_name, authority in tuple(scan_sources.items()):
        premise = authority.namespace_premise
        if premise is None or premise.opaque_activation_sources is None:
            continue
        exact_receivers = receivers_by_module.get(module_name, ())
        if exact_receivers != premise.exact_receivers:
            scan_sources[module_name] = replace(
                authority,
                namespace_premise=replace(premise, exact_receivers=exact_receivers),
            )
    state.derived_premises = {
        name: namespace_premise(
            authority.source_path,
            authority.package_paths,
            state.contexts[name],
            name,
            settled=False,
        )
        for name, authority in scan_sources.items()
    }
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
    purpose: _source_import_scan.SourceImportPurpose,
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
        purpose=purpose,
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
    purpose: _source_import_scan.SourceImportPurpose,
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
    record_premise_reads: bool = False,
) -> _LoadedModuleImportScan:
    """Admit source-only requests, then resolve filesystem edges in this operation."""
    if type(purpose) is not _source_import_scan.SourceImportPurpose:
        raise TypeError("source import scan requires a typed purpose")
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
            precomputed_scan.authority.body_selection.roots
            != native_support_function_roots
        ):
            raise ValueError(f"precomputed source selection changed: {module_name!r}")
        _validate_precomputed_module_import_scan(
            precomputed_scan,
            path=path,
            module_name=module_name,
            import_scan_mode=import_scan_mode,
            purpose=purpose,
            target_python=target_python,
            target_sys_platform=resolution_cache.target_sys_platform,
        )
        if (
            source is None
            and tree is None
            and (
                precomputed_scan.authority.namespace_premise.scan_view()
                if precomputed_scan.authority.namespace_premise is not None
                else None
            )
            == (
                namespace_premise.scan_view() if namespace_premise is not None else None
            )
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
    # Stdlib requests have the same source/namespace/tooling/target admission
    # without an application root. Rootless graph consumers must not redo the
    # same binding fixpoint for every test or wrapper operation. Explicit source
    # or AST remains operation-local and can never be overridden by disk.
    persist_scan = (tree is None and source is None) and (
        project_root is not None
        or (
            stdlib_root is not None
            and resolution_cache.is_stdlib_path(path, stdlib_root)
        )
    )
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
        resolution_cache.source_cache[resolution_cache.resolved_path(path)] = snapshot
    if namespace_premise is not None:
        # The scan, its persisted record and its key read only this view.
        namespace_premise = namespace_premise.scan_view(
            python_namespace_name_universe(source)
        )

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

    if persist_scan:
        persisted = _module_graph_cache._read_persisted_import_scan_record(
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            purpose=purpose,
            source_owner=source_owner,
            source_arguments=source_arguments,
            snapshot=snapshot,
            target_python=target_python,
            namespace_premise=namespace_premise,
            native_support_function_roots=native_support_function_roots,
            runtime_catalog_owner=runtime_catalog_owner,
            target_sys_platform=resolution_cache.target_sys_platform,
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
            ModuleBodySelection.from_requested_roots(native_support_function_roots),
            ast_digest_admission=ast_digest_admission,
            target_python=target_python,
        )
        tree, ast_digest_admission = selected.tree, selected.admission
    premise_reads = ScanPremiseReads() if record_premise_reads else None
    reads_token = SCAN_PREMISE_READS.set(premise_reads)
    try:
        import_projection, requests = _collect_module_import_scan(
            path,
            tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            source_owner=source_owner,
            source_arguments=source_arguments,
            runtime_import_custody=runtime_import_custody,
            ast_digest_admission=ast_digest_admission,
            namespace_premise=namespace_premise,
            purpose=purpose,
            resolution_cache=resolution_cache,
            target_python=target_python,
            source=source,
        )
    finally:
        SCAN_PREMISE_READS.reset(reads_token)
    if persist_scan and snapshot is not None:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_import_scan(
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                purpose=purpose,
                source_owner=source_owner,
                source_arguments=source_arguments,
                scan=requests,
                snapshot=snapshot,
                target_python=target_python,
                namespace_premise=namespace_premise,
                native_support_function_roots=native_support_function_roots,
                runtime_catalog_owner=runtime_catalog_owner,
                target_sys_platform=resolution_cache.target_sys_platform,
            )
    cutoff = None
    if premise_reads is not None:
        universe = None if source is None else python_namespace_name_universe(source)
        scanned_view = namespace_premise

        def view(
            premise: PythonNamespacePremise | None,
        ) -> PythonNamespacePremise | None:
            return None if premise is None else premise.scan_view(universe)

        publish = None
        if persist_scan and snapshot is not None:
            published_snapshot = snapshot

            def publish(premise: PythonNamespacePremise | None) -> None:
                new_view = view(premise)
                if new_view == scanned_view:
                    return
                with contextlib.suppress(OSError):
                    _module_graph_cache._write_persisted_import_scan_alias(
                        path,
                        module_name=module_name,
                        is_package=is_package,
                        import_scan_mode=import_scan_mode,
                        purpose=purpose,
                        source_owner=source_owner,
                        source_arguments=source_arguments,
                        namespace_premise=new_view,
                        same_as_premise=scanned_view,
                        native_support_function_roots=native_support_function_roots,
                        snapshot=published_snapshot,
                        target_python=target_python,
                        runtime_catalog_owner=runtime_catalog_owner,
                        target_sys_platform=resolution_cache.target_sys_platform,
                    )

        cutoff = _ScanCutoff(premise_reads, view, publish)
    return _LoadedModuleImportScan(
        complete(requests),
        False,
        source_parsed,
        source,
        tree,
        snapshot,
        source_pruned,
        cutoff,
    )


def _collect_module_import_scan(
    path: Path,
    tree: ast.AST,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    source_owner: PythonSourceKey | None,
    source_arguments: tuple[str, ...],
    runtime_import_custody: _RuntimeImportScanCustody | None,
    ast_digest_admission: _PythonAstDigestAdmission,
    namespace_premise: PythonNamespacePremise | None,
    purpose: _source_import_scan.SourceImportPurpose,
    resolution_cache: _module_resolution._ModuleResolutionCache,
    target_python: TargetPythonVersion,
    source: str | None,
) -> tuple[_ImportDiscoveryProjection, _ImportScanRequests]:
    """One source's graph projection and its requests under one premise view."""
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
        purpose=purpose,
    )
    requests = _source_import_scan._collect_import_scan_requests(
        import_projection,
        tree,
        is_package=path.name == "__init__.py",
        module_name=module_name,
        import_scan_mode=import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
        source=source,
        namespace_premise=namespace_premise,
        target_sys_platform=resolution_cache.target_sys_platform,
        purpose=purpose,
    )
    return import_projection, requests
