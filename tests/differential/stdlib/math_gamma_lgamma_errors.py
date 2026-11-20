"""Purpose: differential coverage for math domain errors and math.pow.

Through 3.13 every domain error says "math domain error"; from 3.14 most
functions name their expectation and the rejected value, while fmod,
remainder and pow keep the old message.
"""

import math

try:
    math.gamma(0.0)
except Exception as exc:
    print(type(exc).__name__)

try:
    math.lgamma(-1.0)
except Exception as exc:
    print(type(exc).__name__)

NEG_INF = float("-inf")
INF = float("inf")
cases = [
    ("acos", (2.0,)),
    ("asin", (-2.0,)),
    ("acosh", (0.5,)),
    ("atanh", (1.0,)),
    ("atanh", (-3.0,)),
    ("sqrt", (-1.0,)),
    ("sqrt", (NEG_INF,)),
    ("log", (0.0,)),
    ("log", (-2.5,)),
    ("log10", (-1.0,)),
    ("log2", (0.0,)),
    ("log1p", (-1.0,)),
    ("log1p", (-4.0,)),
    ("cos", (INF,)),
    ("sin", (NEG_INF,)),
    ("tan", (INF,)),
    ("gamma", (0.0,)),
    ("gamma", (-2.0,)),
    ("lgamma", (0.0,)),
    ("lgamma", (-3.0,)),
    ("fmod", (1.0, 0.0)),
    ("fmod", (INF, 1.0)),
    ("remainder", (1.0, 0.0)),
    ("pow", (0.0, -1.0)),
    ("pow", (-1.0, 0.5)),
]
for name, args in cases:
    try:
        print(name, args, getattr(math, name)(*args))
    except ValueError as exc:
        print(name, args, "ValueError", exc)

# math.pow decides IEEE specials itself (CPython's math_pow), leaves finite
# operands to libm, and reports overflow as OverflowError.
NAN = float("nan")
for args in (
    (2.0, 10.0),
    (2, -2),
    (-8.0, 1.0 / 3.0),
    (0.0, 0.0),
    (-0.0, -3.0),
    (NAN, 0.0),
    (1.0, NAN),
    (INF, -3.0),
    (NEG_INF, 3.0),
    (NEG_INF, -3.0),
    (NEG_INF, 2.0),
    (0.5, INF),
    (2.0, NEG_INF),
    (-1.0, INF),
    (10.0, 400.0),
):
    try:
        print("pow", args, repr(math.pow(*args)))
    except (ValueError, OverflowError) as exc:
        print("pow", args, type(exc).__name__, exc)
