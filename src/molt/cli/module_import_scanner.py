from __future__ import annotations
from dataclasses import replace
from molt.compiler_analysis.python_imports import (
    SourceAllExports,
    normalize_stdlib_module_name,
)
from molt.compiler_analysis import python_import_scan as _source_import_scan

import ast
import ntpath
import os
import posixpath
from collections.abc import Collection, Mapping, Sequence
from pathlib import Path
from typing import cast

from molt.cli import module_resolution as _module_resolution
from molt.cli.models import (
    ImportScanMode,
    _CompleteImportScan,
    _ModuleGraphScanAuthority,
    _RuntimeImportSupportPolicy,
)
from molt.compiler_analysis.python_imports import _ImportScanRequests, _StaticSourcePath
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
from molt.compiler_analysis.python_identity_catalog_generated import (
    ALL_INVALID_MEMBERS,
    PythonIdentity,
)
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_bindings,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    _PythonAstDigestAdmission,
)


IMPORTER_MODULE_NAME = "_molt_importer"


_RUNTIME_IMPORT_PROTOCOL_MARKERS = (
    "import ",
    "from ",
    "__import__",
    "import_module",
    "find_spec",
)


_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES = (
    "importlib",
    "importlib.util",
    "importlib.machinery",
)


_RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES = frozenset(
    {
        "builtins",
        "_intrinsics",
        *_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES,
        "importlib.abc",
        IMPORTER_MODULE_NAME,
    }
)


def _source_may_use_runtime_import_protocol(source: str) -> bool:
    return any(marker in source for marker in _RUNTIME_IMPORT_PROTOCOL_MARKERS)


def _tree_uses_runtime_import_protocol(
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    include_statements: bool = False,
    namespace_premise: PythonNamespacePremise | None = None,
) -> bool:
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    binding_index = analyze_python_bindings(
        cast(ast.Module, tree),
        source_digest=ast_digest_admission.digest,
        ast_digest_admission=ast_digest_admission,
        policy=PythonBindingPolicy(
            target_python=target_python.feature_version,
            module_name=module_name,
            module_spec_name=module_name,
            module_is_package=is_package,
            module_execution_kind="script" if module_name is None else "imported",
            foreign_namespace_writes=(
                None if namespace_premise is None else namespace_premise.foreign_writes
            ),
            callback_member_writes=(
                ALL_INVALID_MEMBERS
                if namespace_premise is None
                else namespace_premise.member_writes
            ),
        ),
    )
    for node in _source_import_scan._scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        module_name=module_name,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
        namespace_premise=namespace_premise,
    ):
        if not binding_index.module_import_flow.states_for(node):
            continue
        if include_statements:
            if isinstance(node, ast.Import):
                if any(alias.name != "_intrinsics" for alias in node.names):
                    return True
            elif isinstance(node, ast.ImportFrom):
                if node.module == "__future__":
                    continue
                if node.level == 0 and (
                    node.module == "_intrinsics"
                    or node.module is not None
                    and node.module.endswith("._intrinsics")
                ):
                    continue
                return True
        if isinstance(node, ast.Call):
            fact = binding_index.call_fact(node)
            if fact is not None and (
                fact.possible_import_call_kinds()
                or fact.callee_may_be(PythonIdentity.IMPORTLIB_FIND_SPEC)
            ):
                return True
    return False


def _expand_static_package_all_star_children(
    star_modules: tuple[str, ...],
    *,
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolution_cache: _module_resolution._ModuleResolutionCache,
    target_python: TargetPythonVersion,
    module_graph: Mapping[str, Path] | None = None,
    source_requests: Mapping[str, _ImportScanRequests] | None = None,
) -> tuple[dict[str, tuple[str, ...]], dict[str, SourceAllExports]]:
    """Complete source export facts against this operation's actual topology."""
    children: dict[str, tuple[str, ...]] = {}
    exports_by_module: dict[str, SourceAllExports] = {}

    def resolve(name: str) -> Path | None:
        if module_graph is not None:
            return module_graph.get(normalize_stdlib_module_name(name) or name)
        return resolution_cache.resolve_module(
            name, list(roots), stdlib_root, stdlib_allowlist
        )

    for module in dict.fromkeys(star_modules):
        path = resolve(module)
        known = (source_requests or {}).get(
            normalize_stdlib_module_name(module) or module
        )
        if path is None:
            exports = SourceAllExports(
                "incomplete", reason="package source is unavailable"
            )
        elif path.name != "__init__.py":
            exports = SourceAllExports("absent")
        elif known is not None:
            exports = known.all_exports
        else:
            try:
                source = resolution_cache.read_module_source(path, retain=False)
                parsed = resolution_cache.parse_module_ast(
                    path,
                    source,
                    filename=str(path),
                    target_python=target_python,
                    retain=False,
                )
                exports = parsed.all_exports(target_python)
            except (OSError, SyntaxError, UnicodeError) as exc:
                exports = SourceAllExports("incomplete", reason=type(exc).__name__)
        exports_by_module[module] = exports
        children[module] = tuple(
            dict.fromkeys(
                f"{module}.{name}"
                for name in exports.names
                if exports.status == "known" and resolve(f"{module}.{name}") is not None
            )
        )
    return children, exports_by_module


def _resolve_static_source_path(
    request: str | _StaticSourcePath, source_path: Path
) -> str | Path:
    if isinstance(request, str):
        return request
    parts = tuple(
        _resolve_static_source_path(part, source_path) for part in request.parts
    )
    head, *tail = parts
    if request.operation == "join":
        return Path(head).joinpath(*tail)
    joiner = {
        "os_join": os.path.join,
        "posix_join": posixpath.join,
        "nt_join": ntpath.join,
    }.get(request.operation)
    if joiner is not None:
        return joiner(*(os.fspath(part) for part in parts))
    head = Path(head)
    if request.operation in {"resolve", "absolute"}:
        if not head.is_absolute():
            head = source_path.parent / head
        return head.resolve() if request.operation == "resolve" else head.absolute()
    return head


def _complete_import_scan(
    requests: _ImportScanRequests,
    *,
    source_path: Path,
    roots: Sequence[Path] | None,
    stdlib_root: Path | None,
    stdlib_allowlist: set[str] | None,
    resolution_cache: _module_resolution._ModuleResolutionCache,
    target_python: TargetPythonVersion,
    module_graph: Mapping[str, Path] | None = None,
    source_requests: Mapping[str, _ImportScanRequests] | None = None,
) -> _CompleteImportScan:
    """Resolve every filesystem-derived decision in this operation, including misses."""
    imports = requests.imports
    candidates = requests.dynamic_relative_import_candidates
    star_children: dict[str, tuple[str, ...]] = {}
    star_exports: dict[str, SourceAllExports] = {}
    stars = tuple(
        dict.fromkeys((*requests.star_modules, *requests.dynamic_star_modules))
    )
    if stars:
        if module_graph is None and (
            roots is None or stdlib_root is None or stdlib_allowlist is None
        ):
            raise ValueError(
                "star-import completion requires current resolution context"
            )
        star_children, star_exports = _expand_static_package_all_star_children(
            stars,
            roots=roots or (),
            stdlib_root=stdlib_root or source_path.parent,
            stdlib_allowlist=stdlib_allowlist or set(),
            resolution_cache=resolution_cache,
            target_python=target_python,
            module_graph=module_graph,
            source_requests=source_requests,
        )
        imports = tuple(
            dict.fromkeys(
                (
                    *imports,
                    *(
                        child
                        for module in requests.star_modules
                        for child in star_children[module]
                    ),
                )
            )
        )
        candidates = tuple(
            dict.fromkeys(
                (
                    *candidates,
                    *(
                        child
                        for module in requests.dynamic_star_modules
                        for child in star_children[module]
                    ),
                )
            )
        )
    sites = []
    for site in requests.source_sites:
        if site.request is not None and "*" in site.request.fromlist:
            owners = site.owner_plan.modules
            children = tuple(
                child for owner in owners for child in star_children.get(owner, ())
            )
            unknown_exports = tuple(
                (owner, fact)
                for owner in owners
                if (
                    fact := star_exports.get(owner, SourceAllExports("incomplete"))
                ).status
                in {"unknown", "incomplete"}
            )
            site = replace(
                site,
                plan=replace(
                    site.plan,
                    modules=tuple(dict.fromkeys((*site.plan.modules, *children))),
                    requires_runtime=site.plan.requires_runtime
                    or bool(unknown_exports),
                    errors=tuple(
                        dict.fromkeys(
                            (
                                *site.plan.errors,
                                *(
                                    f"unresolved star exports for {owner!r}: "
                                    f"{fact.reason or fact.status}"
                                    for owner, fact in unknown_exports
                                ),
                            )
                        )
                    ),
                ),
            )
        sites.append(site)
    executions: list[tuple[str | None, Path]] = []
    seen: set[tuple[str | None, Path]] = set()
    for request in requests.source_executions:
        resolved = Path(_resolve_static_source_path(request.path, source_path))
        if not resolved.is_absolute():
            resolved = source_path.parent / resolved
        resolved = resolved.resolve()
        if resolved.is_dir():
            resolved = resolved / "__main__.py"
        if resolved.suffix not in {".py", ".pyi"} or not resolved.is_file():
            continue
        execution = (request.module_name, resolved)
        if execution not in seen:
            seen.add(execution)
            executions.append(execution)
    return _CompleteImportScan(
        requests,
        imports,
        tuple(executions),
        candidates,
        requests.requires_runtime_package_anchor
        or any(
            fact.status in {"unknown", "incomplete"} for fact in star_exports.values()
        ),
        requests.source_demands,
        requests.source_bindings,
        requests.source_values,
        tuple(sites),
        tuple(star_exports.items()),
    )


def _explicit_imports_reference_generated_importer(
    explicit_imports: Collection[str],
) -> bool:
    return any(
        name == IMPORTER_MODULE_NAME or name.startswith(f"{IMPORTER_MODULE_NAME}.")
        for name in explicit_imports
    )


def _module_uses_runtime_import_protocol(
    *,
    module_name: str,
    module_path: Path,
    module_resolution_cache: "_module_resolution._ModuleResolutionCache",
    target_python: TargetPythonVersion,
    import_scan_mode: ImportScanMode = "full",
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    tree: ast.AST | None = None,
    is_package: bool | None = None,
    namespace_premise: PythonNamespacePremise | None = None,
) -> bool:
    if module_name in _RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES:
        return False
    if is_package is None:
        is_package = module_path.name == "__init__.py"

    def produce() -> bool:
        scan_tree = tree
        if scan_tree is None:
            try:
                source = module_resolution_cache.read_module_source(
                    module_path, retain=False
                )
            except (OSError, SyntaxError, UnicodeDecodeError):
                # Keep runtime import support enabled when analysis cannot prove the
                # graph is fully static.
                return True
            if not _source_may_use_runtime_import_protocol(source):
                return False
            try:
                parsed = module_resolution_cache.parse_module_ast(
                    module_path,
                    source,
                    filename=str(module_path),
                    retain=True,
                    target_python=target_python,
                )
                scan_tree = parsed.tree
                ast_digest_admission = parsed.admission
            except SyntaxError:
                return True
        else:
            ast_digest_admission = _PythonAstDigestAdmission(scan_tree)
        return _tree_uses_runtime_import_protocol(
            scan_tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            source_owner=source_owner,
            source_arguments=source_arguments,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
            include_statements=True,
            namespace_premise=namespace_premise,
        )

    return module_resolution_cache.uses_runtime_import_protocol(
        module_path,
        producer=produce,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        target_python_tag=target_python.tag,
        namespace_premise=namespace_premise,
    )


def _module_graph_needs_runtime_import_support(
    *,
    module_graph: Mapping[str, Path],
    scan_authority: _ModuleGraphScanAuthority,
    module_resolution_cache: "_module_resolution._ModuleResolutionCache",
    explicit_imports: Collection[str],
    entry_module: str,
    entry_path: Path,
    entry_tree: ast.AST | None,
    target_python: TargetPythonVersion,
) -> _RuntimeImportSupportPolicy:
    scan_authority.validate_graph(module_graph)
    scan_authority.validate_source_generations()
    try:
        needs_generated_importer = _explicit_imports_reference_generated_importer(
            explicit_imports
        )
        if needs_generated_importer:
            return _RuntimeImportSupportPolicy(
                needs_generated_importer=True,
                needs_runtime_import_support=True,
            )
        for module_name, module_path in sorted(module_graph.items()):
            tree = (
                entry_tree
                if module_name == entry_module and module_path == entry_path
                else None
            )
            import_scan_mode = scan_authority.mode_for(module_name, module_path)
            owners = (
                ()
                if import_scan_mode == "full"
                else sorted(scan_authority.by_module[module_name].deferred_owners)
            )
            for source_owner, source_arguments in ((None, ()), *owners):
                if _module_uses_runtime_import_protocol(
                    module_name=module_name,
                    module_path=module_path,
                    module_resolution_cache=module_resolution_cache,
                    target_python=target_python,
                    import_scan_mode=import_scan_mode,
                    source_owner=source_owner,
                    source_arguments=source_arguments,
                    tree=tree,
                    is_package=scan_authority.by_module[module_name].is_package,
                    namespace_premise=scan_authority.by_module[
                        module_name
                    ].namespace_premise,
                ):
                    return _RuntimeImportSupportPolicy(
                        needs_generated_importer=False,
                        needs_runtime_import_support=True,
                    )
        return _RuntimeImportSupportPolicy(
            needs_generated_importer=False,
            needs_runtime_import_support=False,
        )
    finally:
        scan_authority.validate_source_generations()
