"""Minimal `symtable` subset for Molt."""

from __future__ import annotations

import ast

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_SYMTABLE_RUNTIME_READY = _require_intrinsic(
    "molt_symtable_runtime_ready", globals()
)

_FUNCTION_NODE_TYPES = tuple(
    cls
    for cls in (
        getattr(ast, "FunctionDef", None),
        getattr(ast, "AsyncFunctionDef", None),
    )
    if cls is not None
)
_CLASS_NODE_TYPE = getattr(ast, "ClassDef", None)
_LOAD_NODE_TYPE = getattr(ast, "Load", None)
_STORE_NODE_TYPE = getattr(ast, "Store", None)
_DEL_NODE_TYPE = getattr(ast, "Del", None)


class _SymbolTable:
    def __init__(
        self,
        *,
        name: str,
        table_type: str,
        nested: bool,
        identifiers: list[str],
        children: list["_SymbolTable"],
        frees: list[str],
    ) -> None:
        self._name = name
        self._type = table_type
        self._nested = nested
        self._identifiers = identifiers
        self._children = children
        self._frees = frees

    def get_type(self) -> str:
        return self._type

    def get_identifiers(self) -> list[str]:
        return list(self._identifiers)

    def get_children(self) -> list["_SymbolTable"]:
        return list(self._children)

    def get_name(self) -> str:
        return self._name

    def is_nested(self) -> bool:
        return self._nested

    def get_frees(self) -> tuple[str, ...]:
        return tuple(self._frees)

    def get_free_vars(self) -> tuple[str, ...]:
        # Compatibility shim used by older differential tests.
        return self.get_frees()


def _collect_scope(
    body: list[ast.stmt],
) -> tuple[list[str], list[str], list[ast.FunctionDef | ast.AsyncFunctionDef]]:
    locals_out: list[str] = []
    loads_out: list[str] = []
    children: list[ast.FunctionDef | ast.AsyncFunctionDef] = []
    local_set: set[str] = set()
    load_set: set[str] = set()

    def add_local(name: str) -> None:
        if name not in local_set:
            local_set.add(name)
            locals_out.append(name)

    def add_load(name: str) -> None:
        if name not in load_set:
            load_set.add(name)
            loads_out.append(name)

    def walk(node: ast.AST) -> None:
        if _FUNCTION_NODE_TYPES and isinstance(node, _FUNCTION_NODE_TYPES):
            add_local(node.name)
            children.append(node)
            return
        if _CLASS_NODE_TYPE is not None and isinstance(node, _CLASS_NODE_TYPE):
            add_local(node.name)
            return
        if isinstance(node, ast.Name):
            if _LOAD_NODE_TYPE is not None and isinstance(node.ctx, _LOAD_NODE_TYPE):
                add_load(node.id)
            elif (
                _STORE_NODE_TYPE is not None and isinstance(node.ctx, _STORE_NODE_TYPE)
            ) or (_DEL_NODE_TYPE is not None and isinstance(node.ctx, _DEL_NODE_TYPE)):
                add_local(node.id)
        for child in ast.iter_child_nodes(node):
            walk(child)

    for stmt in body:
        walk(stmt)
    return locals_out, loads_out, children


def _function_arg_names(node: ast.FunctionDef | ast.AsyncFunctionDef) -> list[str]:
    args = node.args
    out: list[str] = []
    for item in args.posonlyargs + args.args + args.kwonlyargs:
        out.append(item.arg)
    if args.vararg is not None:
        out.append(args.vararg.arg)
    if args.kwarg is not None:
        out.append(args.kwarg.arg)
    return out


def _build_function_table(
    node: ast.FunctionDef | ast.AsyncFunctionDef,
    parent_visible: set[str],
    *,
    nested: bool,
) -> _SymbolTable:
    locals_out, loads_out, children_nodes = _collect_scope(node.body)
    for name in _function_arg_names(node):
        if name not in locals_out:
            locals_out.insert(0, name)

    local_set = set(locals_out)
    frees = [
        name for name in loads_out if name not in local_set and name in parent_visible
    ]
    visible = set(parent_visible)
    visible.update(local_set)
    children = [
        _build_function_table(child, visible, nested=True) for child in children_nodes
    ]
    return _SymbolTable(
        name=node.name,
        table_type="function",
        nested=nested,
        identifiers=locals_out,
        children=children,
        frees=frees,
    )


def symtable(code: str, filename: str, compile_type: str) -> _SymbolTable:
    _MOLT_SYMTABLE_RUNTIME_READY()
    if not isinstance(code, str):
        raise TypeError("code must be str")
    if not isinstance(filename, str):
        raise TypeError("filename must be str")
    if compile_type not in {"exec", "eval", "single"}:
        raise ValueError("compile_type must be 'exec', 'eval', or 'single'")

    tree = ast.parse(code, filename=filename, mode=compile_type)
    body = tree.body if isinstance(tree, ast.Module) else [tree]
    locals_out, _loads_out, children_nodes = _collect_scope(body)
    top_visible = set(locals_out)
    children = [
        _build_function_table(child, top_visible, nested=False)
        for child in children_nodes
    ]
    return _SymbolTable(
        name="top",
        table_type="module",
        nested=False,
        identifiers=locals_out,
        children=children,
        frees=[],
    )


__all__ = ["symtable"]
