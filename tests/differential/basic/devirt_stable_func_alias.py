"""Differential coverage for stable module function aliases."""


def f(x):
    return x * 2


def g():
    h = f
    return h(5) + h(6)


def left():
    return "left"


def right():
    return "right"


def pick(flag):
    # Each branch binds a different function: the call reads the binding
    # the executed branch left, not the one lowered last.
    if flag:
        chosen = left
    else:
        chosen = right
    return chosen()


lf = f
print(g())
print(lf(3))
print(f(7))
print(pick(True), pick(False))
