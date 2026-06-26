from __future__ import annotations

import ast
from pathlib import Path
from typing import Mapping

STATUS_INTRINSIC = "intrinsic-backed"
STATUS_INTRINSIC_PARTIAL = "intrinsic-partial"
STATUS_INTRINSIC_SUPPORT = "intrinsic-support"
STATUS_POLICY_GATE = "policy-gate"
STATUS_PROBE_ONLY = "probe-only"
STATUS_PYTHON_ONLY = "python-only"

INTRINSIC_CALL_NAMES = frozenset(
    {
        "load_intrinsic",
        "require_intrinsic",
        "require_optional_intrinsic",
        "_load_intrinsic",
        "_intrinsic_load",
        "_intrinsics_require",
        "_intrinsic_require",
        "_require_intrinsic",
        "_require_callable_intrinsic",
    }
)
LAZY_INTRINSIC_CALL_NAMES = frozenset({"_lazy_intrinsic"})
STDLIB_PROBE_INTRINSIC = "molt_stdlib_probe"


def is_fail_closed_import_policy_gate(text: str) -> bool:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return False
    body = list(tree.body)
    if (
        body
        and isinstance(body[0], ast.Expr)
        and isinstance(body[0].value, ast.Constant)
        and isinstance(body[0].value.value, str)
    ):
        body = body[1:]
    while (
        body and isinstance(body[0], ast.ImportFrom) and body[0].module == "__future__"
    ):
        body = body[1:]
    if len(body) != 1 or not isinstance(body[0], ast.Raise):
        return False
    exc = body[0].exc
    if isinstance(exc, ast.Call):
        exc = exc.func
    if isinstance(exc, ast.Name):
        return exc.id == "ImportError"
    if isinstance(exc, ast.Attribute):
        return exc.attr == "ImportError"
    return False


def _call_name(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def intrinsic_names_from_source(source: str) -> frozenset[str]:
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return frozenset()

    intrinsic_names: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        call_name = _call_name(node.func)
        if call_name not in INTRINSIC_CALL_NAMES | LAZY_INTRINSIC_CALL_NAMES:
            continue
        first: ast.expr | None = None
        if node.args:
            first = node.args[0]
        else:
            for keyword in node.keywords:
                if keyword.arg == "name":
                    first = keyword.value
                    break
        if not isinstance(first, ast.Constant) or not isinstance(first.value, str):
            continue
        name = first.value
        if name.startswith("molt_"):
            intrinsic_names.add(name)
    return frozenset(intrinsic_names)


def module_required_intrinsic_names(path: Path) -> frozenset[str]:
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return frozenset()
    return intrinsic_names_from_source(source)


def stdlib_module_intrinsic_status_from_source(source: str, path_name: str) -> str:
    if path_name == "_intrinsics.py":
        return STATUS_INTRINSIC

    intrinsic_names = intrinsic_names_from_source(source)
    if not intrinsic_names:
        if is_fail_closed_import_policy_gate(source):
            return STATUS_POLICY_GATE
        return STATUS_PYTHON_ONLY
    if intrinsic_names == {STDLIB_PROBE_INTRINSIC}:
        return STATUS_PROBE_ONLY
    return STATUS_INTRINSIC


def stdlib_module_intrinsic_status(path: Path) -> str:
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return STATUS_PYTHON_ONLY
    return stdlib_module_intrinsic_status_from_source(source, path.name)


def module_relative_import_base(
    module_name: str,
    path: Path,
    *,
    level: int,
    imported_module: str | None,
) -> str | None:
    if level <= 0:
        return imported_module
    package_parts = module_name.split(".")
    if path.name != "__init__.py":
        package_parts = package_parts[:-1]
    if level > 1:
        package_parts = package_parts[: max(0, len(package_parts) - (level - 1))]
    if not package_parts:
        return imported_module
    base = ".".join(package_parts)
    if imported_module:
        return f"{base}.{imported_module}"
    return base


def stdlib_module_static_imports(module_name: str, path: Path) -> frozenset[str]:
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return frozenset()
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return frozenset()

    imports: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.add(alias.name)
            continue
        if not isinstance(node, ast.ImportFrom):
            continue
        base = module_relative_import_base(
            module_name,
            path,
            level=node.level,
            imported_module=node.module,
        )
        if base is None:
            continue
        imports.add(base)
        if node.names and all(alias.name != "*" for alias in node.names):
            imports.update(f"{base}.{alias.name}" for alias in node.names)
        if (
            isinstance(node, ast.ImportFrom)
            and node.module == "importlib.util"
            and node.level == 0
        ):
            imports.add("importlib.util")
    imports.update(_stdlib_module_support_file_references(module_name, tree))
    return frozenset(imports)


def _stdlib_module_support_file_references(module_name: str, tree: ast.AST) -> set[str]:
    if "." in module_name:
        package_name = module_name.rsplit(".", 1)[0]
    else:
        package_name = ""
    references: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Constant) or not isinstance(node.value, str):
            continue
        value = node.value
        if not value.endswith(".py"):
            continue
        stem = Path(value).stem
        if not stem.startswith("_") or stem == "__init__":
            continue
        if package_name:
            references.add(f"{package_name}.{stem}")
        else:
            references.add(stem)
    return references


def _module_family_matches(owner: str, support: str) -> bool:
    if "." in owner:
        owner_package = owner.rsplit(".", 1)[0]
        support_package = support.rsplit(".", 1)[0] if "." in support else ""
        return owner_package == support_package
    return support.startswith(f"{owner}_")


def _is_private_support_module(module_name: str) -> bool:
    leaf = module_name.rsplit(".", 1)[-1]
    return leaf.startswith("_") and leaf != "__init__"


def _is_intrinsic_status(status: str | None) -> bool:
    return status in {
        STATUS_INTRINSIC,
        STATUS_INTRINSIC_PARTIAL,
        STATUS_INTRINSIC_SUPPORT,
    }


def _closed_intrinsic_statuses(
    module_graph: Mapping[str, Path],
    statuses: Mapping[str, str],
) -> dict[str, str]:
    closed = dict(statuses)
    imports_by_module = {
        module_name: stdlib_module_static_imports(module_name, path)
        for module_name, path in module_graph.items()
        if path and path.suffix == ".py"
    }
    changed = True
    while changed:
        changed = False
        for module_name, imports in imports_by_module.items():
            if _is_intrinsic_status(closed.get(module_name)):
                continue
            if closed.get(module_name) != STATUS_PYTHON_ONLY:
                continue
            package_root = module_name.split(".", 1)[0]
            if any(
                _is_intrinsic_status(closed.get(imported))
                and imported.split(".", 1)[0] == package_root
                for imported in imports
            ):
                closed[module_name] = STATUS_INTRINSIC
                changed = True
                continue
            if _is_private_support_module(module_name) and any(
                _is_intrinsic_status(closed.get(owner))
                and module_name in owner_imports
                and _module_family_matches(owner, module_name)
                for owner, owner_imports in imports_by_module.items()
            ):
                closed[module_name] = STATUS_INTRINSIC_SUPPORT
                changed = True
    return closed


def same_package_intrinsic_import_closure(
    module_graph: Mapping[str, Path],
    statuses: Mapping[str, str],
) -> frozenset[str]:
    closed = _closed_intrinsic_statuses(module_graph, statuses)
    return frozenset(
        module_name
        for module_name, status in closed.items()
        if _is_intrinsic_status(status)
    )


def classify_stdlib_module_statuses(
    module_graph: Mapping[str, Path],
) -> dict[str, str]:
    statuses = {
        module_name: stdlib_module_intrinsic_status(path)
        for module_name, path in module_graph.items()
        if path and path.suffix == ".py"
    }
    return _closed_intrinsic_statuses(module_graph, statuses)
