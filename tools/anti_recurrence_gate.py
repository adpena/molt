#!/usr/bin/env python3
"""Advisory classifier for COMPLETE bug-class claims missing durable learning."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from tools import claims_status
from tools.hooks import _common, landing_gate

BUG_CLASS_WORDS = ("bug class", "recurring", "root cause", "class-fix", "metabug")
GATE_MARKERS = ("gate.py", "guard.py", "recurring_harms.toml")
LESSON_MARKERS = ("memory", "lesson", "crux", "docs/design", "docs/agent")


@dataclass(frozen=True)
class Finding:
    lane: str
    message: str


def inspect(claims_text: str, changed_files: Iterable[str]) -> list[Finding]:
    files = tuple(path.replace("\\", "/").lower() for path in changed_files)
    rows = claims_status.parse_rows(claims_text)
    if not rows:
        return []
    has_teeth = any(
        path.startswith("tests/") or any(mark in path for mark in GATE_MARKERS)
        for path in files
    )
    findings: list[Finding] = []
    for row in rows:
        note = row.note.lower()
        if row.status != "COMPLETE" or not any(
            word in note for word in BUG_CLASS_WORDS
        ):
            continue
        has_lesson = any(mark in note for mark in LESSON_MARKERS) or any(
            any(mark in path for mark in LESSON_MARKERS) for path in files
        )
        if has_teeth and has_lesson:
            continue
        missing = []
        if not has_teeth:
            missing.append("anti-recurrence test/gate")
        if not has_lesson:
            missing.append("durable lesson pointer")
        findings.append(Finding(row.lane, "missing " + " and ".join(missing)))
    return findings


def self_test() -> bool:
    claims = "## Log\n| BUG | agent | 2026-07-11T00:00:00Z | COMPLETE | closes recurring bug class |\n"
    return bool(inspect(claims, ["src/x.py"])) and not inspect(
        claims, ["tests/test_x.py", "docs/agent/lesson.md"]
    )


def evaluate(root: Path) -> list[Finding]:
    marker = _common.read_window_marker(root, landing_gate.MARKER_NAME)
    base = (
        marker.get("start_head") if isinstance(marker.get("start_head"), str) else None
    )
    claims_diff = _common.git_window_diff(root, base, "docs/agent/CLAIMS.md")
    claims_text = "## Log\n" + "\n".join(_common.added_lines_from_diff(claims_diff))
    return inspect(claims_text, _common.git_window_files(root, base))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--claims", default="docs/agent/CLAIMS.md")
    parser.add_argument("--changed-file", action="append", default=[])
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    if args.check:
        if not self_test():
            print("anti-recurrence: DEAD self-test")
            return 1
        return 0
    try:
        text = Path(args.claims).read_text(encoding="utf-8", errors="replace")
        for finding in inspect(text, args.changed_file):
            print(f"ADVISORY anti-recurrence [{finding.lane}]: {finding.message}")
    except Exception as exc:
        print(f"ADVISORY anti-recurrence fail-open: {type(exc).__name__}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
