"""Target admission for comprehension AST syntax introduced by PEP 798."""

from __future__ import annotations

import ast

COMPREHENSION_UNPACKING_MIN_VERSION: tuple[int, int] = (3, 15)


def require_comprehension_target(
    node: ast.ListComp | ast.SetComp | ast.DictComp | ast.GeneratorExp,
    target_python: tuple[int, int],
    *,
    filename: str | None = None,
) -> None:
    # CPython 3.15's feature_version parser does not gate PEP 798. The selected
    # target also owns admission for clients supplying ASTs directly.
    expanded = (
        node.value is None
        if isinstance(node, ast.DictComp)
        else isinstance(node.elt, ast.Starred)
    )
    if target_python < COMPREHENSION_UNPACKING_MIN_VERSION and expanded:
        message = (
            "dict unpacking cannot be used in dict comprehension"
            if isinstance(node, ast.DictComp)
            else "iterable unpacking cannot be used in comprehension"
        )
        raise SyntaxError(
            message,
            (
                filename,
                getattr(node, "lineno", 1),
                getattr(node, "col_offset", 0) + 1,
                None,
            ),
        )
