"""Frame-elision legality for frontend expression substitution.

The effect lattice owns callback safety. This projection additionally requires
that every name is an explicit parameter and every accepted form has a total,
scope-independent lowering; unknown future syntax fails closed.
"""

from __future__ import annotations

import ast
from collections.abc import Collection

from molt.compiler_analysis.python_effects import expression_may_execute_python


def inline_expression_is_frame_independent(
    expression: ast.expr, parameters: Collection[str]
) -> bool:
    if expression_may_execute_python(expression):
        return False

    def supported(node: ast.expr) -> bool:
        if isinstance(node, ast.Constant):
            return True
        if isinstance(node, ast.Name):
            return isinstance(node.ctx, ast.Load) and node.id in parameters
        if isinstance(node, (ast.Tuple, ast.List)):
            return isinstance(node.ctx, ast.Load) and all(
                supported(element) for element in node.elts
            )
        return False

    return supported(expression)
