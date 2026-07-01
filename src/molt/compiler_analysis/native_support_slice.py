from __future__ import annotations

import ast
import copy
from collections.abc import Collection, Sequence

SupportDef = ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef
SupportBinding = SupportDef | ast.Assign | ast.AnnAssign
if hasattr(ast, "TypeAlias"):
    SupportBinding = SupportBinding | ast.TypeAlias  # type: ignore[attr-defined]


def top_level_support_defs(tree: ast.Module) -> dict[str, SupportDef]:
    return {
        stmt.name: stmt
        for stmt in tree.body
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))
    }


def _assignment_bound_names(stmt: ast.stmt) -> set[str]:
    names: set[str] = set()

    def add_target(target: ast.AST) -> None:
        if isinstance(target, ast.Name):
            names.add(target.id)
        elif isinstance(target, (ast.Tuple, ast.List)):
            for elt in target.elts:
                add_target(elt)

    if isinstance(stmt, ast.Assign):
        for target in stmt.targets:
            add_target(target)
    elif isinstance(stmt, ast.AnnAssign):
        add_target(stmt.target)
    elif hasattr(ast, "TypeAlias") and isinstance(stmt, ast.TypeAlias):
        name = stmt.name
        if isinstance(name, ast.Name):
            names.add(name.id)
    return names


def top_level_support_bindings(tree: ast.Module) -> dict[str, ast.stmt]:
    bindings: dict[str, ast.stmt] = {}
    for stmt in tree.body:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            bindings[stmt.name] = stmt
            continue
        for name in sorted(_assignment_bound_names(stmt)):
            bindings[name] = stmt
    return bindings


def used_names(nodes: Sequence[ast.AST]) -> set[str]:
    names: set[str] = set()

    class _Visitor(ast.NodeVisitor):
        def visit_Name(self, node: ast.Name) -> None:
            if isinstance(node.ctx, ast.Load):
                names.add(node.id)

    visitor = _Visitor()
    for node in nodes:
        visitor.visit(node)
    return names


def _support_runtime_node(node: ast.stmt) -> ast.stmt:
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        stripped = copy.copy(node)
        stripped.decorator_list = []
        return stripped
    return node


def import_bound_names(stmt: ast.stmt) -> set[str]:
    if isinstance(stmt, ast.Import):
        return {
            alias.asname or alias.name.split(".", 1)[0]
            for alias in stmt.names
            if alias.name != "*"
        }
    if isinstance(stmt, ast.ImportFrom):
        return {alias.asname or alias.name for alias in stmt.names if alias.name != "*"}
    return set()


def reachable_support_bindings(
    tree: ast.Module,
    roots: Collection[str],
) -> tuple[frozenset[str], tuple[str, ...]]:
    root_set = frozenset(name for name in roots if name)
    bindings = top_level_support_bindings(tree)
    missing = tuple(sorted(name for name in root_set if name not in bindings))
    if missing:
        return frozenset(), missing
    reachable = set(root_set)
    queue = list(root_set)
    while queue:
        current = queue.pop()
        node = _support_runtime_node(bindings[current])
        for ref in sorted(used_names((node,))):
            if ref not in bindings or ref in reachable:
                continue
            reachable.add(ref)
            queue.append(ref)
    return frozenset(reachable), ()


def prune_native_support_module(
    tree: ast.Module,
    roots: Collection[str],
) -> tuple[ast.Module, frozenset[str], tuple[str, ...]]:
    root_set = frozenset(name for name in roots if name)
    if not root_set:
        return tree, frozenset(), ()
    reachable, missing = reachable_support_bindings(tree, root_set)
    if missing:
        return ast.Module(body=[], type_ignores=tree.type_ignores), reachable, missing
    bindings = top_level_support_bindings(tree)
    reachable_nodes = tuple(
        _support_runtime_node(bindings[name]) for name in sorted(reachable)
    )
    needed_imports = used_names(reachable_nodes)
    body: list[ast.stmt] = []
    for stmt in tree.body:
        if isinstance(stmt, ast.ImportFrom) and stmt.module == "__future__":
            body.append(stmt)
            continue
        if isinstance(stmt, (ast.Import, ast.ImportFrom)):
            if import_bound_names(stmt).intersection(needed_imports):
                body.append(stmt)
            continue
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if stmt.name in reachable:
                body.append(_support_runtime_node(stmt))
            continue
        bound_names = _assignment_bound_names(stmt)
        if bound_names and bound_names.intersection(reachable):
            body.append(stmt)
    return ast.Module(body=body, type_ignores=tree.type_ignores), reachable, ()
