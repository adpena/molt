"""Purpose: validate pinned upstream pyperformance manifest catalog wiring."""

from pathlib import Path
import re


GROUP_HEADER_RE = re.compile(r"^\[group ([^\]]+)\]\s*$")
MANIFEST = (
    Path(__file__).resolve().parent / "fixtures" / "pyperformance_manifest_1_14_0.txt"
)

benchmark_names: list[str] = []
groups: list[str] = []
section: str | None = None

for raw_line in MANIFEST.read_text(encoding="utf-8").splitlines():
    line = raw_line.strip()
    if not line or line.startswith("#"):
        continue
    if line == "[benchmarks]":
        section = "benchmarks"
        continue
    group_match = GROUP_HEADER_RE.match(line)
    if group_match is not None:
        section = "group"
        groups.append(group_match.group(1))
        continue
    if line.startswith("["):
        section = None
        continue
    if section != "benchmarks":
        continue
    if line.startswith("name") and "metafile" in line:
        continue
    parts = line.split()
    if not parts:
        continue
    benchmark_names.append(parts[0])

group_set = sorted(set(groups))
print(f"benchmark_count={len(benchmark_names)}")
print("groups=" + ",".join(group_set))
print("contains_nbody=" + str("nbody" in benchmark_names))
print("contains_fannkuch=" + str("fannkuch" in benchmark_names))
print("contains_default_group=" + str("default" in group_set))
