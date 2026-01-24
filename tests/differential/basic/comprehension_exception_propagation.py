"""Purpose: differential coverage for comprehension exception propagation."""


def boom():
    raise RuntimeError("boom")


def guard(value):
    if value == 1:
        boom()
    return True


try:
    [x for x in range(3) if guard(x)]
except Exception as exc:
    print("listcomp", type(exc).__name__)

try:
    {x: x for x in range(3) if guard(x)}
except Exception as exc:
    print("dictcomp", type(exc).__name__)

try:
    {x for x in range(3) if guard(x)}
except Exception as exc:
    print("setcomp", type(exc).__name__)

try:
    list(x for x in range(3) if guard(x))
except Exception as exc:
    print("genexpr", type(exc).__name__)
