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


def python_pattern_capture_names(pattern: ast.pattern) -> tuple[str, ...]:
    """Return one pattern's captures in deterministic source order."""

    names: list[str] = []
    seen: set[str] = set()

    def add(name: str | None) -> None:
        if name and name != "_" and name not in seen:
            seen.add(name)
            names.append(name)

    def visit(current: ast.pattern) -> None:
        if isinstance(current, ast.MatchAs):
            if current.pattern is not None:
                visit(current.pattern)
            add(current.name)
        elif isinstance(current, ast.MatchStar):
            add(current.name)
        elif isinstance(current, ast.MatchMapping):
            for child in current.patterns:
                visit(child)
            add(current.rest)
        elif isinstance(current, ast.MatchSequence):
            for child in current.patterns:
                visit(child)
        elif isinstance(current, ast.MatchClass):
            for child in (*current.patterns, *current.kwd_patterns):
                visit(child)
        elif isinstance(current, ast.MatchOr):
            for child in current.patterns:
                visit(child)

    visit(pattern)
    return tuple(names)


__all__ = [
    "PythonSourceKey",
    "python_node_source_key",
    "python_pattern_capture_names",
]
