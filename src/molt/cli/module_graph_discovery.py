from __future__ import annotations

import ast
import contextlib
import re
from collections.abc import Collection, Mapping, MutableMapping, Sequence
from pathlib import Path

from molt.cli.config_resolution import STATIC_IMPORT_MODULES_ENV
from molt.cli import module_dependencies as _module_dependency_authority
from molt.cli import module_graph_cache as _module_graph_cache
from molt.cli import module_import_scanner as _module_import_scanner
from molt.cli import module_resolution as _module_resolution
from molt.cli.models import ImportScanMode, _ImportAdmissionPolicy
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)


# Submodule prefixes excluded from the module graph because they target
# platforms that Molt does not support (e.g. Emscripten/Pyodide).  The import
# scanner still discovers them but the graph walker skips any candidate whose
# dotted name starts with one of these prefixes.
PLATFORM_EXCLUDED_SUBMODULES = ("urllib3.contrib.emscripten",)


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


def _extend_module_graph_with_closure(
    module_graph: MutableMapping[str, Path],
    *,
    entry_paths: Sequence[Path],
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
    nested_stdlib_scan_modules: set[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> frozenset[str]:
    if not entry_paths:
        return frozenset()
    closure_graph, _ = _discover_module_graph_from_paths(
        entry_paths,
        list(roots),
        list(module_roots),
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    for name, path in closure_graph.items():
        _record_module_reason(module_reasons, name, reason)
        module_graph.setdefault(name, path)
    return frozenset(closure_graph)


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


def _extend_module_graph_with_static_import_modules(
    *,
    module_graph: MutableMapping[str, Path],
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
        entry_paths=tuple(resolved.values()),
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


def _module_graph_import_scan_mode(
    *,
    path: Path,
    module_name: str,
    entry_paths: frozenset[Path],
    nested_scan_modules: Collection[str],
    resolution_cache: _module_resolution._ModuleResolutionCache,
) -> ImportScanMode:
    resolved_path = resolution_cache.resolved_path(path)
    if resolved_path in entry_paths or module_name in nested_scan_modules:
        return "full"
    return "module_init"


def _discover_module_graph_from_paths(
    entry_paths: Sequence[Path],
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_imports_by_path: Mapping[Path, Collection[str]] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[dict[str, Path], set[str]]:
    entry_paths = tuple(entry_paths)
    if not entry_paths:
        return {}, set()
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    nested_stdlib_scan_modules = (
        _module_import_scanner.STDLIB_NESTED_IMPORT_SCAN_MODULES
        if nested_stdlib_scan_modules is None
        else nested_stdlib_scan_modules
    )
    explicit_imports: set[str] = set()
    seen_import_names: set[str] = set()
    queue = list(reversed(entry_paths))
    queued_paths = set(entry_paths)
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    resolved_entry_paths = frozenset(
        resolution_cache.resolved_path(path) for path in entry_paths
    )

    persisted_graph_paths: dict[str, Path] = {}
    dirty_persisted_modules: set[str] = set()
    use_persisted_graph_cache = project_root is not None and len(entry_paths) == 1
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
            nested_stdlib_scan_modules=nested_stdlib_scan_modules,
            stdlib_allowlist=stdlib_allowlist,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=allow_entry_external_imports,
            resolution_cache=resolution_cache,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted_graph is not None:
            if not persisted_graph.dirty_modules:
                return persisted_graph.graph, persisted_graph.explicit_imports
            persisted_graph_paths = dict(persisted_graph.graph)
            dirty_persisted_modules = set(persisted_graph.dirty_modules)

    def resolve_candidate(candidate: str) -> Path | None:
        persisted_path = persisted_graph_paths.get(candidate)
        if persisted_path is not None and candidate not in dirty_persisted_modules:
            return persisted_path
        return resolution_cache.resolve_module(
            candidate, roots, stdlib_root, stdlib_allowlist
        )

    while queue:
        path = queue.pop()
        queued_paths.discard(path)
        module_name = resolution_cache.module_name_from_path(
            path, module_roots, stdlib_root
        )
        if module_name in graph:
            continue
        graph[module_name] = path
        is_package = path.name == "__init__.py"
        import_scan_mode = _module_graph_import_scan_mode(
            path=path,
            module_name=module_name,
            entry_paths=resolved_entry_paths,
            nested_scan_modules=nested_stdlib_scan_modules,
            resolution_cache=resolution_cache,
        )
        precomputed_imports = (
            precomputed_imports_by_path.get(path)
            if precomputed_imports_by_path is not None
            else None
        )
        if precomputed_imports is not None:
            imports = precomputed_imports
            if use_persisted_graph_cache:
                with contextlib.suppress(OSError):
                    _module_graph_cache._write_persisted_import_scan(
                        cache_project_root,
                        path,
                        module_name=module_name,
                        is_package=is_package,
                        import_scan_mode=import_scan_mode,
                        imports=imports,
                        target_python=target_python,
                        capability_config_digest=capability_config_digest,
                    )
        else:
            persisted_imports = None
            if project_root is not None:
                persisted_imports = _module_graph_cache._read_persisted_import_scan(
                    project_root,
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                )
            if persisted_imports is None:
                try:
                    source = resolution_cache.read_module_source(path)
                except (OSError, SyntaxError, UnicodeDecodeError):
                    continue
                try:
                    tree = resolution_cache.parse_module_ast(
                        path,
                        source,
                        filename=str(path),
                        target_python=target_python,
                    )
                except SyntaxError:
                    continue
                imports = _load_module_imports(
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
                )
            else:
                imports = persisted_imports
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
                if resolved is None or resolved in queued_paths:
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
                if resolved not in queued_paths:
                    queued_paths.add(resolved)
                    queue.append(resolved)
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
                nested_stdlib_scan_modules=nested_stdlib_scan_modules,
                stdlib_allowlist=stdlib_allowlist,
                import_admission_policy=import_admission_policy,
                allow_entry_external_imports=allow_entry_external_imports,
                graph=graph,
                explicit_imports=explicit_imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return graph, explicit_imports


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_imports: Collection[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[dict[str, Path], set[str]]:
    precomputed_imports_by_path = (
        {entry_path: precomputed_imports} if precomputed_imports is not None else None
    )
    return _discover_module_graph_from_paths(
        (entry_path,),
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        resolver_cache=resolver_cache,
        precomputed_imports_by_path=precomputed_imports_by_path,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )


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
) -> tuple[str, ...]:
    if project_root is not None:
        persisted_imports = _module_graph_cache._read_persisted_import_scan(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted_imports is not None:
            return persisted_imports
    imports = resolution_cache.collect_imports(
        path,
        tree,
        collector=_module_import_scanner._collect_imports,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )
    if roots is not None and stdlib_root is not None and stdlib_allowlist is not None:
        imports = (
            _module_import_scanner._expand_imports_with_static_package_all_star_children(
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
            )
        )
    if project_root is not None:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                imports=imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return imports
