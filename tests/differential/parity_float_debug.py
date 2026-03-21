"""Debug: show exact values for the two failing sum() tests on molt."""
print("sum([0.1]*10):", repr(sum([0.1] * 10)))
print("expected:      ", repr(1.0))
print("equal?         ", sum([0.1] * 10) == 1.0)
print()
print("sum([1e100, 1.0, -1e100, 1.0]):", repr(sum([1e100, 1.0, -1e100, 1.0])))
print("expected:                       ", repr(2.0))
print("equal?                          ", sum([1e100, 1.0, -1e100, 1.0]) == 2.0)

# Also check naive addition to see if it's a sum()-specific issue
total = 0.0
for v in [0.1] * 10:
    total += v
print()
print("manual loop sum [0.1]*10:", repr(total))
print("manual == 1.0?          ", total == 1.0)
