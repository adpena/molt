"""Purpose: differential coverage for close when finally yields then raises."""


def gen():
    try:
        yield 1
    finally:
        yield 2
        raise RuntimeError("boom")


g = gen()
print("first", next(g))
try:
    g.close()
except Exception as exc:
    print("close", type(exc).__name__)
