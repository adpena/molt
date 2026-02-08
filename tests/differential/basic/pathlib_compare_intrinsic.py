"""Purpose: validate intrinsic-backed pathlib comparison semantics."""

from pathlib import Path


a = Path("/tmp/a")
b = Path("/tmp/b")
c = Path("/tmp/a")

print(a < b)
print(a <= b)
print(b > a)
print(b >= a)
print(a == c)
print(a != b)
