"""Purpose: differential coverage for generator close when finally yields."""


def gen():
    try:
        yield 1
    finally:
        yield 2


g = gen()
print("first", next(g))
try:
    g.close()
except Exception as exc:
    print("close", type(exc).__name__)
