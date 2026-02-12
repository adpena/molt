"""Purpose: differential coverage for math.factorial errors."""

import math

try:
    math.factorial(-1)
except Exception as exc:
    print(type(exc).__name__)
