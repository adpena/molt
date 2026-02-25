"""Purpose: differential coverage for sys.float_info named tuple."""

import sys

fi = sys.float_info
print("type:", type(fi).__name__)
print("max type:", type(fi.max).__name__)
print("max > 0:", fi.max > 0)
print("min > 0:", fi.min > 0)
print("epsilon > 0:", fi.epsilon > 0)
print("dig:", fi.dig)
print("mant_dig:", fi.mant_dig)
print("radix:", fi.radix)
print("max_exp:", fi.max_exp)
print("min_exp:", fi.min_exp)
print("max_10_exp:", fi.max_10_exp)
print("min_10_exp:", fi.min_10_exp)
print("rounds:", fi.rounds)
print("len:", len(fi))
