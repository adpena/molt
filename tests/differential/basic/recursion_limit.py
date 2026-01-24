"""Purpose: differential coverage for recursion limit."""

import sys

print(sys.getrecursionlimit() > 0)
orig = sys.getrecursionlimit()
sys.setrecursionlimit(orig + 5)
print(sys.getrecursionlimit() == orig + 5)


def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)


sys.setrecursionlimit(10)
try:
    recurse(50)
except RecursionError as exc:
    print(f"recursion-error:{exc}")

try:
    sys.setrecursionlimit(0)
except ValueError as exc:
    print(f"recursion-low:{exc}")

try:
    sys.setrecursionlimit("x")
except TypeError as exc:
    print(f"recursion-type:{exc}")
