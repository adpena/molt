"""Purpose: a returning frame releases its locals in the target's slot order, and each finalizer sees the caller as the executing frame."""

import sys

events = []


class Value:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        frame = sys._getframe(1)
        events.append((self.label, frame.f_code.co_name))


def two_locals():
    first = Value("first")
    second = Value("second")
    return first.label + second.label


def many_locals():
    a = Value("a")
    b = Value("b")
    c = Value("c")
    d = Value("d")
    e = Value("e")
    f = Value("f")
    g = Value("g")
    h = Value("h")
    i = Value("i")
    j = Value("j")
    return len([a, b, c, d, e, f, g, h, i, j])


def raising():
    kept = Value("kept across raise")
    other = Value("other across raise")
    raise ValueError(kept.label + other.label)


def caller():
    two_locals()
    many_locals()
    try:
        raising()
    except ValueError as exc:
        events.append(("caught", str(exc)))
        # The traceback keeps the frame, and so its locals, alive.
        exc = None


if __name__ == "__main__":
    caller()
    for event in events:
        print(*event)
