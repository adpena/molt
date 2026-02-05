"""Decimal shim for Molt backed by Rust intrinsics."""

from __future__ import annotations

import builtins as _builtins
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic


# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete Decimal API parity (arithmetic ops, exp/log/pow/sqrt, context quantize/signals edge cases, NaN payloads, and formatting helpers).

_MOLT_DECIMAL_CONTEXT_NEW = _require_intrinsic("molt_decimal_context_new", globals())
_MOLT_DECIMAL_CONTEXT_GET_CURRENT = _require_intrinsic(
    "molt_decimal_context_get_current", globals()
)
_MOLT_DECIMAL_CONTEXT_SET_CURRENT = _require_intrinsic(
    "molt_decimal_context_set_current", globals()
)
_MOLT_DECIMAL_CONTEXT_COPY = _require_intrinsic("molt_decimal_context_copy", globals())
_MOLT_DECIMAL_CONTEXT_DROP = _require_intrinsic("molt_decimal_context_drop", globals())
_MOLT_DECIMAL_CONTEXT_GET_PREC = _require_intrinsic(
    "molt_decimal_context_get_prec", globals()
)
_MOLT_DECIMAL_CONTEXT_SET_PREC = _require_intrinsic(
    "molt_decimal_context_set_prec", globals()
)
_MOLT_DECIMAL_CONTEXT_GET_ROUNDING = _require_intrinsic(
    "molt_decimal_context_get_rounding", globals()
)
_MOLT_DECIMAL_CONTEXT_SET_ROUNDING = _require_intrinsic(
    "molt_decimal_context_set_rounding", globals()
)
_MOLT_DECIMAL_CONTEXT_CLEAR_FLAGS = _require_intrinsic(
    "molt_decimal_context_clear_flags", globals()
)
_MOLT_DECIMAL_CONTEXT_GET_FLAG = _require_intrinsic(
    "molt_decimal_context_get_flag", globals()
)
_MOLT_DECIMAL_CONTEXT_SET_FLAG = _require_intrinsic(
    "molt_decimal_context_set_flag", globals()
)
_MOLT_DECIMAL_CONTEXT_GET_TRAP = _require_intrinsic(
    "molt_decimal_context_get_trap", globals()
)
_MOLT_DECIMAL_CONTEXT_SET_TRAP = _require_intrinsic(
    "molt_decimal_context_set_trap", globals()
)

_MOLT_DECIMAL_FROM_STR = _require_intrinsic("molt_decimal_from_str", globals())
_MOLT_DECIMAL_FROM_INT = _require_intrinsic("molt_decimal_from_int", globals())
_MOLT_DECIMAL_CLONE = _require_intrinsic("molt_decimal_clone", globals())
_MOLT_DECIMAL_DROP = _require_intrinsic("molt_decimal_drop", globals())
_MOLT_DECIMAL_TO_STRING = _require_intrinsic("molt_decimal_to_string", globals())
_MOLT_DECIMAL_AS_TUPLE = _require_intrinsic("molt_decimal_as_tuple", globals())
_MOLT_DECIMAL_TO_FLOAT = _require_intrinsic("molt_decimal_to_float", globals())
_MOLT_DECIMAL_DIV = _require_intrinsic("molt_decimal_div", globals())
_MOLT_DECIMAL_QUANTIZE = _require_intrinsic("molt_decimal_quantize", globals())
_MOLT_DECIMAL_COMPARE = _require_intrinsic("molt_decimal_compare", globals())
_MOLT_DECIMAL_COMPARE_TOTAL = _require_intrinsic("molt_decimal_compare_total", globals())
_MOLT_DECIMAL_NORMALIZE = _require_intrinsic("molt_decimal_normalize", globals())
_MOLT_DECIMAL_EXP = _require_intrinsic("molt_decimal_exp", globals())

# mpdecimal status flags (subset used in tests)
_MPD_CLAMPED = 0x00000001
_MPD_CONVERSION_SYNTAX = 0x00000002
_MPD_DIVISION_BY_ZERO = 0x00000004
_MPD_DIVISION_IMPOSSIBLE = 0x00000008
_MPD_DIVISION_UNDEFINED = 0x00000010
_MPD_FPU_ERROR = 0x00000020
_MPD_INEXACT = 0x00000040
_MPD_INVALID_CONTEXT = 0x00000080
_MPD_INVALID_OPERATION = 0x00000100
_MPD_MALLOC_ERROR = 0x00000200
_MPD_NOT_IMPLEMENTED = 0x00000400
_MPD_OVERFLOW = 0x00000800
_MPD_ROUNDED = 0x00001000
_MPD_SUBNORMAL = 0x00002000
_MPD_UNDERFLOW = 0x00004000


class DecimalException(ArithmeticError):
    pass


class InvalidOperation(DecimalException):
    pass


class DivisionByZero(DecimalException, ZeroDivisionError):
    pass


class Inexact(DecimalException):
    pass


class Overflow(DecimalException):
    pass


class Underflow(DecimalException):
    pass


class Subnormal(DecimalException):
    pass


class Rounded(DecimalException):
    pass


class Clamped(DecimalException):
    pass


class ConversionSyntax(InvalidOperation):
    pass


class DivisionImpossible(InvalidOperation):
    pass


class DivisionUndefined(InvalidOperation):
    pass


class InvalidContext(InvalidOperation):
    pass


class FloatOperation(DecimalException):
    pass


_SIGNAL_MAP = {
    InvalidOperation: _MPD_INVALID_OPERATION,
    ConversionSyntax: _MPD_CONVERSION_SYNTAX,
    DivisionByZero: _MPD_DIVISION_BY_ZERO,
    DivisionImpossible: _MPD_DIVISION_IMPOSSIBLE,
    DivisionUndefined: _MPD_DIVISION_UNDEFINED,
    InvalidContext: _MPD_INVALID_CONTEXT,
    Inexact: _MPD_INEXACT,
    Overflow: _MPD_OVERFLOW,
    Underflow: _MPD_UNDERFLOW,
    Subnormal: _MPD_SUBNORMAL,
    Rounded: _MPD_ROUNDED,
    Clamped: _MPD_CLAMPED,
    FloatOperation: _MPD_FPU_ERROR,
}


for _name, _cls in {
    "DecimalException": DecimalException,
    "InvalidOperation": InvalidOperation,
    "DivisionByZero": DivisionByZero,
    "Inexact": Inexact,
    "Overflow": Overflow,
    "Underflow": Underflow,
    "Subnormal": Subnormal,
    "Rounded": Rounded,
    "Clamped": Clamped,
    "ConversionSyntax": ConversionSyntax,
    "DivisionImpossible": DivisionImpossible,
    "DivisionUndefined": DivisionUndefined,
    "InvalidContext": InvalidContext,
    "FloatOperation": FloatOperation,
}.items():
    setattr(_builtins, _name, _cls)


ROUND_UP = "ROUND_UP"
ROUND_DOWN = "ROUND_DOWN"
ROUND_CEILING = "ROUND_CEILING"
ROUND_FLOOR = "ROUND_FLOOR"
ROUND_HALF_UP = "ROUND_HALF_UP"
ROUND_HALF_DOWN = "ROUND_HALF_DOWN"
ROUND_HALF_EVEN = "ROUND_HALF_EVEN"
ROUND_05UP = "ROUND_05UP"

_ROUNDING_NAME_TO_ID = {
    ROUND_UP: 0,
    ROUND_DOWN: 1,
    ROUND_CEILING: 2,
    ROUND_FLOOR: 3,
    ROUND_HALF_UP: 4,
    ROUND_HALF_DOWN: 5,
    ROUND_HALF_EVEN: 6,
    ROUND_05UP: 7,
}

_ROUNDING_ID_TO_NAME = {value: key for key, value in _ROUNDING_NAME_TO_ID.items()}


class DecimalTuple(tuple):
    __slots__ = ()

    def __new__(cls, sign: int, digits: tuple[int, ...], exponent: int | str):
        return tuple.__new__(cls, (sign, digits, exponent))

    def __repr__(self) -> str:
        return (
            "DecimalTuple(sign="
            + repr(self[0])
            + ", digits="
            + repr(self[1])
            + ", exponent="
            + repr(self[2])
            + ")"
        )

    def __str__(self) -> str:
        return self.__repr__()

    @property
    def sign(self) -> int:
        return int(self[0])

    @property
    def digits(self) -> tuple[int, ...]:
        return tuple(self[1])

    @property
    def exponent(self) -> int | str:
        return self[2]


class _SignalDict:
    __slots__ = ("_ctx", "_is_trap")

    def __init__(self, ctx: "Context", is_trap: bool) -> None:
        self._ctx = ctx
        self._is_trap = is_trap

    def __getitem__(self, key: type[BaseException]) -> bool:
        code = _signal_code(key)
        if self._is_trap:
            return bool(_MOLT_DECIMAL_CONTEXT_GET_TRAP(self._ctx._handle, code))
        return bool(_MOLT_DECIMAL_CONTEXT_GET_FLAG(self._ctx._handle, code))

    def __setitem__(self, key: type[BaseException], value: bool) -> None:
        code = _signal_code(key)
        if self._is_trap:
            _MOLT_DECIMAL_CONTEXT_SET_TRAP(self._ctx._handle, code, bool(value))
        else:
            _MOLT_DECIMAL_CONTEXT_SET_FLAG(self._ctx._handle, code, bool(value))

    def __repr__(self) -> str:
        kind = "traps" if self._is_trap else "flags"
        return f"<decimal.SignalDict {kind}>"


class Context:
    __slots__ = ("_handle", "traps", "flags")

    def __init__(self, _handle: object | None = None) -> None:
        self._handle = _handle if _handle is not None else _MOLT_DECIMAL_CONTEXT_NEW()
        self.traps = _SignalDict(self, True)
        self.flags = _SignalDict(self, False)

    def copy(self) -> "Context":
        return Context(_MOLT_DECIMAL_CONTEXT_COPY(self._handle))

    def clear_flags(self) -> None:
        _MOLT_DECIMAL_CONTEXT_CLEAR_FLAGS(self._handle)

    @property
    def prec(self) -> int:
        return int(_MOLT_DECIMAL_CONTEXT_GET_PREC(self._handle))

    @prec.setter
    def prec(self, value: int) -> None:
        _MOLT_DECIMAL_CONTEXT_SET_PREC(self._handle, int(value))

    @property
    def rounding(self) -> str:
        rid = int(_MOLT_DECIMAL_CONTEXT_GET_ROUNDING(self._handle))
        return _ROUNDING_ID_TO_NAME.get(rid, ROUND_HALF_EVEN)

    @rounding.setter
    def rounding(self, value: str) -> None:
        rid = _ROUNDING_NAME_TO_ID.get(value)
        if rid is None:
            raise ValueError("invalid rounding mode")
        _MOLT_DECIMAL_CONTEXT_SET_ROUNDING(self._handle, rid)

    def create_decimal(self, value: object) -> "Decimal":
        return Decimal(value, context=self)

    def divide(self, a: "Decimal", b: "Decimal") -> "Decimal":
        return _decimal_from_handle(_MOLT_DECIMAL_DIV(self._handle, a._handle, b._handle))

    def __del__(self) -> None:
        try:
            _MOLT_DECIMAL_CONTEXT_DROP(self._handle)
        except Exception:
            pass


class Decimal:
    __slots__ = ("_handle",)

    def __new__(cls, value: object = 0, context: Context | None = None) -> "Decimal":
        handle = _decimal_handle(value, context)
        self = super().__new__(cls)
        self._handle = handle
        return self

    def __repr__(self) -> str:
        return f"Decimal('{self}')"

    def __str__(self) -> str:
        return str(_MOLT_DECIMAL_TO_STRING(self._handle))

    def __float__(self) -> float:
        return float(_MOLT_DECIMAL_TO_FLOAT(self._handle))

    def as_tuple(self) -> DecimalTuple:
        sign, digits, exponent = _MOLT_DECIMAL_AS_TUPLE(self._handle)
        return DecimalTuple(int(sign), tuple(digits), exponent)

    def normalize(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_NORMALIZE(ctx, self._handle))
        )

    def quantize(self, exp: "Decimal") -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_QUANTIZE(ctx, self._handle, exp._handle)
            )
        )

    def compare(self, other: "Decimal") -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
            )
        )

    def compare_total(self, other: "Decimal") -> "Decimal":
        return _decimal_from_handle(_MOLT_DECIMAL_COMPARE_TOTAL(self._handle, other._handle))

    def exp(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_EXP(ctx, self._handle))
        )

    def __truediv__(self, other: "Decimal") -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_DIV(ctx, self._handle, other._handle)
            )
        )

    def __del__(self) -> None:
        try:
            _MOLT_DECIMAL_DROP(self._handle)
        except Exception:
            pass


def _signal_code(key: type[BaseException]) -> int:
    code = _SIGNAL_MAP.get(key)
    if code is None:
        raise KeyError(key)
    return code


def _with_current_context(func: Any) -> Any:
    ctx_handle = _MOLT_DECIMAL_CONTEXT_GET_CURRENT()
    try:
        return func(ctx_handle)
    finally:
        _MOLT_DECIMAL_CONTEXT_DROP(ctx_handle)


def _decimal_handle(value: object, context: Context | None) -> object:
    if isinstance(context, Context):
        ctx_handle = context._handle
        return _decimal_handle_with_ctx(value, ctx_handle)
    ctx_handle = _MOLT_DECIMAL_CONTEXT_GET_CURRENT()
    try:
        return _decimal_handle_with_ctx(value, ctx_handle)
    finally:
        _MOLT_DECIMAL_CONTEXT_DROP(ctx_handle)


def _decimal_handle_with_ctx(value: object, ctx_handle: object) -> object:
    if isinstance(value, Decimal):
        return _MOLT_DECIMAL_CLONE(value._handle)
    if isinstance(value, str):
        return _MOLT_DECIMAL_FROM_STR(ctx_handle, value)
    if isinstance(value, int):
        return _MOLT_DECIMAL_FROM_INT(ctx_handle, value)
    raise TypeError("decimal value must be int or str")


def _decimal_from_handle(handle: object) -> Decimal:
    obj = object.__new__(Decimal)
    obj._handle = handle
    return obj


def getcontext() -> Context:
    return Context(_MOLT_DECIMAL_CONTEXT_GET_CURRENT())


def setcontext(ctx: Context) -> None:
    if not isinstance(ctx, Context):
        raise TypeError("context must be a decimal.Context")
    prev = _MOLT_DECIMAL_CONTEXT_SET_CURRENT(ctx._handle)
    if prev is not None:
        _MOLT_DECIMAL_CONTEXT_DROP(prev)


def localcontext(ctx: Context | None = None) -> "_LocalContext":
    if ctx is not None and not isinstance(ctx, Context):
        raise TypeError("context must be a decimal.Context")
    return _LocalContext(ctx)


class _LocalContext:
    __slots__ = ("_ctx", "_saved")

    def __init__(self, ctx: Context | None) -> None:
        if ctx is None:
            ctx = getcontext().copy()
        else:
            ctx = ctx.copy()
        self._ctx = ctx
        self._saved = None

    def __enter__(self) -> Context:
        self._saved = _MOLT_DECIMAL_CONTEXT_SET_CURRENT(self._ctx._handle)
        return self._ctx

    def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
        if self._saved is not None:
            prev = _MOLT_DECIMAL_CONTEXT_SET_CURRENT(self._saved)
            _MOLT_DECIMAL_CONTEXT_DROP(self._saved)
            self._saved = None
            if prev is not None:
                _MOLT_DECIMAL_CONTEXT_DROP(prev)
        return False


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
