"""timedelta division and abs() in exact microseconds (CPython delta_truedivide,
divide_and_round, microseconds_to_delta), printed without the timedelta repr."""

from datetime import timedelta


def show(label, thunk):
    try:
        value = thunk()
    except Exception as error:
        print(label, type(error).__name__, str(error))
        return
    if isinstance(value, timedelta):
        print(label, (value.days, value.seconds, value.microseconds))
    else:
        print(label, repr(value))


lefts = (
    timedelta(microseconds=7),
    timedelta(microseconds=-7),
    timedelta(microseconds=5),
    timedelta.max,
    timedelta.min,
    timedelta(days=104, microseconds=1),
)
divisors = (True, False, -3, -1, 2, 3, 2**70, 1.5, -0.3, 2.0, 0.0, 1e-300, float("nan"), float("inf"))
for index, left in enumerate(lefts):
    for divisor in divisors:
        show(f"div {index} {divisor!r}", lambda: left / divisor)
    for divisor in (True, -1, 3, 2**70):
        show(f"floor {index} {divisor!r}", lambda: left // divisor)
    for other in (timedelta(microseconds=-3), timedelta(microseconds=7), timedelta(days=3, microseconds=1)):
        show(f"ratio {index} {other.days} {other.microseconds}", lambda: left / other)
    show(f"abs {index}", lambda: abs(left))
    show(f"neg-abs {index}", lambda: abs(-left) if left != timedelta.min else abs(left))

# The constructor accumulates exact microseconds (CPython delta_new/accum).
for label, value in (("max", timedelta.max), ("min", timedelta.min), ("resolution", timedelta.resolution)):
    show(label, lambda: value)
for kwargs in (
    {"days": 999999999, "hours": 23, "minutes": 59, "seconds": 59, "microseconds": 999999},
    {"days": 1e9},
    {"days": "x"},
    {"seconds": float("nan")},
    {"hours": float("inf")},
    {"microseconds": 0.5},
    {"microseconds": 1.5},
    {"microseconds": 2.5},
    {"milliseconds": 0.0005},
    {"days": -0.5, "microseconds": 0.5},
    {"days": 104.25, "microseconds": 7},
    {"weeks": 1.5, "seconds": -1e-7},
    {"days": True, "seconds": 2**40},
):
    show(f"ctor {sorted(kwargs.items())!r}", lambda: timedelta(**kwargs))
