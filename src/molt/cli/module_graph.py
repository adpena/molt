from __future__ import annotations

import ast
import json
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
from molt.cli.config_resolution import (
    DEFAULT_STDLIB_PROFILE,
    MOLT_STDLIB_PROFILE_ENV,
)
from molt.cli import module_stdlib_policy as _module_stdlib_policy
from molt.cli.models import (
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _BinaryImageScope,
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
from molt.compiler_analysis import native_support_slice as _native_support_slice
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}


STUB_PARENT_MODULES = {"molt"}


ENTRY_OVERRIDE_SPAWN = "multiprocessing.spawn"


_NATIVE_SUPPORT_ARTIFACT_SOURCE_SUFFIXES = (".pyx", ".c", ".cc", ".cpp", ".cxx")


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


_ENTRY_REACHABLE_REASONS = frozenset({"entry_closure"})
_RUNTIME_SUPPORT_REASONS = frozenset(
    {"runtime_import_support", "importer_generated", "spawn_closure"}
)
_STDLIB_SUPPORT_REASONS = frozenset({"core_required", "core_closure"})
_PACKAGE_PARENT_REASONS = frozenset(
    {"package_parent", "package_parent_closure", "namespace_stub"}
)


def _modules_with_any_reason(
    module_graph: Mapping[str, Path],
    module_reasons: Mapping[str, set[str]],
    reasons: Collection[str],
) -> frozenset[str]:
    reason_set = set(reasons)
    return frozenset(
        name
        for name in module_graph
        if module_reasons.get(name, set()).intersection(reason_set)
    )


def _native_support_source_admission_policy(
    native_artifact_plan,
) -> _ImportAdmissionPolicy:
    return _ImportAdmissionPolicy(
        external_roots=native_artifact_plan.package_source_roots(),
        admitted_external_packages=frozenset(),
        native_artifact_source_packages=frozenset(
            artifact.package for artifact in native_artifact_plan.artifacts
        ),
        native_artifact_plan=native_artifact_plan,
    )


def _extend_native_support_source_closure(
    *,
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    native_artifact_plan,
    artifacts_root: Path,
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    target_python: TargetPythonVersion,
    capability_config_digest: str,
) -> frozenset[str]:
    support_paths_by_module = native_artifact_plan.support_source_paths_by_module()
    if not support_paths_by_module:
        return frozenset()
    native_support_function_roots_by_module = (
        _native_support_function_roots_by_module(native_artifact_plan)
    )
    entry_paths = tuple(
        path
        for module_name, path in support_paths_by_module.items()
        if (
            module_name not in module_graph
            and module_name in native_support_function_roots_by_module
        )
    )
    if not entry_paths:
        return frozenset()
    module_roots = [root for root in roots if root != stdlib_root]
    pruned_support_paths = _materialize_pruned_native_support_sources(
        native_artifact_plan=native_artifact_plan,
        roots_by_module=native_support_function_roots_by_module,
        artifacts_root=artifacts_root,
    )
    closure_graph, explicit_imports = _graph_discovery._discover_module_graph_from_paths(
        entry_paths,
        roots,
        module_roots,
        stdlib_root,
        project_root=None,
        stdlib_allowlist=stdlib_allowlist,
        skip_modules=STUB_MODULES,
        stub_parents=STUB_PARENT_MODULES,
        resolver_cache=resolver_cache,
        precomputed_imports_by_path=_native_support_source_imports_by_path(
            native_artifact_plan,
            native_support_function_roots_by_module,
        ),
        import_admission_policy=_native_support_source_admission_policy(
            native_artifact_plan
        ),
        allow_entry_external_imports=False,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    for module_name, path in closure_graph.items():
        module_graph.setdefault(
            module_name,
            pruned_support_paths.get(path.resolve(), path),
        )
        _graph_discovery._record_module_reason(
            module_reasons,
            module_name,
            (
                "native_support_source"
                if module_name in support_paths_by_module
                else "native_support_source_closure"
            ),
        )
    return frozenset(explicit_imports)


def _relative_import_module_name(
    current_module: str,
    *,
    level: int,
    module: str | None,
) -> str:
    if level <= 0:
        return module or ""
    package_parts = current_module.split(".")[:-1]
    if level > 1:
        package_parts = package_parts[: max(0, len(package_parts) - (level - 1))]
    tail = [part for part in (module or "").split(".") if part]
    return ".".join([*package_parts, *tail])


def _support_source_import_bindings(
    module_name: str,
    tree: ast.Module,
) -> tuple[dict[str, tuple[str, str]], dict[str, str]]:
    imported_functions: dict[str, tuple[str, str]] = {}
    imported_modules: dict[str, str] = {}
    for stmt in tree.body:
        if isinstance(stmt, ast.Import):
            for alias in stmt.names:
                bind_name = alias.asname or alias.name.split(".", 1)[0]
                imported_modules[bind_name] = alias.name
        elif isinstance(stmt, ast.ImportFrom):
            source_module = _relative_import_module_name(
                module_name,
                level=stmt.level,
                module=stmt.module,
            )
            for alias in stmt.names:
                if alias.name == "*":
                    continue
                bind_name = alias.asname or alias.name
                if stmt.module is None:
                    imported_modules[bind_name] = ".".join(
                        part for part in (source_module, alias.name) if part
                    )
                else:
                    imported_functions[bind_name] = (source_module, alias.name)
    return imported_functions, imported_modules


def _support_source_top_level_defs(
    tree: ast.Module,
) -> dict[str, ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef]:
    return _native_support_slice.top_level_support_defs(tree)


def _native_support_function_roots_by_module(native_artifact_plan) -> dict[str, tuple[str, ...]]:
    support_paths_by_module = native_artifact_plan.support_source_paths_by_module()
    if not support_paths_by_module:
        return {}
    roots: dict[str, set[str]] = {}
    for artifact in native_artifact_plan.artifacts:
        for export in artifact.callable_exports:
            provider_module = export.provider_module
            if provider_module is None or provider_module not in support_paths_by_module:
                continue
            roots.setdefault(provider_module, set()).add(export.name)
    parsed: dict[str, ast.Module] = {}
    support_defs_by_module: dict[
        str, dict[str, ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef]
    ] = {}
    imports_by_module: dict[
        str, tuple[dict[str, tuple[str, str]], dict[str, str]]
    ] = {}

    def module_tree(module: str) -> ast.Module | None:
        if module in parsed:
            return parsed[module]
        path = support_paths_by_module.get(module)
        if path is None:
            return None
        try:
            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(path))
        except (OSError, SyntaxError, UnicodeDecodeError):
            return None
        parsed[module] = tree
        support_defs_by_module[module] = _support_source_top_level_defs(tree)
        imports_by_module[module] = _support_source_import_bindings(module, tree)
        return tree

    class _ReferenceVisitor(ast.NodeVisitor):
        def __init__(self, module: str) -> None:
            self.module = module
            self.local_refs: set[str] = set()
            self.import_refs: set[tuple[str, str]] = set()

        def visit_Name(self, node: ast.Name) -> None:
            if not isinstance(node.ctx, ast.Load):
                return
            local_defs = support_defs_by_module.get(self.module, {})
            if node.id in local_defs:
                self.local_refs.add(node.id)
            imported_functions, _imported_modules = imports_by_module.get(
                self.module,
                ({}, {}),
            )
            target = imported_functions.get(node.id)
            if target is not None:
                self.import_refs.add(target)

        def visit_Attribute(self, node: ast.Attribute) -> None:
            if isinstance(node.value, ast.Name):
                _imported_functions, imported_modules = imports_by_module.get(
                    self.module,
                    ({}, {}),
                )
                target_module = imported_modules.get(node.value.id)
                if target_module is not None:
                    self.import_refs.add((target_module, node.attr))
            self.generic_visit(node)

    queue: list[tuple[str, str]] = [
        (module, root) for module, module_roots in roots.items() for root in module_roots
    ]
    seen: set[tuple[str, str]] = set()
    while queue:
        module, root = queue.pop()
        if (module, root) in seen:
            continue
        seen.add((module, root))
        if module_tree(module) is None:
            continue
        support_defs = support_defs_by_module.get(module, {})
        support_def = support_defs.get(root)
        if support_def is None:
            continue
        visitor = _ReferenceVisitor(module)
        visitor.visit(support_def)
        for local_ref in sorted(visitor.local_refs):
            if local_ref not in roots.setdefault(module, set()):
                roots[module].add(local_ref)
                queue.append((module, local_ref))
        for target_module, target_name in sorted(visitor.import_refs):
            if target_module not in support_paths_by_module:
                continue
            target_defs = support_defs_by_module.get(target_module)
            if target_defs is None:
                if module_tree(target_module) is None:
                    continue
                target_defs = support_defs_by_module.get(target_module, {})
            if target_name not in target_defs:
                continue
            if target_name not in roots.setdefault(target_module, set()):
                roots[target_module].add(target_name)
                queue.append((target_module, target_name))
    return {module: tuple(sorted(module_roots)) for module, module_roots in sorted(roots.items())}


def _native_support_source_imports_by_path(
    native_artifact_plan,
    roots_by_module: Mapping[str, Sequence[str]],
) -> dict[Path, tuple[str, ...]]:
    imports_by_path: dict[Path, tuple[str, ...]] = {}
    support_paths_by_module = native_artifact_plan.support_source_paths_by_module()
    for module, path in support_paths_by_module.items():
        try:
            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(path))
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        roots = frozenset(roots_by_module.get(module, ()))
        if not roots:
            imports_by_path[path] = tuple(
                _module_import_scanner._collect_imports(
                    tree,
                    module_name=module,
                    is_package=path.name == "__init__.py",
                    import_scan_mode="module_init",
                )
            )
            continue
        pruned_tree, _reachable, _missing = (
            _native_support_slice.prune_native_support_module(tree, roots)
        )
        imports_by_path[path] = tuple(
            _module_import_scanner._collect_imports(
                pruned_tree,
                module_name=module,
                is_package=path.name == "__init__.py",
                import_scan_mode="full",
            )
        )
    return imports_by_path


def _native_support_generated_path(module_name: str, artifacts_root: Path) -> Path:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", module_name).strip("_")
    if not safe:
        safe = "module"
    return artifacts_root / f"native_support_{safe}.py"


def _materialize_pruned_native_support_sources(
    *,
    native_artifact_plan,
    roots_by_module: Mapping[str, Sequence[str]],
    artifacts_root: Path,
) -> dict[Path, Path]:
    generated_paths: dict[Path, Path] = {}
    support_paths_by_module = native_artifact_plan.support_source_paths_by_module()
    for module, source_path in support_paths_by_module.items():
        roots = frozenset(roots_by_module.get(module, ()))
        if not roots:
            continue
        try:
            source = source_path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(source_path))
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        pruned_tree, _reachable, missing = (
            _native_support_slice.prune_native_support_module(tree, roots)
        )
        if missing:
            continue
        ast.fix_missing_locations(pruned_tree)
        generated_path = _native_support_generated_path(module, artifacts_root)
        _write_text_if_changed(generated_path, ast.unparse(pruned_tree) + "\n")
        generated_paths[source_path.resolve()] = generated_path
    return generated_paths


def _longest_module_prefix(
    module_name: str,
    candidates: Collection[str],
) -> str | None:
    for candidate in reversed(
        _module_dependency_authority._expand_module_chain_cached(module_name)
    ):
        if candidate in candidates:
            return candidate
    return None


def _missing_native_support_artifact_imports(
    *,
    support_explicit_imports: Collection[str],
    module_graph: Mapping[str, Path],
    native_artifact_plan,
) -> tuple[str, ...]:
    if not support_explicit_imports or not native_artifact_plan.artifacts:
        return ()
    native_packages = frozenset(
        artifact.package for artifact in native_artifact_plan.artifacts
    )
    native_modules = native_artifact_plan.native_module_names()
    exact_native_providers = frozenset(
        {artifact.module for artifact in native_artifact_plan.artifacts}
        | native_artifact_plan.support_source_module_names()
    )
    source_modules = frozenset(module_graph)
    missing: list[str] = []
    for import_name in sorted(set(support_explicit_imports)):
        if import_name in source_modules or import_name in native_modules:
            continue
        if not any(
            import_name == package or import_name.startswith(package + ".")
            for package in native_packages
        ):
            continue
        source_prefix = _longest_module_prefix(import_name, source_modules)
        if source_prefix is not None:
            source_path = module_graph[source_prefix]
            if source_prefix == import_name or source_path.name != "__init__.py":
                continue
        native_prefix = _longest_module_prefix(import_name, native_modules)
        if native_prefix in exact_native_providers:
            continue
        missing.append(import_name)
    return tuple(missing)


def _native_support_artifact_source_candidates(
    *,
    native_artifact_plan,
    module_name: str,
) -> tuple[Path, ...]:
    candidates: list[Path] = []
    seen: set[Path] = set()
    for artifact in native_artifact_plan.artifacts:
        package = artifact.package
        if module_name == package:
            continue
        if not module_name.startswith(package + "."):
            continue
        module_parts = tuple(
            part for part in module_name[len(package) + 1 :].split(".") if part
        )
        if not module_parts:
            continue
        leaf = module_parts[-1]
        parent_parts = module_parts[:-1]
        search_roots = (
            artifact.package_dir.joinpath(*parent_parts),
            artifact.package_dir.joinpath(*parent_parts, "src"),
            artifact.package_dir / "src",
            *_native_support_artifact_manifest_search_roots(artifact),
        )
        for search_root in search_roots:
            for suffix in _NATIVE_SUPPORT_ARTIFACT_SOURCE_SUFFIXES:
                candidate = (search_root / f"{leaf}{suffix}").resolve()
                if candidate in seen or not candidate.is_file():
                    continue
                seen.add(candidate)
                candidates.append(candidate)
    return tuple(candidates)


def _native_support_artifact_manifest_search_roots(artifact) -> tuple[Path, ...]:
    try:
        manifest = json.loads(artifact.manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return ()
    if not isinstance(manifest, Mapping):
        return ()
    roots: list[Path] = []
    for raw_path in manifest.get("sources") or ():
        if isinstance(raw_path, str) and raw_path.strip():
            roots.append(Path(raw_path).expanduser().resolve().parent)
    build = manifest.get("build")
    if isinstance(build, Mapping):
        for raw_path in build.get("include_dirs") or ():
            if isinstance(raw_path, str) and raw_path.strip():
                roots.append(Path(raw_path).expanduser().resolve())
    return tuple(dict.fromkeys(roots))


def _format_missing_native_support_artifact_imports(
    *,
    missing_imports: Sequence[str],
    native_artifact_plan,
) -> str:
    details: list[str] = []
    for module_name in missing_imports:
        candidates = _native_support_artifact_source_candidates(
            native_artifact_plan=native_artifact_plan,
            module_name=module_name,
        )
        if candidates:
            preview = ", ".join(str(path) for path in candidates[:4])
            suffix = "" if len(candidates) <= 4 else ", ..."
            details.append(f"{module_name} (source candidates: {preview}{suffix})")
        else:
            details.append(
                f"{module_name} (no .pyx/.c/.cpp source candidate found under the "
                "admitted package roots)"
            )
    return (
        "reachable native support source imports native package modules without "
        "source or artifact custody: "
        + "; ".join(details)
        + ". Configure the upstream package for the target, publish reachable "
        "static_link artifacts from that target-specific source plan, and admit "
        "those modules through sidecar custody; Molt will not synthesize native "
        "package modules from package visibility."
    )


def _runtime_import_parent_modules(
    roots: Collection[str],
    *,
    known_modules: Collection[str],
) -> frozenset[str]:
    known = set(known_modules)
    parents: set[str] = set()
    for root in roots:
        parts = root.split(".")
        for idx in range(1, len(parts)):
            parent = ".".join(parts[:idx])
            if parent in known:
                parents.add(parent)
    return frozenset(parents)


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
    support_explicit_imports: set[str] = set()
    native_artifact_plan = _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    for _iteration in range(len(prepared_module_graph.native_artifact_plan.artifacts) + 2):
        native_artifact_reachable_imports = (
            set(module_graph)
            | set(prepared_module_graph.explicit_imports)
            | set(prepared_module_graph.runtime_import_dispatch_roots)
            | support_explicit_imports
        )
        next_native_artifact_plan = (
            prepared_module_graph.native_artifact_plan.with_reachable_imports(
                native_artifact_reachable_imports
            )
        )
        before_modules = frozenset(module_graph)
        before_support_imports = frozenset(support_explicit_imports)
        support_explicit_imports.update(
            _extend_native_support_source_closure(
                module_graph=module_graph,
                module_reasons=module_reasons,
                native_artifact_plan=next_native_artifact_plan,
                artifacts_root=artifacts_root,
                roots=list(prepared_module_graph.roots),
                stdlib_root=stdlib_root,
                stdlib_allowlist=stdlib_allowlist,
                resolver_cache=prepared_module_graph.module_resolution_cache,
                target_python=_DEFAULT_TARGET_PYTHON_VERSION,
                capability_config_digest="",
            )
        )
        if (
            next_native_artifact_plan == native_artifact_plan
            and frozenset(module_graph) == before_modules
            and frozenset(support_explicit_imports) == before_support_imports
        ):
            break
        native_artifact_plan = next_native_artifact_plan
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
        diagnostics_enabled=True,
    )
    namespace_module_names = support_modules.namespace_module_names
    generated_module_source_paths = dict(support_modules.generated_module_source_paths)
    missing_native_support_imports = _missing_native_support_artifact_imports(
        support_explicit_imports=support_explicit_imports,
        module_graph=module_graph,
        native_artifact_plan=native_artifact_plan,
    )
    if missing_native_support_imports:
        raise ValueError(
            _format_missing_native_support_artifact_imports(
                missing_imports=missing_native_support_imports,
                native_artifact_plan=native_artifact_plan,
            )
        )
    source_modules = frozenset(module_graph)
    native_artifact_modules = native_artifact_plan.native_module_names()
    native_support_function_roots_by_module = (
        _native_support_function_roots_by_module(native_artifact_plan)
    )
    known_modules = source_modules | native_artifact_modules
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=set(namespace_module_names),
    )
    declared_root_modules = (
        frozenset({entry_module})
        | prepared_module_graph.declared_root_modules
    )
    entry_reachable_modules = _modules_with_any_reason(
        module_graph, module_reasons, _ENTRY_REACHABLE_REASONS
    ) | frozenset({entry_module})
    runtime_support_modules = _modules_with_any_reason(
        module_graph, module_reasons, _RUNTIME_SUPPORT_REASONS
    )
    stdlib_support_modules = _modules_with_any_reason(
        module_graph, module_reasons, _STDLIB_SUPPORT_REASONS
    )
    package_parent_modules = _modules_with_any_reason(
        module_graph, module_reasons, _PACKAGE_PARENT_REASONS
    )
    explicit_runtime_import_dispatch_roots = (
        prepared_module_graph.runtime_import_dispatch_roots | support_explicit_imports
    )
    runtime_import_dispatch_roots = frozenset(
        explicit_runtime_import_dispatch_roots
        | _runtime_import_parent_modules(
            explicit_runtime_import_dispatch_roots,
            known_modules=known_modules,
        )
    )
    return _ImportPlan(
        image_scope=prepared_module_graph.image_scope,
        stdlib_allowlist=frozenset(stdlib_allowlist),
        roots=tuple(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        module_resolution_cache=prepared_module_graph.module_resolution_cache,
        module_graph=MappingProxyType(dict(module_graph)),
        explicit_imports=frozenset(prepared_module_graph.explicit_imports),
        runtime_import_dispatch_roots=runtime_import_dispatch_roots,
        stub_parents=frozenset(stub_parents),
        spawn_enabled=prepared_module_graph.spawn_enabled,
        runtime_import_support_policy=prepared_module_graph.runtime_import_support_policy,
        namespace_module_names=namespace_module_names,
        generated_module_source_paths=MappingProxyType(generated_module_source_paths),
        known_modules=known_modules,
        direct_call_modules=source_modules,
        declared_root_modules=frozenset(
            name for name in declared_root_modules if name in source_modules
        ),
        entry_reachable_modules=frozenset(
            name for name in entry_reachable_modules if name in source_modules
        ),
        runtime_support_modules=frozenset(
            name for name in runtime_support_modules if name in source_modules
        ),
        stdlib_support_modules=frozenset(
            name for name in stdlib_support_modules if name in source_modules
        ),
        package_parent_modules=frozenset(
            name for name in package_parent_modules if name in source_modules
        ),
        compile_modules=source_modules,
        known_modules_sorted=tuple(sorted(known_modules)),
        stdlib_allowlist_sorted=tuple(sorted(stdlib_allowlist)),
        module_graph_metadata=module_graph_metadata,
        native_artifact_plan=native_artifact_plan,
        native_support_function_roots_by_module=(
            native_support_function_roots_by_module
        ),
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
        os.environ.get(MOLT_STDLIB_PROFILE_ENV, DEFAULT_STDLIB_PROFILE)
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
    image_scope: _BinaryImageScope | None = None,
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
    _graph_discovery._record_module_reason(module_reasons, entry_module, "entry_root")
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
        diagnostics_enabled=True,
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
            diagnostics_enabled=True,
            module_reasons=module_reasons,
            reason="package_parent_closure",
            skip_modules=STUB_MODULES,
            stub_parents=STUB_PARENT_MODULES,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=False,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        _graph_discovery._record_new_module_reasons(
            module_graph,
            before_parent_closure,
            module_reasons,
            "package_parent_closure",
        )
    core_before = set(module_graph)
    _module_stdlib_policy._ensure_core_stdlib_modules(module_graph, stdlib_root)
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
    # Runtime-feature availability is now decided by REACHABILITY, not whole-file
    # import-graph presence (Option b,
    # docs/design/foundation/feature_reachability_tree_shaking.md): the coarse
    # ``_enforce_profile_feature_availability`` gate forced a feature the instant a
    # module appeared anywhere in the static import graph - even when no reached
    # code path ever linked one of its intrinsics - and keyed on the
    # ``MOLT_STDLIB_PROFILE`` env var rather than the resolved ``--stdlib-profile``
    # that actually selects the staticlib (so an env/flag mismatch could pass the
    # gate yet still fail at link). The authoritative requirement check now runs in
    # ``backend_ir._reachability_feature_refusal`` over the finalized merged
    # SimpleIR (``required_features.required_link_features``) against the resolved
    # profile's link-affecting ceiling - it refuses exactly when the REACHED code
    # links an intrinsic the profile excludes, with a truthful, reached-intrinsic
    # message. ``_enforce_profile_feature_availability`` /
    # ``_profile_feature_gap_for_module`` survive only as the whole-file helper the
    # loud-refusal unit tests pin; they no longer drive the build.
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
        diagnostics_enabled=True,
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
            diagnostics_enabled=True,
            module_reasons=module_reasons,
            reason="runtime_import_support",
            import_admission_policy=import_admission_policy,
            target_python=target_python,
        )
        runtime_import_dispatch_roots.update(support_closure_modules)
        _graph_discovery._record_new_module_reasons(
            module_graph,
            before_support,
            module_reasons,
            "runtime_import_support",
        )
    if image_scope is None:
        image_scope = _BinaryImageScope.from_entry(
            kind="entry_script",
            selector_source="legacy:entry",
            entry_module=entry_module,
            source_path=source_path,
            project_root=project_root or source_path.parent,
            module_roots=module_roots,
        )
    image_scope = image_scope.with_root_modules(
        [
            entry_module,
            *(
                name
                for name in sorted(static_import_modules)
                if name in module_graph
            ),
        ]
    )
    return _PreparedEntryModuleGraph(
        image_scope=image_scope,
        declared_root_modules=frozenset(
            name for name in image_scope.root_modules if name in module_graph
        ),
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
