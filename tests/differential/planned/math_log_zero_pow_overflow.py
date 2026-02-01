"""Purpose: differential coverage for math log(0) and pow overflow."""

import math

try:
    math.log(0.0)
except Exception as exc:
    print(type(exc).__name__)

try:
    pow(10.0, 10000)
except Exception as exc:
    print(type(exc).__name__)
