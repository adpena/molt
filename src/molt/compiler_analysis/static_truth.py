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
                )
                if value_truth is True:
                    return True
                if value_truth is None:
                    saw_unknown = True
            return None if saw_unknown else False
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


def static_if_live_branch(
    node: ast.If,
    *,
    type_checking_names: Collection[str] = DEFAULT_TYPE_CHECKING_NAMES,
    type_checking_module_aliases: Collection[
        str
    ] = DEFAULT_TYPE_CHECKING_MODULE_ALIASES,
) -> list[ast.stmt] | None:
    truth = static_test_truthiness(
        node.test,
        type_checking_names=type_checking_names,
        type_checking_module_aliases=type_checking_module_aliases,
    )
    if truth is None:
        return None
    return node.body if truth else node.orelse
