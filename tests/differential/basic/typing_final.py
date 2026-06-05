"""Purpose: differential coverage for typing.final (PEP 591).

``typing.final`` is a decorator that sets ``__final__ = True`` on the decorated
object when the object allows attribute setting, silently ignores the attribute
otherwise (``__slots__`` / read-only / builtin), and returns the object
unchanged.  Regression for ``typing.final`` being entirely absent.
"""

from typing import final


@final
class Sealed:
    pass


class Base:
    @final
    def locked(self):
        return "locked"

    @final
    @staticmethod
    def stat():
        return "stat"

    @final
    @classmethod
    def cls_m(cls):
        return "cls"


@final
def standalone():
    return 7


print("class __final__:", getattr(Sealed, "__final__", "MISSING"))
print("method __final__:", getattr(Base.locked, "__final__", "MISSING"))
print("func __final__:", getattr(standalone, "__final__", "MISSING"))
print("class still works:", Sealed().__class__.__name__)
print("method still works:", Base().locked())
print("staticmethod still works:", Base.stat())
print("classmethod still works:", Base.cls_m())
print("func still works:", standalone())


# final returns the object unchanged (identity).
def victim():
    return 1


print("identity:", final(victim) is victim)


# final is exported from the module namespace.
import typing

print("in dir(typing):", "final" in dir(typing))
print("in __all__:", "final" in typing.__all__)
