"""Purpose: differential coverage for loop target leakage and comprehension scope."""

for i in range(2):
    pass
print("for_after", i)

vals = [j for j in range(3)]
print("listcomp", vals)
try:
    j
except Exception as exc:
    print("listcomp_after", type(exc).__name__)
