from __future__ import annotations

import ast
from collections.abc import Collection

DEFAULT_TYPE_CHECKING_NAMES = frozenset({"TYPE_CHECKING"})
DEFAULT_TYPE_CHECKING_MODULE_ALIASES = frozenset({"typing", "typing_extensions"})


def is_type_checking_test(
    expr: ast.expr,
    *,
    type_checking_names: Collection[str] = DEFAULT_TYPE_CHECKING_NAMES,
    type_checking_module_aliases: Collection[
        str
    ] = DEFAULT_TYPE_CHECKING_MODULE_ALIASES,
) -> bool:
    if isinstance(expr, ast.Name):
        return expr.id in type_checking_names
    if isinstance(expr, ast.Attribute):
        if expr.attr != "TYPE_CHECKING":
            return False
        if isinstance(expr.value, ast.Name):
            return expr.value.id in type_checking_module_aliases
    return False


def static_test_truthiness(
    expr: ast.expr,
    *,
    type_checking_names: Collection[str] = DEFAULT_TYPE_CHECKING_NAMES,
    type_checking_module_aliases: Collection[
        str
    ] = DEFAULT_TYPE_CHECKING_MODULE_ALIASES,
    target_sys_platform: str | None = None,
    sys_platform_module_aliases: Collection[str] = (),
) -> bool | None:
    """Return the compile-time truth value of an if/while test, or None.

    Molt compiles executable code, not type-checker-only paths. A
    TYPE_CHECKING guard is therefore statically false in every compiler analysis
    that decides emitted code, import closure, or binary feature reachability.
    """
    if is_type_checking_test(
        expr,
        type_checking_names=type_checking_names,
        type_checking_module_aliases=type_checking_module_aliases,
    ):
        return False
    if isinstance(expr, ast.Constant):
        return bool(expr.value)
    if isinstance(expr, (ast.Tuple, ast.List, ast.Set)):
        return bool(expr.elts)
    if isinstance(expr, ast.Dict):
        return bool(expr.keys)
    if isinstance(expr, ast.UnaryOp) and isinstance(expr.op, ast.Not):
        operand_truth = static_test_truthiness(
            expr.operand,
            type_checking_names=type_checking_names,
            type_checking_module_aliases=type_checking_module_aliases,
            target_sys_platform=target_sys_platform,
            sys_platform_module_aliases=sys_platform_module_aliases,
        )
        if operand_truth is not None:
            return not operand_truth
        return None
    if isinstance(expr, ast.BoolOp):
        if isinstance(expr.op, ast.And):
            saw_unknown = False
            for value in expr.values:
                value_truth = static_test_truthiness(
                    value,
                    type_checking_names=type_checking_names,
                    type_checking_module_aliases=type_checking_module_aliases,
                    target_sys_platform=target_sys_platform,
                    sys_platform_module_aliases=sys_platform_module_aliases,
                )
                if value_truth is False:
                    return False
                if value_truth is None:
                    saw_unknown = True
            return None if saw_unknown else True
        if isinstance(expr.op, ast.Or):
            saw_unknown = False
            for value in expr.values:
                value_truth = static_test_truthiness(
                    value,
                    type_checking_names=type_checking_names,
                    type_checking_module_aliases=type_checking_module_aliases,
                    target_sys_platform=target_sys_platform,
                    sys_platform_module_aliases=sys_platform_module_aliases,
                )
                if value_truth is True:
                    return True
                if value_truth is None:
                    saw_unknown = True
            return None if saw_unknown else False
    if isinstance(expr, ast.Compare) and len(expr.ops) == 1 and len(expr.comparators) == 1:
        target_platform_truth = _static_sys_platform_compare_truth(
            expr.left,
            expr.ops[0],
            expr.comparators[0],
            target_sys_platform=target_sys_platform,
            sys_platform_module_aliases=sys_platform_module_aliases,
        )
        if target_platform_truth is not None:
            return target_platform_truth
    sys_platform_startswith_truth = _is_sys_platform_startswith_call(
        expr,
        target_sys_platform=target_sys_platform,
        sys_platform_module_aliases=sys_platform_module_aliases,
    )
    if sys_platform_startswith_truth is not None:
        return sys_platform_startswith_truth
    if (
        isinstance(expr, ast.Compare)
        and len(expr.ops) == 1
        and len(expr.comparators) == 1
        and isinstance(expr.comparators[0], ast.Constant)
        and isinstance(expr.comparators[0].value, bool)
    ):
        left_truth = static_test_truthiness(
            expr.left,
            type_checking_names=type_checking_names,
            type_checking_module_aliases=type_checking_module_aliases,
            target_sys_platform=target_sys_platform,
            sys_platform_module_aliases=sys_platform_module_aliases,
        )
        if left_truth is None:
            return None
        comparator = expr.comparators[0].value
        op = expr.ops[0]
        if isinstance(op, (ast.Eq, ast.Is)):
            return left_truth is comparator
        if isinstance(op, (ast.NotEq, ast.IsNot)):
            return left_truth is not comparator
    return None


def _is_sys_platform_expr(
    expr: ast.expr,
    *,
    sys_platform_module_aliases: Collection[str],
) -> bool:
    return (
        isinstance(expr, ast.Attribute)
        and expr.attr == "platform"
        and isinstance(expr.value, ast.Name)
        and expr.value.id in sys_platform_module_aliases
    )


def _static_string_literal(expr: ast.expr) -> str | None:
    if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
        return expr.value
    return None


def _static_string_set(expr: ast.expr) -> frozenset[str] | None:
    if not isinstance(expr, (ast.Set, ast.Tuple, ast.List)):
        return None
    values: list[str] = []
    for elt in expr.elts:
        value = _static_string_literal(elt)
        if value is None:
            return None
        values.append(value)
    return frozenset(values)


def _static_sys_platform_compare_truth(
    left: ast.expr,
    op: ast.cmpop,
    right: ast.expr,
    *,
    target_sys_platform: str | None,
    sys_platform_module_aliases: Collection[str],
) -> bool | None:
    if target_sys_platform is None:
        return None
    left_is_platform = _is_sys_platform_expr(
        left, sys_platform_module_aliases=sys_platform_module_aliases
    )
    right_is_platform = _is_sys_platform_expr(
        right, sys_platform_module_aliases=sys_platform_module_aliases
    )
    if left_is_platform == right_is_platform:
        return None
    literal_expr = right if left_is_platform else left
    literal = _static_string_literal(literal_expr)
    if literal is not None:
        if isinstance(op, ast.Eq):
            return target_sys_platform == literal
        if isinstance(op, ast.NotEq):
            return target_sys_platform != literal
        return None
    literals = _static_string_set(literal_expr)
    if literals is not None:
        if isinstance(op, ast.In):
            return target_sys_platform in literals
        if isinstance(op, ast.NotIn):
            return target_sys_platform not in literals
    return None


def _is_sys_platform_startswith_call(
    expr: ast.expr,
    *,
    target_sys_platform: str | None,
    sys_platform_module_aliases: Collection[str],
) -> bool | None:
    if target_sys_platform is None or not isinstance(expr, ast.Call):
        return None
    if expr.keywords or len(expr.args) != 1:
        return None
    func = expr.func
    if not (
        isinstance(func, ast.Attribute)
        and func.attr == "startswith"
        and _is_sys_platform_expr(
            func.value, sys_platform_module_aliases=sys_platform_module_aliases
        )
    ):
        return None
    prefix = _static_string_literal(expr.args[0])
    if prefix is None:
        return None
    return target_sys_platform.startswith(prefix)


def static_if_live_branch(
    node: ast.If,
    *,
    type_checking_names: Collection[str] = DEFAULT_TYPE_CHECKING_NAMES,
    type_checking_module_aliases: Collection[
        str
    ] = DEFAULT_TYPE_CHECKING_MODULE_ALIASES,
    target_sys_platform: str | None = None,
    sys_platform_module_aliases: Collection[str] = (),
) -> list[ast.stmt] | None:
    truth = static_test_truthiness(
        node.test,
        type_checking_names=type_checking_names,
        type_checking_module_aliases=type_checking_module_aliases,
        target_sys_platform=target_sys_platform,
        sys_platform_module_aliases=sys_platform_module_aliases,
    )
    if truth is None:
        return None
    return node.body if truth else node.orelse
