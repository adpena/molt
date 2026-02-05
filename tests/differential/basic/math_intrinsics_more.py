"""Purpose: differential coverage for math fmod/modf/frexp/ldexp intrinsics."""

import math

print("fmod", math.fmod(5.5, 2.0), math.fmod(-5.5, 2.0))
print("modf", math.modf(3.5), math.modf(-3.5), math.modf(-0.0))
print("frexp", math.frexp(0.0), math.frexp(-0.0), math.frexp(8.0), math.frexp(0.75))
print("ldexp", math.ldexp(0.75, 2), math.ldexp(1.0, -2), math.ldexp(-0.5, 3))
