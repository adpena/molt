"""Low-level decimal helpers used by `decimal`.

CPython exposes this as a C extension module; the public `decimal` Python
module imports `Decimal`, `Context`, and the exception hierarchy from it.
Molt's `decimal` module already implements the full Decimal/Context/
DecimalException surface against runtime intrinsics, so `_decimal` simply
re-exports the same names so any third-party code that imports `_decimal`
directly gets the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


from decimal import (
    Clamped,
    Context,
    ConversionSyntax,
    Decimal,
    DecimalException,
    DecimalTuple,
    DivisionByZero,
    DivisionImpossible,
    DivisionUndefined,
    FloatOperation,
    Inexact,
    InvalidContext,
    InvalidOperation,
    Overflow,
    ROUND_05UP,
    ROUND_CEILING,
    ROUND_DOWN,
    ROUND_FLOOR,
    ROUND_HALF_DOWN,
    ROUND_HALF_EVEN,
    ROUND_HALF_UP,
    ROUND_UP,
    Rounded,
    Subnormal,
    Underflow,
    getcontext,
    localcontext,
    setcontext,
)


__all__ = [
    "Decimal",
    "DecimalTuple",
    "Context",
    "DecimalException",
    "InvalidOperation",
    "DivisionByZero",
    "Inexact",
    "Overflow",
    "Underflow",
    "Subnormal",
    "Rounded",
    "Clamped",
    "ConversionSyntax",
    "DivisionImpossible",
    "DivisionUndefined",
    "InvalidContext",
    "FloatOperation",
    "ROUND_UP",
    "ROUND_DOWN",
    "ROUND_CEILING",
    "ROUND_FLOOR",
    "ROUND_HALF_UP",
    "ROUND_HALF_DOWN",
    "ROUND_HALF_EVEN",
    "ROUND_05UP",
    "getcontext",
    "setcontext",
    "localcontext",
]


globals().pop("_require_intrinsic", None)
