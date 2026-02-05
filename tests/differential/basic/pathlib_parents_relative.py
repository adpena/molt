"""Purpose: differential coverage for pathlib parents and relative_to errors."""

from pathlib import PurePosixPath

p = PurePosixPath("/a/b/c.txt")
print("parents", [str(x) for x in p.parents][:3])

try:
    print("rel", p.relative_to("/x"))
except Exception as exc:
    print("rel_err", type(exc).__name__)
