from __future__ import annotations

import ast
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.cli.target_python import TargetPythonVersion, _parse_source_for_target
from molt.cli.extension_manifest import (
    ExtensionSupportFile,
    _manifest_support_file_payloads,
)
from molt.cli.module_import_scanner import _module_init_scan_nodes
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    analyze_module_import_flow,
    plan_static_import_request,
    require_static_import_modules,
)


def _dotted_parts(name: str) -> tuple[str, ...] | None:
    parts = tuple(part for part in name.split(".") if part)
    if not parts or any(not part.isidentifier() for part in parts):
        return None
    return parts


def _support_source_path_for_module(
    *,
    source_root: Path,
    module_name: str,
) -> Path | None:
    parts = _dotted_parts(module_name)
    if parts is None:
        return None
    module_path = source_root.joinpath(*parts).with_suffix(".py")
    init_path = source_root.joinpath(*parts, "__init__.py")
    for candidate in (module_path, init_path):
        if candidate.is_file():
            return candidate.resolve()
    return None


def _support_module_name_for_path(
    *,
    source_root: Path,
    source_path: Path,
) -> str | None:
    try:
        rel_path = source_path.resolve().relative_to(source_root.resolve())
    except ValueError:
        return None
    if rel_path.suffix != ".py":
        return None
    parts = list(rel_path.parts)
    if not parts:
        return None
    if parts[-1] == "__init__.py":
        module_parts = parts[:-1]
    else:
        module_parts = [*parts[:-1], rel_path.stem]
    if not module_parts or any(not part.isidentifier() for part in module_parts):
        return None
    return ".".join(module_parts)


def _module_init_import_nodes(tree: ast.AST) -> tuple[ast.Import | ast.ImportFrom, ...]:
    return tuple(
        node
        for node in _module_init_scan_nodes(tree)
        if isinstance(node, (ast.Import, ast.ImportFrom))
    )


def _existing_package_module_imports(
    *,
    source_root: Path,
    package: str,
    import_node: ast.Import | ast.ImportFrom,
    import_contexts: tuple[ModuleImportContext, ...],
) -> set[str]:
    package_prefix = f"{package}."
    imported: set[str] = set()

    def add_if_existing(name: str | None) -> bool:
        if name is None:
            return False
        if name != package and not name.startswith(package_prefix):
            return False
        if _support_source_path_for_module(source_root=source_root, module_name=name):
            imported.add(name)
            return True
        return False

    if isinstance(import_node, ast.Import):
        for alias in import_node.names:
            add_if_existing(alias.name)
        return imported

    base_modules = require_static_import_modules(
        plan_static_import_request(
            StaticImportRequest.statement(
                import_node.module or "", level=import_node.level
            ),
            import_contexts,
        ),
        consumer="extension support graph",
    )
    for base_module in base_modules:
        if import_node.module:
            add_if_existing(base_module)
        for alias in import_node.names:
            if alias.name == "*":
                add_if_existing(base_module)
                continue
            child_module = f"{base_module}.{alias.name}" if base_module else alias.name
            if add_if_existing(child_module):
                continue
            if import_node.module:
                add_if_existing(base_module)
    return imported


def _package_internal_imports(
    *,
    source_root: Path,
    package: str,
    module_name: str,
    source_path: Path,
    target_python: TargetPythonVersion,
) -> tuple[str, ...]:
    try:
        tree = _parse_source_for_target(
            source_path.read_text(encoding="utf-8", errors="replace"),
            filename=str(source_path),
            target_python=target_python,
        )
    except (OSError, SyntaxError, UnicodeDecodeError):
        return ()
    base_context = ModuleImportContext(
        module_name,
        is_package=source_path.name == "__init__.py",
        target_python=target_python.feature_version,
    )
    import_flow = analyze_module_import_flow(tree, base_context)
    imports: set[str] = set()
    for import_node in _module_init_import_nodes(tree):
        imports.update(
            _existing_package_module_imports(
                source_root=source_root,
                package=package,
                import_node=import_node,
                import_contexts=tuple(
                    base_context.with_state(state)
                    for state in import_flow.states_for(import_node)
                ),
            )
        )
    return tuple(
        sorted(
            name
            for name in imports
            if name != module_name
        )
    )


def _module_attr_provider_modules(
    *,
    extension_module: str,
    callable_exports: Sequence[Mapping[str, Any]],
) -> tuple[str, ...]:
    modules: set[str] = set()
    for export in callable_exports:
        if export.get("binding") != "module_attr":
            continue
        provider = export.get("provider_module")
        if not isinstance(provider, str) or not provider.strip():
            provider = extension_module
        provider = provider.strip()
        if provider != extension_module:
            modules.add(provider)
    return tuple(sorted(modules))


def _derive_module_attr_support_source_rel_paths(
    *,
    source_root: Path,
    package: str,
    extension_module: str,
    callable_exports: Sequence[Mapping[str, Any]],
    target_python: TargetPythonVersion,
) -> tuple[str, ...]:
    pending = list(
        _module_attr_provider_modules(
            extension_module=extension_module,
            callable_exports=callable_exports,
        )
    )
    seen_modules: set[str] = set()
    rel_paths: set[str] = set()
    resolved_root = source_root.resolve()
    package_prefix = f"{package}."

    while pending:
        module_name = pending.pop()
        if module_name in seen_modules:
            continue
        seen_modules.add(module_name)
        if module_name != package and not module_name.startswith(package_prefix):
            continue
        source_path = _support_source_path_for_module(
            source_root=resolved_root,
            module_name=module_name,
        )
        if source_path is None:
            continue
        discovered_module = _support_module_name_for_path(
            source_root=resolved_root,
            source_path=source_path,
        )
        if discovered_module is None:
            continue
        rel_paths.add(source_path.relative_to(resolved_root).as_posix())
        for imported in reversed(
            _package_internal_imports(
                source_root=resolved_root,
                package=package,
                module_name=discovered_module,
                source_path=source_path,
                target_python=target_python,
            )
        ):
            if imported not in seen_modules:
                pending.append(imported)

    return tuple(sorted(rel_paths))


def module_attr_support_files(
    raw_support_files: Sequence[Any],
    *,
    field_name: str,
    source_root: Path,
    package: str,
    extension_module: str,
    callable_exports: Sequence[Mapping[str, Any]],
    target_python: TargetPythonVersion,
    errors: list[str],
) -> tuple[ExtensionSupportFile, ...]:
    """Combine explicit support files with source-derived module-attr providers.

    Module-attr callable exports are backed by upstream Python provider modules.
    Those providers and their package-internal import-time dependencies must be
    checksummed in the manifest so external-native staging can compile/support
    them without package-specific file lists.
    """
    explicit = _manifest_support_file_payloads(
        list(raw_support_files),
        field_name=field_name,
        root=source_root,
        errors=errors,
    )
    derived_paths = _derive_module_attr_support_source_rel_paths(
        source_root=source_root,
        package=package,
        extension_module=extension_module,
        callable_exports=callable_exports,
        target_python=target_python,
    )
    derived = _manifest_support_file_payloads(
        list(derived_paths),
        field_name=f"{field_name}.module_attr_provider_closure",
        root=source_root,
        errors=errors,
    )
    support_by_rel = {entry.rel_path: entry for entry in explicit}
    for entry in derived:
        support_by_rel.setdefault(entry.rel_path, entry)
    return tuple(support_by_rel[rel_path] for rel_path in sorted(support_by_rel))
