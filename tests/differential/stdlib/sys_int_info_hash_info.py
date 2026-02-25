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
