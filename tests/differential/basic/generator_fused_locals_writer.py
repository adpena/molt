# MOLT_META: min_py=3.13
"""A PEP 667 write through f_locals rebinds a running generator's local."""

import sys


class Rebinder:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def __lt__(self, other):
        sys._getframe(1).f_locals[self.name] = self.value
        return False


def rewritten(n):
    for i in range(n):
        step = 1
        if i == 1:
            Rebinder("step", 100) < 0
        yield i * step


def consumer():
    out = []
    for v in rewritten(4):
        out.append(v)
    return out


print(consumer())
