"""Low-level AST helpers used by `ast`.

CPython exposes this as a built-in module that the public `ast` Python
module imports its node-class hierarchy from. Molt's `ast` module already
defines the supported AST node classes against runtime intrinsics, so
`_ast` re-exports the same names so any third-party code that imports
`_ast` directly gets the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


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


globals().pop("_require_intrinsic", None)
