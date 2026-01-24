"""Purpose: differential coverage for positional-only/kw-only call errors."""


def f(a, /, b, *, c):
    return a, b, c


try:
    f(a=1, b=2, c=3)
except Exception as exc:
    print("posonly", type(exc).__name__)

try:
    f(1, 2, 3)
except Exception as exc:
    print("kwonly", type(exc).__name__)

print("ok", f(1, b=2, c=3))
