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

from pathlib import PureWindowsPath

for cls, left, right in (
    (PurePosixPath, "a//./b/", "a/b"),
    (PurePosixPath, "", "."),
    (PureWindowsPath, "C:/Folder/./Leaf/", "c:\\folder\\leaf"),
    (PureWindowsPath, "//server/share/./", "\\\\server\\share\\"),
):
    first, second = cls(left), cls(right)
    print("canonical-equality", first == second, hash(first) == hash(second), len({first, second}))
    print("canonical-order", first < second, first <= second, first > second, first >= second)
print("component-order", PurePosixPath("a/x") < PurePosixPath("a-z"))
print("dotdot-distinct", PurePosixPath("a/../b") == PurePosixPath("b"))
print("foreign-flavors", PurePosixPath("a") == PureWindowsPath("a"))
try:
    PurePosixPath("a") < PureWindowsPath("b")
except TypeError as error:
    print("foreign-order", type(error).__name__)
