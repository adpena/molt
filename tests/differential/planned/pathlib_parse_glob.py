"""Purpose: differential coverage for pathlib path parsing and matching."""

from pathlib import PurePosixPath

p = PurePosixPath("/a/b/c.txt")
print("parts", p.parts)
print("parent", p.parent)
print("suffix", p.suffix)
print("stem", p.stem)
print("with_suffix", p.with_suffix(".md"))
print("relative", p.relative_to("/a"))
print("match", p.match("**/*.txt"))
print("match2", p.match("*.md"))
