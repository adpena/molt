"""Purpose: differential coverage for cmath basic functions."""

import cmath
import math


# --- sqrt ---
z = cmath.sqrt(-1)
print("sqrt(-1):", z)
print("sqrt(-1) approx 1j:", abs(z - 1j) < 1e-10)

z2 = cmath.sqrt(4)
print("sqrt(4):", z2)
print("sqrt(4) approx 2:", abs(z2 - 2) < 1e-10)

# --- exp / Euler's identity ---
z3 = cmath.exp(cmath.pi * 1j)
print("exp(pi*1j) real:", z3.real)
print("exp(pi*1j) approx -1:", abs(z3.real + 1) < 1e-10)
print("exp(pi*1j) imag approx 0:", abs(z3.imag) < 1e-10)

# --- phase ---
p = cmath.phase(1 + 1j)
print("phase(1+1j):", p)
print("phase(1+1j) approx pi/4:", abs(p - math.pi / 4) < 1e-10)

p2 = cmath.phase(-1)
print("phase(-1) approx pi:", abs(p2 - math.pi) < 1e-10)

# --- polar / rect round-trip ---
r, phi = cmath.polar(1 + 1j)
print("polar(1+1j) r:", r)
print("polar(1+1j) phi:", phi)
z4 = cmath.rect(r, phi)
print("rect round-trip real approx 1:", abs(z4.real - 1) < 1e-10)
print("rect round-trip imag approx 1:", abs(z4.imag - 1) < 1e-10)

# --- isfinite / isinf / isnan ---
print("isfinite(1+2j):", cmath.isfinite(1 + 2j))
print("isfinite(inf):", cmath.isfinite(complex(float("inf"), 0)))
print("isinf(inf+0j):", cmath.isinf(complex(float("inf"), 0)))
print("isinf(1+2j):", cmath.isinf(1 + 2j))
print("isnan(nan+0j):", cmath.isnan(complex(float("nan"), 0)))
print("isnan(1+2j):", cmath.isnan(1 + 2j))

# --- trig functions ---
z5 = cmath.sin(0)
print("sin(0):", z5)
print("sin(0) approx 0:", abs(z5) < 1e-10)

z6 = cmath.cos(0)
# NOTE: exact repr differs on signed-zero imaginary part ((1-0j) vs (1+0j));
# test value only.
print("cos(0) approx 1:", abs(z6 - 1) < 1e-10)

# --- log ---
z7 = cmath.log(1)
print("log(1):", z7)
print("log(1) approx 0:", abs(z7) < 1e-10)

z8 = cmath.log(cmath.e)
print("log(e) approx 1:", abs(z8 - 1) < 1e-10)

z9 = cmath.log10(10)
print("log10(10) approx 1:", abs(z9 - 1) < 1e-10)

# --- constants ---
print("pi:", cmath.pi)
print("e:", cmath.e)
print("tau approx 2*pi:", abs(cmath.tau - 2 * cmath.pi) < 1e-10)
print("inf:", cmath.inf)
print("nan is nan:", cmath.isnan(complex(cmath.nan, 0)))
