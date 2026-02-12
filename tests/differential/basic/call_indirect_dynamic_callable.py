"""Purpose: differential coverage for call_indirect dynamic callable dispatch."""


class Adder:
    def __init__(self, bias):
        self.bias = bias

    def __call__(self, value):
        return self.bias + value


def mult_factory(factor):
    return lambda value: factor * value


callables = [Adder(5), mult_factory(3), lambda value: value - 2]
for fn in callables:
    print(fn(7))
