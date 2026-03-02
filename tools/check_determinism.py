#!/usr/bin/env python3
"""Lint: detect HashMap usage in codegen paths that could cause nondeterminism.

Usage: python tools/check_determinism.py [--strict]

Scans runtime/molt-backend/src/ for HashMap/HashSet usage and reports
locations. In --strict mode, exits non-zero if any HashMap is found
(excluding explicitly allowlisted patterns).
"""

import argparse
import re
import sys
from pathlib import Path

BACKEND_SRC = Path("runtime/molt-backend/src")

# Patterns that are known-safe (lookup-only, never iterated for emission)
ALLOWLIST = [
    # Add patterns here as: (filename, line_content_substring)
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Exit non-zero on any HashMap usage",
    )
    args = parser.parse_args()

    if not BACKEND_SRC.exists():
        print(f"WARNING: {BACKEND_SRC} not found, skipping", file=sys.stderr)
        return 0

    findings: list[tuple[str, int, str]] = []
    pattern = re.compile(r"\bHash(?:Map|Set)\b")

    for rs_file in sorted(BACKEND_SRC.rglob("*.rs")):
        for i, line in enumerate(rs_file.read_text().splitlines(), 1):
            if pattern.search(line) and not line.strip().startswith("//"):
                rel = rs_file.relative_to(Path("."))
                # Check allowlist
                allowed = any(
                    str(rel).endswith(af) and asub in line for af, asub in ALLOWLIST
                )
                if not allowed:
                    findings.append((str(rel), i, line.strip()))

    if findings:
        print(f"Found {len(findings)} HashMap/HashSet usage(s) in codegen paths:")
        for path, lineno, content in findings:
            print(f"  {path}:{lineno}: {content}")
        if args.strict:
            print("\nSTRICT MODE: HashMap/HashSet in codegen is disallowed.")
            print("Use BTreeMap/BTreeSet or add to ALLOWLIST with justification.")
            return 1
    else:
        print("No HashMap/HashSet usage found in codegen paths.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
