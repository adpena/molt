"""Purpose: differential coverage for math gamma/lgamma domain errors."""

import math

try:
    math.gamma(0.0)
except Exception as exc:
    print(type(exc).__name__)

try:
    math.lgamma(-1.0)
except Exception as exc:
    print(type(exc).__name__)
