from __future__ import annotations

import ast
from collections.abc import Callable, Collection, Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from molt.cli import module_resolution as _module_resolution
from molt.cli.models import ImportScanMode, _RuntimeImportSupportPolicy
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
from molt.compiler_analysis.static_truth import (
    static_if_live_branch,
    static_test_truthiness,
)
from molt.compiler_analysis.python_binding_facts import (
    PythonIdentity,
    PythonParameterRef,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_bindings,
    python_ast_digest,
)
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    analyze_module_import_flow,
    bind_static_import_call_arguments,
    dunder_globals_state_from_expression,
    metadata_value_from_expression,
    plan_static_import_request,
    require_static_import_modules,
    resolve_relative_import,
    static_import_candidates,
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


IMPORTER_MODULE_NAME = "_molt_importer"


def _sealed_import_modules(
    request: StaticImportRequest,
    contexts: Sequence[ModuleImportContext],
) -> tuple[str, ...]:
    module_name = contexts[0].module_name if contexts else None
    return require_static_import_modules(
        plan_static_import_request(request, contexts),
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


@dataclass(frozen=True, slots=True)
class _StaticSourceExecution:
    """Compiler-admitted Python source executed through a loader or runpy.

    Unlike an import request, the execution name and source path are independent:
    ``spec_from_file_location`` may intentionally execute one file under an
    arbitrary module name.  Keeping both values prevents the module graph from
    guessing identity from the filesystem layout.
    """

    module_name: str | None
    source_path: Path


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


def _source_may_use_static_source_execution(source: str) -> bool:
    return any(marker in source for marker in _STATIC_SOURCE_EXECUTION_MARKERS)


def _collect_static_source_executions(
    tree: ast.AST,
    *,
    source_path: Path,
    import_scan_mode: ImportScanMode = "full",
    module_name: str | None = None,
) -> tuple[_StaticSourceExecution, ...]:
    """Collect statically addressable loader/runpy source execution roots.

    This is the source-path projection of the import scanner.  It deliberately
    accepts only expressions that can be evaluated without executing user code;
    dynamic paths remain runtime capability work and never become build inputs
    by accident.
    """

    aliases: dict[str, str] = {
        "importlib": "importlib",
        "runpy": "runpy",
        "Path": "pathlib.Path",
    }
    constants: dict[str, str | Path] = {}

    if isinstance(tree, ast.Module):
        for stmt in tree.body:
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

    def static_value(expr: ast.expr) -> str | Path | None:
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
                and isinstance(left, (str, Path))
                and isinstance(right, (str, Path))
            ):
                return Path(left) / Path(right)
            return None
        if isinstance(expr, ast.Call):
            target = qualified_name(expr.func)
            if target == "pathlib.Path" and len(expr.args) == 1 and not expr.keywords:
                value = static_value(expr.args[0])
                return Path(value) if isinstance(value, (str, Path)) else None
            if (
                target in {"os.path.join", "posixpath.join", "ntpath.join"}
                and expr.args
            ):
                parts = [static_value(arg) for arg in expr.args]
                if all(isinstance(part, (str, Path)) for part in parts):
                    path_parts = [Path(part) for part in parts if part is not None]
                    head, *tail = path_parts
                    return head.joinpath(*tail)
            if (
                isinstance(expr.func, ast.Attribute)
                and expr.func.attr in {"resolve", "absolute"}
                and not expr.args
                and not expr.keywords
            ):
                value = static_value(expr.func.value)
                if isinstance(value, (str, Path)):
                    path = Path(value)
                    if not path.is_absolute():
                        path = source_path.parent / path
                    return path.resolve()
        return None

    # Module constants are the common authority for loader paths and remain
    # visible to calls nested in entry-module functions.
    if isinstance(tree, ast.Module):
        for stmt in tree.body:
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

    requests: list[_StaticSourceExecution] = []
    seen: set[tuple[str | None, str]] = set()
    for node in _scan_nodes_for_import_mode(
        tree, import_scan_mode, module_name=module_name
    ):
        if not isinstance(node, ast.Call):
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
        if not isinstance(path_value, (str, Path)):
            continue
        resolved = Path(path_value)
        if not resolved.is_absolute():
            resolved = source_path.parent / resolved
        resolved = resolved.resolve()
        if resolved.is_dir():
            resolved = resolved / "__main__.py"
        if resolved.suffix not in {".py", ".pyi"} or not resolved.is_file():
            continue
        key = request_name, str(resolved)
        if key not in seen:
            seen.add(key)
            requests.append(_StaticSourceExecution(request_name, resolved))
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
    fact_truth: Callable[[ast.expr], bool | None],
) -> tuple[ast.expr, ...]:
    values: list[ast.expr] = []
    if isinstance(node.op, ast.And):
        for idx, value in enumerate(node.values):
            values.append(value)
            value_truth = static_test_truthiness(
                value,
                type_checking_names=(),
                type_checking_module_aliases=(),
                fact_truth=fact_truth,
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
                type_checking_names=(),
                type_checking_module_aliases=(),
                fact_truth=fact_truth,
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
) -> tuple[ast.AST, ...]:
    if not isinstance(tree, ast.Module):
        return tuple(ast.walk(tree))
    binding_index = analyze_python_bindings(
        tree,
        source_digest=python_ast_digest(tree),
        policy=PythonBindingPolicy(),
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
                fact_truth=binding_index.static_truth,
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
                type_checking_names=(),
                type_checking_module_aliases=(),
                fact_truth=binding_index.static_truth,
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


def _module_init_scan_nodes(tree: ast.AST) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(tree, include_function_bodies=False)


def _module_init_static_helper_scan_nodes(
    tree: ast.AST, module_name: str | None
) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(
        tree,
        include_function_bodies=False,
        included_function_qualnames=_static_import_helper_qualnames(
            module_name, "module_init_static_helpers"
        ),
    )


def _full_static_scan_nodes(tree: ast.AST) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(tree, include_function_bodies=True)


def _scan_nodes_for_import_mode(
    tree: ast.AST,
    import_scan_mode: ImportScanMode,
    *,
    module_name: str | None = None,
) -> tuple[ast.AST, ...]:
    _validate_import_scan_mode(import_scan_mode)
    if import_scan_mode == "full":
        return _full_static_scan_nodes(tree)
    if import_scan_mode == "module_init_static_helpers":
        return _module_init_static_helper_scan_nodes(tree, module_name)
    return _module_init_scan_nodes(tree)


def _collect_imports(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
) -> list[str]:
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
        source_digest=python_ast_digest(tree),
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
        if isinstance(value, tuple) and all(
            isinstance(item, str) for item in value
        ):
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
            if payload.target in {"importlib.import_module", "importlib.util.find_spec"}:
                request = StaticImportRequest.import_module(
                    name,
                    metadata_value_from_expression(
                        payload.package, context, resolve_string
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
            for module in _sealed_import_modules(request, (context,)):
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
        if (
            isinstance(node, ast.ImportFrom)
            and not node.level
            and node.module in {"typing", "typing_extensions"}
            and all(alias.name == "TYPE_CHECKING" for alias in node.names)
        ):
            return
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
            _sealed_import_modules(request, _import_contexts(node))
        )

    def _collect_import_call(
        node: ast.Call, *, allow_possible: bool = False
    ) -> None:
        _record_helper_call_imports(node)
        target = _static_call_target(node, allow_possible=allow_possible)
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
                type_checking_names=(),
                type_checking_module_aliases=(),
                fact_truth=binding_index.static_truth,
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
                fact_truth=binding_index.static_truth,
            ):
                _visit(value, qualname_prefix)
            return
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if getattr(node, "type_params", None):
                needs_typing = True
            if isinstance(node, ast.ClassDef):
                _visit_many(
                    node.decorator_list, qualname_prefix
                )
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
            _visit_many(
                list(node.args.defaults), qualname_prefix
            )
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
            _visit_many(
                list(node.args.defaults), qualname_prefix
            )
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
                            _collect_import_call(child, allow_possible=True)
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
) -> dict[str, str]:
    bindings: dict[str, str] = {}
    base_context = ModuleImportContext(module_name, is_package)
    import_flow = analyze_module_import_flow(tree, base_context)
    scan_nodes = _scan_nodes_for_import_mode(
        tree, import_scan_mode, module_name=module_name
    )

    def _register_binding(local_name: str, qualified_name: str) -> None:
        if local_name and qualified_name:
            bindings[local_name] = qualified_name

    for node in scan_nodes:
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
) -> bool:
    alias_bindings = _runtime_import_alias_bindings(
        tree,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )
    scan_nodes = _scan_nodes_for_import_mode(
        tree, import_scan_mode, module_name=module_name
    )
    for node in scan_nodes:
        if not isinstance(node, ast.Call):
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
) -> tuple[str, ...]:
    _validate_import_scan_mode(import_scan_mode)
    base_context = ModuleImportContext(
        module_name,
        is_package,
        target_python=target_python.feature_version,
    )
    import_flow = analyze_module_import_flow(tree, base_context)
    scan_nodes = _scan_nodes_for_import_mode(
        tree, import_scan_mode, module_name=module_name
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
        ):
            if resolved and resolved not in seen:
                seen.add(resolved)
                out.append(resolved)
    return tuple(out)


def _expand_imports_with_static_package_all_star_children(
    imports: Collection[str],
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolution_cache: "_module_resolution._ModuleResolutionCache",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
) -> tuple[str, ...]:
    out: list[str] = []
    seen: set[str] = set()

    def add(name: str) -> None:
        if name and name not in seen:
            seen.add(name)
            out.append(name)

    for name in imports:
        add(name)
    star_modules = _collect_import_star_modules(
        tree,
        module_name,
        is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
    )
    if not star_modules:
        return tuple(out)

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
) -> bool:
    if module_name in _RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES:
        return False
    is_package = module_path.name == "__init__.py"
    if tree is None:
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
            tree = module_resolution_cache.parse_module_ast(
                module_path,
                source,
                filename=str(module_path),
                retain=False,
                target_python=target_python,
            )
        except SyntaxError:
            return True
    scan_nodes = _scan_nodes_for_import_mode(
        tree, import_scan_mode, module_name=module_name
    )
    for node in scan_nodes:
        if isinstance(node, ast.Import):
            if any(alias.name != "_intrinsics" for alias in node.names):
                return True
            continue
        if isinstance(node, ast.ImportFrom):
            if node.module == "__future__":
                continue
            if node.level == 0 and (
                node.module == "_intrinsics"
                or (node.module is not None and node.module.endswith("._intrinsics"))
            ):
                continue
            return True
    return module_resolution_cache.uses_runtime_import_protocol(
        module_path,
        tree,
        detector=_tree_uses_runtime_import_protocol,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )


def _module_graph_needs_runtime_import_support(
    *,
    module_graph: Mapping[str, Path],
    module_resolution_cache: "_module_resolution._ModuleResolutionCache",
    explicit_imports: Collection[str],
    entry_module: str,
    entry_path: Path,
    entry_tree: ast.AST,
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
        import_scan_mode: ImportScanMode = (
            "full"
            if module_name == entry_module and module_path == entry_path
            else "module_init_static_helpers"
            if module_name in STDLIB_STATIC_IMPORT_HELPER_MODULES
            else "module_init"
        )
        if _module_uses_runtime_import_protocol(
            module_name=module_name,
            module_path=module_path,
            module_resolution_cache=module_resolution_cache,
            target_python=target_python,
            import_scan_mode=import_scan_mode,
            tree=tree,
        ):
            return _RuntimeImportSupportPolicy(
                needs_generated_importer=False,
                needs_runtime_import_support=True,
            )
    return _RuntimeImportSupportPolicy(
        needs_generated_importer=False,
        needs_runtime_import_support=False,
    )
