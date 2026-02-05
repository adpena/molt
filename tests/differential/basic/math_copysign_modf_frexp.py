"""Purpose: differential coverage for math copysign/modf/frexp."""

import math

print(math.copysign(1.0, -0.0))
print(math.modf(1.25))
print(math.frexp(8.0))
