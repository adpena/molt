"""Purpose: differential coverage for pathlib basic surface (string/parts/parents)."""

from pathlib import Path


p = Path("a") / "b" / "c.txt"
print(str(p))
print(p.parts)
print(p.name)
print(p.suffix)
print(p.stem)
print(str(p.parent))
print([str(x) for x in p.parents])

q = Path("/a/b/c.txt")
print(str(q.relative_to("/a")))
print(q.is_relative_to("/a"))
print(q.is_relative_to("/x"))
