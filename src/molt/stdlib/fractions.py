"""Minimal intrinsic-gated `fractions` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- intrinsic bindings ---

_MOLT_FRACTION_NEW = _require_intrinsic("molt_fraction_new")
_MOLT_FRACTION_FROM_FLOAT = _require_intrinsic("molt_fraction_from_float")
_MOLT_FRACTION_FROM_STR = _require_intrinsic("molt_fraction_from_str")
_MOLT_FRACTION_ADD = _require_intrinsic("molt_fraction_add")
_MOLT_FRACTION_SUB = _require_intrinsic("molt_fraction_sub")
_MOLT_FRACTION_MUL = _require_intrinsic("molt_fraction_mul")
_MOLT_FRACTION_TRUEDIV = _require_intrinsic("molt_fraction_truediv")
_MOLT_FRACTION_FLOORDIV = _require_intrinsic("molt_fraction_floordiv")
_MOLT_FRACTION_MOD = _require_intrinsic("molt_fraction_mod")
_MOLT_FRACTION_POW = _require_intrinsic("molt_fraction_pow")
_MOLT_FRACTION_NEG = _require_intrinsic("molt_fraction_neg")
_MOLT_FRACTION_ABS = _require_intrinsic("molt_fraction_abs")
_MOLT_FRACTION_EQ = _require_intrinsic("molt_fraction_eq")
_MOLT_FRACTION_LT = _require_intrinsic("molt_fraction_lt")
_MOLT_FRACTION_LE = _require_intrinsic("molt_fraction_le")
_MOLT_FRACTION_NUMERATOR = _require_intrinsic("molt_fraction_numerator")
_MOLT_FRACTION_DENOMINATOR = _require_intrinsic("molt_fraction_denominator")
_MOLT_FRACTION_TO_FLOAT = _require_intrinsic("molt_fraction_to_float")
_MOLT_FRACTION_TO_STR = _require_intrinsic("molt_fraction_to_str")
_MOLT_FRACTION_HASH = _require_intrinsic("molt_fraction_hash")
_MOLT_FRACTION_LIMIT_DENOMINATOR = _require_intrinsic("molt_fraction_limit_denominator")
_MOLT_FRACTION_AS_INTEGER_RATIO = _require_intrinsic("molt_fraction_as_integer_ratio")
_MOLT_FRACTION_DROP = _require_intrinsic("molt_fraction_drop")


def _coerce_to_fraction(value):
    """Coerce a value to a Fraction, returning its handle."""
    if isinstance(value, Fraction):
        return value._handle
    if isinstance(value, int):
        return _MOLT_FRACTION_NEW(value, 1)
    if isinstance(value, float):
        return _MOLT_FRACTION_FROM_FLOAT(value)
    if isinstance(value, str):
        return _MOLT_FRACTION_FROM_STR(value)
    raise TypeError(
        f"argument should be a Fraction, int, float, or str, not '{type(value).__name__}'"
    )


class Fraction:
    """Represents a rational number as numerator/denominator pair.

    All arithmetic is delegated to Rust intrinsics.
    """

    __slots__ = ("_handle",)

    def __init__(self, numerator=0, denominator=None):
        if denominator is None:
            if isinstance(numerator, Fraction):
                self._handle = _MOLT_FRACTION_NEW(
                    _MOLT_FRACTION_NUMERATOR(numerator._handle),
                    _MOLT_FRACTION_DENOMINATOR(numerator._handle),
                )
            elif isinstance(numerator, int):
                self._handle = _MOLT_FRACTION_NEW(numerator, 1)
            elif isinstance(numerator, float):
                self._handle = _MOLT_FRACTION_FROM_FLOAT(numerator)
            elif isinstance(numerator, str):
                self._handle = _MOLT_FRACTION_FROM_STR(numerator)
            else:
                raise TypeError(
                    f"argument should be a str or a Rational instance, "
                    f"not '{type(numerator).__name__}'"
                )
        else:
            if not isinstance(numerator, int):
                raise TypeError(
                    f"both arguments should be Rational instances, "
                    f"not '{type(numerator).__name__}'"
                )
            if not isinstance(denominator, int):
                raise TypeError(
                    f"both arguments should be Rational instances, "
                    f"not '{type(denominator).__name__}'"
                )
            if denominator == 0:
                raise ZeroDivisionError("Fraction(%s, 0)" % numerator)
            self._handle = _MOLT_FRACTION_NEW(numerator, denominator)

    @classmethod
    def from_float(cls, f):
        """Convert a float to a Fraction."""
        if isinstance(f, int):
            return cls(f)
        if not isinstance(f, float):
            raise TypeError(
                f"{cls.__name__}.from_float() only takes floats, not {type(f).__name__!r}"
            )
        result = cls.__new__(cls)
        result._handle = _MOLT_FRACTION_FROM_FLOAT(f)
        return result

    @classmethod
    def from_decimal(cls, dec):
        """Convert a Decimal to a Fraction."""
        # Convert Decimal to its exact rational representation via string.
        result = cls.__new__(cls)
        result._handle = _MOLT_FRACTION_FROM_STR(str(dec))
        return result

    @property
    def numerator(self):
        return int(_MOLT_FRACTION_NUMERATOR(self._handle))

    @property
    def denominator(self):
        return int(_MOLT_FRACTION_DENOMINATOR(self._handle))

    def as_integer_ratio(self):
        return _MOLT_FRACTION_AS_INTEGER_RATIO(self._handle)

    def limit_denominator(self, max_denominator=10**6):
        result = Fraction.__new__(Fraction)
        result._handle = _MOLT_FRACTION_LIMIT_DENOMINATOR(
            self._handle, int(max_denominator)
        )
        return result

    def _binop(self, other, op):
        """Helper for binary operations."""
        other_handle = _coerce_to_fraction(other)
        result = Fraction.__new__(Fraction)
        result._handle = op(self._handle, other_handle)
        return result

    def _rbinop(self, other, op):
        """Helper for reflected binary operations."""
        other_handle = _coerce_to_fraction(other)
        result = Fraction.__new__(Fraction)
        result._handle = op(other_handle, self._handle)
        return result

    def __add__(self, other):
        return self._binop(other, _MOLT_FRACTION_ADD)

    def __radd__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_ADD)

    def __sub__(self, other):
        return self._binop(other, _MOLT_FRACTION_SUB)

    def __rsub__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_SUB)

    def __mul__(self, other):
        return self._binop(other, _MOLT_FRACTION_MUL)

    def __rmul__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_MUL)

    def __truediv__(self, other):
        return self._binop(other, _MOLT_FRACTION_TRUEDIV)

    def __rtruediv__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_TRUEDIV)

    def __floordiv__(self, other):
        return self._binop(other, _MOLT_FRACTION_FLOORDIV)

    def __rfloordiv__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_FLOORDIV)

    def __mod__(self, other):
        return self._binop(other, _MOLT_FRACTION_MOD)

    def __rmod__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_MOD)

    def __pow__(self, other):
        return self._binop(other, _MOLT_FRACTION_POW)

    def __rpow__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_POW)

    def __neg__(self):
        result = Fraction.__new__(Fraction)
        result._handle = _MOLT_FRACTION_NEG(self._handle)
        return result

    def __abs__(self):
        result = Fraction.__new__(Fraction)
        result._handle = _MOLT_FRACTION_ABS(self._handle)
        return result

    def __pos__(self):
        # +Fraction is identity (already normalized)
        result = Fraction.__new__(Fraction)
        result._handle = _MOLT_FRACTION_NEW(
            _MOLT_FRACTION_NUMERATOR(self._handle),
            _MOLT_FRACTION_DENOMINATOR(self._handle),
        )
        return result

    def __eq__(self, other):
        try:
            other_handle = _coerce_to_fraction(other)
        except TypeError:
            return NotImplemented
        return bool(_MOLT_FRACTION_EQ(self._handle, other_handle))

    def __lt__(self, other):
        try:
            other_handle = _coerce_to_fraction(other)
        except TypeError:
            return NotImplemented
        return bool(_MOLT_FRACTION_LT(self._handle, other_handle))

    def __le__(self, other):
        try:
            other_handle = _coerce_to_fraction(other)
        except TypeError:
            return NotImplemented
        return bool(_MOLT_FRACTION_LE(self._handle, other_handle))

    def __gt__(self, other):
        try:
            oh = _coerce_to_fraction(other)
        except TypeError:
            return NotImplemented
        return bool(_MOLT_FRACTION_LT(oh, self._handle))

    def __ge__(self, other):
        try:
            oh = _coerce_to_fraction(other)
        except TypeError:
            return NotImplemented
        return bool(_MOLT_FRACTION_LE(oh, self._handle))

    def __float__(self):
        return float(_MOLT_FRACTION_TO_FLOAT(self._handle))

    def __str__(self):
        return str(_MOLT_FRACTION_TO_STR(self._handle))

    def __repr__(self):
        num = _MOLT_FRACTION_NUMERATOR(self._handle)
        den = _MOLT_FRACTION_DENOMINATOR(self._handle)
        return f"Fraction({num}, {den})"

    def __hash__(self):
        return int(_MOLT_FRACTION_HASH(self._handle))

    def __bool__(self):
        return _MOLT_FRACTION_NUMERATOR(self._handle) != 0

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_FRACTION_DROP(handle)
            except Exception:
                pass


__all__ = ["Fraction"]

globals().pop("_require_intrinsic", None)
