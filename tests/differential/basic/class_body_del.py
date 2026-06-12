"""Purpose: differential coverage for ``del`` at class scope.

CPython's class body is a normal block: ``del name`` removes the binding from
the class namespace (DELETE_NAME).  A name deleted in the body must NOT appear
on the finished class.  ``del`` inside control flow must also work.
"""


class DelSimple:
    keep = 1
    temp = 2
    del temp  # temp must not survive onto the class


class DelInIf:
    a = 10
    b = 20
    if a < b:
        del a  # conditional delete -> a removed, b stays


class DelInLoop:
    survivors = []
    x0 = 0
    x1 = 1
    x2 = 2
    for name in ("x0", "x2"):
        # delete via locals-style pattern is not allowed; delete explicit names
        pass
    del x1  # only x1 removed


class DelHelper:
    # A helper used only to compute a value, then deleted so it is not a
    # class attribute (a common idiom for loop/computation scaffolding).
    _scratch = 0
    for _k in range(5):
        _scratch += _k
    result = _scratch
    del _scratch
    del _k


print("DelSimple", DelSimple.keep, hasattr(DelSimple, "temp"))
print("DelInIf", hasattr(DelInIf, "a"), DelInIf.b)
print(
    "DelInLoop",
    hasattr(DelInLoop, "x0"),
    hasattr(DelInLoop, "x1"),
    hasattr(DelInLoop, "x2"),
)
print(
    "DelHelper",
    DelHelper.result,
    hasattr(DelHelper, "_scratch"),
    hasattr(DelHelper, "_k"),
)
