from __future__ import annotations

import ast


def is_type_checking_test(expr: ast.expr) -> bool:
    if isinstance(expr, ast.Name):
        return expr.id == "TYPE_CHECKING"
    if isinstance(expr, ast.Attribute):
        if expr.attr != "TYPE_CHECKING":
            return False
        if isinstance(expr.value, ast.Name):
            return expr.value.id in {"typing", "typing_extensions"}
    return False


def static_test_truthiness(expr: ast.expr) -> bool | None:
    """Return the compile-time truth value of an if/while test, or None.

    Molt compiles executable code, not type-checker-only paths. A
    TYPE_CHECKING guard is therefore statically false in every compiler analysis
    that decides emitted code, import closure, or binary feature reachability.
    """
    if is_type_checking_test(expr):
        return False
    if isinstance(expr, ast.Constant):
        return bool(expr.value)
    return None


def static_if_live_branch(node: ast.If) -> list[ast.stmt] | None:
    truth = static_test_truthiness(node.test)
    if truth is None:
        return None
    return node.body if truth else node.orelse
