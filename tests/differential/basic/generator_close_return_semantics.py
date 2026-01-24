"""Purpose: differential coverage for generator close return semantics."""


def gen():
    try:
        yield 1
    finally:
        return "done"


g = gen()
print("first", next(g))
try:
    g.close()
    print("close", "ok")
except Exception as exc:
    print("close", type(exc).__name__)
