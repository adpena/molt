"""Purpose: differential coverage for math rounding on -0.0."""

import math

neg_zero = -0.0
print(math.copysign(1.0, math.floor(neg_zero)))
print(math.copysign(1.0, math.ceil(neg_zero)))
