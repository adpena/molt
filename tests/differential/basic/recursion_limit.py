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

sys.setrecursionlimit(orig)


def lower_limit_inside_calls(n):
    if n:
        return lower_limit_inside_calls(n - 1)
    before = sys.getrecursionlimit()
    try:
        sys.setrecursionlimit(2)
    except RecursionError:
        print("active-depth-rejected", sys.getrecursionlimit() == before)
    else:
        print("active-depth-incorrectly-accepted")
        sys.setrecursionlimit(before)


lower_limit_inside_calls(8)
print("unwound", recurse(4))
sys.setrecursionlimit(orig)
