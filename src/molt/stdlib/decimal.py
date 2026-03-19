"""Decimal shim for Molt backed by Rust intrinsics."""

from __future__ import annotations

import builtins as _builtins
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic


# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P3, status:partial): remaining Decimal edge cases (NaN payload propagation, context-aware signal routing, __format__ spec).

_MOLT_DECIMAL_CONTEXT_NEW = _require_intrinsic("molt_decimal_context_new")
_MOLT_DECIMAL_CONTEXT_GET_CURRENT = _require_intrinsic(
    "molt_decimal_context_get_current")
_MOLT_DECIMAL_CONTEXT_SET_CURRENT = _require_intrinsic(
    "molt_decimal_context_set_current")
_MOLT_DECIMAL_CONTEXT_COPY = _require_intrinsic("molt_decimal_context_copy")
_MOLT_DECIMAL_CONTEXT_DROP = _require_intrinsic("molt_decimal_context_drop")
_MOLT_DECIMAL_CONTEXT_GET_PREC = _require_intrinsic(
    "molt_decimal_context_get_prec")
_MOLT_DECIMAL_CONTEXT_SET_PREC = _require_intrinsic(
    "molt_decimal_context_set_prec")
_MOLT_DECIMAL_CONTEXT_GET_ROUNDING = _require_intrinsic(
    "molt_decimal_context_get_rounding")
_MOLT_DECIMAL_CONTEXT_SET_ROUNDING = _require_intrinsic(
    "molt_decimal_context_set_rounding")
_MOLT_DECIMAL_CONTEXT_CLEAR_FLAGS = _require_intrinsic(
    "molt_decimal_context_clear_flags")
_MOLT_DECIMAL_CONTEXT_GET_FLAG = _require_intrinsic(
    "molt_decimal_context_get_flag")
_MOLT_DECIMAL_CONTEXT_SET_FLAG = _require_intrinsic(
    "molt_decimal_context_set_flag")
_MOLT_DECIMAL_CONTEXT_GET_TRAP = _require_intrinsic(
    "molt_decimal_context_get_trap")
_MOLT_DECIMAL_CONTEXT_SET_TRAP = _require_intrinsic(
    "molt_decimal_context_set_trap")

_MOLT_DECIMAL_FROM_STR = _require_intrinsic("molt_decimal_from_str")
_MOLT_DECIMAL_FROM_INT = _require_intrinsic("molt_decimal_from_int")
_MOLT_DECIMAL_CLONE = _require_intrinsic("molt_decimal_clone")
_MOLT_DECIMAL_DROP = _require_intrinsic("molt_decimal_drop")
_MOLT_DECIMAL_TO_STRING = _require_intrinsic("molt_decimal_to_string")
_MOLT_DECIMAL_AS_TUPLE = _require_intrinsic("molt_decimal_as_tuple")
_MOLT_DECIMAL_TO_FLOAT = _require_intrinsic("molt_decimal_to_float")
_MOLT_DECIMAL_DIV = _require_intrinsic("molt_decimal_div")
_MOLT_DECIMAL_QUANTIZE = _require_intrinsic("molt_decimal_quantize")
_MOLT_DECIMAL_COMPARE = _require_intrinsic("molt_decimal_compare")
_MOLT_DECIMAL_COMPARE_TOTAL = _require_intrinsic(
    "molt_decimal_compare_total")
_MOLT_DECIMAL_NORMALIZE = _require_intrinsic("molt_decimal_normalize")
_MOLT_DECIMAL_EXP = _require_intrinsic("molt_decimal_exp")

_MOLT_DECIMAL_ADD = _require_intrinsic("molt_decimal_add")
_MOLT_DECIMAL_SUB = _require_intrinsic("molt_decimal_sub")
_MOLT_DECIMAL_MUL = _require_intrinsic("molt_decimal_mul")
_MOLT_DECIMAL_MOD = _require_intrinsic("molt_decimal_mod")
_MOLT_DECIMAL_FLOORDIV = _require_intrinsic("molt_decimal_floordiv")
_MOLT_DECIMAL_POW = _require_intrinsic("molt_decimal_pow")
_MOLT_DECIMAL_ABS = _require_intrinsic("molt_decimal_abs")
_MOLT_DECIMAL_NEG = _require_intrinsic("molt_decimal_neg")
_MOLT_DECIMAL_POS = _require_intrinsic("molt_decimal_pos")
_MOLT_DECIMAL_SQRT = _require_intrinsic("molt_decimal_sqrt")
_MOLT_DECIMAL_LN = _require_intrinsic("molt_decimal_ln")
_MOLT_DECIMAL_LOG10 = _require_intrinsic("molt_decimal_log10")
_MOLT_DECIMAL_FMA = _require_intrinsic("molt_decimal_fma")
_MOLT_DECIMAL_MAX = _require_intrinsic("molt_decimal_max")
_MOLT_DECIMAL_MIN = _require_intrinsic("molt_decimal_min")
_MOLT_DECIMAL_REMAINDER_NEAR = _require_intrinsic(
    "molt_decimal_remainder_near")
_MOLT_DECIMAL_SCALEB = _require_intrinsic("molt_decimal_scaleb")
_MOLT_DECIMAL_NEXT_MINUS = _require_intrinsic("molt_decimal_next_minus")
_MOLT_DECIMAL_NEXT_PLUS = _require_intrinsic("molt_decimal_next_plus")
_MOLT_DECIMAL_NUMBER_CLASS = _require_intrinsic("molt_decimal_number_class")
_MOLT_DECIMAL_TO_INT = _require_intrinsic("molt_decimal_to_int")
_MOLT_DECIMAL_TO_INTEGRAL_VALUE = _require_intrinsic(
    "molt_decimal_to_integral_value")
_MOLT_DECIMAL_TO_INTEGRAL_EXACT = _require_intrinsic(
    "molt_decimal_to_integral_exact")
_MOLT_DECIMAL_TO_ENG_STRING = _require_intrinsic(
    "molt_decimal_to_eng_string")
_MOLT_DECIMAL_ADJUSTED = _require_intrinsic("molt_decimal_adjusted")
_MOLT_DECIMAL_AS_INTEGER_RATIO = _require_intrinsic(
    "molt_decimal_as_integer_ratio")
_MOLT_DECIMAL_FROM_FLOAT = _require_intrinsic("molt_decimal_from_float")
_MOLT_DECIMAL_IS_FINITE = _require_intrinsic("molt_decimal_is_finite")
_MOLT_DECIMAL_IS_INFINITE = _require_intrinsic("molt_decimal_is_infinite")
_MOLT_DECIMAL_IS_NAN = _require_intrinsic("molt_decimal_is_nan")
_MOLT_DECIMAL_IS_NORMAL = _require_intrinsic("molt_decimal_is_normal")
_MOLT_DECIMAL_IS_SIGNED = _require_intrinsic("molt_decimal_is_signed")
_MOLT_DECIMAL_IS_SUBNORMAL = _require_intrinsic("molt_decimal_is_subnormal")
_MOLT_DECIMAL_IS_ZERO = _require_intrinsic("molt_decimal_is_zero")
_MOLT_DECIMAL_COPY_ABS = _require_intrinsic("molt_decimal_copy_abs")
_MOLT_DECIMAL_COPY_NEGATE = _require_intrinsic("molt_decimal_copy_negate")
_MOLT_DECIMAL_COPY_SIGN = _require_intrinsic("molt_decimal_copy_sign")
_MOLT_DECIMAL_SAME_QUANTUM = _require_intrinsic("molt_decimal_same_quantum")

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
        # Decimal(...) construction is context-invariant; apply this context by routing
        # through a context-aware arithmetic op.
        dec = Decimal(value)
        if isinstance(dec.as_tuple().exponent, str):
            return dec
        return self.divide(dec, Decimal(1))

    def divide(self, a: "Decimal", b: "Decimal") -> "Decimal":
        return _decimal_from_handle(
            _MOLT_DECIMAL_DIV(self._handle, a._handle, b._handle)
        )

    def create_decimal_from_float(self, f: float) -> "Decimal":
        if not isinstance(f, (float, int)):
            raise TypeError("argument must be int or float")
        return _decimal_from_handle(_MOLT_DECIMAL_FROM_FLOAT(self._handle, float(f)))

    # ── Context-aware arithmetic ─────────────────────────────────────────

    def abs(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_ABS(self._handle, a._handle))

    def add(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_ADD(self._handle, a._handle, b._handle)
        )

    def subtract(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_SUB(self._handle, a._handle, b._handle)
        )

    def multiply(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_MUL(self._handle, a._handle, b._handle)
        )

    def divide_int(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_FLOORDIV(self._handle, a._handle, b._handle)
        )

    def remainder(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_MOD(self._handle, a._handle, b._handle)
        )

    def divmod(self, a: "Decimal", b: "Decimal") -> tuple["Decimal", "Decimal"]:
        a = _coerce(a)
        b = _coerce(b)
        return (self.divide_int(a, b), self.remainder(a, b))

    def remainder_near(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_REMAINDER_NEAR(self._handle, a._handle, b._handle)
        )

    def power(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_POW(self._handle, a._handle, b._handle)
        )

    def minus(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_NEG(self._handle, a._handle))

    def plus(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_POS(self._handle, a._handle))

    # ── Context-aware mathematical methods ───────────────────────────────

    def sqrt(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_SQRT(self._handle, a._handle))

    def exp(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_EXP(self._handle, a._handle))

    def ln(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_LN(self._handle, a._handle))

    def log10(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_LOG10(self._handle, a._handle))

    def fma(self, a: "Decimal", b: "Decimal", c: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        c = _coerce(c)
        return _decimal_from_handle(
            _MOLT_DECIMAL_FMA(self._handle, a._handle, b._handle, c._handle)
        )

    def normalize(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_NORMALIZE(self._handle, a._handle))

    def quantize(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_QUANTIZE(self._handle, a._handle, b._handle)
        )

    # ── Context-aware comparison / ordering ──────────────────────────────

    def compare(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_COMPARE(self._handle, a._handle, b._handle)
        )

    def compare_total(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(_MOLT_DECIMAL_COMPARE_TOTAL(a._handle, b._handle))

    def max(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_MAX(self._handle, a._handle, b._handle)
        )

    def min(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_MIN(self._handle, a._handle, b._handle)
        )

    # ── Context-aware rounding / integral ────────────────────────────────

    def to_integral_value(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(
            _MOLT_DECIMAL_TO_INTEGRAL_VALUE(self._handle, a._handle)
        )

    def to_integral_exact(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(
            _MOLT_DECIMAL_TO_INTEGRAL_EXACT(self._handle, a._handle)
        )

    def to_integral(self, a: "Decimal") -> "Decimal":
        return self.to_integral_value(a)

    # ── Context-aware next / scaleb / number_class ───────────────────────

    def next_minus(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_NEXT_MINUS(self._handle, a._handle))

    def next_plus(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_NEXT_PLUS(self._handle, a._handle))

    def scaleb(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(
            _MOLT_DECIMAL_SCALEB(self._handle, a._handle, b._handle)
        )

    def number_class(self, a: "Decimal") -> str:
        a = _coerce(a)
        return str(_MOLT_DECIMAL_NUMBER_CLASS(self._handle, a._handle))

    # ── Context-aware string representations ─────────────────────────────

    def to_eng_string(self, a: "Decimal") -> str:
        a = _coerce(a)
        return str(_MOLT_DECIMAL_TO_ENG_STRING(a._handle))

    def to_sci_string(self, a: "Decimal") -> str:
        a = _coerce(a)
        return str(_MOLT_DECIMAL_TO_STRING(a._handle))

    # ── Context-aware copy operations ────────────────────────────────────

    def copy_abs(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_COPY_ABS(a._handle))

    def copy_negate(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_COPY_NEGATE(a._handle))

    def copy_sign(self, a: "Decimal", b: "Decimal") -> "Decimal":
        a = _coerce(a)
        b = _coerce(b)
        return _decimal_from_handle(_MOLT_DECIMAL_COPY_SIGN(a._handle, b._handle))

    def copy_decimal(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_CLONE(a._handle))

    def same_quantum(self, a: "Decimal", b: "Decimal") -> bool:
        a = _coerce(a)
        b = _coerce(b)
        return bool(_MOLT_DECIMAL_SAME_QUANTUM(a._handle, b._handle))

    # ── Context-aware predicates ─────────────────────────────────────────

    def is_finite(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_FINITE(a._handle))

    def is_infinite(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_INFINITE(a._handle))

    def is_nan(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_NAN(a._handle))

    def is_normal(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_NORMAL(self._handle, a._handle))

    def is_signed(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_SIGNED(a._handle))

    def is_subnormal(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_SUBNORMAL(self._handle, a._handle))

    def is_zero(self, a: "Decimal") -> bool:
        a = _coerce(a)
        return bool(_MOLT_DECIMAL_IS_ZERO(a._handle))

    def is_snan(self, a: "Decimal") -> bool:
        a = _coerce(a)
        t = a.as_tuple()
        return isinstance(t.exponent, str) and t.exponent == "N"

    def is_qnan(self, a: "Decimal") -> bool:
        a = _coerce(a)
        t = a.as_tuple()
        return isinstance(t.exponent, str) and t.exponent == "n"

    def is_canonical(self, a: "Decimal") -> bool:
        return True

    def canonical(self, a: "Decimal") -> "Decimal":
        a = _coerce(a)
        return _decimal_from_handle(_MOLT_DECIMAL_CLONE(a._handle))

    def radix(self) -> "Decimal":
        return Decimal(10)

    def clear_traps(self) -> None:
        for code in _SIGNAL_MAP.values():
            _MOLT_DECIMAL_CONTEXT_SET_TRAP(self._handle, code, False)

    def __del__(self) -> None:
        try:
            _MOLT_DECIMAL_CONTEXT_DROP(self._handle)
        except Exception:
            pass


class Decimal:
    __slots__ = ("_handle",)
    _handle: object

    def __new__(cls, value: object = 0, context: Context | None = None) -> "Decimal":
        if isinstance(value, float):
            handle = _decimal_float_handle(value, context)
        else:
            handle = _decimal_handle(value, context)
        self = super().__new__(cls)
        self._handle = handle
        return self

    @classmethod
    def from_float(cls, f: float) -> "Decimal":
        if not isinstance(f, (float, int)):
            raise TypeError("argument must be int or float")
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_FROM_FLOAT(ctx, float(f)))
        )

    def __repr__(self) -> str:
        return f"Decimal('{self}')"

    def __str__(self) -> str:
        return str(_MOLT_DECIMAL_TO_STRING(self._handle))

    def __float__(self) -> float:
        return float(_MOLT_DECIMAL_TO_FLOAT(self._handle))

    def __int__(self) -> int:
        return int(_MOLT_DECIMAL_TO_INT(self._handle))

    def __bool__(self) -> bool:
        return not bool(_MOLT_DECIMAL_IS_ZERO(self._handle))

    def __hash__(self) -> int:
        if self.is_nan():
            raise TypeError("cannot hash a NaN value")
        return hash(float(self))

    # ── Arithmetic operators ──────────────────────────────────────────────

    def __add__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_ADD(ctx, self._handle, other._handle)
            )
        )

    def __radd__(self, other: object) -> "Decimal":
        return _coerce(other).__add__(self)

    def __sub__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_SUB(ctx, self._handle, other._handle)
            )
        )

    def __rsub__(self, other: object) -> "Decimal":
        return _coerce(other).__sub__(self)

    def __mul__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_MUL(ctx, self._handle, other._handle)
            )
        )

    def __rmul__(self, other: object) -> "Decimal":
        return _coerce(other).__mul__(self)

    def __truediv__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_DIV(ctx, self._handle, other._handle)
            )
        )

    def __rtruediv__(self, other: object) -> "Decimal":
        return _coerce(other).__truediv__(self)

    def __floordiv__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_FLOORDIV(ctx, self._handle, other._handle)
            )
        )

    def __rfloordiv__(self, other: object) -> "Decimal":
        return _coerce(other).__floordiv__(self)

    def __mod__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_MOD(ctx, self._handle, other._handle)
            )
        )

    def __rmod__(self, other: object) -> "Decimal":
        return _coerce(other).__mod__(self)

    def __divmod__(self, other: object) -> tuple["Decimal", "Decimal"]:
        return (self // other, self % other)

    def __rdivmod__(self, other: object) -> tuple["Decimal", "Decimal"]:
        other = _coerce(other)
        return (other // self, other % self)

    def __pow__(self, other: object) -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_POW(ctx, self._handle, other._handle)
            )
        )

    def __rpow__(self, other: object) -> "Decimal":
        return _coerce(other).__pow__(self)

    def __neg__(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_NEG(ctx, self._handle))
        )

    def __pos__(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_POS(ctx, self._handle))
        )

    def __abs__(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_ABS(ctx, self._handle))
        )

    # ── Comparison operators ──────────────────────────────────────────────

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, (Decimal, int, str)):
            return NotImplemented
        other = _coerce(other)
        cmp = _with_current_context(
            lambda ctx: int(
                _MOLT_DECIMAL_TO_STRING(
                    _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
                )
            )
        )
        return cmp == 0

    def __lt__(self, other: object) -> bool:
        other = _coerce(other)
        cmp = _with_current_context(
            lambda ctx: int(
                _MOLT_DECIMAL_TO_STRING(
                    _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
                )
            )
        )
        return cmp < 0

    def __le__(self, other: object) -> bool:
        other = _coerce(other)
        cmp = _with_current_context(
            lambda ctx: int(
                _MOLT_DECIMAL_TO_STRING(
                    _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
                )
            )
        )
        return cmp <= 0

    def __gt__(self, other: object) -> bool:
        other = _coerce(other)
        cmp = _with_current_context(
            lambda ctx: int(
                _MOLT_DECIMAL_TO_STRING(
                    _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
                )
            )
        )
        return cmp > 0

    def __ge__(self, other: object) -> bool:
        other = _coerce(other)
        cmp = _with_current_context(
            lambda ctx: int(
                _MOLT_DECIMAL_TO_STRING(
                    _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
                )
            )
        )
        return cmp >= 0

    # ── Rounding / int conversion ─────────────────────────────────────────

    def __round__(self, ndigits: int | None = None) -> "Decimal":
        if ndigits is None:
            return _with_current_context(
                lambda ctx: _decimal_from_handle(
                    _MOLT_DECIMAL_TO_INTEGRAL_VALUE(ctx, self._handle)
                )
            )
        quant = Decimal(10) ** (-ndigits)
        return self.quantize(quant)

    def __trunc__(self) -> int:
        return int(_MOLT_DECIMAL_TO_INT(self._handle))

    def __floor__(self) -> int:
        if self._is_special():
            raise ValueError("cannot convert special value to int")
        rounded = _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_TO_INTEGRAL_VALUE(ctx, self._handle)
            )
        )
        if rounded > self:
            rounded = rounded - Decimal(1)
        return int(rounded)

    def __ceil__(self) -> int:
        if self._is_special():
            raise ValueError("cannot convert special value to int")
        rounded = _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_TO_INTEGRAL_VALUE(ctx, self._handle)
            )
        )
        if rounded < self:
            rounded = rounded + Decimal(1)
        return int(rounded)

    # ── Tuple / string representations ────────────────────────────────────

    def as_tuple(self) -> DecimalTuple:
        sign, digits, exponent = _MOLT_DECIMAL_AS_TUPLE(self._handle)
        return DecimalTuple(int(sign), tuple(digits), exponent)

    def to_eng_string(self) -> str:
        return str(_MOLT_DECIMAL_TO_ENG_STRING(self._handle))

    def adjusted(self) -> int:
        return int(_MOLT_DECIMAL_ADJUSTED(self._handle))

    def as_integer_ratio(self) -> tuple[int, int]:
        num, den = _MOLT_DECIMAL_AS_INTEGER_RATIO(self._handle)
        return (int(num), int(den))

    # ── Mathematical methods ──────────────────────────────────────────────

    def normalize(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_NORMALIZE(ctx, self._handle))
        )

    def quantize(self, exp: "Decimal") -> "Decimal":
        exp = _coerce(exp)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_QUANTIZE(ctx, self._handle, exp._handle)
            )
        )

    def compare(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_COMPARE(ctx, self._handle, other._handle)
            )
        )

    def compare_total(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _decimal_from_handle(
            _MOLT_DECIMAL_COMPARE_TOTAL(self._handle, other._handle)
        )

    def exp(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_EXP(ctx, self._handle))
        )

    def sqrt(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_SQRT(ctx, self._handle))
        )

    def ln(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_LN(ctx, self._handle))
        )

    def log10(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_LOG10(ctx, self._handle))
        )

    def fma(self, other: "Decimal", third: "Decimal") -> "Decimal":
        other = _coerce(other)
        third = _coerce(third)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_FMA(ctx, self._handle, other._handle, third._handle)
            )
        )

    def max(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_MAX(ctx, self._handle, other._handle)
            )
        )

    def min(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_MIN(ctx, self._handle, other._handle)
            )
        )

    def remainder_near(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_REMAINDER_NEAR(ctx, self._handle, other._handle)
            )
        )

    def scaleb(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_SCALEB(ctx, self._handle, other._handle)
            )
        )

    def next_minus(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_NEXT_MINUS(ctx, self._handle)
            )
        )

    def next_plus(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(_MOLT_DECIMAL_NEXT_PLUS(ctx, self._handle))
        )

    def number_class(self) -> str:
        return _with_current_context(
            lambda ctx: str(_MOLT_DECIMAL_NUMBER_CLASS(ctx, self._handle))
        )

    def to_integral_value(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_TO_INTEGRAL_VALUE(ctx, self._handle)
            )
        )

    def to_integral_exact(self) -> "Decimal":
        return _with_current_context(
            lambda ctx: _decimal_from_handle(
                _MOLT_DECIMAL_TO_INTEGRAL_EXACT(ctx, self._handle)
            )
        )

    # ── Predicates ────────────────────────────────────────────────────────

    def is_finite(self) -> bool:
        return bool(_MOLT_DECIMAL_IS_FINITE(self._handle))

    def is_infinite(self) -> bool:
        return bool(_MOLT_DECIMAL_IS_INFINITE(self._handle))

    def is_nan(self) -> bool:
        return bool(_MOLT_DECIMAL_IS_NAN(self._handle))

    def is_normal(self) -> bool:
        return _with_current_context(
            lambda ctx: bool(_MOLT_DECIMAL_IS_NORMAL(ctx, self._handle))
        )

    def is_signed(self) -> bool:
        return bool(_MOLT_DECIMAL_IS_SIGNED(self._handle))

    def is_subnormal(self) -> bool:
        return _with_current_context(
            lambda ctx: bool(_MOLT_DECIMAL_IS_SUBNORMAL(ctx, self._handle))
        )

    def is_zero(self) -> bool:
        return bool(_MOLT_DECIMAL_IS_ZERO(self._handle))

    def is_snan(self) -> bool:
        t = self.as_tuple()
        return isinstance(t.exponent, str) and t.exponent == "N"

    def is_qnan(self) -> bool:
        t = self.as_tuple()
        return isinstance(t.exponent, str) and t.exponent == "n"

    # ── Copy operations ───────────────────────────────────────────────────

    def copy_abs(self) -> "Decimal":
        return _decimal_from_handle(_MOLT_DECIMAL_COPY_ABS(self._handle))

    def copy_negate(self) -> "Decimal":
        return _decimal_from_handle(_MOLT_DECIMAL_COPY_NEGATE(self._handle))

    def copy_sign(self, other: "Decimal") -> "Decimal":
        other = _coerce(other)
        return _decimal_from_handle(
            _MOLT_DECIMAL_COPY_SIGN(self._handle, other._handle)
        )

    def same_quantum(self, other: "Decimal") -> bool:
        other = _coerce(other)
        return bool(_MOLT_DECIMAL_SAME_QUANTUM(self._handle, other._handle))

    # ── Trivial identity methods ─────────────────────────────────────────

    def canonical(self) -> "Decimal":
        return _decimal_from_handle(_MOLT_DECIMAL_CLONE(self._handle))

    def is_canonical(self) -> bool:
        return True

    def radix(self) -> "Decimal":
        return Decimal(10)

    def conjugate(self) -> "Decimal":
        return _decimal_from_handle(_MOLT_DECIMAL_CLONE(self._handle))

    def to_integral(self) -> "Decimal":
        return self.to_integral_value()

    # ── Internal helpers ──────────────────────────────────────────────────

    def _is_special(self) -> bool:
        return self.is_nan() or self.is_infinite()

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


def _coerce(value: object) -> Decimal:
    if isinstance(value, Decimal):
        return value
    if isinstance(value, int):
        return Decimal(value)
    if isinstance(value, str):
        return Decimal(value)
    return NotImplemented


def _decimal_float_handle(value: float, context: Context | None) -> object:
    if isinstance(context, Context):
        return _MOLT_DECIMAL_FROM_FLOAT(context._handle, value)
    ctx_handle = _MOLT_DECIMAL_CONTEXT_GET_CURRENT()
    try:
        return _MOLT_DECIMAL_FROM_FLOAT(ctx_handle, value)
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

globals().pop("_require_intrinsic", None)
