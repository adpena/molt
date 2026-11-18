"""Purpose: a special method that returns the wrong type raises CPython's text for the target.

CPython 3.15 names both types (`B.__index__() must return an int, not str`);
3.12 to 3.14 keep the per-method texts. A strict subclass result warns.
"""

import math
import operator
import warnings

warnings.simplefilter("error")


class Wrong:
    def __index__(self):
        return "x"

    def __int__(self):
        return 1.5

    def __float__(self):
        return 1

    def __complex__(self):
        return 1

    def __str__(self):
        return 1

    def __repr__(self):
        return 1

    def __bytes__(self):
        return 1

    def __iter__(self):
        return 1

    def __bool__(self):
        return 1

    def __format__(self, spec):
        return 1


class Hint:
    def __length_hint__(self):
        return 1.5


class Outer:
    class Inner:
        def __index__(self):
            return 2.5


class SubInt(int):
    pass


class SubFloat(float):
    pass


class SubComplex(complex):
    pass


class Subclassed:
    def __index__(self):
        return SubInt(1)

    def __int__(self):
        return SubInt(2)

    def __float__(self):
        return SubFloat(1.0)

    def __complex__(self):
        return SubComplex(1)


def show(name, call):
    try:
        result = call()
    except (TypeError, DeprecationWarning) as error:
        print(name, type(error).__name__, error)
    else:
        print(name, "ok", result)


show("index", lambda: operator.index(Wrong()))
show("int", lambda: int(Wrong()))
show("float", lambda: float(Wrong()))
show("complex", lambda: complex(Wrong()))
show("str", lambda: str(Wrong()))
show("repr", lambda: repr(Wrong()))
show("bytes", lambda: bytes(Wrong()))
show("iter", lambda: iter(Wrong()))
show("bool", lambda: bool(Wrong()))
show("length_hint", lambda: operator.length_hint(Hint()))
show("format", lambda: format(Wrong(), ""))
show("nested", lambda: operator.index(Outer.Inner()))
show("range", lambda: range(Wrong()))
show("sub_index", lambda: operator.index(Subclassed()))
show("sub_int", lambda: int(Subclassed()))
show("sub_float", lambda: float(Subclassed()))
show("sub_complex", lambda: complex(Subclassed()))
show("trunc", lambda: math.trunc(1.5))
