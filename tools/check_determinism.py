#!/usr/bin/env python3
"""Lint: detect HashMap usage in codegen struct fields that could cause nondeterminism.

Usage: python tools/check_determinism.py [--strict] [--all]

Scans runtime/molt-backend/src/ for HashMap/HashSet usage in struct field
declarations (which can leak iteration order into emission). Function-local
HashMaps used for analysis/lookup are safe and ignored by default.

In --strict mode, exits non-zero if any struct-field HashMap is found.
In --all mode, reports all usages (including function-local).
"""

import argparse
import re
import sys
from pathlib import Path

BACKEND_SRC = Path("runtime/molt-backend/src")

# Regex for struct field declarations: lines like "  field_name: HashMap<...>"
_STRUCT_FIELD_RE = re.compile(r"^\s+\w+:\s+.*\bHash(?:Map|Set)\b")
# Regex for any HashMap/HashSet mention
_ANY_HASHMAP_RE = re.compile(r"\bHash(?:Map|Set)\b")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Exit non-zero on any struct-field HashMap/HashSet usage",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Report all HashMap/HashSet usages, not just struct fields",
    )
    args = parser.parse_args()

    if not BACKEND_SRC.exists():
        print(f"WARNING: {BACKEND_SRC} not found, skipping", file=sys.stderr)
        return 0

    struct_findings: list[tuple[str, int, str]] = []
    local_findings: list[tuple[str, int, str]] = []

    for rs_file in sorted(BACKEND_SRC.rglob("*.rs")):
        for i, line in enumerate(rs_file.read_text().splitlines(), 1):
            if not _ANY_HASHMAP_RE.search(line):
                continue
            if line.strip().startswith("//"):
                continue
            rel = str(rs_file.relative_to(Path(".")))
            if _STRUCT_FIELD_RE.match(line):
                struct_findings.append((rel, i, line.strip()))
            else:
                local_findings.append((rel, i, line.strip()))

    if struct_findings:
        print(
            f"STRUCT FIELDS: {len(struct_findings)} HashMap/HashSet in struct fields "
            f"(iteration order can leak into output):"
        )
        for path, lineno, content in struct_findings:
            print(f"  {path}:{lineno}: {content}")
    else:
        print("No HashMap/HashSet in struct fields. Emission-path determinism OK.")

    if args.all and local_findings:
        print(
            f"\nFUNCTION-LOCAL: {len(local_findings)} HashMap/HashSet in function "
            f"bodies (safe — lookup only, no emission ordering):"
        )
        for path, lineno, content in local_findings:
            print(f"  {path}:{lineno}: {content}")

    if struct_findings and args.strict:
        print("\nSTRICT MODE: struct-field HashMap/HashSet is disallowed.")
        print(
            "Use BTreeMap/BTreeSet for any map that persists beyond a single function."
        )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
