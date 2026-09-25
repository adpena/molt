from __future__ import annotations

import ast
import contextlib
import re
from collections.abc import Collection, Mapping, MutableMapping, Sequence
from pathlib import Path
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
    _ImportScanRequests,
    _DiscoveredModuleGraph,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _PrecomputedModuleImportScan,
    _ImportAdmissionPolicy,
    _RuntimeImportScanCustody,
)
from molt.compiler_analysis.python_imports import _PythonAstDigestAdmission
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)


# Submodule prefixes excluded from the module graph because they target
# platforms that Molt does not support (e.g. Emscripten/Pyodide).  The import
# scanner still discovers them but the graph walker skips any candidate whose
# dotted name starts with one of these prefixes.
PLATFORM_EXCLUDED_SUBMODULES = ("urllib3.contrib.emscripten",)


class _LoadedModuleImportScan(NamedTuple):
    """One complete scan plus its operation-local source/parse outcome."""

    scan: _CompleteImportScan
    cache_hit: bool
    source_parsed: bool
    source: str | None
    tree: ast.AST | None
    snapshot: _module_source.PythonSourceSnapshot | None = None


def _bind_precomputed_module_import_scan(
    path: Path,
    *,
    module_name: str,
    import_scan_mode: ImportScanMode,
    scan: _CompleteImportScan,
    snapshot: _module_source.PythonSourceSnapshot,
    is_package: bool | None = None,
    target_python: TargetPythonVersion,
    capability_config_digest: str = "",
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
        ),
        snapshot.sha256,
        target_python.tag,
        capability_config_digest,
        scan,
    )


def _validate_precomputed_module_import_scan(
    record: _PrecomputedModuleImportScan,
    *,
    path: Path,
    module_name: str,
    import_scan_mode: ImportScanMode,
    target_python: TargetPythonVersion,
    capability_config_digest: str,
) -> None:
    record.authority.validate(module_name, path)
    if (
        record.authority.mode != import_scan_mode
        or record.authority.requires_runtime_package_anchor
        != record.scan.requires_runtime_package_anchor
        or record.target_python_tag != target_python.tag
        or record.capability_config_digest != capability_config_digest
        or _module_source._source_content_sha256(path, path.stat())
        != record.source_sha256
    ):
        raise ValueError(f"precomputed source scan lost custody: {module_name!r}")


def _merge_discovered_module_graph(
    graph: MutableMapping[str, Path],
    scan_authorities: MutableMapping[str, _ModuleSourceScanAuthority],
    result: _DiscoveredModuleGraph,
    *,
    source_replacements: Mapping[str, _ModuleSourceScanAuthority] | None = None,
) -> None:
    result.scan_authority.validate_graph(result.graph)
    source_replacements = source_replacements or {}
    for name, previous in source_replacements.items():
        prior_path = graph.get(name)
        if prior_path is None:
            raise ValueError(
                f"source generation transfer lacks prior graph authority: {name!r}"
            )
        previous.validate(name, prior_path)
        following = result.scan_authority.by_module.get(name)
        if (
            scan_authorities.get(name) != previous
            or following is None
            or previous.mode != "full"
            or following.mode != "full"
            or previous.is_package != following.is_package
        ):
            raise ValueError(f"source generation transfer lost scan custody: {name!r}")
    merged = _ModuleGraphScanAuthority(
        tuple(
            source
            for name, source in scan_authorities.items()
            if name not in source_replacements
        )
    ).merged(result.scan_authority)
    for name, path in result.graph.items():
        prior = graph.get(name)
        if (
            prior is not None
            and prior.resolve() != path.resolve()
            and name not in source_replacements
        ):
            raise ValueError(f"module {name!r} has conflicting source scan authorities")
    graph.update(result.graph)
    scan_authorities.clear()
    scan_authorities.update(merged.by_module)


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
    stdlib_static_import_helper_modules: set[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
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
        stdlib_static_import_helper_modules=stdlib_static_import_helper_modules,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
        runtime_import_custody=runtime_import_custody,
        enclosing_scan_authority=_ModuleGraphScanAuthority(
            tuple(scan_authorities.values())
        ),
        full_scan_roots=full_scan_roots,
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
    capability_config_digest: str = "",
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
    _extend_module_graph_with_closure(
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
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=False,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
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
    stdlib_static_import_helper_modules: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_scans_by_path: Mapping[Path, _PrecomputedModuleImportScan]
    | None = None,
    enclosing_scan_authority: _ModuleGraphScanAuthority | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    *,
    full_scan_roots: bool,
) -> _DiscoveredModuleGraph:
    # A closure seed is not automatically an executable application root.
    # Profile cores and package parents contribute initialization edges only;
    # explicit roots and custodied runtime owners authorize full-depth scans.
    entry_paths = tuple(entry_paths)
    precomputed_scans_by_path = {
        path.resolve(): scan for path, scan in (precomputed_scans_by_path or {}).items()
    }
    if not entry_paths:
        return _DiscoveredModuleGraph({}, set(), _ModuleGraphScanAuthority())
    graph: dict[str, Path] = {}
    scan_sources: dict[str, _ModuleSourceScanAuthority] = {}
    if runtime_import_custody is not None:
        if not full_scan_roots:
            raise ValueError("runtime import custody requires full-depth owner scans")
        # Catalog dependencies already have graph admission. Re-scan only the
        # protocol owners at full depth; do not reinterpret application sources
        # under the stdlib closure's roots or leak custody into their scans.
        owner_names = dict(runtime_import_custody.owners)
        graph.update(
            (name, path)
            for name, path in runtime_import_custody.catalog
            if name not in owner_names
        )
        if enclosing_scan_authority is None:
            raise ValueError("runtime catalog requires enclosing source scan authority")
        scan_sources.update(enclosing_scan_authority.restricted(graph).by_module)
        if set(scan_sources) != set(graph):
            raise ValueError("runtime catalog lacks enclosing source scan authority")
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    stdlib_static_import_helper_modules = (
        set(_module_import_scanner.STDLIB_STATIC_IMPORT_HELPER_MODULES)
        if stdlib_static_import_helper_modules is None
        else stdlib_static_import_helper_modules
    )
    explicit_imports: set[str] = set()
    seen_import_names: set[str] = set()
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()
    seed_entries: list[tuple[Path, str | None]] = []
    for path in entry_paths:
        resolved_path = path.resolve()
        precomputed = precomputed_scans_by_path.get(resolved_path)
        if precomputed is not None:
            seed_entries.append((path, precomputed.authority.module_name))
            continue
        custody_owner_names = (
            tuple(
                name
                for name, owner_path in runtime_import_custody.owners
                if owner_path == resolved_path
            )
            if runtime_import_custody is not None
            else ()
        )
        if custody_owner_names:
            seed_entries.extend((path, name) for name in custody_owner_names)
        else:
            seed_entries.append((path, None))
    seed_entries = list(dict.fromkeys(seed_entries))
    queue: list[tuple[Path, str | None]] = list(reversed(seed_entries))
    queued_entries = {
        (resolution_cache.resolved_path(path), forced_name)
        for path, forced_name in queue
    }
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    resolved_entry_paths = frozenset(
        resolution_cache.resolved_path(path) for path in entry_paths
    )

    def resolve_candidate(candidate: str) -> Path | None:
        if runtime_import_custody is not None:
            catalog_path = runtime_import_custody.catalog_by_module.get(candidate)
            if catalog_path is not None:
                return catalog_path
        if enclosing_scan_authority is not None:
            admitted = enclosing_scan_authority.by_module.get(candidate)
            if admitted is not None:
                return admitted.source_path
        return resolution_cache.resolve_module(
            candidate, roots, stdlib_root, stdlib_allowlist
        )

    while queue:
        path, forced_module_name = queue.pop()
        queued_entries.discard(
            (resolution_cache.resolved_path(path), forced_module_name)
        )
        module_name = forced_module_name or resolution_cache.module_name_from_path(
            path, module_roots, stdlib_root
        )
        if module_name in graph:
            if resolution_cache.resolved_path(
                graph[module_name]
            ) != resolution_cache.resolved_path(path):
                raise ValueError(
                    f"module {module_name!r} has multiple statically executed source authorities: "
                    f"{graph[module_name]} and {path}"
                )
            continue
        graph[module_name] = path
        admitted_source = (
            enclosing_scan_authority.by_module.get(module_name)
            if enclosing_scan_authority is not None
            else None
        )
        if (
            admitted_source is not None
            and admitted_source.source_path == path.resolve()
        ):
            is_package = bool(admitted_source.is_package)
        else:
            is_package = path.name == "__init__.py"
        import_scan_mode = _module_import_scanner._module_import_scan_mode(
            module_name,
            full_scan=(
                full_scan_roots
                and resolution_cache.resolved_path(path) in resolved_entry_paths
            ),
            static_import_helper_modules=stdlib_static_import_helper_modules,
        )
        precomputed_scan = (
            precomputed_scans_by_path.get(path.resolve())
            if precomputed_scans_by_path is not None
            else None
        )
        if precomputed_scan is not None:
            is_package = bool(precomputed_scan.authority.is_package)
        scan_sources[module_name] = _ModuleSourceScanAuthority(
            module_name, path, import_scan_mode, is_package
        )
        imports: tuple[str, ...]
        dynamic_relative_import_candidates: tuple[str, ...] = ()
        requires_runtime_package_anchor = False
        source_executions: tuple[tuple[str | None, Path], ...]
        if import_admission_policy.owns_source_closure_with_native_artifact_plan(
            module_name,
            path,
        ):
            # Artifact-owned closures must not be reinterpreted as source scans,
            # nor may this exclusion publish an empty strict source record.
            imports = ()
            # Explicit execution roots carry caller custody independently of
            # the artifact's exclusion from source interpretation.
            if precomputed_scan is not None:
                _validate_precomputed_module_import_scan(
                    precomputed_scan,
                    path=path,
                    module_name=module_name,
                    import_scan_mode=import_scan_mode,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                )
            source_executions = (
                precomputed_scan.scan.source_executions if precomputed_scan else ()
            )
            if precomputed_scan is not None:
                dynamic_relative_import_candidates = (
                    precomputed_scan.scan.dynamic_relative_import_candidates
                )
                requires_runtime_package_anchor = (
                    precomputed_scan.scan.requires_runtime_package_anchor
                )
        else:
            try:
                loaded_scan = _load_module_import_scan(
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    resolution_cache=resolution_cache,
                    project_root=project_root,
                    roots=roots,
                    stdlib_root=stdlib_root,
                    stdlib_allowlist=stdlib_allowlist,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                    runtime_import_custody=runtime_import_custody,
                    precomputed_scan=precomputed_scan,
                )
            except (OSError, SyntaxError, UnicodeDecodeError):
                continue
            imports = loaded_scan.scan.imports
            dynamic_relative_import_candidates = (
                loaded_scan.scan.dynamic_relative_import_candidates
            )
            requires_runtime_package_anchor = (
                loaded_scan.scan.requires_runtime_package_anchor
            )
            source_executions = loaded_scan.scan.source_executions
        scan_sources[module_name] = _ModuleSourceScanAuthority(
            module_name,
            path,
            import_scan_mode,
            is_package,
            requires_runtime_package_anchor,
        )
        for requested_name, execution_path in source_executions:
            execution_name = requested_name or resolution_cache.module_name_from_path(
                execution_path, module_roots, stdlib_root
            )
            if not re.fullmatch(
                r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*",
                execution_name,
            ):
                raise ValueError(
                    f"statically executed source uses invalid module name {execution_name!r}"
                )
            from_entry_path = (
                allow_entry_external_imports
                and resolution_cache.resolved_path(path) in resolved_entry_paths
            )
            if not import_admission_policy.admits_import(
                execution_name,
                execution_path,
                from_entry_path=from_entry_path,
            ):
                continue
            existing = graph.get(execution_name)
            if existing is not None:
                if resolution_cache.resolved_path(
                    existing
                ) != resolution_cache.resolved_path(execution_path):
                    raise ValueError(
                        f"module {execution_name!r} has multiple statically executed source "
                        f"authorities: {existing} and {execution_path}"
                    )
                continue
            explicit_imports.add(execution_name)
            entry = (
                resolution_cache.resolved_path(execution_path),
                execution_name,
            )
            if entry not in queued_entries:
                queued_entries.add(entry)
                queue.append((execution_path, execution_name))
        discovery_imports = tuple(
            dict.fromkeys((*imports, *dynamic_relative_import_candidates))
        )
        for name in discovery_imports:
            if name in seen_import_names:
                continue
            seen_import_names.add(name)
            explicit_imports.add(name)
            for candidate in _module_dependency_authority._expand_module_chain_cached(
                name
            ):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                if any(
                    candidate == prefix or candidate.startswith(prefix + ".")
                    for prefix in PLATFORM_EXCLUDED_SUBMODULES
                ):
                    continue
                resolved = resolve_candidate(candidate)
                if resolved is None:
                    continue
                from_entry_path = (
                    allow_entry_external_imports
                    and resolution_cache.resolved_path(path) in resolved_entry_paths
                )
                if not import_admission_policy.admits_import(
                    candidate,
                    resolved,
                    from_entry_path=from_entry_path,
                ):
                    continue
                admitted_name = (
                    candidate
                    if enclosing_scan_authority is not None
                    and candidate in enclosing_scan_authority.by_module
                    else None
                )
                entry = (resolution_cache.resolved_path(resolved), admitted_name)
                if candidate in graph or entry in queued_entries:
                    continue
                queued_entries.add(entry)
                queue.append((resolved, admitted_name))
    return _DiscoveredModuleGraph(
        graph, explicit_imports, _ModuleGraphScanAuthority(tuple(scan_sources.values()))
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
    stdlib_static_import_helper_modules: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_scan: _PrecomputedModuleImportScan | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
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
        stdlib_static_import_helper_modules=stdlib_static_import_helper_modules,
        resolver_cache=resolver_cache,
        precomputed_scans_by_path=(
            {entry_path.resolve(): precomputed_scan}
            if precomputed_scan is not None
            else None
        ),
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
        full_scan_roots=True,
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
    capability_config_digest: str = "",
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
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
        capability_config_digest=capability_config_digest,
        runtime_import_custody=runtime_import_custody,
    ).scan.imports


@_source_tree_fingerprint_transaction()
def _load_module_import_scan(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
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
    capability_config_digest: str = "",
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    precomputed_scan: _PrecomputedModuleImportScan | None = None,
) -> _LoadedModuleImportScan:
    """Admit source-only requests, then resolve filesystem edges in this operation."""
    if runtime_import_custody is not None and not runtime_import_custody.owns(
        module_name, path.resolve()
    ):
        runtime_import_custody = None
    if runtime_import_custody is not None:
        project_root = None
    if runtime_import_custody is not None:
        runtime_import_custody.validate_scan_mode(
            module_name, path.resolve(), import_scan_mode
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
        _validate_precomputed_module_import_scan(
            precomputed_scan,
            path=path,
            module_name=module_name,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        return _LoadedModuleImportScan(
            precomputed_scan.scan, False, False, source, tree
        )
    # Explicit source or AST is operation-local and must not be overridden by disk.
    if tree is not None or source is not None:
        project_root = None
    snapshot = source_snapshot
    if snapshot is not None:
        if snapshot.path != path or source is not None and source != snapshot.text:
            raise ValueError("source scan snapshot does not match supplied source")
        source = snapshot.text
    elif source is None and tree is None:
        snapshot = _module_source.PythonSourceSnapshot.capture(path)
        source = snapshot.text

    if snapshot is not None and source is not None and retain_source:
        resolution_cache.source_cache[resolution_cache.resolved_path(path)] = source

    def complete(requests: _ImportScanRequests) -> _CompleteImportScan:
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
            snapshot=snapshot,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted is not None:
            return _LoadedModuleImportScan(
                complete(persisted), True, False, source, None, snapshot
            )

    source_parsed = False
    if tree is None:
        assert source is not None
        tree = resolution_cache.parse_module_ast(
            path,
            source,
            filename=str(path) if source_filename is None else source_filename,
            retain=retain_tree,
            target_python=target_python,
        )
        source_parsed = True
    assert tree is not None
    ast_digest_admission = _PythonAstDigestAdmission(tree)
    import_projection = resolution_cache.collect_graph_imports(
        path,
        tree,
        collector=_module_import_scanner._collect_imports_for_graph,
        target_python=target_python,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        runtime_import_custody=runtime_import_custody,
        ast_digest_admission=ast_digest_admission,
    )
    requests = _module_import_scanner._collect_import_scan_requests(
        import_projection,
        tree,
        source_path=path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        runtime_import_custody=runtime_import_custody,
        ast_digest_admission=ast_digest_admission,
        source=source,
    )
    if project_root is not None and snapshot is not None:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                scan=requests,
                snapshot=snapshot,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return _LoadedModuleImportScan(
        complete(requests), False, source_parsed, source, tree, snapshot
    )
