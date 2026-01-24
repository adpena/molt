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
