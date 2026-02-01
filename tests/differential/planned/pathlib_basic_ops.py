"""Purpose: differential coverage for pathlib basic ops."""

from pathlib import Path

p = Path("/tmp") / "file.txt"
print(p.name)
print(p.suffix)
print(p.with_suffix(".log").name)
