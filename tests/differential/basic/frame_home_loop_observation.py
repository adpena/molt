"""Purpose: a loop that keeps its int locals in registers still shows each observer the bindings CPython shows."""

import sys


def observe():
    frame = sys._getframe(1)
    return frame.f_locals.get("i"), frame.f_locals.get("total")


def loop_with_call(n):
    i = 0
    total = 0
    seen = []
    while i < n:
        total += i & 7
        if i % 3 == 0:
            seen.append(observe())
        i += 1
    seen.append(observe())
    return total, seen


print("call", loop_with_call(10))


def raise_at(n, stop):
    i = 0
    total = 0
    while i < n:
        total += i
        if i == stop:
            raise ValueError(i)
        i += 1
    return total


def innermost_locals(error):
    tb = error.__traceback__
    while tb.tb_next is not None:
        tb = tb.tb_next
    loc = tb.tb_frame.f_locals
    return loc["i"], loc["total"], tb.tb_lineno - tb.tb_frame.f_code.co_firstlineno


try:
    raise_at(100, 7)
except ValueError as error:
    print("raise", innermost_locals(error))


def overflow(n):
    i = 0
    total = 2**62
    while i < n:
        total += 2**61
        i += 1
    after = observe()
    return total, after


print("overflow", overflow(6))


def overflow_then_raise(n):
    i = 0
    total = 2**62
    while i < n:
        total += 2**61
        if i == 4:
            raise KeyError(total)
        i += 1
    return total


try:
    overflow_then_raise(9)
except KeyError as error:
    print("overflow_raise", innermost_locals(error)[:2])


class Bound:
    def __init__(self, n):
        self.n = n
        self.log = []

    def __gt__(self, other):
        frame = sys._getframe(1)
        self.log.append((frame.f_locals.get("i"), frame.f_locals.get("total")))
        return other < self.n


def loop_bound(bound):
    i = 0
    total = 0
    while i < bound:
        total += i
        i += 1
    return total


bound = Bound(5)
print("bound", loop_bound(bound), bound.log)


def after_loop(n):
    i = 0
    acc = 0
    while i < n:
        acc += 2
        i += 1
    snap = sorted(locals().items())
    del i
    return snap, sorted(locals())


print("after", after_loop(4))


def capture(n):
    frames = []
    i = 0
    while i < n:
        if i == 2:
            frames.append(sys._getframe())
        i += 1
    return frames[0]


print("captured", capture(5).f_locals["i"])


def nested(n):
    total = 0
    row = 0
    while row < n:
        col = 0
        while col < n:
            total += row * col
            col += 1
        if row == 1:
            print("nested_mid", observe(), sys._getframe().f_locals["col"])
        row += 1
    return total, sorted(locals().items())


print("nested", nested(3))


def flags(n):
    i = 0
    flag = False
    ratio = 0.5
    while i < n:
        flag = not flag
        ratio = ratio * 2.0
        i += 1
    frame = sys._getframe()
    return frame.f_locals["flag"], frame.f_locals["ratio"], i


print("flags", flags(5))
