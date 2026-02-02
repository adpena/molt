"""Purpose: differential coverage for operator.methodcaller with kwargs."""

import operator


class Demo:
    def f(self, a, b=0):
        return a + b


if __name__ == "__main__":
    caller = operator.methodcaller("f", 2, b=3)
    print("result", caller(Demo()))
