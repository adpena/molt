"""Minimal intrinsic-gated `fractions` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
import numbers as _numbers
import operator as _operator
import sys as _sys

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


class Fraction(_numbers.Rational):
    """Represents a rational number as numerator/denominator pair.

    Exact rational arithmetic uses native handles; mixed numeric protocols
    follow CPython 3.12–3.14 (PSF license: collections/LICENSE.cpython).
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

    def _binop(self, other, op, fallback, fraction_result=True, handle_complex=True):
        # CPython's forward arithmetic admits only types whose cooperation is
        # known here. Constructor-only strings must reach reflected dispatch.
        if isinstance(other, (Fraction, int)):
            value = op(self._handle, _coerce_to_fraction(other))
            if not fraction_result:
                return value
            result = Fraction.__new__(Fraction)
            result._handle = value
            return result
        if isinstance(other, float):
            return fallback(float(self), other)
        if (handle_complex or _sys.version_info < (3, 13)) and isinstance(
            other, complex
        ):
            value = float(self) if _sys.version_info >= (3, 14) else complex(self)
            return fallback(value, other)
        return NotImplemented

    def _rbinop(self, other, op, fallback, fraction_result=True, handle_complex=True):
        if isinstance(other, _numbers.Rational):
            if isinstance(other, (Fraction, int)):
                handle = _coerce_to_fraction(other)
            else:
                handle = _MOLT_FRACTION_NEW(other.numerator, other.denominator)
            value = op(handle, self._handle)
            if not fraction_result:
                return value
            result = Fraction.__new__(Fraction)
            result._handle = value
            return result
        if isinstance(other, _numbers.Real):
            return fallback(float(other), float(self))
        if (handle_complex or _sys.version_info < (3, 13)) and isinstance(
            other, _numbers.Complex
        ):
            value = float(self) if _sys.version_info >= (3, 14) else complex(self)
            return fallback(complex(other), value)
        return NotImplemented

    def __add__(self, other):
        return self._binop(other, _MOLT_FRACTION_ADD, _operator.add)

    def __radd__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_ADD, _operator.add)

    def __sub__(self, other):
        return self._binop(other, _MOLT_FRACTION_SUB, _operator.sub)

    def __rsub__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_SUB, _operator.sub)

    def __mul__(self, other):
        return self._binop(other, _MOLT_FRACTION_MUL, _operator.mul)

    def __rmul__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_MUL, _operator.mul)

    def __truediv__(self, other):
        return self._binop(other, _MOLT_FRACTION_TRUEDIV, _operator.truediv)

    def __rtruediv__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_TRUEDIV, _operator.truediv)

    def __floordiv__(self, other):
        return self._binop(
            other, _MOLT_FRACTION_FLOORDIV, _operator.floordiv, False, False
        )

    def __rfloordiv__(self, other):
        return self._rbinop(
            other, _MOLT_FRACTION_FLOORDIV, _operator.floordiv, False, False
        )

    def __mod__(self, other):
        return self._binop(other, _MOLT_FRACTION_MOD, _operator.mod, True, False)

    def __rmod__(self, other):
        return self._rbinop(other, _MOLT_FRACTION_MOD, _operator.mod, True, False)

    def _power(self, exponent):
        # Adapted from pinned CPython Fraction.__pow__; integer powers use the
        # general integer owner rather than a truncated native exponent lane.
        if isinstance(exponent, _numbers.Rational):
            if exponent.denominator == 1:
                power = exponent.numerator
                numerator = _MOLT_FRACTION_NUMERATOR(self._handle)
                denominator = _MOLT_FRACTION_DENOMINATOR(self._handle)
                if power >= 0:
                    return Fraction(numerator**power, denominator**power)
                if numerator > 0:
                    return Fraction(denominator**-power, numerator**-power)
                if numerator == 0:
                    raise ZeroDivisionError("Fraction(%s, 0)" % (denominator**-power))
                return Fraction((-denominator) ** -power, (-numerator) ** -power)
            return float(self) ** float(exponent)
        if isinstance(exponent, (float, complex)):
            return float(self) ** exponent
        return NotImplemented

    def _reverse_power(self, base):
        numerator = _MOLT_FRACTION_NUMERATOR(self._handle)
        denominator = _MOLT_FRACTION_DENOMINATOR(self._handle)
        if denominator == 1 and numerator >= 0:
            return base**numerator
        if isinstance(base, _numbers.Rational):
            return Fraction(base.numerator, base.denominator) ** self
        if denominator == 1:
            return base**numerator
        return base ** float(self)

    # CPython 3.14 adds the explicit ternary-power calling contract. Keep one
    # arithmetic body while retaining the target-specific public signatures.
    if _sys.version_info >= (3, 14):

        def __pow__(self, other, modulo=None):
            if modulo is not None:
                return NotImplemented
            return Fraction._power(self, other)

        def __rpow__(self, other, modulo=None):
            if modulo is not None:
                return NotImplemented
            return Fraction._reverse_power(self, other)
    else:

        def __pow__(self, other):
            return Fraction._power(self, other)

        def __rpow__(self, other):
            return Fraction._reverse_power(self, other)

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

    def __int__(self):
        numerator = _MOLT_FRACTION_NUMERATOR(self._handle)
        denominator = _MOLT_FRACTION_DENOMINATOR(self._handle)
        if numerator < 0:
            return -(-numerator // denominator)
        return numerator // denominator

    def __trunc__(self):
        # Inherited truncation must not dispatch a subclass's __int__ override.
        return Fraction.__int__(self)

    def __floor__(self):
        return _MOLT_FRACTION_NUMERATOR(self._handle) // _MOLT_FRACTION_DENOMINATOR(
            self._handle
        )

    def __ceil__(self):
        return -(
            -_MOLT_FRACTION_NUMERATOR(self._handle)
            // _MOLT_FRACTION_DENOMINATOR(self._handle)
        )

    def __round__(self, ndigits=None):
        # Adapted from CPython 3.14.9 Lib/fractions.py, ead82c07db1f111d613926ed1b6667411d768125.
        # PSF license: collections/LICENSE.cpython. Keep exact ties-to-even and
        # the original ndigits abs/power protocol (not implicit index coercion).
        if ndigits is None:
            denominator = _MOLT_FRACTION_DENOMINATOR(self._handle)
            floor, remainder = divmod(
                _MOLT_FRACTION_NUMERATOR(self._handle), denominator
            )
            if remainder * 2 < denominator:
                return floor
            if remainder * 2 > denominator:
                return floor + 1
            return floor if floor % 2 == 0 else floor + 1
        shift = 10 ** abs(ndigits)
        if ndigits > 0:
            return Fraction(round(self * shift), shift)
        return Fraction(round(self / shift) * shift)

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

    def __del__(self, _drop=_MOLT_FRACTION_DROP) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        object.__setattr__(self, "_handle", None)
        try:
            _drop(handle)
        except Exception:
            pass


__all__ = ["Fraction"]

globals().pop("_require_intrinsic", None)
