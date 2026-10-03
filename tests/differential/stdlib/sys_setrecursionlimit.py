"""Purpose: differential coverage for sys setrecursionlimit."""

import sys


old = sys.getrecursionlimit()
sys.setrecursionlimit(old + 10)
print(sys.getrecursionlimit() == old + 10)

try:
    sys.setrecursionlimit(0)
except Exception as exc:
    print(type(exc).__name__)

sys.setrecursionlimit(old)


class RecursionIndex:
    def __index__(self):
        return 2000


try:
    for limit in [RecursionIndex(), 2**31 - 1, 2**31, 2**50, 2**80, -(2**40), 700.0]:
        before = sys.getrecursionlimit()
        try:
            sys.setrecursionlimit(limit)
            print("recursion index:", sys.getrecursionlimit())
        except (TypeError, OverflowError) as exc:
            print("recursion range:", type(exc).__name__, sys.getrecursionlimit() == before)
finally:
    sys.setrecursionlimit(old)
