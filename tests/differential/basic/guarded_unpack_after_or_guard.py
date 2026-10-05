"""Unpacking after a short-circuit `or` guard keeps the element values."""


def source(value):
    return value


def guarded_or(value):
    pair = source(value)
    if not isinstance(pair, tuple) or len(pair) != 2:
        raise RuntimeError("bad shape")
    first, second = pair
    return first, second


def guarded_and(value):
    pair = source(value)
    if isinstance(pair, tuple) and len(pair) == 2:
        first, second = pair
        return first, second
    raise RuntimeError("bad shape")


def guarded_or_three(value):
    triple = source(value)
    if not isinstance(triple, tuple) or len(triple) != 3:
        raise RuntimeError("bad shape")
    a, b, c = triple
    return a, b, c


for fn, value in (
    (guarded_or, (None, None)),
    (guarded_or, (1, "x")),
    (guarded_or, (0.5, [1, 2])),
    (guarded_and, (None, None)),
    (guarded_or_three, (True, None, 7)),
):
    print(fn.__name__, fn(value))

try:
    guarded_or([1, 2])
except RuntimeError as exc:
    print("rejected", exc)
