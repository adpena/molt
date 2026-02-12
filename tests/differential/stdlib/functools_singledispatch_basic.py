"""Purpose: differential coverage for functools.singledispatch."""

from functools import singledispatch


@singledispatch
def f(arg):
    return "default"


@f.register

def _(arg: int):
    return "int"


if __name__ == "__main__":
    print("int", f(1))
    print("str", f("x"))
