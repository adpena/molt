"""Behavior: math.isfinite/math.isnan accept Decimal inputs and report NaN status.
Why: Decimal participates in the numeric tower; math predicates must match CPython.
Pitfalls: Decimal NaN payloads shouldn't raise; relies on CPython's Decimal coercion.
"""

import math
from decimal import Decimal

print(math.isfinite(Decimal("1.0")))
print(math.isnan(Decimal("NaN")))
