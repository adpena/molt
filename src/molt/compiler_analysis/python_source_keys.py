"""Canonical stable source identity for parsed and synthetic Python AST nodes."""

from __future__ import annotations

import ast
from typing import TypeAlias


PythonSourceKey: TypeAlias = tuple[int, int, int, int, str]


def python_node_source_key(node: ast.AST) -> PythonSourceKey:
    """Return a stable key, tolerating CPython's ``None`` synthetic end spans."""
    lineno = getattr(node, "lineno", None)
    col_offset = getattr(node, "col_offset", None)
    start_line = int(lineno) if lineno is not None else 0
    start_column = int(col_offset) if col_offset is not None else 0
    end_lineno = getattr(node, "end_lineno", None)
    end_col_offset = getattr(node, "end_col_offset", None)
    return (
        start_line,
        start_column,
        int(end_lineno) if end_lineno is not None else start_line,
        int(end_col_offset) if end_col_offset is not None else start_column,
        type(node).__name__,
    )


__all__ = ["PythonSourceKey", "python_node_source_key"]
