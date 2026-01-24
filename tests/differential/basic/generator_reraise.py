"""Purpose: differential coverage for generator reraise."""


def gen():
    try:
        raise TypeError("foo")
    except:
        yield 1
        raise


g = gen()
print(next(g))
try:
    next(g)
except Exception as exc:
    print(type(exc).__name__, str(exc))

try:
    next(g)
except Exception as exc:
    print(type(exc).__name__)
