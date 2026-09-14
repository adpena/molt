from __future__ import annotations

import ast
import contextlib
import hashlib
import json
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
    _DiscoveredModuleGraph,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _PrecomputedModuleImportScan,
    _ImportAdmissionPolicy,
    _RuntimeImportScanCustody,
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


class _LoadedModuleImportScan(NamedTuple):
    """One complete scan plus its operation-local source/parse outcome."""

    scan: _module_graph_cache._PersistedImportScan
    cache_hit: bool
    source_parsed: bool
    source: str | None
    tree: ast.AST | None


def _bind_precomputed_module_import_scan(
    path: Path,
    *,
    module_name: str,
    import_scan_mode: ImportScanMode,
    scan: _CompleteImportScan,
    is_package: bool | None = None,
    target_python: TargetPythonVersion,
    capability_config_digest: str = "",
) -> _PrecomputedModuleImportScan:
    digest = _module_source._source_content_sha256(path, path.stat())
    if digest is None:
        raise ValueError(f"cannot bind source scan: {path}")
    return _PrecomputedModuleImportScan(
        _ModuleSourceScanAuthority(module_name, path, import_scan_mode, is_package),
        digest,
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
        # Persisted scans are strict. Their schema deliberately has no runtime
        # custody lane; only the per-build, custody-keyed memory cache is used.
        project_root = None
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
    queue: list[tuple[Path, str | None]] = [
        (
            path,
            precomputed_scans_by_path[path.resolve()].authority.module_name
            if path.resolve() in precomputed_scans_by_path
            else None,
        )
        for path in reversed(entry_paths)
    ]
    queued_entries = {
        (resolution_cache.resolved_path(path), forced_name)
        for path, forced_name in queue
    }
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    resolved_entry_paths = frozenset(
        resolution_cache.resolved_path(path) for path in entry_paths
    )

    persisted_graph_paths: dict[str, Path] = {}
    dirty_persisted_modules: set[str] = set()
    use_persisted_graph_cache = project_root is not None and len(entry_paths) == 1
    scan_input_digest = (
        hashlib.sha256(
            json.dumps(
                {
                    "precomputed": [
                        (
                            str(path),
                            record.authority.module_name,
                            record.authority.mode,
                            record.authority.is_package,
                            record.source_sha256,
                            record.target_python_tag,
                            record.capability_config_digest,
                            record.scan.imports,
                            [
                                (name, str(source_path))
                                for name, source_path in record.scan.source_executions
                            ],
                        )
                        for path, record in sorted(precomputed_scans_by_path.items())
                    ],
                    "admitted_sources": enclosing_scan_authority.payload()
                    if enclosing_scan_authority is not None
                    else [],
                },
                separators=(",", ":"),
            ).encode("utf-8")
        ).hexdigest()
        if precomputed_scans_by_path or enclosing_scan_authority is not None
        else ""
    )
    source_inputs = (
        dict(enclosing_scan_authority.by_module)
        if enclosing_scan_authority is not None
        else {}
    )
    source_inputs.update(
        (record.authority.module_name, record.authority)
        for record in precomputed_scans_by_path.values()
    )
    if use_persisted_graph_cache:
        cache_project_root = project_root
        assert cache_project_root is not None
        entry_path = entry_paths[0]
        persisted_graph = _module_graph_cache._read_persisted_module_graph(
            cache_project_root,
            entry_path,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            skip_modules=skip_modules,
            stub_parents=stub_parents,
            stdlib_static_import_helper_modules=stdlib_static_import_helper_modules,
            stdlib_allowlist=stdlib_allowlist,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=allow_entry_external_imports,
            resolution_cache=resolution_cache,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
            full_scan_roots=full_scan_roots,
            scan_input_digest=scan_input_digest,
            source_input_authority=_ModuleGraphScanAuthority(
                tuple(source_inputs.values())
            ),
        )
        if persisted_graph is not None:
            if not persisted_graph.dirty_modules:
                return _DiscoveredModuleGraph(
                    persisted_graph.graph,
                    persisted_graph.explicit_imports,
                    persisted_graph.scan_authority,
                )
            persisted_graph_paths = dict(persisted_graph.graph)
            dirty_persisted_modules = set(persisted_graph.dirty_modules)

    def resolve_candidate(candidate: str) -> Path | None:
        if runtime_import_custody is not None:
            catalog_path = runtime_import_custody.catalog_by_module.get(candidate)
            if catalog_path is not None:
                return catalog_path
        if enclosing_scan_authority is not None:
            admitted = enclosing_scan_authority.by_module.get(candidate)
            if admitted is not None:
                return admitted.source_path
        persisted_path = persisted_graph_paths.get(candidate)
        if persisted_path is not None and candidate not in dirty_persisted_modules:
            return persisted_path
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
        source_executions: tuple[_module_import_scanner._StaticSourceExecution, ...]
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
            source_executions = tuple(
                _module_import_scanner._StaticSourceExecution(name, source_path)
                for name, source_path in (
                    precomputed_scan.scan.source_executions if precomputed_scan else ()
                )
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
            source_executions = tuple(
                _module_import_scanner._StaticSourceExecution(name, source_path)
                for name, source_path in loaded_scan.scan.source_executions
            )
        for execution in source_executions:
            execution_name = (
                execution.module_name
                or resolution_cache.module_name_from_path(
                    execution.source_path, module_roots, stdlib_root
                )
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
                execution.source_path,
                from_entry_path=from_entry_path,
            ):
                continue
            existing = graph.get(execution_name)
            if existing is not None:
                if resolution_cache.resolved_path(
                    existing
                ) != resolution_cache.resolved_path(execution.source_path):
                    raise ValueError(
                        f"module {execution_name!r} has multiple statically executed source "
                        f"authorities: {existing} and {execution.source_path}"
                    )
                continue
            explicit_imports.add(execution_name)
            entry = (
                resolution_cache.resolved_path(execution.source_path),
                execution_name,
            )
            if entry not in queued_entries:
                queued_entries.add(entry)
                queue.append((execution.source_path, execution_name))
        for name in imports:
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
    if use_persisted_graph_cache:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_module_graph(
                cache_project_root,
                entry_paths[0],
                roots=roots,
                module_roots=module_roots,
                stdlib_root=stdlib_root,
                skip_modules=skip_modules,
                stub_parents=stub_parents,
                stdlib_static_import_helper_modules=stdlib_static_import_helper_modules,
                stdlib_allowlist=stdlib_allowlist,
                import_admission_policy=import_admission_policy,
                allow_entry_external_imports=allow_entry_external_imports,
                graph=graph,
                scan_authority=_ModuleGraphScanAuthority(tuple(scan_sources.values())),
                explicit_imports=explicit_imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
                full_scan_roots=full_scan_roots,
                scan_input_digest=scan_input_digest,
            )
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
    """Load or produce both source-scan projections exactly once.

    A hit is complete and is never rewritten. Precomputed records carry both
    projections and are checked against the exact source and scan authority.
    The returned source/AST belong only to this operation, letting analysis reuse
    a cold scan without a second source read or parse.
    """
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
    if project_root is not None:
        persisted = _module_graph_cache._read_persisted_import_scan_record(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted is not None:
            return _LoadedModuleImportScan(persisted, True, False, None, None)

    source_parsed = False
    if tree is None:
        if source is None:
            source = resolution_cache.read_module_source(path, retain=retain_source)
        tree = resolution_cache.parse_module_ast(
            path,
            source,
            filename=str(path) if source_filename is None else source_filename,
            retain=retain_tree,
            target_python=target_python,
        )
        source_parsed = True
    assert tree is not None
    imports = resolution_cache.collect_imports(
        path,
        tree,
        collector=_module_import_scanner._collect_imports,
        target_python=target_python,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        runtime_import_custody=runtime_import_custody,
    )
    if roots is not None and stdlib_root is not None and stdlib_allowlist is not None:
        imports = _module_import_scanner._expand_imports_with_static_package_all_star_children(
            imports,
            tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=resolution_cache,
            target_python=target_python,
            runtime_import_custody=runtime_import_custody,
            source_path=path.resolve(),
        )
    assert tree is not None
    executions = (
        ()
        if source is not None
        and not _module_import_scanner._source_may_use_static_source_execution(source)
        else _module_import_scanner._collect_static_source_executions(
            tree,
            source_path=path,
            import_scan_mode=import_scan_mode,
            module_name=module_name,
        )
    )
    scan = _module_graph_cache._PersistedImportScan(
        tuple(imports),
        tuple(
            (execution.module_name, execution.source_path) for execution in executions
        ),
    )
    if project_root is not None:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                scan=scan,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return _LoadedModuleImportScan(scan, False, source_parsed, source, tree)
