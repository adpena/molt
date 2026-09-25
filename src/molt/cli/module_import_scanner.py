from __future__ import annotations

import ast
import ntpath
import os
import posixpath
from collections.abc import Callable, Collection, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal, cast

from molt.cli import module_resolution as _module_resolution
from molt.cli.models import (
    ImportScanMode,
    _ImportDiscoveryProjection,
    _ImportScanRequests,
    _CompleteImportScan,
    _StaticSourceExecutionRequest,
    _StaticSourcePath,
    _RuntimeImportScanCustody,
    _ModuleGraphScanAuthority,
    _RuntimeImportSupportPolicy,
)
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    static_if_live_branch,
    static_test_truthiness,
)
from molt.compiler_analysis.python_binding_facts import (
    PythonIdentity,
    PythonParameterRef,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingFlowPolicy,
    PythonBindingPolicy,
    analyze_python_binding_facts,
    analyze_python_bindings,
)
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    _PythonAstDigestAdmission,
    analyze_module_import_flow,
    bind_static_import_call_arguments,
    dunder_globals_state_from_expression,
    metadata_value_from_expression,
    plan_static_import_request,
    require_static_import_modules,
    resolve_relative_import,
    static_import_candidates,
    UnresolvedStaticImportError,
)


# Runtime helper bodies whose imports are required static graph edges. This is
# intentionally qualname-based: stdlib modules stay module-init scanned unless a
# specific helper body is part of Molt's compiled runtime contract.
STDLIB_STATIC_IMPORT_HELPER_QUALNAMES: Mapping[str, frozenset[str]] = {
    "collections": frozenset({"UserDict.copy"}),
    # EmailMessage inherits MIMEPart.__init__, which supplies email.policy.default.
    "email.message": frozenset({"MIMEPart.__init__"}),
}
STDLIB_STATIC_IMPORT_HELPER_MODULES = frozenset(STDLIB_STATIC_IMPORT_HELPER_QUALNAMES)

_IMPORT_SCAN_MODES = frozenset({"full", "module_init", "module_init_static_helpers"})


def _module_import_scan_mode(
    module_name: str,
    *,
    full_scan: bool,
    static_import_helper_modules: Collection[str] | None = None,
) -> ImportScanMode:
    """Project scan authority, independent of graph seeding or import admission."""
    if full_scan:
        return "full"
    helpers = (
        STDLIB_STATIC_IMPORT_HELPER_MODULES
        if static_import_helper_modules is None
        else static_import_helper_modules
    )
    if module_name in helpers:
        return "module_init_static_helpers"
    return "module_init"


IMPORTER_MODULE_NAME = "_molt_importer"

_DYNAMIC_RELATIVE_ANCHOR_ERRORS = frozenset(
    {"unknown_package", "unknown_spec", "unknown_name"}
)


@dataclass(slots=True)
class _DynamicRelativeImportDiscovery:
    """Graph candidates for imports whose runtime package anchor is dynamic."""

    candidates: list[str] = field(default_factory=list)
    seen: set[str] = field(default_factory=set)
    required: bool = False

    @classmethod
    def from_projection(
        cls, projection: _ImportDiscoveryProjection
    ) -> _DynamicRelativeImportDiscovery:
        candidates = list(projection.dynamic_relative_import_candidates)
        return cls(
            candidates, set(candidates), projection.requires_runtime_package_anchor
        )

    def record(
        self,
        request: StaticImportRequest,
        contexts: Sequence[ModuleImportContext],
        *,
        lexical_request: StaticImportRequest | None = None,
    ) -> None:
        self.required = True
        if not contexts:
            return
        if lexical_request is None:
            if request.kind != "statement" or request.level <= 0:
                return
            lexical_request = request
        lexical_contexts = tuple(
            ModuleImportContext(
                context.module_name,
                context.is_package,
                spec_name=context.spec_name,
                target_python=context.target_python,
                execution_kind=context.execution_kind,
            )
            for context in contexts
        )
        lexical_plan = plan_static_import_request(lexical_request, lexical_contexts)
        if lexical_plan.requires_runtime or lexical_plan.errors:
            return
        for candidate in lexical_plan.modules:
            if candidate not in self.seen:
                self.seen.add(candidate)
                self.candidates.append(candidate)

    def projection(self, imports: Collection[str]) -> _ImportDiscoveryProjection:
        return _ImportDiscoveryProjection(
            tuple(imports), tuple(self.candidates), self.required
        )


def _sealed_import_modules(
    request: StaticImportRequest,
    contexts: Sequence[ModuleImportContext],
    *,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    source_path: Path | None = None,
    source_ast_digest: str | None = None,
    dynamic_relative_import_discovery: _DynamicRelativeImportDiscovery | None = None,
    lexical_discovery_request: StaticImportRequest | None = None,
) -> tuple[str, ...]:
    module_name = contexts[0].module_name if contexts else None
    plan = plan_static_import_request(request, contexts)
    unclassified_errors = tuple(
        error for error in plan.errors if error not in _DYNAMIC_RELATIVE_ANCHOR_ERRORS
    )
    if plan.requires_runtime and unclassified_errors:
        raise UnresolvedStaticImportError(
            "module import scanner "
            f"({module_name or '<script>'}: {request.name!r}) cannot resolve import: "
            + ", ".join(unclassified_errors)
        )
    if plan.requires_runtime and dynamic_relative_import_discovery is not None:
        dynamic_relative_import_discovery.record(
            request,
            contexts,
            lexical_request=lexical_discovery_request,
        )
    if (
        plan.requires_runtime
        and runtime_import_custody is not None
        and runtime_import_custody.admits_scan(
            module_name, source_path, source_ast_digest
        )
    ):
        # Do not replace poisoned metadata with the lexical package. The runtime
        # keeps Python's relative-import semantics and may select any retained
        # catalog row (or fail closed if the requested module is not admitted).
        return tuple(dict.fromkeys((*plan.modules, *runtime_import_custody.modules)))
    if plan.requires_runtime and dynamic_relative_import_discovery is not None:
        # Graph discovery retains only statically proven semantic alternatives
        # here. Lexical owner candidates travel in a distinct projection and
        # cannot become a runtime relative-import fallback.
        return plan.modules
    return require_static_import_modules(
        plan,
        consumer=f"module import scanner ({module_name or '<script>'}: {request.name!r})",
    )


_RUNTIME_IMPORT_PROTOCOL_MARKERS = (
    "import ",
    "from ",
    "__import__",
    "import_module",
    "find_spec",
)


_RUNTIME_IMPORT_PROTOCOL_TARGETS = frozenset(
    {
        "__import__",
        "builtins.__import__",
        "importlib.import_module",
        "importlib.util.find_spec",
    }
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


@dataclass(frozen=True, slots=True)
class _StaticImportCallPayload:
    target: str
    name: ast.expr
    package: ast.expr | None = None
    globals: ast.expr | None = None
    fromlist: ast.expr | None = None
    level: ast.expr | None = None


_STATIC_SOURCE_LOADER_TARGETS = frozenset(
    {
        "importlib.util.spec_from_file_location",
        "importlib.machinery.SourceFileLoader",
        "importlib.machinery.SourcelessFileLoader",
    }
)

_STATIC_SOURCE_EXECUTION_MARKERS = (
    "spec_from_file_location",
    "SourceFileLoader",
    "SourcelessFileLoader",
    "run_path",
)

_STATIC_SOURCE_PATH_JOIN_OPERATIONS: Mapping[
    str, Literal["os_join", "posix_join", "nt_join"]
] = {
    "os.path.join": "os_join",
    "posixpath.join": "posix_join",
    "ntpath.join": "nt_join",
}


def _source_may_use_static_source_execution(source: str) -> bool:
    return any(marker in source for marker in _STATIC_SOURCE_EXECUTION_MARKERS)


def _collect_static_source_execution_requests(
    tree: ast.AST,
    *,
    source_path: Path,
    import_scan_mode: ImportScanMode = "full",
    module_name: str | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[_StaticSourceExecutionRequest, ...]:
    """Collect statically addressable loader/runpy source execution roots.

    This is the source-path projection of the import scanner.  It deliberately
    accepts only expressions that can be evaluated without executing user code;
    dynamic paths remain runtime capability work and never become build inputs
    by accident.
    """

    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    import_flow = analyze_module_import_flow(
        tree,
        ModuleImportContext(
            module_name,
            source_path.name == "__init__.py",
            target_python=target_python.feature_version,
        ),
        ast_digest_admission=ast_digest_admission,
    )
    aliases: dict[str, str] = {
        "importlib": "importlib",
        "runpy": "runpy",
        "Path": "pathlib.Path",
    }
    constants: dict[str, str | _StaticSourcePath] = {}

    if isinstance(tree, ast.Module):
        for stmt in tree.body:
            if not import_flow.states_for(stmt):
                continue
            if isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    bound = alias.asname or alias.name.split(".", 1)[0]
                    aliases[bound] = alias.name if alias.asname else bound
            elif isinstance(stmt, ast.ImportFrom) and stmt.level == 0 and stmt.module:
                for alias in stmt.names:
                    aliases[alias.asname or alias.name] = f"{stmt.module}.{alias.name}"

    def qualified_name(expr: ast.expr) -> str | None:
        if isinstance(expr, ast.Name):
            return aliases.get(expr.id, expr.id)
        if isinstance(expr, ast.Attribute):
            base = qualified_name(expr.value)
            return None if base is None else f"{base}.{expr.attr}"
        return None

    def static_value(expr: ast.expr) -> str | _StaticSourcePath | None:
        if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
            return expr.value
        if isinstance(expr, ast.Name):
            return constants.get(expr.id)
        if isinstance(expr, ast.BinOp):
            left = static_value(expr.left)
            right = static_value(expr.right)
            if (
                isinstance(expr.op, ast.Add)
                and isinstance(left, str)
                and isinstance(right, str)
            ):
                return left + right
            if (
                isinstance(expr.op, ast.Div)
                and isinstance(left, (str, _StaticSourcePath))
                and isinstance(right, (str, _StaticSourcePath))
            ):
                return _StaticSourcePath("join", (left, right))
            return None
        if isinstance(expr, ast.Call):
            target = qualified_name(expr.func)
            if target == "pathlib.Path" and len(expr.args) == 1 and not expr.keywords:
                value = static_value(expr.args[0])
                return (
                    _StaticSourcePath("path", (value,)) if value is not None else None
                )
            if (
                target is not None
                and target in _STATIC_SOURCE_PATH_JOIN_OPERATIONS
                and expr.args
            ):
                parts = [static_value(arg) for arg in expr.args]
                if all(part is not None for part in parts):
                    return _StaticSourcePath(
                        _STATIC_SOURCE_PATH_JOIN_OPERATIONS[target],
                        tuple(part for part in parts if part is not None),
                    )
            if (
                isinstance(expr.func, ast.Attribute)
                and expr.func.attr in {"resolve", "absolute"}
                and not expr.args
                and not expr.keywords
            ):
                value = static_value(expr.func.value)
                if value is not None:
                    return _StaticSourcePath(
                        "resolve" if expr.func.attr == "resolve" else "absolute",
                        (value,),
                    )
        return None

    # Module constants are the common authority for loader paths and remain
    # visible to calls nested in entry-module functions.
    if isinstance(tree, ast.Module):
        for stmt in tree.body:
            if not import_flow.states_for(stmt):
                continue
            assignment: tuple[ast.expr, ast.expr] | None = None
            if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
                assignment = stmt.targets[0], stmt.value
            elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
                assignment = stmt.target, stmt.value
            if assignment is None or not isinstance(assignment[0], ast.Name):
                continue
            value = static_value(assignment[1])
            if value is not None:
                constants[assignment[0].id] = value

    def call_argument(call: ast.Call, position: int, keyword: str) -> ast.expr | None:
        if position < len(call.args):
            return call.args[position]
        return next((item.value for item in call.keywords if item.arg == keyword), None)

    requests: list[_StaticSourceExecutionRequest] = []
    seen: set[_StaticSourceExecutionRequest] = set()
    for node in _scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        module_name=module_name,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    ):
        if not isinstance(node, ast.Call):
            continue
        if not import_flow.states_for(node):
            continue
        target = qualified_name(node.func)
        request_name: str | None
        path_expr: ast.expr | None
        if target in _STATIC_SOURCE_LOADER_TARGETS:
            name_expr = call_argument(node, 0, "name")
            path_expr = call_argument(node, 1, "location")
            if target != "importlib.util.spec_from_file_location":
                path_expr = call_argument(node, 1, "path")
            name_value = static_value(name_expr) if name_expr is not None else None
            if not isinstance(name_value, str):
                continue
            request_name = name_value
        elif target == "runpy.run_path":
            request_name = None
            path_expr = call_argument(node, 0, "path_name")
        else:
            continue
        path_value = static_value(path_expr) if path_expr is not None else None
        if path_value is None:
            continue
        request = _StaticSourceExecutionRequest(request_name, path_value)
        if request not in seen:
            seen.add(request)
            requests.append(request)
    return tuple(requests)


def _validate_import_scan_mode(import_scan_mode: ImportScanMode) -> None:
    if import_scan_mode not in _IMPORT_SCAN_MODES:
        raise ValueError(f"unknown import scan mode: {import_scan_mode}")


def _static_import_helper_qualnames(
    module_name: str | None, import_scan_mode: ImportScanMode
) -> frozenset[str]:
    _validate_import_scan_mode(import_scan_mode)
    if import_scan_mode != "module_init_static_helpers":
        return frozenset()
    if module_name is None:
        raise ValueError("module_init_static_helpers requires module_name")
    helper_qualnames = STDLIB_STATIC_IMPORT_HELPER_QUALNAMES.get(module_name)
    if helper_qualnames is None:
        raise ValueError(
            f"module_init_static_helpers has no helper policy for {module_name}"
        )
    return helper_qualnames


def _qualified_child(prefix: tuple[str, ...], name: str) -> tuple[str, ...]:
    return (*prefix, name)


def _statically_executed_boolop_values(
    node: ast.BoolOp,
    *,
    fact_result: Callable[[ast.expr], StaticExpressionResult | None],
) -> tuple[ast.expr, ...]:
    values: list[ast.expr] = []
    if isinstance(node.op, ast.And):
        for idx, value in enumerate(node.values):
            values.append(value)
            value_truth = static_test_truthiness(
                value,
                fact_result=fact_result,
            )
            if value_truth is False:
                return tuple(values)
            if value_truth is None:
                values.extend(node.values[idx + 1 :])
                return tuple(values)
        return tuple(values)
    if isinstance(node.op, ast.Or):
        for idx, value in enumerate(node.values):
            values.append(value)
            value_truth = static_test_truthiness(
                value,
                fact_result=fact_result,
            )
            if value_truth is True:
                return tuple(values)
            if value_truth is None:
                values.extend(node.values[idx + 1 :])
                return tuple(values)
        return tuple(values)
    return tuple(node.values)


def _function_parameter_names_from_args(args: ast.arguments) -> list[str]:
    names = [arg.arg for arg in args.posonlyargs]
    names.extend(arg.arg for arg in args.args)
    names.extend(arg.arg for arg in args.kwonlyargs)
    if args.vararg is not None:
        names.append(args.vararg.arg)
    if args.kwarg is not None:
        names.append(args.kwarg.arg)
    return names


def _static_scan_nodes(
    tree: ast.AST,
    *,
    include_function_bodies: bool,
    included_function_qualnames: Collection[str] = frozenset(),
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    if not isinstance(tree, ast.Module):
        return tuple(ast.walk(tree))
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    binding_index = analyze_python_binding_facts(
        tree,
        source_digest=ast_digest_admission.digest,
        policy=PythonBindingFlowPolicy(target_python=target_python.feature_version),
    )
    nodes: list[ast.AST] = []
    included_qualnames = frozenset(included_function_qualnames)

    def visit(
        node: ast.AST,
        qualname_prefix: tuple[str, ...] = (),
    ) -> None:
        nodes.append(node)
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            return
        if isinstance(node, ast.Assign):
            visit(node.value, qualname_prefix)
            for target in node.targets:
                visit(target, qualname_prefix)
            return
        if isinstance(node, ast.AnnAssign):
            visit(node.annotation, qualname_prefix)
            if node.value is not None:
                visit(node.value, qualname_prefix)
            visit(node.target, qualname_prefix)
            return
        if isinstance(node, ast.AugAssign):
            visit(node.target, qualname_prefix)
            visit(node.value, qualname_prefix)
            return
        if isinstance(node, ast.Delete):
            for target in node.targets:
                visit(target, qualname_prefix)
            return
        if isinstance(node, ast.NamedExpr):
            visit(node.value, qualname_prefix)
            visit(node.target, qualname_prefix)
            return
        if isinstance(node, ast.BoolOp):
            for value in _statically_executed_boolop_values(
                node,
                fact_result=binding_index.expression_result,
            ):
                visit(value, qualname_prefix)
            return
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            function_qualname = ".".join(_qualified_child(qualname_prefix, node.name))
            for decorator in node.decorator_list:
                visit(decorator, qualname_prefix)
            for default in list(node.args.defaults) + [
                default for default in node.args.kw_defaults if default is not None
            ]:
                visit(default, qualname_prefix)
            for arg in (
                list(node.args.posonlyargs)
                + list(node.args.args)
                + list(node.args.kwonlyargs)
            ):
                if arg.annotation is not None:
                    visit(arg.annotation, qualname_prefix)
            if node.args.vararg is not None and node.args.vararg.annotation is not None:
                visit(node.args.vararg.annotation, qualname_prefix)
            if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
                visit(node.args.kwarg.annotation, qualname_prefix)
            if node.returns is not None:
                visit(node.returns, qualname_prefix)
            for type_param in getattr(node, "type_params", ()):
                visit(type_param, qualname_prefix)
            if include_function_bodies or function_qualname in included_qualnames:
                function_prefix = _qualified_child(qualname_prefix, node.name)
                for stmt in node.body:
                    visit(stmt, function_prefix)
            return
        if isinstance(node, ast.Lambda):
            for default in list(node.args.defaults) + [
                default for default in node.args.kw_defaults if default is not None
            ]:
                visit(default, qualname_prefix)
            if include_function_bodies:
                visit(node.body, qualname_prefix)
            return
        if isinstance(node, ast.ClassDef):
            for decorator in node.decorator_list:
                visit(decorator, qualname_prefix)
            for base in node.bases:
                visit(base, qualname_prefix)
            for keyword in node.keywords:
                if keyword.value is not None:
                    visit(keyword.value, qualname_prefix)
            for type_param in getattr(node, "type_params", ()):
                visit(type_param, qualname_prefix)
            class_prefix = _qualified_child(qualname_prefix, node.name)
            for stmt in node.body:
                visit(stmt, class_prefix)
            return
        if isinstance(node, ast.If):
            visit(node.test, qualname_prefix)
            static_branch = static_if_live_branch(
                node,
                fact_result=binding_index.expression_result,
            )
            if static_branch is not None:
                for stmt in static_branch:
                    visit(stmt, qualname_prefix)
            else:
                for stmt in node.body:
                    visit(stmt, qualname_prefix)
                for stmt in node.orelse:
                    visit(stmt, qualname_prefix)
            return
        for child in ast.iter_child_nodes(node):
            visit(child, qualname_prefix)

    for stmt in tree.body:
        visit(stmt)
    return tuple(nodes)


def _module_init_scan_nodes(
    tree: ast.AST,
    *,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(
        tree,
        include_function_bodies=False,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )


def _module_init_static_helper_scan_nodes(
    tree: ast.AST,
    module_name: str | None,
    *,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(
        tree,
        include_function_bodies=False,
        included_function_qualnames=_static_import_helper_qualnames(
            module_name, "module_init_static_helpers"
        ),
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )


def _full_static_scan_nodes(
    tree: ast.AST,
    *,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(
        tree,
        include_function_bodies=True,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )


def _scan_nodes_for_import_mode(
    tree: ast.AST,
    import_scan_mode: ImportScanMode,
    *,
    module_name: str | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    _validate_import_scan_mode(import_scan_mode)
    if import_scan_mode == "full":
        return _full_static_scan_nodes(
            tree,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
        )
    if import_scan_mode == "module_init_static_helpers":
        return _module_init_static_helper_scan_nodes(
            tree,
            module_name,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
        )
    return _module_init_scan_nodes(
        tree,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )


def _collect_imports(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    source_path: Path | None = None,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    _dynamic_relative_import_discovery: _DynamicRelativeImportDiscovery | None = None,
) -> list[str]:
    if runtime_import_custody is not None:
        runtime_import_custody.validate_scan_mode(
            module_name, source_path, import_scan_mode
        )
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    _validate_import_scan_mode(import_scan_mode)
    selected_static_helper_qualnames = _static_import_helper_qualnames(
        module_name, import_scan_mode
    )
    imports: list[str] = []
    needs_typing = False
    needs_string_templatelib = False
    type_alias_cls = getattr(ast, "TypeAlias", None)
    template_str_cls = getattr(ast, "TemplateStr", None)
    helper_string_functions: dict[str, tuple[list[str], ast.expr]] = {}
    helper_import_calls: dict[
        str,
        tuple[list[str], set[str], list[_StaticImportCallPayload]],
    ] = {}
    base_import_context = ModuleImportContext(
        module_name,
        is_package,
        spec_name=module_name,
        target_python=target_python.feature_version,
        execution_kind="script" if module_name is None else "imported",
    )
    binding_index = analyze_python_bindings(
        cast(ast.Module, tree),
        source_digest=ast_digest_admission.digest,
        policy=PythonBindingPolicy(
            target_python=target_python.feature_version,
            module_name=module_name,
            module_spec_name=module_name,
            module_is_package=is_package,
            module_execution_kind="script" if module_name is None else "imported",
        ),
    )
    import_flow = binding_index.module_import_flow

    def _import_contexts(node: ast.AST) -> tuple[ModuleImportContext, ...]:
        return tuple(
            base_import_context.with_state(state)
            for state in import_flow.states_for(node)
        )

    module_body = list(getattr(tree, "body", []))
    function_walks: list[
        tuple[ast.FunctionDef | ast.AsyncFunctionDef, tuple[ast.AST, ...]]
    ] = []

    def _static_call_target(
        call: ast.Call, *, allow_possible: bool = False
    ) -> str | None:
        fact = binding_index.call_fact(call)
        if fact is not None:
            exact_kind = fact.exact_import_call_kind()
            if exact_kind == "dunder_import":
                return "builtins.__import__"
            if exact_kind == "import_module":
                return "importlib.import_module"
            if fact.callee_is(PythonIdentity.IMPORTLIB_FIND_SPEC):
                return "importlib.util.find_spec"
            if allow_possible:
                possible_kinds = fact.possible_import_call_kinds()
                if "dunder_import" in possible_kinds:
                    return "builtins.__import__"
                if "import_module" in possible_kinds:
                    return "importlib.import_module"
                if fact.callee_may_be(PythonIdentity.IMPORTLIB_FIND_SPEC):
                    return "importlib.util.find_spec"
        return call.func.id if isinstance(call.func, ast.Name) else None

    def _is_static_import_target(target: str | None) -> bool:
        return target in {
            "builtins.__import__",
            "importlib.import_module",
            "importlib.util.find_spec",
            "_MOLT_IMPORTLIB_IMPORT_TRANSACTION",
            "molt_importlib_import_transaction",
        }

    def _bound_static_value(
        node: ast.expr,
        bindings: Mapping[str, object],
    ) -> object | None:
        value = binding_index.static_value(node)
        if isinstance(value, PythonParameterRef):
            return bindings.get(value.name)
        return value

    def _resolve_string_sequence(
        node: ast.expr, bindings: dict[str, object], seen: set[str]
    ) -> list[str] | None:
        value = _bound_static_value(node, bindings)
        if isinstance(value, tuple) and all(isinstance(item, str) for item in value):
            return [cast(str, item) for item in value]
        if isinstance(value, list) and all(isinstance(item, str) for item in value):
            return list(cast(list[str], value))
        return None

    def _resolve_string_constant(
        node: ast.expr,
        bindings: dict[str, object] | None = None,
        seen: set[str] | None = None,
    ) -> str | None:
        bindings = bindings or {}
        seen = seen or set()
        value = _bound_static_value(node, bindings)
        if isinstance(value, str):
            return value
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = _resolve_string_constant(node.left, bindings, seen)
            right = _resolve_string_constant(node.right, bindings, seen)
            if left is not None and right is not None:
                return left + right
            return None
        if isinstance(node, ast.Call):
            target = _static_call_target(node)
            if (
                target
                in {
                    "_MOLT_IMPORTLIB_RESOLVE_NAME",
                    "molt_importlib_resolve_name",
                }
                and node.args
            ):
                resolved = _resolve_string_constant(node.args[0], bindings, seen)
                if resolved is None:
                    return None
                if not resolved.startswith("."):
                    return resolved
                if len(node.args) < 2:
                    return None
                package = _resolve_string_constant(node.args[1], bindings, seen)
                if package is None:
                    return None
                level = len(resolved) - len(resolved.lstrip("."))
                module = resolved[level:] or None
                return resolve_relative_import(
                    module,
                    level,
                    ModuleImportContext(module_name=package, is_package=True),
                ).module
            if (
                isinstance(node.func, ast.Attribute)
                and node.func.attr == "join"
                and len(node.args) == 1
            ):
                sep = _resolve_string_constant(node.func.value, bindings, seen)
                if sep is None:
                    return None
                items = _resolve_string_sequence(node.args[0], bindings, seen)
                if items is None:
                    return None
                return sep.join(items)
            if isinstance(node.func, ast.Name):
                func_name = node.func.id
                if func_name in seen:
                    return None
                helper = helper_string_functions.get(func_name)
                if helper is None:
                    return None
                params, expr = helper
                if len(node.args) != len(params) or node.keywords:
                    return None
                child_bindings: dict[str, object] = dict(bindings)
                for param, arg in zip(params, node.args):
                    scalar = _resolve_string_constant(arg, bindings, seen)
                    if scalar is not None:
                        child_bindings[param] = scalar
                        continue
                    seq = _resolve_string_sequence(arg, bindings, seen)
                    if seq is not None:
                        child_bindings[param] = seq
                        continue
                    return None
                return _resolve_string_constant(
                    expr, child_bindings, seen | {func_name}
                )
        return None

    def _function_required_param_names(
        stmt: ast.FunctionDef | ast.AsyncFunctionDef, params: list[str]
    ) -> set[str]:
        positional = list(stmt.args.posonlyargs) + list(stmt.args.args)
        required_positional_count = max(0, len(positional) - len(stmt.args.defaults))
        required = {arg.arg for arg in positional[:required_positional_count]}
        for arg, default in zip(stmt.args.kwonlyargs, stmt.args.kw_defaults):
            if default is None:
                required.add(arg.arg)
        return required.intersection(params)

    def _simple_function_local_expr_bindings(
        stmt: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> dict[str, ast.expr]:
        values: dict[str, ast.expr] = {}
        repeated: set[str] = set()
        for node in ast.walk(stmt):
            assignment: tuple[ast.expr, ast.expr] | None = None
            if isinstance(node, ast.Assign) and len(node.targets) == 1:
                assignment = (node.targets[0], node.value)
            elif isinstance(node, ast.AnnAssign):
                if node.value is not None:
                    assignment = (node.target, node.value)
            if assignment is None:
                continue
            target, value = assignment
            if not isinstance(target, ast.Name):
                continue
            if target.id in values:
                repeated.add(target.id)
                continue
            values[target.id] = value
        for name in repeated:
            values.pop(name, None)
        return values

    def _resolve_local_expr_binding(
        expr: ast.expr, local_expr_bindings: dict[str, ast.expr]
    ) -> ast.expr:
        seen: set[str] = set()
        current = expr
        while isinstance(current, ast.Name) and current.id in local_expr_bindings:
            if current.id in seen:
                return expr
            seen.add(current.id)
            current = local_expr_bindings[current.id]
        return current

    def _resolve_int_constant(
        node: ast.expr | None, bindings: Mapping[str, object]
    ) -> int | None:
        if node is None:
            return None
        value = _bound_static_value(node, bindings)
        return value if isinstance(value, int) and not isinstance(value, bool) else None

    def _static_import_call_payload(
        call: ast.Call,
        target: str,
        *,
        local_expr_bindings: Mapping[str, ast.expr] | None = None,
    ) -> _StaticImportCallPayload | None:
        operation_kind = (
            "import_module"
            if target in {"importlib.import_module", "importlib.util.find_spec"}
            else "dunder_import"
        )
        arguments = bind_static_import_call_arguments(call, operation_kind)

        def resolve_local(expr: ast.expr | None) -> ast.expr | None:
            if expr is None or local_expr_bindings is None:
                return expr
            return _resolve_local_expr_binding(expr, dict(local_expr_bindings))

        name_expr = cast(ast.expr, resolve_local(arguments.name))
        if target in {"importlib.import_module", "importlib.util.find_spec"}:
            return _StaticImportCallPayload(
                target=target,
                name=name_expr,
                package=resolve_local(arguments.package),
            )
        return _StaticImportCallPayload(
            target=target,
            name=name_expr,
            globals=resolve_local(arguments.globals),
            fromlist=resolve_local(arguments.fromlist),
            level=resolve_local(arguments.level),
        )

    def _resolve_static_import_call(
        payload: _StaticImportCallPayload,
        call: ast.Call,
        bindings: dict[str, object] | None = None,
    ) -> tuple[str, ...]:
        bindings = bindings or {}
        name = _resolve_string_constant(payload.name, bindings, set())
        if name is None:
            return ()

        def resolve_string(expression: ast.expr) -> str | None:
            return _resolve_string_constant(expression, bindings, set())

        contexts = _import_contexts(call)
        modules: list[str] = []
        seen: set[str] = set()
        for context in contexts:
            lexical_context = ModuleImportContext(
                context.module_name,
                context.is_package,
                spec_name=context.spec_name,
                target_python=context.target_python,
                execution_kind=context.execution_kind,
            )
            if payload.target in {
                "importlib.import_module",
                "importlib.util.find_spec",
            }:
                request = StaticImportRequest.import_module(
                    name,
                    metadata_value_from_expression(
                        payload.package, context, resolve_string
                    ),
                )
                lexical_request = StaticImportRequest.import_module(
                    name,
                    metadata_value_from_expression(
                        payload.package, lexical_context, resolve_string
                    ),
                )
            else:
                fromlist = (
                    _resolve_string_sequence(payload.fromlist, bindings, set())
                    if payload.fromlist is not None
                    else []
                )
                level = _resolve_int_constant(payload.level, bindings)
                if fromlist is None:
                    if payload.fromlist is not None:
                        raise ValueError(
                            "non-literal __import__ fromlist requires runtime import custody"
                        )
                    fromlist = []
                if payload.level is not None and level is None:
                    raise ValueError(
                        "non-literal __import__ level requires runtime import custody"
                    )
                if "*" in fromlist:
                    if payload.target == "_MOLT_IMPORTLIB_IMPORT_TRANSACTION":
                        fromlist = []
                    else:
                        raise ValueError(
                            "dynamic __import__ star fromlist requires runtime import custody"
                        )
                request = StaticImportRequest(
                    "dunder_import",
                    name,
                    level=0 if level is None else level,
                    fromlist=tuple(fromlist),
                    globals_state=dunder_globals_state_from_expression(
                        payload.globals, context, resolve_string
                    ),
                    globals_were_supplied=payload.globals is not None,
                )
                lexical_request = StaticImportRequest(
                    "dunder_import",
                    name,
                    level=0 if level is None else level,
                    fromlist=tuple(fromlist),
                    globals_state=dunder_globals_state_from_expression(
                        payload.globals, lexical_context, resolve_string
                    ),
                    globals_were_supplied=payload.globals is not None,
                )
            for module in _sealed_import_modules(
                request,
                (context,),
                runtime_import_custody=runtime_import_custody,
                source_path=source_path,
                source_ast_digest=ast_digest_admission.digest,
                dynamic_relative_import_discovery=(_dynamic_relative_import_discovery),
                lexical_discovery_request=lexical_request,
            ):
                if module not in seen:
                    seen.add(module)
                    modules.append(module)
        return tuple(modules)

    def _bind_helper_call_arguments(
        call: ast.Call, params: list[str], required_params: set[str]
    ) -> dict[str, object] | None:
        if len(call.args) > len(params):
            return None
        bindings: dict[str, object] = {}
        for idx, arg in enumerate(call.args):
            param = params[idx]
            scalar = _resolve_string_constant(arg)
            if scalar is not None:
                bindings[param] = scalar
                continue
            integer = _resolve_int_constant(arg, {})
            if integer is not None:
                bindings[param] = integer
                continue
            seq = _resolve_string_sequence(arg, {}, set())
            if seq is not None:
                bindings[param] = seq
        for keyword in call.keywords:
            if keyword.arg is None or keyword.arg not in params:
                return None
            if keyword.arg in bindings:
                return None
            scalar = _resolve_string_constant(keyword.value)
            if scalar is not None:
                bindings[keyword.arg] = scalar
                continue
            integer = _resolve_int_constant(keyword.value, {})
            if integer is not None:
                bindings[keyword.arg] = integer
                continue
            seq = _resolve_string_sequence(keyword.value, {}, set())
            if seq is not None:
                bindings[keyword.arg] = seq
        if not required_params.issubset(bindings):
            return None
        return bindings

    module_import_helper_scan = isinstance(tree, ast.Module)

    if module_import_helper_scan:
        for stmt in module_body:
            if not import_flow.states_for(stmt):
                continue
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                stmt_nodes = tuple(ast.walk(stmt))
                function_walks.append((stmt, stmt_nodes))
                if len(stmt.body) != 1 or not isinstance(stmt.body[0], ast.Return):
                    continue
                ret_expr = stmt.body[0].value
                if ret_expr is None:
                    continue
                params = [
                    arg.arg
                    for arg in (
                        list(stmt.args.posonlyargs)
                        + list(stmt.args.args)
                        + list(stmt.args.kwonlyargs)
                    )
                ]
                if stmt.args.vararg is not None or stmt.args.kwarg is not None:
                    continue
                helper_string_functions[stmt.name] = (params, ret_expr)

        for stmt, stmt_nodes in function_walks:
            params = [
                arg.arg
                for arg in (
                    list(stmt.args.posonlyargs)
                    + list(stmt.args.args)
                    + list(stmt.args.kwonlyargs)
                )
            ]
            if stmt.args.vararg is not None:
                params.append(stmt.args.vararg.arg)
            if stmt.args.kwarg is not None:
                params.append(stmt.args.kwarg.arg)
            if not params:
                continue
            required_params = _function_required_param_names(stmt, params)
            local_expr_bindings = _simple_function_local_expr_bindings(stmt)
            for node in stmt_nodes:
                if not isinstance(node, ast.Call):
                    continue
                if not import_flow.states_for(node):
                    continue
                target = _static_call_target(node, allow_possible=True)
                if not _is_static_import_target(target):
                    continue
                assert target is not None
                payload = _static_import_call_payload(
                    node,
                    target,
                    local_expr_bindings=local_expr_bindings,
                )
                if payload is None:
                    continue
                helper_entry = helper_import_calls.get(stmt.name)
                if helper_entry is None:
                    helper_import_calls[stmt.name] = (
                        params,
                        required_params,
                        [payload],
                    )
                else:
                    helper_entry[2].append(payload)

    def _record_helper_call_imports(node: ast.Call) -> None:
        if module_import_helper_scan:
            if not isinstance(node.func, ast.Name):
                return
            helper_call_entry = helper_import_calls.get(node.func.id)
            if helper_call_entry is not None:
                params, required_params, payloads = helper_call_entry
                call_bindings = _bind_helper_call_arguments(
                    node, params, required_params
                )
                if call_bindings is not None:
                    for payload in payloads:
                        imports.extend(
                            _resolve_static_import_call(payload, node, call_bindings)
                        )

    def _record_import_statement(
        node: ast.Import | ast.ImportFrom,
    ) -> None:
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
            return
        if node.level == 0:
            imports.extend(
                static_import_candidates(
                    node.module or "",
                    tuple(alias.name for alias in node.names),
                )
            )
            return
        request = StaticImportRequest.statement(
            node.module or "",
            level=node.level,
            fromlist=tuple(alias.name for alias in node.names),
        )
        imports.extend(
            _sealed_import_modules(
                request,
                _import_contexts(node),
                runtime_import_custody=runtime_import_custody,
                source_path=source_path,
                source_ast_digest=ast_digest_admission.digest,
                dynamic_relative_import_discovery=(_dynamic_relative_import_discovery),
            )
        )

    def _collect_import_call(node: ast.Call) -> None:
        if not import_flow.states_for(node):
            return
        _record_helper_call_imports(node)
        # Graph custody includes every admitted import alternative after a
        # callback; exact callee identity is a lowering/specialization condition.
        target = _static_call_target(node, allow_possible=True)
        if not _is_static_import_target(target):
            return
        assert target is not None
        payload = _static_import_call_payload(node, target)
        if payload is not None:
            imports.extend(_resolve_static_import_call(payload, node))

    def _function_parameter_names(
        node: ast.Lambda | ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> list[str]:
        return _function_parameter_names_from_args(node.args)

    def _visit_many(
        nodes: Iterable[ast.AST],
        qualname_prefix: tuple[str, ...] = (),
    ) -> None:
        for child in nodes:
            _visit(child, qualname_prefix)

    def _visit(
        node: ast.AST,
        qualname_prefix: tuple[str, ...] = (),
    ) -> None:
        nonlocal needs_string_templatelib, needs_typing
        if isinstance(node, (ast.stmt, ast.Call)) and not import_flow.states_for(node):
            return
        if isinstance(node, ast.Module):
            _visit_many(node.body)
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            _record_import_statement(node)
            return
        if isinstance(node, ast.Assign):
            _visit(node.value, qualname_prefix)
            _visit_many(node.targets, qualname_prefix)
            return
        if isinstance(node, ast.AnnAssign):
            _visit(node.annotation, qualname_prefix)
            if node.value is not None:
                _visit(node.value, qualname_prefix)
            _visit(node.target, qualname_prefix)
            return
        if isinstance(node, ast.AugAssign):
            _visit(node.target, qualname_prefix)
            _visit(node.value, qualname_prefix)
            return
        if isinstance(node, ast.Delete):
            _visit_many(node.targets, qualname_prefix)
            return
        if isinstance(node, ast.If):
            _visit(node.test, qualname_prefix)
            static_branch = static_if_live_branch(
                node,
                fact_result=binding_index.expression_result,
            )
            if static_branch is not None:
                _visit_many(static_branch, qualname_prefix)
            else:
                _visit_many(node.body, qualname_prefix)
                _visit_many(node.orelse, qualname_prefix)
            return
        if isinstance(node, ast.NamedExpr):
            _visit(node.value, qualname_prefix)
            _visit(node.target, qualname_prefix)
            return
        if isinstance(node, ast.BoolOp):
            for value in _statically_executed_boolop_values(
                node,
                fact_result=binding_index.expression_result,
            ):
                _visit(value, qualname_prefix)
            return
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if getattr(node, "type_params", None):
                needs_typing = True
            if isinstance(node, ast.ClassDef):
                _visit_many(node.decorator_list, qualname_prefix)
                _visit_many(node.bases, qualname_prefix)
                _visit_many(
                    [keyword.value for keyword in node.keywords if keyword.value],
                    qualname_prefix,
                )
                _visit_many(
                    getattr(node, "type_params", ()),
                    qualname_prefix,
                )
                class_prefix = _qualified_child(qualname_prefix, node.name)
                _visit_many(node.body, class_prefix)
                return
            _visit_many(node.decorator_list, qualname_prefix)
            _visit_many(list(node.args.defaults), qualname_prefix)
            _visit_many(
                [default for default in node.args.kw_defaults if default is not None],
                qualname_prefix,
            )
            for arg in (
                list(node.args.posonlyargs)
                + list(node.args.args)
                + list(node.args.kwonlyargs)
            ):
                if arg.annotation is not None:
                    _visit(arg.annotation, qualname_prefix)
            if node.args.vararg is not None and node.args.vararg.annotation is not None:
                _visit(
                    node.args.vararg.annotation,
                    qualname_prefix,
                )
            if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
                _visit(
                    node.args.kwarg.annotation,
                    qualname_prefix,
                )
            if node.returns is not None:
                _visit(node.returns, qualname_prefix)
            _visit_many(
                getattr(node, "type_params", ()),
                qualname_prefix,
            )
            function_qualname = ".".join(_qualified_child(qualname_prefix, node.name))
            if (
                import_scan_mode == "full"
                or function_qualname in selected_static_helper_qualnames
            ):
                function_prefix = _qualified_child(qualname_prefix, node.name)
                _visit_many(node.body, function_prefix)
            return
        if isinstance(node, ast.Lambda):
            _visit_many(list(node.args.defaults), qualname_prefix)
            _visit_many(
                [default for default in node.args.kw_defaults if default is not None],
                qualname_prefix,
            )
            if import_scan_mode == "full":
                _visit(node.body)
            return
        if type_alias_cls is not None and isinstance(node, type_alias_cls):
            type_alias = cast(ast.TypeAlias, node)
            needs_typing = True
            if import_scan_mode == "full":
                deferred_expressions: list[ast.expr] = []
                for type_param in type_alias.type_params:
                    for attribute in ("bound", "default_value"):
                        value = getattr(type_param, attribute, None)
                        if isinstance(value, ast.expr):
                            deferred_expressions.append(value)
                deferred_expressions.append(type_alias.value)
                for expression in deferred_expressions:
                    _visit(expression, qualname_prefix)
                    for child in ast.walk(expression):
                        if not isinstance(child, ast.Call):
                            continue
                        fact = binding_index.call_fact(child)
                        if fact is not None and fact.exact_import_call_kind() is None:
                            _collect_import_call(child)
            return
        if template_str_cls is not None and isinstance(node, template_str_cls):
            # PEP 750 t-strings desugar to string.templatelib.{Template,Interpolation}
            # at the molt frontend layer, so the import must be reflected in the
            # module graph closure even though no `import` statement appears.
            needs_string_templatelib = True
            return
        if isinstance(node, ast.Call):
            _collect_import_call(node)
        for child in ast.iter_child_nodes(node):
            _visit(child, qualname_prefix)

    _visit(tree)
    if needs_typing:
        imports.append("typing")
    if needs_string_templatelib:
        imports.append("string.templatelib")
    return imports


def _collect_imports_for_graph(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    source_path: Path | None = None,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> _ImportDiscoveryProjection:
    discovery = _DynamicRelativeImportDiscovery()
    imports = _collect_imports(
        tree,
        module_name,
        is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        runtime_import_custody=runtime_import_custody,
        source_path=source_path,
        ast_digest_admission=ast_digest_admission,
        _dynamic_relative_import_discovery=discovery,
    )
    return discovery.projection(imports)


def _source_may_use_runtime_import_protocol(source: str) -> bool:
    return any(marker in source for marker in _RUNTIME_IMPORT_PROTOCOL_MARKERS)


def _resolve_runtime_import_expr_name(
    expr: ast.expr,
    alias_bindings: Mapping[str, str],
) -> str | None:
    if isinstance(expr, ast.Name):
        return alias_bindings.get(expr.id, expr.id)
    if (
        isinstance(expr, ast.Call)
        and isinstance(expr.func, ast.Name)
        and expr.func.id == "getattr"
        and len(expr.args) >= 2
        and not expr.keywords
    ):
        base = _resolve_runtime_import_expr_name(expr.args[0], alias_bindings)
        attr_node = expr.args[1]
        if (
            base is not None
            and isinstance(attr_node, ast.Constant)
            and isinstance(attr_node.value, str)
        ):
            return f"{base}.{attr_node.value}"
        return None
    if isinstance(expr, ast.Attribute):
        base = _resolve_runtime_import_expr_name(expr.value, alias_bindings)
        if base is None:
            return None
        return f"{base}.{expr.attr}"
    return None


def _runtime_import_alias_bindings(
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> dict[str, str]:
    bindings: dict[str, str] = {}
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    base_context = ModuleImportContext(
        module_name, is_package, target_python=target_python.feature_version
    )
    import_flow = analyze_module_import_flow(
        tree, base_context, ast_digest_admission=ast_digest_admission
    )
    scan_nodes = _scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        module_name=module_name,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )

    def _register_binding(local_name: str, qualified_name: str) -> None:
        if local_name and qualified_name:
            bindings[local_name] = qualified_name

    for node in scan_nodes:
        if not import_flow.states_for(node):
            continue
        if isinstance(node, ast.Import):
            for alias in node.names:
                local_name = alias.asname or alias.name.split(".", 1)[0]
                qualified_name = alias.name if alias.asname else local_name
                _register_binding(local_name, qualified_name)
            continue
        if not isinstance(node, ast.ImportFrom):
            continue
        contexts = tuple(
            base_context.with_state(state) for state in import_flow.states_for(node)
        )
        resolved_modules = _sealed_import_modules(
            StaticImportRequest.statement(node.module or "", level=node.level),
            contexts,
        )
        if not resolved_modules:
            continue
        for alias in node.names:
            if alias.name == "*":
                continue
            local_name = alias.asname or alias.name
            candidates = tuple(
                f"{resolved_module}.{alias.name}"
                for resolved_module in resolved_modules
            )
            preferred = next(
                (
                    candidate
                    for candidate in candidates
                    if candidate in _RUNTIME_IMPORT_PROTOCOL_TARGETS
                ),
                candidates[0],
            )
            _register_binding(local_name, preferred)

    for node in scan_nodes:
        if not import_flow.states_for(node):
            continue
        value: ast.expr | None = None
        target_names: list[str] = []
        if isinstance(node, ast.Assign):
            value = node.value
            target_names = [
                target.id for target in node.targets if isinstance(target, ast.Name)
            ]
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            value = node.value
            target_names = [node.target.id]
        if value is None or not target_names:
            continue
        resolved_value = _resolve_runtime_import_expr_name(value, bindings)
        if resolved_value not in _RUNTIME_IMPORT_PROTOCOL_TARGETS:
            continue
        for target_name in target_names:
            _register_binding(target_name, resolved_value)
    return bindings


def _tree_uses_runtime_import_protocol(
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> bool:
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    import_flow = analyze_module_import_flow(
        tree,
        ModuleImportContext(
            module_name, is_package, target_python=target_python.feature_version
        ),
        ast_digest_admission=ast_digest_admission,
    )
    alias_bindings = _runtime_import_alias_bindings(
        tree,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )
    scan_nodes = _scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        module_name=module_name,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )
    for node in scan_nodes:
        if not isinstance(node, ast.Call):
            continue
        if not import_flow.states_for(node):
            continue
        target = _resolve_runtime_import_expr_name(node.func, alias_bindings)
        if target in _RUNTIME_IMPORT_PROTOCOL_TARGETS:
            return True
    return False


def _static_string_sequence(node: ast.expr) -> tuple[str, ...] | None:
    if not isinstance(node, (ast.Tuple, ast.List)):
        return None
    out: list[str] = []
    for item in node.elts:
        if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
            return None
        out.append(item.value)
    return tuple(out)


def _static_module_all_exports(tree: ast.AST) -> tuple[str, ...] | None:
    body = getattr(tree, "body", ())
    exports: tuple[str, ...] | None = None
    for stmt in body:
        if isinstance(stmt, ast.Assign):
            if not any(
                isinstance(target, ast.Name) and target.id == "__all__"
                for target in stmt.targets
            ):
                continue
            sequence = _static_string_sequence(stmt.value)
            if sequence is None:
                return None
            exports = sequence
            continue
        if isinstance(stmt, ast.AnnAssign):
            if not isinstance(stmt.target, ast.Name) or stmt.target.id != "__all__":
                continue
            if stmt.value is None:
                return None
            sequence = _static_string_sequence(stmt.value)
            if sequence is None:
                return None
            exports = sequence
            continue
        if isinstance(stmt, (ast.AugAssign, ast.Delete)):
            targets = [stmt.target] if isinstance(stmt, ast.AugAssign) else stmt.targets
            if any(
                isinstance(target, ast.Name) and target.id == "__all__"
                for target in targets
            ):
                return None
        if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call):
            func = stmt.value.func
            if (
                isinstance(func, ast.Attribute)
                and func.attr
                in {"append", "extend", "insert", "remove", "pop", "clear"}
                and isinstance(func.value, ast.Name)
                and func.value.id == "__all__"
            ):
                return None
    return exports


def _collect_import_star_modules(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    source_path: Path | None = None,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    _dynamic_relative_import_discovery: _DynamicRelativeImportDiscovery | None = None,
) -> tuple[str, ...]:
    if runtime_import_custody is not None:
        runtime_import_custody.validate_scan_mode(
            module_name, source_path, import_scan_mode
        )
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    _validate_import_scan_mode(import_scan_mode)
    base_context = ModuleImportContext(
        module_name,
        is_package,
        target_python=target_python.feature_version,
    )
    import_flow = analyze_module_import_flow(
        tree,
        base_context,
        ast_digest_admission=ast_digest_admission,
    )
    scan_nodes = _scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        module_name=module_name,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )
    out: list[str] = []
    seen: set[str] = set()
    for node in scan_nodes:
        if not isinstance(node, ast.ImportFrom):
            continue
        if not any(alias.name == "*" for alias in node.names):
            continue
        contexts = tuple(
            base_context.with_state(state) for state in import_flow.states_for(node)
        )
        for resolved in _sealed_import_modules(
            StaticImportRequest.statement(node.module or "", level=node.level),
            contexts,
            runtime_import_custody=runtime_import_custody,
            source_path=source_path,
            source_ast_digest=ast_digest_admission.digest,
            dynamic_relative_import_discovery=(_dynamic_relative_import_discovery),
        ):
            if resolved and resolved not in seen:
                seen.add(resolved)
                out.append(resolved)
    return tuple(out)


def _expand_static_package_all_star_children(
    imports: Collection[str],
    star_modules: tuple[str, ...],
    *,
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolution_cache: _module_resolution._ModuleResolutionCache,
    target_python: TargetPythonVersion,
) -> tuple[str, ...]:
    out = list(dict.fromkeys(imports))
    seen = set(out)

    def add(name: str) -> None:
        if name and name not in seen:
            seen.add(name)
            out.append(name)

    roots_list = list(roots)
    for star_module in star_modules:
        package_path = resolution_cache.resolve_module(
            star_module,
            roots_list,
            stdlib_root,
            stdlib_allowlist,
        )
        if package_path is None or package_path.name != "__init__.py":
            continue
        try:
            package_source = resolution_cache.read_module_source(
                package_path,
                retain=False,
            )
            package_tree = resolution_cache.parse_module_ast(
                package_path,
                package_source,
                filename=str(package_path),
                retain=False,
                target_python=target_python,
            )
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        exports = _static_module_all_exports(package_tree)
        if exports is None:
            continue
        for export_name in exports:
            child_name = f"{star_module}.{export_name}"
            if (
                resolution_cache.resolve_module(
                    child_name,
                    roots_list,
                    stdlib_root,
                    stdlib_allowlist,
                )
                is not None
            ):
                add(child_name)
    return tuple(out)


def _collect_import_scan_requests(
    projection: _ImportDiscoveryProjection,
    tree: ast.AST,
    *,
    source_path: Path,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    target_python: TargetPythonVersion,
    runtime_import_custody: _RuntimeImportScanCustody | None,
    ast_digest_admission: _PythonAstDigestAdmission,
    source: str | None = None,
) -> _ImportScanRequests:
    discovery = _DynamicRelativeImportDiscovery.from_projection(projection)
    star_modules = _collect_import_star_modules(
        tree,
        module_name,
        is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        runtime_import_custody=runtime_import_custody,
        source_path=source_path,
        ast_digest_admission=ast_digest_admission,
        _dynamic_relative_import_discovery=discovery,
    )
    executions = (
        ()
        if source is not None and not _source_may_use_static_source_execution(source)
        else _collect_static_source_execution_requests(
            tree,
            source_path=source_path,
            import_scan_mode=import_scan_mode,
            module_name=module_name,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
        )
    )
    return _ImportScanRequests(
        projection.imports,
        executions,
        star_modules,
        tuple(discovery.candidates),
        discovery.required,
    )


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
) -> _CompleteImportScan:
    """Resolve every filesystem-derived decision in this operation, including misses."""
    imports = requests.imports
    if requests.star_modules and (
        roots is None or stdlib_root is None or stdlib_allowlist is None
    ):
        raise ValueError("star-import completion requires current resolution context")
    if roots is not None and stdlib_root is not None and stdlib_allowlist is not None:
        imports = _expand_static_package_all_star_children(
            imports,
            requests.star_modules,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=resolution_cache,
            target_python=target_python,
        )
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
        imports,
        tuple(executions),
        requests.dynamic_relative_import_candidates,
        requests.requires_runtime_package_anchor,
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
    tree: ast.AST | None = None,
    is_package: bool | None = None,
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
                scan_tree = module_resolution_cache.parse_module_ast(
                    module_path,
                    source,
                    filename=str(module_path),
                    retain=False,
                    target_python=target_python,
                )
            except SyntaxError:
                return True
        ast_digest_admission = _PythonAstDigestAdmission(scan_tree)
        scan_nodes = _scan_nodes_for_import_mode(
            scan_tree,
            import_scan_mode,
            module_name=module_name,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
        )
        import_flow = analyze_module_import_flow(
            scan_tree,
            ModuleImportContext(
                module_name, is_package, target_python=target_python.feature_version
            ),
            ast_digest_admission=ast_digest_admission,
        )
        for node in scan_nodes:
            if not import_flow.states_for(node):
                continue
            if isinstance(node, ast.Import):
                if any(alias.name != "_intrinsics" for alias in node.names):
                    return True
                continue
            if isinstance(node, ast.ImportFrom):
                if node.module == "__future__":
                    continue
                if node.level == 0 and (
                    node.module == "_intrinsics"
                    or (
                        node.module is not None and node.module.endswith("._intrinsics")
                    )
                ):
                    continue
                return True
        return _tree_uses_runtime_import_protocol(
            scan_tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
        )

    return module_resolution_cache.uses_runtime_import_protocol(
        module_path,
        producer=produce,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python_tag=target_python.tag,
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
        if _module_uses_runtime_import_protocol(
            module_name=module_name,
            module_path=module_path,
            module_resolution_cache=module_resolution_cache,
            target_python=target_python,
            import_scan_mode=import_scan_mode,
            tree=tree,
            is_package=scan_authority.by_module[module_name].is_package,
        ):
            return _RuntimeImportSupportPolicy(
                needs_generated_importer=False,
                needs_runtime_import_support=True,
            )
    return _RuntimeImportSupportPolicy(
        needs_generated_importer=False,
        needs_runtime_import_support=False,
    )
