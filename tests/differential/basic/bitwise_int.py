"""Purpose: differential coverage for bitwise int."""

print(f"int_and:{5 & 3}")
print(f"int_or:{5 | 2}")
print(f"int_xor:{5 ^ 3}")
print(f"int_invert:{~5}")
print(f"int_lshift:{5 << 3}")
print(f"int_rshift:{5 >> 1}")

print(f"bool_and:{True & False}")
print(f"bool_or:{True | False}")
print(f"bool_xor:{True ^ False}")
print(f"bool_invert:{~True}")

big = 1 << 70
print(f"big_lshift:{big}")
print(f"big_rshift:{big >> 65}")
print(f"big_xor:{(big ^ (big >> 3)) >> 60}")

try:
    _ = 1 << -1
except ValueError as exc:
    print(f"neg_shift_err:{exc}")
