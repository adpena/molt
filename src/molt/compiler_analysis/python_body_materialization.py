"""Source-owned proofs for publishing an unread callable without its body."""

from __future__ import annotations

import ast
from typing import Literal

from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    python_node_source_key,
)

BodyMaterializationMode = Literal["eager", "demanded"]


def flat_callable_declarations(
    tree: ast.Module,
) -> tuple[tuple[PythonSourceKey, str], ...]:
    """Collect candidate declarations with no deferred declarations.

    This first selector also retains bodies that could affect the integrated
    control-flow proof of another function or allocate a function cell. A
    body with only local/explicit global assignments and plain returns has
    neither of those effects. Its writer facts still come from source.
    """
    declarations = []
    for node in tree.body:
        if (
            not isinstance(node, ast.FunctionDef)
            or node.decorator_list
            or not all(_has_only_plain_bindings(statement) for statement in node.body)
        ):
            continue
        declarations.append((python_node_source_key(node), node.name))
    return tuple(sorted(declarations))


def _has_only_plain_bindings(statement: ast.stmt) -> bool:
    match statement:
        case ast.Pass() | ast.Global():
            return True
        case ast.Return(value=None | ast.Constant() | ast.Name()):
            return True
        case ast.Expr(value=ast.Constant()):
            return True
        case ast.Assign(targets=targets, value=ast.Constant() | ast.Name()):
            return all(isinstance(target, ast.Name) for target in targets)
        case _:
            return False
