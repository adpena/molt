"""Purpose: differential coverage for yield from StopIteration.value propagation."""


def sub():
    yield 1
    return "done"


def main():
    res = yield from sub()
    return res


g = main()
print("first", next(g))
try:
    next(g)
except StopIteration as exc:
    print("value", exc.value)
