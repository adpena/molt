"""Low-level AST helpers used by `ast`.

CPython exposes this as a built-in module that the public `ast` Python
module imports its node-class hierarchy from. Molt's `ast` module already
defines the supported AST node classes against runtime intrinsics, so
`_ast` re-exports the same names so any third-party code that imports
`_ast` directly gets the working implementation.
"""

from __future__ import annotations

from ast import (
    AST,
    Add,
    Assign,
    BinOp,
    Constant,
    Expr,
    Expression,
    FunctionDef,
    Load,
    Module,
    Name,
    PyCF_ALLOW_TOP_LEVEL_AWAIT,
    Return,
    Store,
    arg,
    arguments,
)


__all__ = [
    "AST",
    "Add",
    "Assign",
    "BinOp",
    "Constant",
    "Expr",
    "Expression",
    "FunctionDef",
    "Load",
    "Module",
    "Name",
    "Return",
    "Store",
    "arg",
    "arguments",
    "PyCF_ALLOW_TOP_LEVEL_AWAIT",
]
