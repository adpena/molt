"""Pure-Python decimal arithmetic — CPython parity surface.

CPython distributes both `_decimal` (C extension) and `_pydecimal` (pure-
Python fallback). Tests and tooling sometimes pin against `_pydecimal`
explicitly to bypass the C path. Molt has a single `decimal` module
backed by runtime intrinsics; both `_decimal` and `_pydecimal` re-export
the same public surface so direct importers get the working
implementation either way.
"""

from __future__ import annotations

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
