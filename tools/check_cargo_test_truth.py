#!/usr/bin/env python3
"""Fail closed when Cargo test binaries can be masked or silently unexecuted."""

from __future__ import annotations

import re
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
CI = ROOT / ".github" / "workflows" / "ci.yml"
DEV_GATES = ROOT / "tools" / "molt_dev_gates.toml"
RUNNER = ROOT / "tools" / "run_cargo_test_truth.py"
_CARGO_TEST = re.compile(r"cargo\s+test\b[^\n\"']*")
_CANONICAL = "cargo test --workspace --tests --no-fail-fast"
_RUNNER_COMMAND = "python3 tools/run_cargo_test_truth.py"


def _commands(path: Path) -> list[tuple[int, str]]:
    commands = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        match = _CARGO_TEST.search(line)
        if match:
            commands.append((line_number, match.group(0).strip()))
    return commands


def violations() -> list[str]:
    failures = []
    if CI.read_text(encoding="utf-8").count(_RUNNER_COMMAND) != 1:
        failures.append(
            f"{CI.relative_to(ROOT)} must contain exactly one canonical truth runner: "
            f"{_RUNNER_COMMAND!r}"
        )
    if RUNNER.read_text(encoding="utf-8").count(
        '("cargo", "test", "--workspace", "--tests", "--no-fail-fast")'
    ) != 1:
        failures.append(
            f"{RUNNER.relative_to(ROOT)} must execute exactly {_CANONICAL!r}"
        )
    for path in (CI, DEV_GATES):
        for line_number, command in _commands(path):
            single_executable = any(
                selector in command for selector in (" --lib", " --doc", " --test ")
            )
            source_line = path.read_text(encoding="utf-8").splitlines()[line_number - 1]
            if not single_executable and "--no-fail-fast" not in source_line:
                failures.append(
                    f"{path.relative_to(ROOT)}:{line_number}: multi-executable Cargo "
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
