#!/usr/bin/env python3
"""Fail closed when Cargo test binaries can be masked or silently unexecuted."""

from __future__ import annotations

import re
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PROOF_PLAN = ROOT / "tools" / "proof_plan.toml"
RUNNER = ROOT / "tools" / "run_cargo_test_truth.py"
_CARGO_TEST = re.compile(r"cargo\s+test\b[^\n\"']*")
_CARGO_COMPILE = re.compile(r"cargo\s+(?:build|check|clippy|test)\b[^\n\"']*")
_CANONICAL = "cargo test --locked --workspace --tests --no-fail-fast"
_RUNNER_ID = "rust.test.default-truth"
_RUNNER_ARGV = ["python3", "tools/run_cargo_test_truth.py"]


def _commands(path: Path) -> list[tuple[int, str]]:
    commands = []
    for line_number, line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), 1
    ):
        match = _CARGO_TEST.search(line)
        if match:
            commands.append((line_number, match.group(0).strip()))
    return commands


def _compile_commands(path: Path) -> list[tuple[int, str]]:
    commands = []
    for line_number, line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), 1
    ):
        match = _CARGO_COMPILE.search(line)
        if match:
            commands.append((line_number, match.group(0).strip()))
    return commands


def _display_path(path: Path) -> Path:
    try:
        return path.relative_to(ROOT)
    except ValueError:
        return path


def violations() -> list[str]:
    failures = []
    plan = tomllib.loads(PROOF_PLAN.read_text(encoding="utf-8"))
    runner_commands = [
        command
        for command in plan.get("command", [])
        if command.get("id") == _RUNNER_ID
    ]
    if len(runner_commands) != 1 or runner_commands[0].get("argv") != _RUNNER_ARGV:
        failures.append(
            f"{_display_path(PROOF_PLAN)} must contain exactly one {_RUNNER_ID!r} "
            f"command with argv {_RUNNER_ARGV!r}"
        )
    if (
        RUNNER.read_text(encoding="utf-8").count(
            '"test",\n    "--locked",\n    "--workspace",\n    "--tests",\n    "--no-fail-fast",'
        )
        != 1
    ):
        failures.append(
            f"{RUNNER.relative_to(ROOT)} must execute exactly {_CANONICAL!r}"
        )
    proof_plan_lines = PROOF_PLAN.read_text(encoding="utf-8").splitlines()
    for line_number, command in _compile_commands(PROOF_PLAN):
        source_line = proof_plan_lines[line_number - 1]
        if "--locked" not in source_line:
            failures.append(
                f"{_display_path(PROOF_PLAN)}:{line_number}: Cargo compilation "
                f"command lacks --locked dependency authority: {command}"
            )
    for line_number, command in _commands(PROOF_PLAN):
        single_executable = any(
            selector in command for selector in (" --lib", " --doc", " --test ")
        )
        source_line = proof_plan_lines[line_number - 1]
        if not single_executable and "--no-fail-fast" not in source_line:
            failures.append(
                f"{_display_path(PROOF_PLAN)}:{line_number}: multi-executable Cargo "
                f"test command lacks --no-fail-fast: {command}"
            )
    return failures


def main() -> int:
    failures = violations()
    if failures:
        print("cargo-test-truth: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("cargo-test-truth: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
