"""Purpose: differential coverage for generator throw(GeneratorExit)."""


def gen():
    try:
        yield 1
    finally:
        print("finally")


g = gen()
print("first", next(g))
try:
    g.throw(GeneratorExit())
except Exception as exc:
    print("throw", type(exc).__name__)
