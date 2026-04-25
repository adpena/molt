"""Abstract Base Classes (ABCs) for numbers, according to PEP 3141.

Direct port of CPython 3.12's numbers module. Pure-Python; depends only
on `abc`. The numeric ABC tower (Number → Complex → Real → Rational →
Integral) drives `isinstance`/`issubclass` checks for code that uses
PEP 3141 numeric polymorphism.
"""

from abc import ABCMeta, abstractmethod

__all__ = ["Number", "Complex", "Real", "Rational", "Integral"]


class Number(metaclass=ABCMeta):
    """All numbers inherit from this class.

    If you just want to check if an argument x is a number, without
    caring what kind, use isinstance(x, Number).
    """

    __slots__ = ()

    __hash__ = None


class Complex(Number):
    """Complex defines the operations that work on the builtin complex type."""

    __slots__ = ()

    @abstractmethod
    def __complex__(self):
        """Return a builtin complex instance."""

    def __bool__(self):
        return self != 0

    @property
    @abstractmethod
    def real(self):
        raise NotImplementedError

    @property
    @abstractmethod
    def imag(self):
        raise NotImplementedError

    @abstractmethod
    def __add__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __radd__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __neg__(self):
        raise NotImplementedError

    @abstractmethod
    def __pos__(self):
        raise NotImplementedError

    def __sub__(self, other):
        return self + -other

    def __rsub__(self, other):
        return -self + other

    @abstractmethod
    def __mul__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rmul__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __truediv__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rtruediv__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __pow__(self, exponent):
        raise NotImplementedError

    @abstractmethod
    def __rpow__(self, base):
        raise NotImplementedError

    @abstractmethod
    def __abs__(self):
        raise NotImplementedError

    @abstractmethod
    def conjugate(self):
        raise NotImplementedError

    @abstractmethod
    def __eq__(self, other):
        raise NotImplementedError


Complex.register(complex)


class Real(Complex):
    """Real adds the operations that work on real numbers."""

    __slots__ = ()

    @abstractmethod
    def __float__(self):
        raise NotImplementedError

    @abstractmethod
    def __trunc__(self):
        raise NotImplementedError

    @abstractmethod
    def __floor__(self):
        raise NotImplementedError

    @abstractmethod
    def __ceil__(self):
        raise NotImplementedError

    @abstractmethod
    def __round__(self, ndigits=None):
        raise NotImplementedError

    def __divmod__(self, other):
        return (self // other, self % other)

    def __rdivmod__(self, other):
        return (other // self, other % self)

    @abstractmethod
    def __floordiv__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rfloordiv__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __mod__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rmod__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __lt__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __le__(self, other):
        raise NotImplementedError

    def __complex__(self):
        return complex(float(self))

    @property
    def real(self):
        return +self

    @property
    def imag(self):
        return 0

    def conjugate(self):
        return +self


Real.register(float)


class Rational(Real):
    """.numerator and .denominator should be in lowest terms."""

    __slots__ = ()

    @property
    @abstractmethod
    def numerator(self):
        raise NotImplementedError

    @property
    @abstractmethod
    def denominator(self):
        raise NotImplementedError

    def __float__(self):
        return int(self.numerator) / int(self.denominator)


class Integral(Rational):
    """Integral adds methods that work on integral numbers."""

    __slots__ = ()

    @abstractmethod
    def __int__(self):
        raise NotImplementedError

    def __index__(self):
        return int(self)

    @abstractmethod
    def __pow__(self, exponent, modulus=None):
        raise NotImplementedError

    @abstractmethod
    def __lshift__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rlshift__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rshift__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rrshift__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __and__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rand__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __xor__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __rxor__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __or__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __ror__(self, other):
        raise NotImplementedError

    @abstractmethod
    def __invert__(self):
        raise NotImplementedError

    def __float__(self):
        return float(int(self))

    @property
    def numerator(self):
        return +self

    @property
    def denominator(self):
        return 1


Integral.register(int)
