"""Regression for custom-object loop phi ownership.

A fresh object local is used both as a stable RHS operand and as the initial
value of a loop-carried accumulator. The phi entry edge must retain the local
for the accumulator without later retaining a stale loop-carried object.
"""


class Box:
    def __init__(self, v):
        self.v = v

    def __matmul__(self, other):
        return Box(self.v if self.v == other.v else self.v + other.v)


def loop_matmul_obj(base_v, n):
    base = Box(base_v)
    x = base
    i = 0
    while i < n:
        x = x @ base
        i += 1
    return x.v


BIG = 1 << 60
print(loop_matmul_obj(BIG, 0))
print(loop_matmul_obj(BIG, 7))
print(loop_matmul_obj(BIG, 200))
