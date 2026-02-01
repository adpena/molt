"""Purpose: differential coverage for math.isclose special cases."""

import math

print(math.isclose(float("inf"), float("inf")))
print(math.isclose(float("inf"), float("-inf")))
print(math.isclose(float("nan"), float("nan")))
