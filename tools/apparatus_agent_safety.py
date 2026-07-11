#!/usr/bin/env python3
"""Semantic source gate: apparatus code cannot actuate process termination."""

from __future__ import annotations

import argparse
import ast
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

ROOT = Path(__file__).resolve().parents[1]
SCAN_PATHS = (
    "tools/hooks/_common.py",
    "tools/hooks/bash_guard.py",
    "tools/hooks/landing_gate.py",
    "tools/hooks/path_guard.py",
    "tools/hooks/session_digest.py",
    "tools/hooks/session_learning.py",
    "tools/hooks/stop_gates.py",
    "tools/hooks/waivers.py",
    "tools/forbidden_checkout_guard.py",
    "tools/anti_recurrence_gate.py",
    "tools/apparatus_agent_safety.py",
    "tools/disk_guard.py",
)
FORBIDDEN_CALLS = frozenset({"kill", "killpg", "terminate", "send_signal", "Popen"})
FORBIDDEN_NAMES = frozenset({"SIGTERM", "SIGKILL", "taskkill", "TerminateProcess"})


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    symbol: str


def scan_source(source: str, path: str = "<memory>") -> list[Violation]:
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError):
        return []
    out: list[Violation] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            name = (
                node.func.id
                if isinstance(node.func, ast.Name)
                else node.func.attr
                if isinstance(node.func, ast.Attribute)
                else ""
            )
            if name in FORBIDDEN_CALLS:
                out.append(Violation(path, node.lineno, name))
        elif isinstance(node, ast.Name) and node.id in FORBIDDEN_NAMES:
            out.append(Violation(path, node.lineno, node.id))
    return out


def iter_paths(root: Path = ROOT) -> Iterable[Path]:
    for relpath in SCAN_PATHS:
        yield root / relpath


def scan_tree(root: Path = ROOT) -> list[Violation]:
    violations: list[Violation] = []
    for path in iter_paths(root):
        try:
            violations.extend(
                scan_source(
                    path.read_text(encoding="utf-8"), str(path.relative_to(root))
                )
            )
        except OSError:
            continue
    return violations


def self_test() -> bool:
    bad = scan_source("import os\nos.kill(7, 9)\nsubprocess.Popen(['x'])\n")
    good = scan_source(
        "import subprocess\nsubprocess.run(['git', 'status'], timeout=1)\n"
    )
    return {v.symbol for v in bad} == {"kill", "Popen"} and not good


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    if args.check and not self_test():
        print("apparatus-agent-safety: DEAD self-test")
        return 1
    try:
        violations = scan_tree()
    except Exception as exc:
        print(f"apparatus-agent-safety: fail-open internal error: {type(exc).__name__}")
        return 0
    for violation in violations:
        print(
            f"{violation.path}:{violation.line}: forbidden process actuation {violation.symbol}"
        )
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
