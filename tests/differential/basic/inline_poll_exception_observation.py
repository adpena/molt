"""A poll's empty work edge still observes its preceding raising operation."""


class RaisingAdd:
    def __iadd__(self, other):
        raise ValueError("add at latch")


try:
    value = RaisingAdd()
    for i in range(3):
        value += i
except ValueError as exc:
    print(type(exc).__name__, exc)


def fail():
    raise RuntimeError("call return")


try:
    for i in range(3):
        fail()
except RuntimeError as exc:
    print(type(exc).__name__, exc)


try:
    try:
        raise ValueError("original finally exception")
    finally:
        fail()
except RuntimeError as exc:
    print("finally replacement", type(exc).__name__, exc)
    print("finally context", type(exc.__context__).__name__, exc.__context__)
