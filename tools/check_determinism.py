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

# Regex for any HashMap/HashSet mention
_ANY_HASHMAP_RE = re.compile(r"\bHash(?:Map|Set)\b")


def _classify_hashmap_usages(
    lines: list[str],
) -> tuple[list[tuple[int, str]], list[tuple[int, str]]]:
    """Classify HashMap/HashSet usages as struct-field or other.

    Returns (struct_findings, other_findings) as lists of (line_number, line_text).
    Uses brace-depth tracking to determine if we're inside a struct/enum definition.
    """
    struct_findings: list[tuple[int, str]] = []
    other_findings: list[tuple[int, str]] = []

    in_struct = False
    brace_depth = 0
    struct_brace_depth = 0

    for i, line in enumerate(lines, 1):
        stripped = line.strip()

        # Skip comments and use/import lines
        if stripped.startswith("//") or stripped.startswith("use "):
            if _ANY_HASHMAP_RE.search(line):
                other_findings.append((i, stripped))
            continue

        # Detect struct/enum definitions (the line before the opening brace)
        if re.match(r"^(?:pub\s+)?(?:struct|enum)\s+\w+", stripped):
            in_struct = True
            struct_brace_depth = brace_depth

        # Track brace depth
        brace_depth += line.count("{") - line.count("}")

        # Detect end of struct/enum
        if in_struct and brace_depth <= struct_brace_depth:
            in_struct = False

        if not _ANY_HASHMAP_RE.search(line):
            continue

        if in_struct and brace_depth > struct_brace_depth:
            # We're inside a struct/enum body — this is a field declaration
            struct_findings.append((i, stripped))
        else:
            other_findings.append((i, stripped))

    return struct_findings, other_findings


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
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

    struct_total: list[tuple[str, int, str]] = []
    local_total: list[tuple[str, int, str]] = []

    for rs_file in sorted(BACKEND_SRC.rglob("*.rs")):
        content = rs_file.read_text()
        lines = content.splitlines()
        rel = str(rs_file.relative_to(Path(".")))

        struct_findings, other_findings = _classify_hashmap_usages(lines)
        for lineno, text in struct_findings:
            struct_total.append((rel, lineno, text))
        for lineno, text in other_findings:
            local_total.append((rel, lineno, text))

    if struct_total:
        print(
            f"STRUCT FIELDS: {len(struct_total)} HashMap/HashSet in struct fields "
            f"(iteration order can leak into output):"
        )
        for path, lineno, content in struct_total:
            print(f"  {path}:{lineno}: {content}")
    else:
        print("No HashMap/HashSet in struct fields. Emission-path determinism OK.")

    if args.all and local_total:
        print(
            f"\nFUNCTION-LOCAL: {len(local_total)} HashMap/HashSet in function "
            f"bodies (safe — lookup only, no emission ordering):"
        )
        for path, lineno, content in local_total:
            print(f"  {path}:{lineno}: {content}")

    if struct_total and args.strict:
        print("\nSTRICT MODE: struct-field HashMap/HashSet is disallowed.")
        print(
            "Use BTreeMap/BTreeSet for any map that persists beyond a single function."
        )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
