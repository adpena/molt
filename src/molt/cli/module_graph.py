from __future__ import annotations

import ast
import os
import re
from collections.abc import Collection, Mapping, MutableMapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

from molt.cli.atomic_io import _write_text_if_changed
from molt.cli import module_dependencies as _module_dependency_authority
from molt.cli import module_graph_discovery as _graph_discovery
from molt.cli import module_import_scanner as _module_import_scanner
from molt.cli import module_resolution as _module_resolution
from molt.cli import module_source as _module_source
from molt.cli import module_stdlib_policy as _module_stdlib_policy
from molt.cli.models import (
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ImportAdmissionPolicy,
    _ImportPlan,
    _ModuleGraphAugmentation,
    _ModuleGraphMetadata,
    _PreparedEntryModuleGraph,
    _SupportModuleAugmentation,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}


STUB_PARENT_MODULES = {"molt"}


ENTRY_OVERRIDE_SPAWN = "multiprocessing.spawn"


@dataclass(frozen=True)
class ModuleSyntaxErrorInfo:
    message: str
    filename: str
    lineno: int | None
    offset: int | None
    text: str | None


def _write_importer_module(output_dir: Path) -> Path:
    lines = [
        '"""Auto-generated import dispatcher for Molt-compiled modules."""',
        "",
        "from __future__ import annotations",
        "",
    ]
    lines.extend(
        [
            "from _intrinsics import require_intrinsic as _require_intrinsic",
            "",
            "_IMPORT_TRANSACTION = _require_intrinsic(",
            "    'molt_importlib_import_transaction', globals()",
            ")",
            "",
            "def _molt_import(name, globals=None, locals=None, fromlist=(), level=0):",
            "    return _IMPORT_TRANSACTION(name, globals, locals, fromlist, level)",
        ]
    )
    path = output_dir / f"{_module_import_scanner.IMPORTER_MODULE_NAME}.py"
    _write_text_if_changed(path, "\n".join(lines) + "\n")
    return path


def _collect_namespace_parents(
    module_graph: Mapping[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: Collection[str] | None = None,
    *,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
) -> set[str]:
    namespace_parents: set[str] = set()
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()

    def maybe_add(name: str) -> None:
        if name in module_graph:
            return
        if (
            resolution_cache.resolve_module(name, roots, stdlib_root, stdlib_allowlist)
            is not None
        ):
            return
        if resolution_cache.has_namespace_dir(
            name, roots, stdlib_root, stdlib_allowlist
        ):
            namespace_parents.add(name)

    for module_name in module_graph:
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            maybe_add(".".join(parts[:idx]))

    if explicit_imports:
        for name in explicit_imports:
            for candidate in _module_dependency_authority._expand_module_chain_cached(name):
                maybe_add(candidate)
    return namespace_parents


def _namespace_paths(name: str, roots: list[Path]) -> list[str]:
    rel = Path(*name.split("."))
    paths: list[str] = []
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            paths.append(str(candidate))
    return list(dict.fromkeys(paths))


def _write_namespace_module(name: str, paths: list[str], output_dir: Path) -> Path:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", name.replace(".", "_")).strip("_")
    if not safe:
        safe = "root"
    stub_path = output_dir / f"namespace_{safe}.py"
    lines = [
        '"""Auto-generated namespace package stub for Molt."""',
        "",
        f"__package__ = {name!r}",
        f"__path__ = {paths!r}",
        "try:",
        "    spec = __spec__",
        "except NameError:",
        "    spec = None",
        "if spec is not None:",
        "    try:",
        "        spec.submodule_search_locations = list(__path__)",
        "    except Exception:",
        "        pass",
        "",
    ]
    stub_path.parent.mkdir(parents=True, exist_ok=True)
    _write_text_if_changed(stub_path, "\n".join(lines))
    return stub_path


def _logical_generated_module_path(module_name: str) -> str:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", module_name).strip("_")
    if not safe:
        safe = "module"
    return f"/__molt_generated__/{safe}.py"


def _collect_package_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    *,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
) -> set[str]:
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    pending: set[str] = set()
    added: set[str] = set()
    for module_name in list(module_graph):
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            pending.add(".".join(parts[:idx]))

    while pending:
        parent = pending.pop()
        if parent in module_graph:
            continue
        resolved = resolution_cache.resolve_module(
            parent, roots, stdlib_root, stdlib_allowlist
        )
        if resolved is None or resolved.name != "__init__.py":
            continue
        if not import_admission_policy.admits_package_parent(
            parent,
            resolved,
            existing_modules=module_graph.keys(),
        ):
            continue
        module_graph[parent] = resolved
        added.add(parent)
        parent_parts = parent.split(".")
        for idx in range(1, len(parent_parts)):
            ancestor = ".".join(parent_parts[:idx])
            if ancestor not in module_graph:
                pending.add(ancestor)
    return added


def _build_module_lowering_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
) -> tuple[
    dict[str, str],
    dict[str, str | None],
    dict[str, bool],
    dict[str, bool],
]:
    logical_source_path_by_module: dict[str, str] = {}
    entry_override_by_module: dict[str, str | None] = {}
    module_is_namespace_by_module: dict[str, bool] = {}
    module_is_package_by_module: dict[str, bool] = {}
    namespace_modules = set(namespace_module_names)
    for module_name in sorted(module_graph):
        module_path = module_graph[module_name]
        logical_source_path_by_module[module_name] = generated_module_source_paths.get(
            module_name, str(module_path)
        )
        # Every lowered module needs to know the canonical entry module name.
        # The frontend uses `entry_module` together with `module_name` to
        # recognize the real entry module and emit __main__ cache/name
        # semantics for it. Passing `None` for the entry module itself causes
        # `__name__` to lower as the ordinary module name instead of "__main__".
        entry_override_by_module[module_name] = entry_module
        module_is_namespace_by_module[module_name] = module_name in namespace_modules
        module_is_package_by_module[module_name] = module_path.name == "__init__.py"
    return (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    )


def _build_frontend_module_costs(
    module_names: Collection[str],
    *,
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _module_source._ModuleSourceCatalog | None = None,
    module_graph: Mapping[str, Path] | None = None,
    module_deps: Mapping[str, set[str]],
) -> dict[str, float]:
    module_costs: dict[str, float] = {}
    for module_name in sorted(module_names):
        source_size = 0
        if module_source_catalog is not None:
            source_size = module_source_catalog.source_size(
                module_name,
                module_graph.get(module_name) if module_graph is not None else None,
            )
        elif module_sources is not None:
            source_size = len(module_sources.get(module_name, ""))
        source_cost = max(1.0, float(source_size))
        dep_cost = float(max(0, len(module_deps.get(module_name, set()))) * 512)
        module_costs[module_name] = source_cost + dep_cost
    return module_costs


def _build_module_graph_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _module_source._ModuleSourceCatalog | None = None,
    module_deps: Mapping[str, set[str]] | None = None,
) -> _ModuleGraphMetadata:
    (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    ) = _build_module_lowering_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
    )
    frontend_module_costs = None
    if module_deps is not None and (
        module_sources is not None or module_source_catalog is not None
    ):
        frontend_module_costs = _build_frontend_module_costs(
            module_graph,
            module_sources=module_sources,
            module_source_catalog=module_source_catalog,
            module_graph=module_graph,
            module_deps=module_deps,
        )
    stdlib_like_by_module = (
        _module_stdlib_policy._build_stdlib_like_module_flags(module_graph)
        if module_deps is not None
        else None
    )
    return _ModuleGraphMetadata(
        logical_source_path_by_module=MappingProxyType(logical_source_path_by_module),
        entry_override_by_module=MappingProxyType(entry_override_by_module),
        module_is_namespace_by_module=MappingProxyType(module_is_namespace_by_module),
        module_is_package_by_module=MappingProxyType(module_is_package_by_module),
        frontend_module_costs=(
            MappingProxyType(frontend_module_costs)
            if frontend_module_costs is not None
            else None
        ),
        stdlib_like_by_module=(
            MappingProxyType(stdlib_like_by_module)
            if stdlib_like_by_module is not None
            else None
        ),
    )


def _requires_spawn_entry_override(
    module_graph: Mapping[str, Path], explicit_imports: Collection[str]
) -> bool:
    names: set[str] = set(module_graph)
    names.update(explicit_imports)
    for name in names:
        if name == ENTRY_OVERRIDE_SPAWN or name.startswith("multiprocessing."):
            return True
        if name == "multiprocessing":
            return True
    return False


def _augment_support_modules(
    *,
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: Collection[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    artifacts_root: Path,
    stub_parents: Collection[str],
    entry_module: str,
    needs_generated_importer: bool,
    diagnostics_enabled: bool,
) -> _SupportModuleAugmentation:
    namespace_parents = _collect_namespace_parents(
        module_graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        explicit_imports,
        resolver_cache=resolver_cache,
    )
    namespace_modules: dict[str, Path] = {}
    if namespace_parents:
        for name in sorted(namespace_parents):
            paths = _namespace_paths(
                name,
                _module_resolution._roots_for_module(
                    name,
                    roots,
                    stdlib_root,
                    stdlib_allowlist,
                ),
            )
            if not paths:
                continue
            stub_path = _write_namespace_module(name, paths, artifacts_root)
            namespace_modules[name] = stub_path
        if namespace_modules:
            module_graph.update(namespace_modules)
            if diagnostics_enabled:
                for name in namespace_modules:
                    _graph_discovery._record_module_reason(
                        module_reasons, name, "namespace_stub"
                    )
    generated_module_source_paths: dict[str, str] = {
        name: _logical_generated_module_path(name) for name in namespace_modules
    }
    for stub in stub_parents:
        if stub != entry_module and stub in namespace_modules:
            module_graph.pop(stub, None)
    if (
        needs_generated_importer
        and _module_import_scanner.IMPORTER_MODULE_NAME not in module_graph
    ):
        importer_path = _write_importer_module(artifacts_root)
        module_graph[_module_import_scanner.IMPORTER_MODULE_NAME] = importer_path
        if diagnostics_enabled:
            _graph_discovery._record_module_reason(
                module_reasons,
                _module_import_scanner.IMPORTER_MODULE_NAME,
                "importer_generated",
            )
    if (
        needs_generated_importer
        and _module_import_scanner.IMPORTER_MODULE_NAME in module_graph
    ):
        generated_module_source_paths.setdefault(
            _module_import_scanner.IMPORTER_MODULE_NAME,
            _logical_generated_module_path(
                _module_import_scanner.IMPORTER_MODULE_NAME
            ),
        )
    return _SupportModuleAugmentation(
        namespace_module_names=frozenset(namespace_modules),
        generated_module_source_paths=generated_module_source_paths,
    )


def _materialize_import_plan(
    *,
    prepared_module_graph: _PreparedEntryModuleGraph,
    module_reasons: MutableMapping[str, set[str]],
    stdlib_root: Path,
    artifacts_root: Path,
    entry_module: str,
    diagnostics_enabled: bool,
) -> _ImportPlan:
    module_graph = dict(prepared_module_graph.module_graph)
    stdlib_allowlist = set(prepared_module_graph.stdlib_allowlist)
    stub_parents = set(prepared_module_graph.stub_parents)
    support_modules = _augment_support_modules(
        module_graph=module_graph,
        module_reasons=module_reasons,
        roots=list(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        explicit_imports=prepared_module_graph.explicit_imports,
        resolver_cache=prepared_module_graph.module_resolution_cache,
        artifacts_root=artifacts_root,
        stub_parents=stub_parents,
        entry_module=entry_module,
        needs_generated_importer=(
            prepared_module_graph.runtime_import_support_policy.needs_generated_importer
        ),
        diagnostics_enabled=diagnostics_enabled,
    )
    namespace_module_names = support_modules.namespace_module_names
    generated_module_source_paths = dict(support_modules.generated_module_source_paths)
    known_modules = frozenset(module_graph)
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=set(namespace_module_names),
    )
    return _ImportPlan(
        stdlib_allowlist=frozenset(stdlib_allowlist),
        roots=tuple(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        module_resolution_cache=prepared_module_graph.module_resolution_cache,
        module_graph=MappingProxyType(dict(module_graph)),
        explicit_imports=frozenset(prepared_module_graph.explicit_imports),
        runtime_import_dispatch_roots=frozenset(
            prepared_module_graph.runtime_import_dispatch_roots
        ),
        stub_parents=frozenset(stub_parents),
        spawn_enabled=prepared_module_graph.spawn_enabled,
        runtime_import_support_policy=prepared_module_graph.runtime_import_support_policy,
        namespace_module_names=namespace_module_names,
        generated_module_source_paths=MappingProxyType(generated_module_source_paths),
        known_modules=known_modules,
        known_modules_sorted=tuple(sorted(known_modules)),
        stdlib_allowlist_sorted=tuple(sorted(stdlib_allowlist)),
        module_graph_metadata=module_graph_metadata,
        native_artifact_plan=prepared_module_graph.native_artifact_plan,
    )


def _augment_module_graph_for_entry_and_runtime(
    *,
    source_path: Path,
    entry_module: str,
    module_roots: Sequence[Path],
    stdlib_root: Path,
    roots: Sequence[Path],
    project_root: Path | None,
    stdlib_allowlist: set[str],
    entry_imports: Collection[str],
    module_resolution_cache: "_module_resolution._ModuleResolutionCache",
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    diagnostics_enabled: bool,
    json_output: bool,
    target: str,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[_ModuleGraphAugmentation, _CliFailure | None]:
    roots = list(roots)
    module_roots = list(module_roots)
    entry_imports = set(entry_imports)
    explicit_imports = set(entry_imports)
    stub_skip_modules = STUB_MODULES - entry_imports
    stub_parents = STUB_PARENT_MODULES - entry_imports
    core_module_names = _module_stdlib_policy._core_stdlib_module_names_for_profile(
        os.environ.get("MOLT_STDLIB_PROFILE", "micro")
    )
    core_paths = [
        path
        for name in core_module_names
        if (path := module_graph.get(name)) is not None
    ]
    _graph_discovery._extend_module_graph_with_closure(
        module_graph,
        entry_paths=core_paths,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=module_resolution_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        reason="core_closure",
        skip_modules=stub_skip_modules,
        stub_parents=stub_parents,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    spawn_enabled = False
    spawn_required = target != "wasm" and _requires_spawn_entry_override(
        module_graph, explicit_imports
    )
    if spawn_required:
        spawn_path = module_resolution_cache.resolve_module(
            ENTRY_OVERRIDE_SPAWN,
            roots,
            stdlib_root,
            stdlib_allowlist,
        )
        if spawn_path is None:
            return _ModuleGraphAugmentation(
                spawn_enabled=False,
                explicit_imports=explicit_imports,
                stub_parents=stub_parents,
            ), _fail(
                (
                    f"Missing required stdlib module: {ENTRY_OVERRIDE_SPAWN}. "
                    "multiprocessing spawn entry override cannot be lowered."
                ),
                json_output,
                command="build",
            )
        spawn_enabled = True
        _graph_discovery._extend_module_graph_with_closure(
            module_graph,
            entry_paths=[spawn_path],
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            project_root=project_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="spawn_closure",
            skip_modules=stub_skip_modules,
            stub_parents=stub_parents,
            import_admission_policy=import_admission_policy,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
    return _ModuleGraphAugmentation(
        spawn_enabled=spawn_enabled,
        explicit_imports=explicit_imports,
        stub_parents=stub_parents,
    ), None


def _prepare_entry_module_graph(
    *,
    source_path: Path,
    entry_module: str,
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    entry_tree: ast.AST,
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    json_output: bool,
    target: str,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[_PreparedEntryModuleGraph | None, _CliFailure | None]:
    stdlib_allowlist = _module_stdlib_policy._stdlib_allowlist()
    roots = module_roots + [stdlib_root]
    module_resolution_cache = _module_resolution._ModuleResolutionCache()
    entry_is_package = source_path.name == "__init__.py"
    entry_imports = (
        _module_import_scanner._expand_imports_with_static_package_all_star_children(
            tuple(
                _module_import_scanner._collect_imports(
                    entry_tree,
                    entry_module,
                    entry_is_package,
                )
            ),
            entry_tree,
            module_name=entry_module,
            is_package=entry_is_package,
            import_scan_mode="full",
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=module_resolution_cache,
            target_python=target_python,
        )
    )
    module_graph, explicit_imports = _graph_discovery._discover_module_graph(
        source_path,
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=STUB_MODULES,
        stub_parents=STUB_PARENT_MODULES,
        resolver_cache=module_resolution_cache,
        precomputed_imports=entry_imports,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if diagnostics_enabled:
        for name in module_graph:
            _graph_discovery._record_module_reason(module_reasons, name, "entry_closure")
    static_import_modules, static_import_error = (
        _graph_discovery._parse_static_import_modules_from_env(os.environ)
    )
    if static_import_error is not None:
        return None, _fail(static_import_error, json_output, command="build")
    static_import_errors = _graph_discovery._extend_module_graph_with_static_import_modules(
        module_graph=module_graph,
        explicit_imports=explicit_imports,
        module_names=static_import_modules,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=module_resolution_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if static_import_errors:
        return None, _fail(
            "; ".join(static_import_errors),
            json_output,
            command="build",
        )
    while True:
        package_before = set(module_graph)
        added_package_parents = _collect_package_parents(
            module_graph,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            import_admission_policy=import_admission_policy,
        )
        if diagnostics_enabled:
            _graph_discovery._record_new_module_reasons(
                module_graph,
                package_before,
                module_reasons,
                "package_parent",
            )
        if not added_package_parents:
            break
        package_parent_paths = [
            module_graph[name]
            for name in sorted(added_package_parents)
            if name in module_graph
        ]
        before_parent_closure = set(module_graph)
        _graph_discovery._extend_module_graph_with_closure(
            module_graph,
            entry_paths=package_parent_paths,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            project_root=project_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="package_parent_closure",
            skip_modules=STUB_MODULES,
            stub_parents=STUB_PARENT_MODULES,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=False,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if diagnostics_enabled:
            _graph_discovery._record_new_module_reasons(
                module_graph,
                before_parent_closure,
                module_reasons,
                "package_parent_closure",
            )
    core_before = set(module_graph)
    _module_stdlib_policy._ensure_core_stdlib_modules(module_graph, stdlib_root)
    if diagnostics_enabled:
        _graph_discovery._record_new_module_reasons(
            module_graph,
            core_before,
            module_reasons,
            "core_required",
        )
    intrinsic_enforced = _module_stdlib_policy._enforce_intrinsic_stdlib(
        module_graph, stdlib_root, json_output
    )
    if intrinsic_enforced is not None:
        return None, intrinsic_enforced
    # MOLT_STDLIB_PROFILE is the single canonical profile signal the module-graph
    # construction and the runtime staticlib build both read (see the
    # `--stdlib-profile` propagation note); use the same source here so the
    # feature-availability refusal matches the profile that will actually link.
    feature_availability_enforced = (
        _module_stdlib_policy._enforce_profile_feature_availability(
            module_graph,
            stdlib_root,
            os.environ.get("MOLT_STDLIB_PROFILE", "micro"),
            target,
            json_output,
        )
    )
    if feature_availability_enforced is not None:
        return None, feature_availability_enforced
    augmentation, augmentation_error = _augment_module_graph_for_entry_and_runtime(
        source_path=source_path,
        entry_module=entry_module,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        roots=roots,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        entry_imports=explicit_imports,
        module_resolution_cache=module_resolution_cache,
        module_graph=module_graph,
        module_reasons=module_reasons,
        diagnostics_enabled=diagnostics_enabled,
        json_output=json_output,
        target=target,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if augmentation_error is not None:
        return None, augmentation_error
    runtime_import_dispatch_roots: set[str] = set(augmentation.explicit_imports)
    runtime_import_support_policy = (
        _module_import_scanner._module_graph_needs_runtime_import_support(
            module_graph=module_graph,
            module_resolution_cache=module_resolution_cache,
            explicit_imports=augmentation.explicit_imports,
            entry_module=entry_module,
            entry_path=source_path,
            entry_tree=entry_tree,
            target_python=target_python,
        )
    )
    if runtime_import_support_policy.needs_runtime_import_support:
        import_support_paths: list[Path] = []
        for module_name in _module_import_scanner._RUNTIME_IMPORT_SUPPORT_ROOT_MODULES:
            module_path = _module_resolution._resolve_module_path(
                module_name,
                [stdlib_root],
            )
            if module_path is None:
                return None, _fail(
                    f"Missing required stdlib support module: {module_name}",
                    json_output,
                    command="build",
                )
            import_support_paths.append(module_path)
        before_support = set(module_graph)
        support_closure_modules = _graph_discovery._extend_module_graph_with_closure(
            module_graph,
            entry_paths=import_support_paths,
            roots=[stdlib_root],
            module_roots=[stdlib_root],
            stdlib_root=stdlib_root,
            project_root=None,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="runtime_import_support",
            import_admission_policy=import_admission_policy,
            target_python=target_python,
        )
        runtime_import_dispatch_roots.update(support_closure_modules)
        if diagnostics_enabled:
            _graph_discovery._record_new_module_reasons(
                module_graph,
                before_support,
                module_reasons,
                "runtime_import_support",
            )
    return _PreparedEntryModuleGraph(
        stdlib_allowlist=stdlib_allowlist,
        roots=roots,
        module_resolution_cache=module_resolution_cache,
        module_graph=dict(module_graph),
        explicit_imports=augmentation.explicit_imports,
        runtime_import_dispatch_roots=frozenset(runtime_import_dispatch_roots),
        stub_parents=augmentation.stub_parents,
        spawn_enabled=augmentation.spawn_enabled,
        runtime_import_support_policy=runtime_import_support_policy,
        native_artifact_plan=(
            import_admission_policy.native_artifact_plan
            if import_admission_policy is not None
            else _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
        ),
    ), None
