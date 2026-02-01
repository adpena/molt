"""Purpose: differential coverage for math predicates/constants."""

import math

print(math.isfinite(1.0))
print(math.isfinite(float("inf")))
print(math.isnan(float("nan")))
print(math.isinf(float("inf")))
print(round(math.pi, 6))
