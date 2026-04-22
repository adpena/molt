#!/usr/bin/env python3
"""Check Lean sorry count against a committed baseline.

Scans all .lean files under formal/lean/ for uses of ``sorry``, excluding
files in Meta/, SorryAudit/, and Completeness/ directories.  Compares the
count to the value stored in ``formal/lean/SORRY_BASELINE``.

Exit codes:
    0 — sorry count is at or below baseline (PASS)
    1 — sorry count exceeds baseline (FAIL)
    2 — usage / data error

Usage:
    python tools/check_lean_sorry_count.py
    python tools/check_lean_sorry_count.py --update  # write current count to baseline
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LEAN_DIR = ROOT / "formal" / "lean"
BASELINE_FILE = LEAN_DIR / "SORRY_BASELINE"

# Directories whose sorrys are expected / audited separately.
EXCLUDED_DIRS = {"Meta", "SorryAudit", "Completeness"}

SORRY_RE = re.compile(r"\bsorry\b")

# ── Terminal helpers ────────────────────────────────────────────────

IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(text: str) -> str:
    return _c("32", text)


def red(text: str) -> str:
    return _c("31", text)


def yellow(text: str) -> str:
    return _c("33", text)


# ── Core logic ──────────────────────────────────────────────────────


def _is_excluded(path: Path) -> bool:
    """Return True if *path* is inside one of the excluded directories."""
    parts = path.relative_to(LEAN_DIR).parts
    return any(part in EXCLUDED_DIRS for part in parts)


def count_sorrys_in_text(text: str) -> int:
    """Count sorry tactics in *text*, ignoring comments and string literals.

    Handles:
    - Block comments: ``/- ... -/`` (including nested)
    - Line comments: ``-- ...``
    - String literals: ``"..."``
    """
    # 1. Remove block comments (handle nesting via repeated passes).
    prev = None
    cleaned = text
    while prev != cleaned:
        prev = cleaned
        cleaned = re.sub(
            r"/\-(?:(?!/\-)(?:(?!\-/).|\n))*?\-/", " ", cleaned, flags=re.DOTALL
        )

    # 2. Process line by line: strip line comments, then string literals.
    total = 0
    for line in cleaned.splitlines():
        # Strip full-line comments.
        stripped = line.lstrip()
        if stripped.startswith("--"):
            continue
        # Strip inline comment (-- not inside a string).
        line = re.sub(r"--.*", "", line)
        # Strip string literals.
        line = re.sub(r'"(?:[^"\\]|\\.)*"', '""', line)
        total += len(SORRY_RE.findall(line))
    return total


def count_sorrys() -> int:
    """Count sorry occurrences in non-excluded .lean files."""
    total = 0
    for _count, _path in count_sorrys_by_file():
        total += _count
    return total


def count_sorrys_by_file() -> list[tuple[int, Path]]:
    """Return per-file sorry counts for non-excluded .lean files."""
    counts: list[tuple[int, Path]] = []
    for lean_file in sorted(LEAN_DIR.rglob("*.lean")):
        if _is_excluded(lean_file):
            continue
        try:
            text = lean_file.read_text(encoding="utf-8")
        except OSError:
            continue
        cnt = count_sorrys_in_text(text)
        if cnt:
            counts.append((cnt, lean_file))
    return counts


def read_baseline() -> int:
    """Read the integer baseline from SORRY_BASELINE."""
    if not BASELINE_FILE.exists():
        print(red(f"ERROR: baseline file not found: {BASELINE_FILE}"), file=sys.stderr)
        sys.exit(2)
    try:
        return int(BASELINE_FILE.read_text(encoding="utf-8").strip())
    except ValueError:
        print(
            red(f"ERROR: baseline file does not contain an integer: {BASELINE_FILE}"),
            file=sys.stderr,
        )
        sys.exit(2)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Check Lean sorry count against baseline"
    )
    parser.add_argument(
        "--update",
        action="store_true",
        help="Write current sorry count to SORRY_BASELINE and exit",
    )
    args = parser.parse_args()

    current = count_sorrys()

    if args.update:
        BASELINE_FILE.write_text(f"{current}\n", encoding="utf-8")
        print(f"Updated {BASELINE_FILE} to {current}")
        return

    baseline = read_baseline()

    if current > baseline:
        print(red(f"Sorry count: {current} (baseline: {baseline}) — FAIL"))
        print(
            f"The sorry count increased by {current - baseline}. "
            "Please resolve new sorrys before merging."
        )
        file_counts = count_sorrys_by_file()
        if file_counts:
            print("Open sorrys by file:")
            for cnt, path in file_counts:
                print(f"  {cnt:>2}  {path.relative_to(ROOT)}")
        sys.exit(1)
    elif current < baseline:
        print(green(f"Sorry count: {current} (baseline: {baseline}) — PASS"))
        print(
            yellow(
                f"Sorry count decreased by {baseline - current}. "
                f"Consider updating the baseline:\n"
                f"  echo {current} > formal/lean/SORRY_BASELINE"
            )
        )
    else:
        print(green(f"Sorry count: {current} (baseline: {baseline}) — PASS"))


if __name__ == "__main__":
    main()
