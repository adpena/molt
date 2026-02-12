"""Purpose: differential coverage for functools.singledispatchmethod."""

from functools import singledispatchmethod


class Demo:
    @singledispatchmethod
    def f(self, arg):
        return "default"

    @f.register
    def _(self, arg: int):
        return "int"


if __name__ == "__main__":
    demo = Demo()
    print("int", demo.f(1))
    print("str", demo.f("x"))
