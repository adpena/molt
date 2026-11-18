"""Purpose: a callback that rewrites its caller's local through
sys._getframe(1).f_locals is seen by the caller on 3.13+ (PEP 667) and not
before; the program holds a frame-locals writer, so its locals stay in frame
homes across callbacks (HF-901)."""

import sys


class Rebinder:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def _rebind(self):
        sys._getframe(2).f_locals[self.name] = self.value

    def __lt__(self, other):
        self._rebind()
        return False

    def __eq__(self, other):
        self._rebind()
        return False

    def __bool__(self):
        self._rebind()
        return True

    __hash__ = None


def straight_line():
    x = 1
    probe = Rebinder("x", 42)
    probe < 0
    return x


def counted_loop():
    total = 0
    for i in range(5):
        total += i
        if i == 2:
            Rebinder("total", 1000) < 0
    return total


def while_loop():
    i = 0
    seen = []
    while i < 6:
        seen.append(i)
        if i == 1:
            Rebinder("i", 4) == 0
        i += 1
    return seen


def truth_test():
    flag = "before"
    if Rebinder("flag", "after"):
        return flag
    return None


def parameter(value):
    Rebinder("value", value * 10) < 0
    return value


print("straight", straight_line())
print("counted", counted_loop())
print("while", while_loop())
print("truth", truth_test())
print("parameter", parameter(3))
