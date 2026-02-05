"""Purpose: differential coverage for core math intrinsics."""

import math

values = [0.0, -0.0, 1.5, -2.5, 3, -4, float("inf"), float("-inf"), float("nan")]

for value in values:
    print("isfinite", value, math.isfinite(value))
    print("isinf", value, math.isinf(value))
    print("isnan", value, math.isnan(value))

print("fabs", math.fabs(-0.0), math.fabs(-3.5))
print("copysign", math.copysign(3.0, -0.0), math.copysign(-2.0, 5.0))
print("sqrt", math.sqrt(0.0), math.sqrt(4.0), math.sqrt(2.25))
print("floor", math.floor(1.2), math.floor(-1.2), math.floor(3.0))
print("ceil", math.ceil(1.2), math.ceil(-1.2), math.ceil(3.0))
print("trunc", math.trunc(1.2), math.trunc(-1.2), math.trunc(3.0))
