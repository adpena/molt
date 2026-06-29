from __future__ import annotations

import ast
from collections.abc import Collection, Iterable, Mapping, Sequence
from pathlib import Path
from typing import cast

from molt.cli import module_resolution as _module_resolution
from molt.cli.models import ImportScanMode, _RuntimeImportSupportPolicy
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
from molt.compiler_analysis.static_truth import static_if_live_branch, static_test_truthiness


# Runtime helper bodies whose imports are required static graph edges. This is
# intentionally qualname-based: stdlib modules stay module-init scanned unless a
# specific helper body is part of Molt's compiled runtime contract.
STDLIB_STATIC_IMPORT_HELPER_QUALNAMES: Mapping[str, frozenset[str]] = {
    "collections": frozenset({"UserDict.copy"}),
    # EmailMessage inherits MIMEPart.__init__, which supplies email.policy.default.
    "email.message": frozenset({"MIMEPart.__init__"}),
}
STDLIB_STATIC_IMPORT_HELPER_MODULES = frozenset(
    STDLIB_STATIC_IMPORT_HELPER_QUALNAMES
)

_IMPORT_SCAN_MODES = frozenset(
    {"full", "module_init", "module_init_static_helpers"}
)


IMPORTER_MODULE_NAME = "_molt_importer"


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


def _spec_parent(spec_name: str, is_package: bool) -> str:
    if is_package:
        return spec_name
    return spec_name.rpartition(".")[0]


def _is_modulespec_ctor(node: ast.AST) -> bool:
    if isinstance(node, ast.Name):
        return node.id == "ModuleSpec"
    if isinstance(node, ast.Attribute):
        return node.attr == "ModuleSpec"
    return False


def _parse_modulespec_override(
    value: ast.AST,
) -> tuple[str, bool | None] | None:
    if not isinstance(value, ast.Call):
        return None
    if not _is_modulespec_ctor(value.func):
        return None
    spec_name = None
    if value.args:
        first = value.args[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            spec_name = first.value
    for kw in value.keywords:
        if (
            kw.arg == "name"
            and spec_name is None
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, str)
        ):
            spec_name = kw.value.value
    if spec_name is None:
        return None
    is_package = None
    if len(value.args) >= 4:
        arg = value.args[3]
        if isinstance(arg, ast.Constant) and isinstance(arg.value, bool):
            is_package = arg.value
    for kw in value.keywords:
        if (
            kw.arg == "is_package"
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, bool)
        ):
            is_package = kw.value.value
    return spec_name, is_package


def _infer_module_overrides(
    tree: ast.AST,
) -> tuple[bool, str | None, bool, str | None, bool | None]:
    package_override_set = False
    package_override: str | None = None
    spec_override_set = False
    spec_override: str | None = None
    spec_override_is_package: bool | None = None
    for stmt in getattr(tree, "body", []):
        if isinstance(stmt, ast.Assign):
            targets = stmt.targets
            value = stmt.value
        elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
            targets = [stmt.target]
            value = stmt.value
        else:
            continue
        for target in targets:
            if not isinstance(target, ast.Name):
                continue
            if target.id == "__package__":
                package_override_set = True
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    package_override = value.value
                elif isinstance(value, ast.Constant) and value.value is None:
                    package_override = None
                else:
                    package_override = None
            elif target.id == "__spec__":
                if isinstance(value, ast.Constant) and value.value is None:
                    spec_override_set = False
                    spec_override = None
                    spec_override_is_package = None
                else:
                    parsed = _parse_modulespec_override(value)
                    if parsed is None:
                        continue
                    spec_override_set = True
                    spec_override, spec_override_is_package = parsed
    return (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    )


def _resolve_relative_import(
    module_name: str,
    *,
    is_package: bool,
    level: int,
    module: str | None,
    package_override: str | None = None,
    package_override_set: bool = False,
    spec_override: str | None = None,
    spec_override_set: bool = False,
    spec_override_is_package: bool | None = None,
) -> str | None:
    if level <= 0:
        return module
    package = ""
    if package_override_set:
        package = package_override or ""
    else:
        if spec_override_set and spec_override:
            override_is_package = (
                spec_override_is_package
                if spec_override_is_package is not None
                else is_package
            )
            package = _spec_parent(spec_override, override_is_package)
        else:
            if is_package:
                package = module_name
            elif "." in module_name:
                package = module_name.rsplit(".", 1)[0]
            else:
                package = ""
    if not package:
        return None
    parts = package.split(".")
    if level > len(parts):
        return None
    base_parts = parts[: len(parts) - (level - 1)]
    base_name = ".".join(base_parts)
    if module:
        if base_name:
            return f"{base_name}.{module}"
        return module
    return base_name or None


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
    type_checking_names: Collection[str],
    type_checking_module_aliases: Collection[str],
) -> tuple[ast.expr, ...]:
    values: list[ast.expr] = []
    if isinstance(node.op, ast.And):
        for idx, value in enumerate(node.values):
            values.append(value)
            value_truth = static_test_truthiness(
                value,
                type_checking_names=type_checking_names,
                type_checking_module_aliases=type_checking_module_aliases,
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
                type_checking_names=type_checking_names,
                type_checking_module_aliases=type_checking_module_aliases,
            )
            if value_truth is True:
                return tuple(values)
            if value_truth is None:
                values.extend(node.values[idx + 1 :])
                return tuple(values)
        return tuple(values)
    return tuple(node.values)


_TYPE_CHECKING_MODULES = frozenset({"typing", "typing_extensions"})


def _function_parameter_names_from_args(args: ast.arguments) -> list[str]:
    names = [arg.arg for arg in args.posonlyargs]
    names.extend(arg.arg for arg in args.args)
    names.extend(arg.arg for arg in args.kwonlyargs)
    if args.vararg is not None:
        names.append(args.vararg.arg)
    if args.kwarg is not None:
        names.append(args.kwarg.arg)
    return names


class _StaticTruthBindings:
    def __init__(self) -> None:
        self.type_checking_names: set[str] = {"TYPE_CHECKING"}
        self.type_checking_module_aliases: set[str] = set(_TYPE_CHECKING_MODULES)

    def fork(self) -> "_StaticTruthBindings":
        forked = _StaticTruthBindings()
        forked.type_checking_names = set(self.type_checking_names)
        forked.type_checking_module_aliases = set(self.type_checking_module_aliases)
        return forked

    def record_import_aliases(self, node: ast.Import | ast.ImportFrom) -> None:
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name in _TYPE_CHECKING_MODULES:
                    self.type_checking_module_aliases.add(alias.asname or alias.name)
            return
        if node.level or node.module not in _TYPE_CHECKING_MODULES:
            return
        for alias in node.names:
            if alias.name == "TYPE_CHECKING":
                self.type_checking_names.add(alias.asname or alias.name)

    def is_type_checking_only_import(self, node: ast.Import | ast.ImportFrom) -> bool:
        return (
            isinstance(node, ast.ImportFrom)
            and not node.level
            and node.module in _TYPE_CHECKING_MODULES
            and all(alias.name == "TYPE_CHECKING" for alias in node.names)
        )

    def record_rebinding_target(self, target: ast.expr) -> None:
        if isinstance(target, ast.Name):
            self.type_checking_names.discard(target.id)
            self.type_checking_module_aliases.discard(target.id)

    def record_assignment_target(self, target: ast.expr, value: ast.AST) -> None:
        if (
            isinstance(target, ast.Name)
            and isinstance(value, ast.Constant)
            and value.value is False
        ):
            self.type_checking_names.add(target.id)
            self.type_checking_module_aliases.discard(target.id)
            return
        self.record_rebinding_target(target)

    def static_if_live_branch(self, node: ast.If) -> list[ast.stmt] | None:
        return static_if_live_branch(
            node,
            type_checking_names=self.type_checking_names,
            type_checking_module_aliases=self.type_checking_module_aliases,
        )


def _static_scan_nodes(
    tree: ast.AST,
    *,
    include_function_bodies: bool,
    included_function_qualnames: Collection[str] = frozenset(),
) -> tuple[ast.AST, ...]:
    if not isinstance(tree, ast.Module):
        return tuple(ast.walk(tree))
    nodes: list[ast.AST] = []
    included_qualnames = frozenset(included_function_qualnames)

    def visit(
        node: ast.AST,
        qualname_prefix: tuple[str, ...] = (),
        truth_bindings: _StaticTruthBindings | None = None,
    ) -> None:
        truth_bindings = truth_bindings or _StaticTruthBindings()
        nodes.append(node)
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            truth_bindings.record_import_aliases(node)
            return
        if isinstance(node, ast.Assign):
            visit(node.value, qualname_prefix, truth_bindings)
            for target in node.targets:
                visit(target, qualname_prefix, truth_bindings)
                truth_bindings.record_assignment_target(target, node.value)
            return
        if isinstance(node, ast.AnnAssign):
            visit(node.annotation, qualname_prefix, truth_bindings)
            if node.value is not None:
                visit(node.value, qualname_prefix, truth_bindings)
            visit(node.target, qualname_prefix, truth_bindings)
            if node.value is not None:
                truth_bindings.record_assignment_target(node.target, node.value)
            else:
                truth_bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.AugAssign):
            visit(node.target, qualname_prefix, truth_bindings)
            visit(node.value, qualname_prefix, truth_bindings)
            truth_bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.Delete):
            for target in node.targets:
                visit(target, qualname_prefix, truth_bindings)
                truth_bindings.record_rebinding_target(target)
            return
        if isinstance(node, ast.NamedExpr):
            visit(node.value, qualname_prefix, truth_bindings)
            visit(node.target, qualname_prefix, truth_bindings)
            truth_bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.BoolOp):
            for value in _statically_executed_boolop_values(
                node,
                type_checking_names=truth_bindings.type_checking_names,
                type_checking_module_aliases=(
                    truth_bindings.type_checking_module_aliases
                ),
            ):
                visit(value, qualname_prefix, truth_bindings)
            return
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            function_qualname = ".".join(_qualified_child(qualname_prefix, node.name))
            for decorator in node.decorator_list:
                visit(decorator, qualname_prefix, truth_bindings)
            for default in list(node.args.defaults) + [
                default for default in node.args.kw_defaults if default is not None
            ]:
                visit(default, qualname_prefix, truth_bindings)
            for arg in (
                list(node.args.posonlyargs)
                + list(node.args.args)
                + list(node.args.kwonlyargs)
            ):
                if arg.annotation is not None:
                    visit(arg.annotation, qualname_prefix, truth_bindings)
            if node.args.vararg is not None and node.args.vararg.annotation is not None:
                visit(node.args.vararg.annotation, qualname_prefix, truth_bindings)
            if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
                visit(node.args.kwarg.annotation, qualname_prefix, truth_bindings)
            if node.returns is not None:
                visit(node.returns, qualname_prefix, truth_bindings)
            for type_param in getattr(node, "type_params", ()):
                visit(type_param, qualname_prefix, truth_bindings)
            if include_function_bodies or function_qualname in included_qualnames:
                function_truth = truth_bindings.fork()
                for name in _function_parameter_names_from_args(node.args):
                    function_truth.type_checking_names.discard(name)
                    function_truth.type_checking_module_aliases.discard(name)
                function_prefix = _qualified_child(qualname_prefix, node.name)
                for stmt in node.body:
                    visit(stmt, function_prefix, function_truth)
            return
        if isinstance(node, ast.Lambda):
            for default in list(node.args.defaults) + [
                default for default in node.args.kw_defaults if default is not None
            ]:
                visit(default, qualname_prefix, truth_bindings)
            if include_function_bodies:
                lambda_truth = truth_bindings.fork()
                for name in _function_parameter_names_from_args(node.args):
                    lambda_truth.type_checking_names.discard(name)
                    lambda_truth.type_checking_module_aliases.discard(name)
                visit(node.body, qualname_prefix, lambda_truth)
            return
        if isinstance(node, ast.ClassDef):
            for decorator in node.decorator_list:
                visit(decorator, qualname_prefix, truth_bindings)
            for base in node.bases:
                visit(base, qualname_prefix, truth_bindings)
            for keyword in node.keywords:
                if keyword.value is not None:
                    visit(keyword.value, qualname_prefix, truth_bindings)
            for type_param in getattr(node, "type_params", ()):
                visit(type_param, qualname_prefix, truth_bindings)
            class_prefix = _qualified_child(qualname_prefix, node.name)
            class_truth = truth_bindings.fork()
            for stmt in node.body:
                visit(stmt, class_prefix, class_truth)
            return
        if isinstance(node, ast.If):
            visit(node.test, qualname_prefix, truth_bindings)
            static_branch = truth_bindings.static_if_live_branch(node)
            if static_branch is not None:
                for stmt in static_branch:
                    visit(stmt, qualname_prefix, truth_bindings)
            else:
                for stmt in node.body:
                    visit(stmt, qualname_prefix, truth_bindings)
                for stmt in node.orelse:
                    visit(stmt, qualname_prefix, truth_bindings)
            return
        for child in ast.iter_child_nodes(node):
            visit(child, qualname_prefix, truth_bindings)

    truth_bindings = _StaticTruthBindings()
    for stmt in tree.body:
        visit(stmt, truth_bindings=truth_bindings)
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
    module_string_constants: dict[str, str] = {}
    helper_string_functions: dict[str, tuple[list[str], ast.expr]] = {}
    helper_param_import_positions: dict[str, set[int]] = {}
    helper_import_arg_exprs: dict[str, tuple[list[str], set[str], list[ast.expr]]] = {}
    (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    ) = _infer_module_overrides(tree)
    module_body = list(getattr(tree, "body", []))
    function_walks: list[
        tuple[
            ast.FunctionDef | ast.AsyncFunctionDef,
            tuple[ast.AST, ...],
            "_ImportlibStaticBindings",
        ]
    ] = []

    class _ImportlibStaticBindings:
        def __init__(self) -> None:
            self.module_aliases: set[str] = {"importlib"}
            self.util_aliases: set[str] = set()
            self.import_module_aliases: set[str] = set()
            self.module_import_module_mutated = False
            self.module_util_mutated = False

        def fork(self) -> "_ImportlibStaticBindings":
            forked = _ImportlibStaticBindings()
            forked.module_aliases = set(self.module_aliases)
            forked.util_aliases = set(self.util_aliases)
            forked.import_module_aliases = set(self.import_module_aliases)
            forked.module_import_module_mutated = self.module_import_module_mutated
            forked.module_util_mutated = self.module_util_mutated
            return forked

        def record_aliases(self, node: ast.Import | ast.ImportFrom) -> None:
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if alias.name == "importlib":
                        self.module_aliases.add(alias.asname or "importlib")
                    elif alias.name == "importlib.util":
                        if alias.asname:
                            if not self.module_util_mutated:
                                self.util_aliases.add(alias.asname)
                        else:
                            self.module_aliases.add("importlib")
                    elif alias.name.startswith("importlib.") and not alias.asname:
                        self.module_aliases.add("importlib")
                return
            if node.level or node.module != "importlib":
                return
            for alias in node.names:
                bind_name = alias.asname or alias.name
                if alias.name == "import_module":
                    if not self.module_import_module_mutated:
                        self.import_module_aliases.add(bind_name)
                elif alias.name == "util":
                    if not self.module_util_mutated:
                        self.util_aliases.add(bind_name)

        def invalidate_name(self, name: str) -> None:
            self.module_aliases.discard(name)
            self.util_aliases.discard(name)
            self.import_module_aliases.discard(name)

        def record_rebinding_target(self, target: ast.expr) -> None:
            if isinstance(target, ast.Name):
                self.invalidate_name(target.id)
                return
            if (
                isinstance(target, ast.Attribute)
                and isinstance(target.value, ast.Name)
                and target.value.id in self.module_aliases
            ):
                if target.attr == "import_module":
                    self.module_import_module_mutated = True
                elif target.attr == "util":
                    self.module_util_mutated = True

        def target(self, func: ast.expr) -> str | None:
            if isinstance(func, ast.Name):
                if func.id in self.import_module_aliases:
                    return "importlib.import_module"
                return func.id
            if (
                isinstance(func, ast.Attribute)
                and func.attr == "import_module"
                and isinstance(func.value, ast.Name)
                and func.value.id in self.module_aliases
            ):
                if self.module_import_module_mutated:
                    return None
                return "importlib.import_module"
            if isinstance(func, ast.Attribute) and func.attr == "find_spec":
                if (
                    isinstance(func.value, ast.Name)
                    and func.value.id in self.util_aliases
                ):
                    return "importlib.util.find_spec"
                if (
                    isinstance(func.value, ast.Attribute)
                    and func.value.attr == "util"
                    and isinstance(func.value.value, ast.Name)
                    and func.value.value.id in self.module_aliases
                ):
                    if self.module_util_mutated:
                        return None
                    return "importlib.util.find_spec"
            if isinstance(func, ast.Attribute):
                parts: list[str] = []
                current: ast.expr | None = func
                while isinstance(current, ast.Attribute):
                    parts.append(current.attr)
                    current = current.value
                if isinstance(current, ast.Name):
                    parts.append(current.id)
                    return ".".join(reversed(parts))
            return None

    helper_importlib_bindings = _ImportlibStaticBindings()

    def _is_static_import_target(target: str | None) -> bool:
        return target in {
            "__import__",
            "builtins.__import__",
            "importlib.import_module",
            "importlib.util.find_spec",
            "_MOLT_IMPORTLIB_IMPORT_TRANSACTION",
            "molt_importlib_import_transaction",
        }

    def _resolve_string_sequence(
        node: ast.expr, bindings: dict[str, object], seen: set[str]
    ) -> list[str] | None:
        if isinstance(node, (ast.Tuple, ast.List)):
            out: list[str] = []
            for element in node.elts:
                value = _resolve_string_constant(element, bindings, seen)
                if value is None:
                    return None
                out.append(value)
            return out
        if isinstance(node, ast.Name):
            bound = bindings.get(node.id)
            if isinstance(bound, list) and all(isinstance(item, str) for item in bound):
                return list(cast(list[str], bound))
        return None

    def _resolve_string_constant(
        node: ast.expr,
        bindings: dict[str, object] | None = None,
        seen: set[str] | None = None,
    ) -> str | None:
        bindings = bindings or {}
        seen = seen or set()
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Name):
            bound = bindings.get(node.id)
            if isinstance(bound, str):
                return bound
            return module_string_constants.get(node.id)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = _resolve_string_constant(node.left, bindings, seen)
            right = _resolve_string_constant(node.right, bindings, seen)
            if left is not None and right is not None:
                return left + right
            return None
        if isinstance(node, ast.Call):
            target = helper_importlib_bindings.target(node.func)
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
                return _resolve_relative_import(
                    package,
                    is_package=True,
                    level=level,
                    module=module,
                    package_override=package_override,
                    package_override_set=package_override_set,
                    spec_override=spec_override,
                    spec_override_set=spec_override_set,
                    spec_override_is_package=spec_override_is_package,
                )
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
            seq = _resolve_string_sequence(keyword.value, {}, set())
            if seq is not None:
                bindings[keyword.arg] = seq
        if not required_params.issubset(bindings):
            return None
        return bindings

    module_import_helper_scan = isinstance(tree, ast.Module)

    if module_import_helper_scan:
        for stmt in module_body:
            if isinstance(stmt, (ast.Import, ast.ImportFrom)):
                helper_importlib_bindings.record_aliases(stmt)
            if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
                target = stmt.targets[0]
                for rebind_target in stmt.targets:
                    helper_importlib_bindings.record_rebinding_target(rebind_target)
                if isinstance(target, ast.Name):
                    value = _resolve_string_constant(stmt.value)
                    if value is not None:
                        module_string_constants[target.id] = value
            elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name):
                helper_importlib_bindings.record_rebinding_target(stmt.target)
                value = _resolve_string_constant(stmt.value) if stmt.value else None
                if value is not None:
                    module_string_constants[stmt.target.id] = value
            elif isinstance(stmt, ast.AugAssign):
                helper_importlib_bindings.record_rebinding_target(stmt.target)
            elif isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                stmt_nodes = tuple(ast.walk(stmt))
                function_walks.append(
                    (stmt, stmt_nodes, helper_importlib_bindings.fork())
                )
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

        for stmt, stmt_nodes, stmt_importlib_bindings in function_walks:
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
            param_set = set(params)
            param_positions = {name: idx for idx, name in enumerate(params)}
            required_params = _function_required_param_names(stmt, params)
            local_expr_bindings = _simple_function_local_expr_bindings(stmt)
            for node in stmt_nodes:
                if not isinstance(node, ast.Call) or not node.args:
                    continue
                target = stmt_importlib_bindings.target(node.func)
                if not _is_static_import_target(target):
                    continue
                first = _resolve_local_expr_binding(node.args[0], local_expr_bindings)
                helper_entry = helper_import_arg_exprs.get(stmt.name)
                if helper_entry is None:
                    helper_import_arg_exprs[stmt.name] = (
                        params,
                        required_params,
                        [first],
                    )
                else:
                    helper_entry[2].append(first)
                if isinstance(first, ast.Name) and first.id in param_set:
                    pos = param_positions[first.id]
                    helper_param_import_positions.setdefault(stmt.name, set()).add(pos)

    def _record_helper_call_imports(node: ast.Call) -> None:
        if module_import_helper_scan:
            if not isinstance(node.func, ast.Name):
                return
            positions = helper_param_import_positions.get(node.func.id)
            if positions:
                for pos in positions:
                    if pos < len(node.args):
                        resolved = _resolve_string_constant(node.args[pos])
                        if resolved is not None:
                            imports.append(resolved)
            helper_expr_entry = helper_import_arg_exprs.get(node.func.id)
            if helper_expr_entry is not None:
                params, required_params, exprs = helper_expr_entry
                call_bindings = _bind_helper_call_arguments(
                    node, params, required_params
                )
                if call_bindings is not None:
                    for expr in exprs:
                        resolved = _resolve_string_constant(expr, call_bindings, set())
                        if resolved is not None:
                            imports.append(resolved)

    def _record_import_statement(
        node: ast.Import | ast.ImportFrom,
        bindings: _ImportlibStaticBindings,
        truth_bindings: _StaticTruthBindings,
    ) -> None:
        truth_bindings.record_import_aliases(node)
        if truth_bindings.is_type_checking_only_import(node):
            return
        bindings.record_aliases(node)
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
            return
        if node.level:
            if module_name:
                resolved = _resolve_relative_import(
                    module_name,
                    is_package=is_package,
                    level=node.level,
                    module=node.module,
                    package_override=package_override,
                    package_override_set=package_override_set,
                    spec_override=spec_override,
                    spec_override_set=spec_override_set,
                    spec_override_is_package=spec_override_is_package,
                )
                if resolved:
                    imports.append(resolved)
                    for alias in node.names:
                        if alias.name != "*":
                            imports.append(f"{resolved}.{alias.name}")
            return
        if node.module:
            imports.append(node.module)
            for alias in node.names:
                if alias.name != "*":
                    imports.append(f"{node.module}.{alias.name}")

    def _collect_import_call(
        node: ast.Call, bindings: _ImportlibStaticBindings
    ) -> None:
        _record_helper_call_imports(node)
        if _is_static_import_target(bindings.target(node.func)):
            resolved = _resolve_string_constant(node.args[0])
            if resolved is not None:
                imports.append(resolved)

    def _function_parameter_names(
        node: ast.Lambda | ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> list[str]:
        return _function_parameter_names_from_args(node.args)

    def _visit_many(
        nodes: Iterable[ast.AST],
        bindings: _ImportlibStaticBindings,
        truth_bindings: _StaticTruthBindings,
        qualname_prefix: tuple[str, ...] = (),
    ) -> None:
        for child in nodes:
            _visit(child, bindings, truth_bindings, qualname_prefix)

    def _visit(
        node: ast.AST,
        bindings: _ImportlibStaticBindings,
        truth_bindings: _StaticTruthBindings,
        qualname_prefix: tuple[str, ...] = (),
    ) -> None:
        nonlocal needs_string_templatelib, needs_typing
        if isinstance(node, ast.Module):
            _visit_many(node.body, bindings, truth_bindings)
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            _record_import_statement(node, bindings, truth_bindings)
            return
        if isinstance(node, ast.Assign):
            _visit(node.value, bindings, truth_bindings, qualname_prefix)
            _visit_many(node.targets, bindings, truth_bindings, qualname_prefix)
            for target in node.targets:
                bindings.record_rebinding_target(target)
                truth_bindings.record_assignment_target(target, node.value)
            return
        if isinstance(node, ast.AnnAssign):
            _visit(node.annotation, bindings, truth_bindings, qualname_prefix)
            if node.value is not None:
                _visit(node.value, bindings, truth_bindings, qualname_prefix)
            _visit(node.target, bindings, truth_bindings, qualname_prefix)
            bindings.record_rebinding_target(node.target)
            if node.value is not None:
                truth_bindings.record_assignment_target(node.target, node.value)
            else:
                truth_bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.AugAssign):
            _visit(node.target, bindings, truth_bindings, qualname_prefix)
            _visit(node.value, bindings, truth_bindings, qualname_prefix)
            bindings.record_rebinding_target(node.target)
            truth_bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.Delete):
            _visit_many(node.targets, bindings, truth_bindings, qualname_prefix)
            for target in node.targets:
                bindings.record_rebinding_target(target)
                truth_bindings.record_rebinding_target(target)
            return
        if isinstance(node, ast.If):
            _visit(node.test, bindings, truth_bindings, qualname_prefix)
            static_branch = truth_bindings.static_if_live_branch(node)
            if static_branch is not None:
                _visit_many(static_branch, bindings, truth_bindings, qualname_prefix)
            else:
                _visit_many(node.body, bindings, truth_bindings, qualname_prefix)
                _visit_many(node.orelse, bindings, truth_bindings, qualname_prefix)
            return
        if isinstance(node, ast.NamedExpr):
            _visit(node.value, bindings, truth_bindings, qualname_prefix)
            _visit(node.target, bindings, truth_bindings, qualname_prefix)
            bindings.record_rebinding_target(node.target)
            truth_bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.BoolOp):
            for value in _statically_executed_boolop_values(
                node,
                type_checking_names=truth_bindings.type_checking_names,
                type_checking_module_aliases=(
                    truth_bindings.type_checking_module_aliases
                ),
            ):
                _visit(value, bindings, truth_bindings, qualname_prefix)
            return
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if getattr(node, "type_params", None):
                needs_typing = True
            if isinstance(node, ast.ClassDef):
                _visit_many(
                    node.decorator_list, bindings, truth_bindings, qualname_prefix
                )
                _visit_many(node.bases, bindings, truth_bindings, qualname_prefix)
                _visit_many(
                    [keyword.value for keyword in node.keywords if keyword.value],
                    bindings,
                    truth_bindings,
                    qualname_prefix,
                )
                _visit_many(
                    getattr(node, "type_params", ()),
                    bindings,
                    truth_bindings,
                    qualname_prefix,
                )
                class_bindings = bindings.fork()
                class_truth_bindings = truth_bindings.fork()
                class_prefix = _qualified_child(qualname_prefix, node.name)
                _visit_many(
                    node.body, class_bindings, class_truth_bindings, class_prefix
                )
                bindings.module_import_module_mutated |= (
                    class_bindings.module_import_module_mutated
                )
                bindings.module_util_mutated |= class_bindings.module_util_mutated
                return
            _visit_many(node.decorator_list, bindings, truth_bindings, qualname_prefix)
            _visit_many(
                list(node.args.defaults), bindings, truth_bindings, qualname_prefix
            )
            _visit_many(
                [default for default in node.args.kw_defaults if default is not None],
                bindings,
                truth_bindings,
                qualname_prefix,
            )
            for arg in (
                list(node.args.posonlyargs)
                + list(node.args.args)
                + list(node.args.kwonlyargs)
            ):
                if arg.annotation is not None:
                    _visit(arg.annotation, bindings, truth_bindings, qualname_prefix)
            if node.args.vararg is not None and node.args.vararg.annotation is not None:
                _visit(
                    node.args.vararg.annotation,
                    bindings,
                    truth_bindings,
                    qualname_prefix,
                )
            if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
                _visit(
                    node.args.kwarg.annotation,
                    bindings,
                    truth_bindings,
                    qualname_prefix,
                )
            if node.returns is not None:
                _visit(node.returns, bindings, truth_bindings, qualname_prefix)
            _visit_many(
                getattr(node, "type_params", ()),
                bindings,
                truth_bindings,
                qualname_prefix,
            )
            function_qualname = ".".join(_qualified_child(qualname_prefix, node.name))
            if (
                import_scan_mode == "full"
                or function_qualname in selected_static_helper_qualnames
            ):
                function_bindings = bindings.fork()
                function_truth_bindings = truth_bindings.fork()
                for name in _function_parameter_names(node):
                    function_bindings.invalidate_name(name)
                    function_truth_bindings.record_rebinding_target(ast.Name(id=name))
                function_prefix = _qualified_child(qualname_prefix, node.name)
                _visit_many(
                    node.body, function_bindings, function_truth_bindings, function_prefix
                )
            return
        if isinstance(node, ast.Lambda):
            _visit_many(
                list(node.args.defaults), bindings, truth_bindings, qualname_prefix
            )
            _visit_many(
                [default for default in node.args.kw_defaults if default is not None],
                bindings,
                truth_bindings,
                qualname_prefix,
            )
            if import_scan_mode == "full":
                lambda_bindings = bindings.fork()
                lambda_truth_bindings = truth_bindings.fork()
                for name in _function_parameter_names(node):
                    lambda_bindings.invalidate_name(name)
                    lambda_truth_bindings.record_rebinding_target(ast.Name(id=name))
                _visit(node.body, lambda_bindings, lambda_truth_bindings)
            return
        if type_alias_cls is not None and isinstance(node, type_alias_cls):
            needs_typing = True
            return
        if template_str_cls is not None and isinstance(node, template_str_cls):
            # PEP 750 t-strings desugar to string.templatelib.{Template,Interpolation}
            # at the molt frontend layer, so the import must be reflected in the
            # module graph closure even though no `import` statement appears.
            needs_string_templatelib = True
            return
        if isinstance(node, ast.Call) and node.args:
            _collect_import_call(node, bindings)
        for child in ast.iter_child_nodes(node):
            _visit(child, bindings, truth_bindings, qualname_prefix)

    _visit(tree, _ImportlibStaticBindings(), _StaticTruthBindings())
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
        if node.level:
            if module_name is None:
                continue
            resolved_module = _resolve_relative_import(
                module_name,
                is_package=is_package,
                level=node.level,
                module=node.module,
            )
        else:
            resolved_module = node.module
        if not resolved_module:
            continue
        for alias in node.names:
            if alias.name == "*":
                continue
            local_name = alias.asname or alias.name
            _register_binding(local_name, f"{resolved_module}.{alias.name}")

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
) -> tuple[str, ...]:
    _validate_import_scan_mode(import_scan_mode)
    (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    ) = _infer_module_overrides(tree)
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
        resolved: str | None
        if node.level:
            if not module_name:
                continue
            resolved = _resolve_relative_import(
                module_name,
                is_package=is_package,
                level=node.level,
                module=node.module,
                package_override=package_override,
                package_override_set=package_override_set,
                spec_override=spec_override,
                spec_override_set=spec_override_set,
                spec_override_is_package=spec_override_is_package,
            )
        else:
            resolved = node.module
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
