"""Purpose: differential coverage for sys.int_info and sys.hash_info."""

import sys

ii = sys.int_info
print("int_info type:", type(ii).__name__)
print("bits_per_digit:", ii.bits_per_digit)
print("sizeof_digit:", ii.sizeof_digit)
hi = sys.hash_info
print("hash_info type:", type(hi).__name__)
print("width:", hi.width)
print("modulus type:", type(hi.modulus).__name__)
print("inf:", hi.inf)
print("nan:", hi.nan)
print("algorithm:", hi.algorithm)

# Metadata and the actual hash operations must share the target hash width.
modulus = hi.modulus
print("modulus boundary:", hash(modulus), hash(modulus + 1), hash(-modulus - 1))
print("numeric equality:", hash(1.5) == hash(3 / 2), hash(float(2**40)) == hash(2**40))
print("hash range:", -(2 ** (hi.width - 1)) <= hash(b"molt metadata") < 2 ** (hi.width - 1))
print("complex multiplier:", hash(1j) == hi.imag)
