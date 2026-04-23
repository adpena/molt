"""Abstract numeric types for Molt — full CPython 3.12 parity.

Implements the numeric tower defined in PEP 3141:
  Number > Complex > Real > Rational > Integral

These are pure-Python ABCs; no native backing is required.  Libraries that
do ``isinstance(x, numbers.Integral)`` rely on this hierarchy at runtime.
"""

from __future__ import annotations

from abc import ABCMeta, abstractmethod

__all__ = ["Number", "Complex", "Real", "Rational", "Integral"]


class Number(metaclass=ABCMeta):
    """Most base class of the numeric hierarchy.

    Just defines that instances can be used as numbers.  If you want to
    constrain an argument to the numeric tower, use ``isinstance(x,
    numbers.Number)``.
    """

    __slots__ = ()
    __hash__ = None  # type: ignore[assignment]


class Complex(Number):
    """Complex defines the operations that work on the builtin complex type.

    In short, those are: a conversion to complex, .real, .imag, +, -,
    *, /, **, abs(), .conjugate, ==, and !=.

    If it is given heterogeneous arguments, and doesn't know how to
    implement the operation with the other type, it should return
    NotImplemented instead.
    """

    __slots__ = ()

    @abstractmethod
    def __complex__(self) -> complex:
        """Return a builtin complex instance."""

    def __bool__(self) -> bool:
        """True if self != 0."""
        return self != 0

    @property
    @abstractmethod
    def real(self):
        """Retrieve the real component of this number."""

    @property
    @abstractmethod
    def imag(self):
        """Retrieve the imaginary component of this number."""

    @abstractmethod
    def __add__(self, other):
        """self + other"""

    @abstractmethod
    def __radd__(self, other):
        """other + self"""

    @abstractmethod
    def __neg__(self):
        """-self"""

    @abstractmethod
    def __pos__(self):
        """+self"""

    def __sub__(self, other):
        """self - other"""
        return self + -other

    def __rsub__(self, other):
        """other - self"""
        return -self + other

    @abstractmethod
    def __mul__(self, other):
        """self * other"""

    @abstractmethod
    def __rmul__(self, other):
        """other * self"""

    @abstractmethod
    def __truediv__(self, other):
        """self / other: Should promote to float when necessary."""

    @abstractmethod
    def __rtruediv__(self, other):
        """other / self"""

    @abstractmethod
    def __pow__(self, exponent):
        """self ** exponent; should promote to float or complex when necessary."""

    @abstractmethod
    def __rpow__(self, base):
        """base ** self"""

    @abstractmethod
    def __abs__(self) -> "Real":
        """Returns the Real distance from 0. Called for abs(self)."""

    @abstractmethod
    def conjugate(self):
        """(x+y*i).conjugate() returns (x-y*i)."""

    @abstractmethod
    def __eq__(self, other) -> bool:
        """self == other"""


class Real(Complex):
    """To Complex, Real adds the operations that work on real numbers.

    In short, those are: a conversion to float, trunc(), divmod,
    %, <, <=, >, and >=.

    Real also provides defaults for the derived operations.
    """

    __slots__ = ()

    @abstractmethod
    def __float__(self) -> float:
        """Any Real can be converted to a native Python float object."""

    @abstractmethod
    def __trunc__(self) -> int:
        """trunc(self): Truncates self to an Integral.

        Returns an Integral i such that:
          * i>0 iff self>0;
          * abs(i) <= abs(self);
          * for any Integral j satisfying the first two conditions,
            abs(i) >= abs(j) [i.e. i has the greatest absolute value of any
            Integral satisfying conditions 1 and 2].
        i.e. "truncate towards 0".
        """

    @abstractmethod
    def __floor__(self) -> int:
        """Finds the greatest Integral <= self."""

    @abstractmethod
    def __ceil__(self) -> int:
        """Finds the least Integral >= self."""

    @abstractmethod
    def __round__(self, ndigits=None):
        """Rounds self to ndigits decimal places, defaulting to 0.

        If ndigits is omitted or None, returns an Integral, otherwise
        returns a Real. Rounds half toward even.
        """

    def __divmod__(self, other):
        """divmod(self, other): The pair (self // other, self % other).

        Sometimes this can be computed faster than each operation separately.
        """
        return (self // other, self % other)

    def __rdivmod__(self, other):
        """divmod(other, self): The pair (other // self, other % self).

        Sometimes this can be computed faster than each operation separately.
        """
        return (other // self, other % self)

    @abstractmethod
    def __floordiv__(self, other) -> "Integral":
        """self // other: The floor() of self/other."""

    @abstractmethod
    def __rfloordiv__(self, other) -> "Integral":
        """other // self: The floor() of other/self."""

    @abstractmethod
    def __mod__(self, other):
        """self % other"""

    @abstractmethod
    def __rmod__(self, other):
        """other % self"""

    @abstractmethod
    def __lt__(self, other) -> bool:
        """self < other

        < on Reals defines a total ordering, except perhaps for NaN.
        """

    @abstractmethod
    def __le__(self, other) -> bool:
        """self <= other"""

    def __complex__(self) -> complex:
        """complex(self) == complex(float(self), 0)"""
        return complex(float(self))

    @property
    def real(self):
        """Real numbers are their own real component."""
        return +self

    @property
    def imag(self):
        """Real numbers have no imaginary component."""
        return 0

    def conjugate(self):
        """Conjugate is a no-op for Reals."""
        return +self


class Rational(Real):
    """.numerator and .denominator should be in lowest terms."""

    __slots__ = ()

    @property
    @abstractmethod
    def numerator(self) -> int:
        """Numerator in lowest terms."""

    @property
    @abstractmethod
    def denominator(self) -> int:
        """Denominator in lowest terms."""

    def __float__(self) -> float:
        """float(self) = self.numerator / self.denominator

        It's important that this conversion use the integer's "true"
        division rather than casting one side to float before dividing
        so that ratios of huge integers convert without overflowing.
        """
        return int(self.numerator) / int(self.denominator)


class Integral(Rational):
    """Integral adds a conversion to int and the bit-string operations."""

    __slots__ = ()

    @abstractmethod
    def __int__(self) -> int:
        """int(self)"""

    def __index__(self) -> int:
        """Called whenever an index is needed, such as in slicing."""
        return int(self)

    @abstractmethod
    def __pow__(self, exponent, modulus=None):
        """self ** exponent % modulus, but maybe faster.

        Accept the modulus argument if you want to support the
        3-argument version of pow(). Raise a TypeError if exponent < 0
        or any argument isn't Integral. Otherwise, just implement the
        2-argument version described in Complex.
        """

    @abstractmethod
    def __lshift__(self, other):
        """self << other"""

    @abstractmethod
    def __rlshift__(self, other):
        """other << self"""

    @abstractmethod
    def __rshift__(self, other):
        """self >> other"""

    @abstractmethod
    def __rrshift__(self, other):
        """other >> self"""

    @abstractmethod
    def __and__(self, other):
        """self & other"""

    @abstractmethod
    def __rand__(self, other):
        """other & self"""

    @abstractmethod
    def __xor__(self, other):
        """self ^ other"""

    @abstractmethod
    def __rxor__(self, other):
        """other ^ self"""

    @abstractmethod
    def __or__(self, other):
        """self | other"""

    @abstractmethod
    def __ror__(self, other):
        """other | self"""

    @abstractmethod
    def __invert__(self):
        """~self"""

    def __float__(self) -> float:
        """float(self) == float(int(self))"""
        return float(int(self))

    @property
    def numerator(self) -> int:
        """Integers are their own numerators."""
        return +self  # type: ignore[return-value]

    @property
    def denominator(self) -> int:
        """Integers have a denominator of 1."""
        return 1

    def __trunc__(self) -> int:
        return int(self)

    def __floor__(self) -> int:
        return int(self)

    def __ceil__(self) -> int:
        return int(self)

    def __round__(self, ndigits=None):
        if ndigits is None:
            return int(self)
        return type(self)(round(int(self), ndigits))


# Register the built-in numeric types into the hierarchy.
# This makes isinstance(42, numbers.Integral) == True, etc.
Complex.register(complex)
Real.register(float)
Rational.register(int)  # int is also Rational
Integral.register(int)
