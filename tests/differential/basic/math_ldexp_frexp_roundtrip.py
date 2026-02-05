"""Purpose: differential coverage for math ldexp/frexp round-trip."""

import math

value = 6.5
mant, exp = math.frexp(value)
print(math.ldexp(mant, exp))
