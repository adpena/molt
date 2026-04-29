"""Low-level tokenizer helpers used by `tokenize`.

CPython exposes this as a C extension module that the public `tokenize`
Python module imports the token-type constants and TokenInfo from. Molt's
`tokenize` module already implements a working tokenizer against runtime
intrinsics, so `_tokenize` re-exports the same names so any third-party
code that imports `_tokenize` directly gets the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


from tokenize import (
    COMMENT,
    ENCODING,
    ENDMARKER,
    NAME,
    NEWLINE,
    NL,
    NUMBER,
    OP,
    TokenInfo,
    tokenize,
)


__all__ = [
    "COMMENT",
    "ENCODING",
    "ENDMARKER",
    "NAME",
    "NEWLINE",
    "NL",
    "NUMBER",
    "OP",
    "TokenInfo",
    "tokenize",
]


globals().pop("_require_intrinsic", None)
