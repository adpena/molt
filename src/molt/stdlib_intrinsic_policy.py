from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from molt.target_python import TargetPythonVersion, _parse_source_for_target
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    StaticImportPlan,
    UnresolvedStaticImportError,
    analyze_module_import_flow,
    plan_static_import_request,
    require_static_import_modules,
)

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


@dataclass(frozen=True)
class StdlibModuleImportEvidence:
    """Intrinsic relationships are evidence, not runtime graph admission.

    Only individually resolved sites contribute proven edges. Unresolved sites
    remain explicit obligations and cannot promote a Python-only module or a
    private support fragment through a guessed package or runtime catalog.
    """

    source_path: Path
    proven_modules: frozenset[str]
    unresolved_sites: tuple[tuple[int, StaticImportRequest, StaticImportPlan], ...]


@dataclass(frozen=True)
class StdlibIntrinsicClassification:
    statuses: Mapping[str, str]
    import_evidence: Mapping[str, StdlibModuleImportEvidence]

    def unresolved_imports_payload(self) -> list[dict[str, object]]:
        return [
            {
                "module": module_name,
                "path": str(evidence.source_path),
                "line": line,
                "name": request.name,
                "level": request.level,
                "fromlist": list(request.fromlist),
                "requires_runtime": plan.requires_runtime,
                "errors": list(plan.errors),
            }
            for module_name, evidence in sorted(self.import_evidence.items())
            for line, request, plan in evidence.unresolved_sites
        ]


def stdlib_module_import_evidence(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
) -> StdlibModuleImportEvidence:
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise UnresolvedStaticImportError(
            f"stdlib intrinsic import evidence ({module_name}: {path}, "
            f"Python {target_python.short}) cannot read source: {exc}"
        ) from exc
    try:
        tree = _parse_source_for_target(
            source,
            filename=str(path),
            target_python=target_python,
        )
    except SyntaxError as exc:
        raise UnresolvedStaticImportError(
            f"stdlib intrinsic import evidence ({module_name}: {path}:{exc.lineno}, "
            f"Python {target_python.short}) cannot parse source: {exc.msg}"
        ) from exc

    imports: set[str] = set()
    unresolved_sites: list[tuple[int, StaticImportRequest, StaticImportPlan]] = []
    base_context = ModuleImportContext(
        module_name,
        is_package=path.name == "__init__.py",
        target_python=target_python.feature_version,
    )
    import_flow = analyze_module_import_flow(tree, base_context)

    def contexts_for(node: ast.AST) -> tuple[ModuleImportContext, ...]:
        return tuple(
            base_context.with_state(state) for state in import_flow.states_for(node)
        )

    def record_request(node: ast.AST, request: StaticImportRequest) -> None:
        plan = plan_static_import_request(request, contexts_for(node))
        if plan.requires_runtime or plan.errors:
            unresolved_sites.append((getattr(node, "lineno", 0), request, plan))
        else:
            imports.update(plan.modules)

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                record_request(node, StaticImportRequest.statement(alias.name))
            continue
        if not isinstance(node, ast.ImportFrom):
            continue
        record_request(
            node,
            StaticImportRequest.statement(
                node.module or "",
                level=node.level,
                fromlist=tuple(alias.name for alias in node.names),
            ),
        )
    return StdlibModuleImportEvidence(path, frozenset(imports), tuple(unresolved_sites))


def stdlib_module_static_imports(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
) -> frozenset[str]:
    evidence = stdlib_module_import_evidence(
        module_name, path, target_python=target_python
    )
    for line, request, plan in evidence.unresolved_sites:
        require_static_import_modules(
            plan,
            consumer=(
                f"stdlib intrinsic support graph ({module_name}: "
                f"{path}:{line}, {request.name!r})"
            ),
        )
    return evidence.proven_modules


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
    *,
    target_python: TargetPythonVersion,
) -> StdlibIntrinsicClassification:
    closed = dict(statuses)
    evidence_by_module = {
        module_name: stdlib_module_import_evidence(
            module_name,
            path,
            target_python=target_python,
        )
        for module_name, path in module_graph.items()
        if path and path.suffix == ".py"
    }
    imports_by_module = {
        name: evidence.proven_modules for name, evidence in evidence_by_module.items()
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
    return StdlibIntrinsicClassification(
        MappingProxyType(closed), MappingProxyType(evidence_by_module)
    )


def same_package_intrinsic_import_closure(
    module_graph: Mapping[str, Path],
    statuses: Mapping[str, str],
    *,
    target_python: TargetPythonVersion,
) -> frozenset[str]:
    closed = _closed_intrinsic_statuses(
        module_graph,
        statuses,
        target_python=target_python,
    )
    return frozenset(
        module_name
        for module_name, status in closed.statuses.items()
        if _is_intrinsic_status(status)
    )


def classify_stdlib_module_statuses(
    module_graph: Mapping[str, Path],
    *,
    target_python: TargetPythonVersion,
) -> StdlibIntrinsicClassification:
    statuses = {
        module_name: stdlib_module_intrinsic_status(path)
        for module_name, path in module_graph.items()
        if path and path.suffix == ".py"
    }
    return _closed_intrinsic_statuses(
        module_graph,
        statuses,
        target_python=target_python,
    )
