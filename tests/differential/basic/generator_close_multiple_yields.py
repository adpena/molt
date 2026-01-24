"""Purpose: differential coverage for close when finally yields multiple times."""


def gen():
    try:
        yield 1
    finally:
        yield 2
        yield 3


g = gen()
print("first", next(g))
try:
    g.close()
except Exception as exc:
    print("close", type(exc).__name__)
