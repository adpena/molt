"""Purpose: differential coverage for operator.methodcaller."""

import operator


class Demo:
    def scale(self, value):
        return value * 2


if __name__ == "__main__":
    caller = operator.methodcaller("scale", 3)
    print("result", caller(Demo()))
