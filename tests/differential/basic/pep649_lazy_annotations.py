"""Purpose: PEP 649 lazy annotation evaluation and __annotate__ formats."""

# Requires CPython 3.14+ for PEP 649 parity.

import sys


calls: list[str] = []


def mark(label: str) -> str:
    calls.append(label)
    return label


x: mark("module")


def f(a: mark("arg")) -> mark("ret"):
    return a


class C:
    v = 7
    y: mark("class")
    z: v


print("calls0", calls)
print("f.__annotate__(2)", f.__annotate__(2))
print("calls1", calls)
print("f.__annotations__", f.__annotations__)
print("calls2", calls)
print("C.__annotations__", C.__annotations__)
print("calls3", calls)
print("module.__annotations__", sys.modules[__name__].__annotations__)
print("calls4", calls)


class D:
    pass


def show_del(label: str, func) -> None:
    try:
        func()
        print(label, "ok")
    except Exception as exc:  # noqa: BLE001 - explicit for diff parity
        print(label, type(exc).__name__)


show_del("del D.__annotations__", lambda: delattr(D, "__annotations__"))
show_del("del C.__annotate__", lambda: delattr(C, "__annotate__"))
show_del(
    "del module.__annotate__", lambda: delattr(sys.modules[__name__], "__annotate__")
)
