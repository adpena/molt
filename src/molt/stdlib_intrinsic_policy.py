from __future__ import annotations

import ast
from molt.compiler_analysis.python_import_scan import (
    _collect_imports_for_graph,
    _collect_import_scan_requests,
)
from molt.compiler_analysis.python_source_keys import (
    _PythonAstDigestAdmission,
    python_node_source_key,
)
from molt.compiler_analysis.python_imports import (
    _ImportScanRequests,
)
from molt.compiler_analysis.python_namespace_graph import (
    PythonNamespacePremise,
    graph_python_namespace_writes,
)
from molt.compiler_analysis.python_namespace_writes import (
    PythonNamespaceSummary,
    python_namespace_summary,
    python_package_name,
)
from collections import deque
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path
from types import MappingProxyType
from typing import Mapping, cast

from molt.target_python import TargetPythonVersion, _parse_source_for_target
from molt.compiler_analysis.python_imports import (
    StaticImportRequest,
    StaticImportPlan,
    UnresolvedStaticImportError,
    normalize_stdlib_module_name,
    require_static_import_modules,
)

STATUS_INTRINSIC = "intrinsic-backed"
STATUS_INTRINSIC_PARTIAL = "intrinsic-partial"
STATUS_INTRINSIC_SUPPORT = "intrinsic-support"
STATUS_POLICY_GATE = "policy-gate"
# Molt compiles the module's own Python source; it reads no intrinsic.
STATUS_PYTHON_COMPILED = "python-compiled"
# A generated stand-in for a module Molt has not lowered (tools/gen_stdlib_stubs.py).
STATUS_STUB = "stub"
# The canonical gap error every generated stub raises; the stub generator owns it.
STDLIB_STUB_MARKER = (
    "is not fully lowered yet; only an intrinsic-first stub is available."
)

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


def is_fail_closed_import_policy_gate(text: str | bytes) -> bool:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return False
    return _is_fail_closed_import_policy_gate_tree(tree)


def _is_fail_closed_import_policy_gate_tree(tree: ast.Module) -> bool:
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


def _required_intrinsic_name(node: ast.Call) -> str | None:
    """The literal ``molt_*`` name a loader call requires, if it names one."""
    if _call_name(node.func) not in INTRINSIC_CALL_NAMES | LAZY_INTRINSIC_CALL_NAMES:
        return None
    first: ast.expr | None = None
    if node.args:
        first = node.args[0]
    else:
        for keyword in node.keywords:
            if keyword.arg == "name":
                first = keyword.value
                break
    if not isinstance(first, ast.Constant) or not isinstance(first.value, str):
        return None
    return first.value if first.value.startswith("molt_") else None


def intrinsic_names_from_source(source: str | bytes) -> frozenset[str]:
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return frozenset()
    return _intrinsic_names_from_tree(tree)


def _intrinsic_names_from_tree(tree: ast.Module) -> frozenset[str]:
    return frozenset(
        name
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and (name := _required_intrinsic_name(node)) is not None
    )


def module_required_intrinsic_names(path: Path) -> frozenset[str]:
    try:
        source = path.read_bytes()
    except Exception:
        return frozenset()
    return intrinsic_names_from_source(source)


@dataclass(frozen=True)
class StdlibIntrinsicBinding:
    """A private module-level name bound to an intrinsic, unread in its module."""

    name: str
    intrinsic: str
    line: int


@dataclass(frozen=True)
class StdlibModuleIntrinsicUse:
    """The intrinsics a module reads, and the requirements it never reads.

    A module reads a requirement when it loads or exports (public name or
    ``__all__`` entry) the module-level name bound to it, requires it inside a
    function or class body, or consumes it in an expression. A requirement
    whose value a statement discards is not a read, and neither is a private
    module-level binding the module never loads. Another module can still
    import such a binding by name, so classification decides it over the
    whole graph.
    """

    used: frozenset[str]
    unread_bindings: tuple[StdlibIntrinsicBinding, ...]
    discarded: tuple[tuple[str, int], ...]


def _module_all_names(tree: ast.Module) -> frozenset[str]:
    names: set[str] = set()
    for node in ast.walk(tree):
        value: ast.expr | None = None
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in node.targets
        ):
            value = node.value
        elif (
            isinstance(node, (ast.AnnAssign, ast.AugAssign))
            and isinstance(node.target, ast.Name)
            and node.target.id == "__all__"
        ):
            value = node.value
        elif (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "__all__"
        ):
            value = ast.Tuple(elts=list(node.args), ctx=ast.Load())
        if value is None:
            continue
        names.update(
            item.value
            for item in ast.walk(value)
            if isinstance(item, ast.Constant) and isinstance(item.value, str)
        )
    return frozenset(names)


def _intrinsic_use_from_tree(tree: ast.Module) -> StdlibModuleIntrinsicUse:
    parents: dict[ast.AST, ast.AST] = {}
    loads: set[str] = set()
    for node in ast.walk(tree):
        for child in ast.iter_child_nodes(node):
            parents[child] = node
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
            loads.add(node.id)
    exported = _module_all_names(tree)

    def at_module_scope(node: ast.AST) -> bool:
        scope = parents.get(node)
        while scope is not None:
            if isinstance(
                scope, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)
            ):
                return False
            scope = parents.get(scope)
        return True

    used: set[str] = set()
    unread: list[StdlibIntrinsicBinding] = []
    discarded: list[tuple[str, int]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        name = _required_intrinsic_name(node)
        if name is None:
            continue
        parent = parents.get(node)
        if isinstance(parent, ast.Expr):
            discarded.append((name, node.lineno))
            continue
        if (
            isinstance(parent, (ast.Assign, ast.AnnAssign))
            and parent.value is node
            and at_module_scope(parent)
        ):
            targets = (
                parent.targets if isinstance(parent, ast.Assign) else [parent.target]
            )
            if all(isinstance(target, ast.Name) for target in targets):
                bound = [cast(ast.Name, target).id for target in targets]
                if any(
                    binding in loads
                    or not binding.startswith("_")
                    or binding in exported
                    for binding in bound
                ):
                    used.add(name)
                else:
                    unread.extend(
                        StdlibIntrinsicBinding(binding, name, node.lineno)
                        for binding in bound
                    )
                continue
        used.add(name)
    return StdlibModuleIntrinsicUse(
        frozenset(used),
        tuple(sorted(unread, key=lambda item: (item.line, item.name))),
        tuple(sorted(discarded, key=lambda item: (item[1], item[0]))),
    )


def is_stdlib_stub_source(source: str | bytes) -> bool:
    """Whether the source is a stub: a module ``__getattr__`` raising the gap error.

    The audit and the stub generator share this one structural test.
    """
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return False
    return _is_generated_stub_tree(tree)


def _is_generated_stub_tree(tree: ast.Module) -> bool:
    """The generator's stub: a module ``__getattr__`` raising the gap error."""
    for node in tree.body:
        if not isinstance(node, ast.FunctionDef) or node.name != "__getattr__":
            continue
        for raised in ast.walk(node):
            if (
                isinstance(raised, ast.Raise)
                and isinstance(raised.exc, ast.Call)
                and _call_name(raised.exc.func) == "RuntimeError"
                and any(
                    isinstance(arg, ast.Constant)
                    and isinstance(arg.value, str)
                    and STDLIB_STUB_MARKER in arg.value
                    for arg in raised.exc.args
                )
            ):
                return True
    return False


def _stdlib_module_intrinsic_status_from_tree(
    tree: ast.Module, path_name: str, use: StdlibModuleIntrinsicUse
) -> str:
    """The status a module's own source proves, before graph relationships."""
    if path_name == "_intrinsics.py":
        return STATUS_INTRINSIC
    if _is_generated_stub_tree(tree):
        return STATUS_STUB
    if use.used:
        return STATUS_INTRINSIC
    if _is_fail_closed_import_policy_gate_tree(tree):
        return STATUS_POLICY_GATE
    return STATUS_PYTHON_COMPILED


@dataclass(frozen=True)
class StdlibFacadeBinding:
    export_name: str
    owner_module: str | None
    imported_name: str
    line: int
    # Filled by classification only when the fromlist name is a real graph node.
    imported_module: str | None = None


@dataclass(frozen=True)
class StdlibFacadeEvidence:
    """Pure forwarding syntax, not proof of symbol existence or runtime parity."""

    bindings: tuple[StdlibFacadeBinding, ...]

    @property
    def owners(self) -> frozenset[str]:
        return frozenset(
            owner
            for binding in self.bindings
            for owner in (binding.owner_module, binding.imported_module)
            if owner is not None
        )

    @property
    def resolved(self) -> bool:
        return bool(self.bindings) and all(
            binding.owner_module is not None for binding in self.bindings
        )


@dataclass(frozen=True)
class StdlibImportSite:
    line: int
    # None denotes an actual dynamic-import call whose static request is opaque.
    request: StaticImportRequest | None
    plan: StaticImportPlan
    # The from-list candidates in plan are not required module owners.
    owner_plan: StaticImportPlan

    @property
    def unresolved(self) -> bool:
        return self.request is None or any(
            plan.errors or plan.requires_runtime
            for plan in (self.plan, self.owner_plan)
        )


@dataclass(frozen=True)
class StdlibModuleImportEvidence:
    """Source-bound plans, not runtime graph admission or cached closure results."""

    source_path: Path
    source_scan: _ImportScanRequests
    facade: StdlibFacadeEvidence | None
    private_imports: frozenset[tuple[str, str]]
    completed_sites: tuple[StdlibImportSite, ...] | None = None
    completed_imports: frozenset[str] | None = None

    @property
    def sites(self) -> tuple[StdlibImportSite, ...]:
        if self.completed_sites is not None:
            return self.completed_sites
        return tuple(
            StdlibImportSite(
                site.line,
                site.request,
                site.plan,
                site.owner_plan,
            )
            for site in self.source_scan.source_sites
        )

    @property
    def proven_modules(self) -> frozenset[str]:
        if self.completed_imports is not None:
            return self.completed_imports
        return frozenset(
            module
            for site in self.sites
            if not site.unresolved
            for module in site.plan.modules
        )

    @property
    def unresolved_sites(
        self,
    ) -> tuple[tuple[int, StaticImportRequest, StaticImportPlan], ...]:
        return tuple(
            (site.line, site.request, site.plan)
            for site in self.sites
            if site.unresolved and site.request is not None
        )


@dataclass(frozen=True)
class StdlibModuleIntrinsicFacts:
    """One source generation's facts, before graph-dependent classification."""

    status: str
    import_evidence: StdlibModuleImportEvidence
    intrinsic_use: StdlibModuleIntrinsicUse
    namespace_summary: PythonNamespaceSummary


@dataclass(frozen=True)
class StdlibIntrinsicClassification:
    """Graph-closed statuses, and the intrinsic reads that decided them.

    ``used_intrinsics`` holds what each module reads, including private
    bindings another module imports. ``unused_bindings`` and
    ``discarded_requirements`` are requirements nothing reads.
    """

    statuses: Mapping[str, str]
    import_evidence: Mapping[str, StdlibModuleImportEvidence]
    used_intrinsics: Mapping[str, frozenset[str]]
    unused_bindings: Mapping[str, tuple[StdlibIntrinsicBinding, ...]]
    discarded_requirements: Mapping[str, tuple[tuple[str, int], ...]]

    def facades_payload(self) -> list[dict[str, object]]:
        return [
            {
                "module": module_name,
                "path": str(evidence.source_path),
                "status": self.statuses.get(module_name),
                "reason": (
                    "pure-reexport"
                    if self.statuses.get(module_name) == STATUS_INTRINSIC_SUPPORT
                    else None
                ),
                "resolved": facade.resolved,
                "owners": sorted(facade.owners),
                "bindings": [
                    {
                        "export_name": binding.export_name,
                        "owner_module": binding.owner_module,
                        "imported_name": binding.imported_name,
                        "imported_module": binding.imported_module,
                        "line": binding.line,
                    }
                    for binding in facade.bindings
                ],
            }
            for module_name, evidence in sorted(self.import_evidence.items())
            if (facade := evidence.facade) is not None
        ]

    def unresolved_imports_payload(self) -> list[dict[str, object]]:
        return [
            {
                "module": module_name,
                "path": str(evidence.source_path),
                "line": site.line,
                "name": site.request.name
                if site.request is not None
                else "<opaque import call>",
                "level": site.request.level if site.request is not None else None,
                "fromlist": list(site.request.fromlist)
                if site.request is not None
                else [],
                "requires_runtime": site.plan.requires_runtime,
                "requires_runtime_execution": site.plan.requires_runtime_execution,
                "errors": list(site.plan.errors),
            }
            for module_name, evidence in sorted(self.import_evidence.items())
            for site in evidence.sites
            if site.unresolved
        ]


def _pure_facade_imports(tree: ast.Module) -> tuple[ast.ImportFrom, ...] | None:
    """Recognize a literal forwarding subset of the existing import AST.

    No export evaluation or alternate import resolution belongs here. Anything
    outside this subset remains subject to the existing non-facade rules.
    """
    body = list(tree.body)
    if (
        body
        and isinstance(body[0], ast.Expr)
        and isinstance(body[0].value, ast.Constant)
        and isinstance(body[0].value.value, str)
    ):
        body.pop(0)
    future_bindings: set[str] = set()
    while body:
        future = body[0]
        if not (
            isinstance(future, ast.ImportFrom)
            and future.level == 0
            and future.module == "__future__"
        ):
            break
        body.pop(0)
        for alias in future.names:
            # Future aliases could mutate module metadata or collide with exports.
            if alias.name == "*" or alias.asname is not None:
                return None
            future_bindings.add(alias.name)
    if not body:
        return None
    # Providers can own target-dependent export lists. Forwarding support
    # depends on every resolved owner, never public/private module spelling.
    # This recognizes syntax only; import execution still validates exports.
    if all(isinstance(node, ast.ImportFrom) for node in body):
        imports = cast(list[ast.ImportFrom], body)
        for node in imports:
            for alias in node.names:
                binding = alias.asname or alias.name
                if binding in future_bindings or (
                    binding.startswith("__")
                    and binding.endswith("__")
                    and binding != "__all__"
                ):
                    return None
        return tuple(imports)
    if len(body) < 2:
        return None
    declaration = body.pop()
    if not (
        isinstance(declaration, ast.Assign)
        and len(declaration.targets) == 1
        and isinstance(declaration.targets[0], ast.Name)
        and declaration.targets[0].id == "__all__"
        and isinstance(declaration.value, (ast.List, ast.Tuple))
    ):
        return None
    exports: list[str] = []
    for item in declaration.value.elts:
        if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
            return None
        exports.append(item.value)
    imports: list[ast.ImportFrom] = []
    bindings: set[str] = set()
    for node in body:
        if not isinstance(node, ast.ImportFrom) or node.module == "__future__":
            return None
        for alias in node.names:
            binding = alias.asname or alias.name
            if (
                alias.name == "*"
                or binding in bindings
                or binding in future_bindings
                or (binding.startswith("__") and binding.endswith("__"))
            ):
                return None
            bindings.add(binding)
        imports.append(node)
    if not bindings or len(exports) != len(bindings) or set(exports) != bindings:
        return None
    return tuple(imports)


def stdlib_module_import_evidence(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
) -> StdlibModuleImportEvidence:
    return stdlib_module_intrinsic_facts(
        module_name, path, target_python=target_python
    ).import_evidence


def _stdlib_module_source_tree(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
    source: str | bytes | None = None,
) -> ast.Module:
    try:
        if source is None:
            source = path.read_bytes()
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

    return tree


def stdlib_module_namespace_summary(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
) -> PythonNamespaceSummary:
    tree = _stdlib_module_source_tree(module_name, path, target_python=target_python)
    return python_namespace_summary(
        tree,
        package=python_package_name(module_name, is_package=path.name == "__init__.py"),
        target_python=target_python.feature_version,
    )


@dataclass(frozen=True, slots=True)
class StdlibNamespaceInventory:
    module_graph: Mapping[str, Path]
    summaries: Mapping[str, PythonNamespaceSummary]
    premises: Mapping[str, PythonNamespacePremise]


def stdlib_namespace_inventory(
    module_graph: Mapping[str, Path],
    *,
    target_python: TargetPythonVersion,
    namespace_provider: Callable[[str, Path], PythonNamespaceSummary] | None = None,
) -> StdlibNamespaceInventory:
    """Derive one premise over an explicit complete inventory, without discovery.

    Only summaries survive each source visit; no whole-inventory AST retention
    or binding-analysis warmup is needed. Package bindings include extension
    modules and namespace descendants from the same supplied topology.
    """
    summaries = {
        name: (
            namespace_provider(name, path)
            if namespace_provider is not None
            else stdlib_module_namespace_summary(
                name, path, target_python=target_python
            )
        )
        for name, path in module_graph.items()
        if path and path.suffix == ".py"
    }
    writes = graph_python_namespace_writes(
        (write for summary in summaries.values() for write in summary.foreign_writes),
        module_graph.get,
    )
    children: dict[str, set[str]] = {}
    for name in module_graph:
        parent, _, child = name.rpartition(".")
        while parent:
            children.setdefault(parent, set()).add(child)
            parent, _, child = parent.rpartition(".")
    return StdlibNamespaceInventory(
        MappingProxyType(dict(module_graph)),
        MappingProxyType(summaries),
        MappingProxyType(
            {
                name: writes.premise(
                    module_graph[name], frozenset(children.get(name, ()))
                )
                for name in summaries
            }
        ),
    )


def stdlib_module_intrinsic_facts(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
    source: str | bytes | None = None,
    namespace_premise: PythonNamespacePremise | None = None,
) -> StdlibModuleIntrinsicFacts:
    tree = _stdlib_module_source_tree(
        module_name, path, target_python=target_python, source=source
    )
    use = _intrinsic_use_from_tree(tree)
    evidence = _stdlib_module_import_evidence_from_tree(
        module_name,
        path,
        tree,
        target_python=target_python,
        namespace_premise=namespace_premise,
    )
    namespace_summary = evidence.source_scan.namespace_summary
    assert namespace_summary is not None, "full-module scan requires namespace syntax"
    return StdlibModuleIntrinsicFacts(
        _stdlib_module_intrinsic_status_from_tree(tree, path.name, use),
        evidence,
        use,
        namespace_summary,
    )


def _stdlib_module_import_evidence_from_tree(
    module_name: str,
    path: Path,
    tree: ast.Module,
    *,
    target_python: TargetPythonVersion,
    namespace_premise: PythonNamespacePremise | None = None,
) -> StdlibModuleImportEvidence:
    admission = _PythonAstDigestAdmission(tree)
    projection = _collect_imports_for_graph(
        tree,
        module_name,
        path.name == "__init__.py",
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=admission,
    )
    scan = _collect_import_scan_requests(
        projection,
        tree,
        source_path=path,
        module_name=module_name,
        import_scan_mode="full",
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=admission,
    )
    facade_imports = _pure_facade_imports(tree)
    facade_keys = {python_node_source_key(node) for node in facade_imports or ()}
    facade_bindings: list[StdlibFacadeBinding] = []
    private_imports: set[tuple[str, str]] = set()
    # Export bindings and owner plans come from the canonical source producer.
    for site in scan.source_sites:
        if not site.bindings:
            continue
        owner_plan = site.owner_plan
        owner = (
            normalize_stdlib_module_name(owner_plan.modules[0]) or owner_plan.modules[0]
            if len(owner_plan.modules) == 1
            and not owner_plan.errors
            and not owner_plan.requires_runtime
            and not owner_plan.requires_runtime_execution
            else None
        )
        if owner is not None:
            private_imports.update(
                (owner, name)
                for _export, name in site.bindings
                if name.startswith("_") and name != "*"
            )
        if site.source in facade_keys:
            facade_bindings.extend(
                StdlibFacadeBinding(export, owner, name, site.line)
                for export, name in site.bindings
            )
    return StdlibModuleImportEvidence(
        path,
        scan,
        StdlibFacadeEvidence(tuple(facade_bindings))
        if facade_imports is not None
        else None,
        frozenset(private_imports),
    )


def stdlib_module_static_imports(
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
) -> frozenset[str]:
    evidence = stdlib_module_import_evidence(
        module_name, path, target_python=target_python
    )
    for site in evidence.sites:
        if site.unresolved:
            require_static_import_modules(
                site.plan,
                consumer=(
                    f"stdlib intrinsic support graph ({module_name}: "
                    f"{path}:{site.line}, "
                    f"{site.request.name if site.request else '<opaque import call>'!r})"
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
    facts: Mapping[str, StdlibModuleIntrinsicFacts],
) -> StdlibIntrinsicClassification:
    closed = {name: fact.status for name, fact in facts.items()}
    evidence_by_module = {name: fact.import_evidence for name, fact in facts.items()}
    # A private binding its own module never loads is still read when another
    # module imports it by name (asyncio submodules import the package's
    # intrinsic bindings; collections imports _collections._count_elements).
    imported_private: dict[str, set[str]] = {}
    for evidence in evidence_by_module.values():
        for owner, name in evidence.private_imports:
            imported_private.setdefault(owner, set()).add(name)
    used_intrinsics: dict[str, frozenset[str]] = {}
    unused_bindings: dict[str, tuple[StdlibIntrinsicBinding, ...]] = {}
    discarded: dict[str, tuple[tuple[str, int], ...]] = {}
    for module_name, fact in facts.items():
        use = fact.intrinsic_use
        imported = imported_private.get(module_name, set())
        used = set(use.used)
        unused: list[StdlibIntrinsicBinding] = []
        for binding in use.unread_bindings:
            if binding.name in imported:
                used.add(binding.intrinsic)
            else:
                unused.append(binding)
        used_intrinsics[module_name] = frozenset(used)
        if unused:
            unused_bindings[module_name] = tuple(unused)
        if use.discarded:
            discarded[module_name] = use.discarded
        if used and closed[module_name] == STATUS_PYTHON_COMPILED:
            closed[module_name] = STATUS_INTRINSIC
    for module_name, evidence in evidence_by_module.items():
        facade = evidence.facade
        if facade is None:
            continue
        bindings: list[StdlibFacadeBinding] = []
        for binding in facade.bindings:
            candidate = (
                f"{binding.owner_module}.{binding.imported_name}"
                if binding.owner_module is not None
                else None
            )
            bindings.append(
                replace(
                    binding,
                    imported_module=candidate if candidate in module_graph else None,
                )
            )
        # A real child can be the forwarded value (including when an earlier
        # import replaced the parent's attribute). Require both the explicit
        # base and that child, never an invented owner.Symbol graph entry. Use
        # the full graph so non-Python children without status fail closed too.
        evidence_by_module[module_name] = replace(
            evidence, facade=StdlibFacadeEvidence(tuple(bindings))
        )
    # Each promotion and dependency edge is processed once. Facades wait for
    # every owner; private support needs an independently anchored family edge.
    facade_waiters: dict[str, list[str]] = {}
    remaining: dict[str, int] = {}
    private_children: dict[str, set[str]] = {}
    for name, evidence in evidence_by_module.items():
        if closed.get(name) != STATUS_PYTHON_COMPILED:
            continue
        facade = evidence.facade
        if facade is not None:
            if facade.resolved and all(
                owner in evidence_by_module for owner in facade.owners
            ):
                remaining[name] = len(facade.owners)
                for owner in facade.owners:
                    facade_waiters.setdefault(owner, []).append(name)
    for owner, evidence in evidence_by_module.items():
        for imported in evidence.proven_modules:
            child = normalize_stdlib_module_name(imported) or imported
            if (
                child in evidence_by_module
                and evidence_by_module[child].facade is None
                and _is_private_support_module(child)
                and _module_family_matches(owner, child)
            ):
                private_children.setdefault(owner, set()).add(child)
    ready = deque(
        name for name, status in closed.items() if _is_intrinsic_status(status)
    )
    while ready:
        owner = ready.popleft()
        for child in facade_waiters.get(owner, ()):
            remaining[child] -= 1
            if remaining[child] == 0 and closed[child] == STATUS_PYTHON_COMPILED:
                closed[child] = STATUS_INTRINSIC_SUPPORT
                ready.append(child)
        for child in private_children.get(owner, ()):
            if closed[child] == STATUS_PYTHON_COMPILED:
                closed[child] = STATUS_INTRINSIC_SUPPORT
                ready.append(child)
    return StdlibIntrinsicClassification(
        MappingProxyType(closed),
        MappingProxyType(evidence_by_module),
        MappingProxyType(used_intrinsics),
        MappingProxyType(unused_bindings),
        MappingProxyType(discarded),
    )


def classify_stdlib_module_statuses(
    module_graph: Mapping[str, Path],
    *,
    target_python: TargetPythonVersion,
    facts_provider: Callable[
        [str, Path, PythonNamespacePremise], StdlibModuleIntrinsicFacts
    ]
    | None = None,
    namespace_inventory: StdlibNamespaceInventory | None = None,
) -> StdlibIntrinsicClassification:
    inventory = namespace_inventory or stdlib_namespace_inventory(
        module_graph, target_python=target_python
    )
    if inventory.module_graph != module_graph:
        raise ValueError(
            "namespace inventory does not match the classified module graph"
        )
    facts = {
        module_name: (
            facts_provider(module_name, path, inventory.premises[module_name])
            if facts_provider is not None
            else stdlib_module_intrinsic_facts(
                module_name,
                path,
                target_python=target_python,
                namespace_premise=inventory.premises[module_name],
            )
        )
        for module_name, path in module_graph.items()
        if path and path.suffix == ".py"
    }
    for name, fact in facts.items():
        if fact.namespace_summary != inventory.summaries[name]:
            raise UnresolvedStaticImportError(
                f"stdlib namespace changed during classification: {name}: {module_graph[name]}"
            )
    from molt.cli.module_import_scanner import _complete_import_scan
    from molt.cli.module_resolution import _ModuleResolutionCache

    resolution = _ModuleResolutionCache()
    requests = {name: fact.import_evidence.source_scan for name, fact in facts.items()}
    for name, fact in facts.items():
        scan = _complete_import_scan(
            requests[name],
            source_path=module_graph[name],
            roots=None,
            stdlib_root=None,
            stdlib_allowlist=None,
            resolution_cache=resolution,
            target_python=target_python,
            module_graph=module_graph,
            source_requests=requests,
        )
        sites = tuple(
            StdlibImportSite(site.line, site.request, site.plan, site.owner_plan)
            for site in scan.source_sites
        )
        facts[name] = replace(
            fact,
            import_evidence=replace(
                fact.import_evidence,
                completed_sites=sites,
                completed_imports=frozenset(
                    module
                    for site in sites
                    if not site.unresolved
                    for module in site.plan.modules
                ),
            ),
        )
    return _closed_intrinsic_statuses(module_graph, facts)
