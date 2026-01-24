"""Purpose: differential coverage for PurePath comparisons and ordering."""

from pathlib import PurePosixPath

p1 = PurePosixPath("/a/b")
p2 = PurePosixPath("/a/b")
p3 = PurePosixPath("/a/c")

print("eq", p1 == p2, p1 == p3)
try:
    print("lt", p1 < p3)
except Exception as exc:
    print("lt", type(exc).__name__)
