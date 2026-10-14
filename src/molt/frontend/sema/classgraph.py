"""Static class-body analysis (doc 44 §F2b ``classgraph.py``).

Free functions over ``ast.Module`` — the ``cfg_analysis.py`` house shape. The
class-body block-exec decision and C3 linearization are computed outside the
lowering generator and are unit-testable on bare facts. Class identity is not
decided here: lowering keys every class fact by its ``ClassId``.
"""

from __future__ import annotations

import ast
from collections.abc import Hashable, Sequence

from molt.frontend.sema.result import ClassFacts


CLASS_BODY_SIMPLE_STMTS = (
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.ClassDef,
    ast.Assign,
    ast.AnnAssign,
    ast.Expr,
    ast.Pass,
)


def class_body_needs_block_exec(body: Sequence[ast.stmt]) -> bool:
    """True when a class body must execute through the mutable namespace block."""
    for stmt in body:
        if not isinstance(stmt, CLASS_BODY_SIMPLE_STMTS):
            return True
        if isinstance(stmt, ast.Assign):
            if any(not isinstance(t, ast.Name) for t in stmt.targets):
                return True
        elif isinstance(stmt, ast.AnnAssign):
            if not isinstance(stmt.target, ast.Name):
                return True
    return False


def build_class_facts(node: ast.Module) -> ClassFacts:
    """Collect the class bodies that must execute through the namespace block."""
    return ClassFacts(
        block_exec_class_nodes=frozenset(
            id(stmt)
            for stmt in ast.walk(node)
            if isinstance(stmt, ast.ClassDef) and class_body_needs_block_exec(stmt.body)
        )
    )


def c3_merge[T: Hashable](seqs: Sequence[Sequence[T]]) -> list[T] | None:
    """Merge C3 parent linearizations, returning ``None`` for inconsistency."""
    merged: list[T] = []
    working = [list(seq) for seq in seqs]
    heads = [0] * len(working)
    tail_counts: dict[T, int] = {}
    for seq in working:
        for name in seq[1:]:
            tail_counts[name] = tail_counts.get(name, 0) + 1

    while True:
        remaining = 0
        for idx, seq in enumerate(working):
            if heads[idx] < len(seq):
                remaining += 1
        if remaining == 0:
            return merged

        candidate: T | None = None
        for idx, seq in enumerate(working):
            head_idx = heads[idx]
            if head_idx >= len(seq):
                continue
            head = seq[head_idx]
            if tail_counts.get(head, 0) == 0:
                candidate = head
                break

        if candidate is None:
            return None

        merged.append(candidate)
        for idx, seq in enumerate(working):
            head_idx = heads[idx]
            if head_idx < len(seq) and seq[head_idx] == candidate:
                heads[idx] += 1
                next_head_idx = heads[idx]
                if next_head_idx < len(seq):
                    next_head = seq[next_head_idx]
                    count = tail_counts.get(next_head, 0)
                    if count <= 1:
                        tail_counts.pop(next_head, None)
                    else:
                        tail_counts[next_head] = count - 1
