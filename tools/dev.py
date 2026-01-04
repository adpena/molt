#!/usr/bin/env python3
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST_PYTHONS = ["3.12", "3.13", "3.14"]


def run_uv(args: list[str], python: str | None = None) -> None:
    cmd = ["uv", "run"]
    if python:
        cmd.extend(["--python", python])
    cmd.extend(args)
    subprocess.check_call(cmd, cwd=ROOT)


def main() -> None:
    cmd = sys.argv[1:] or ["help"]
    if cmd[0] == "lint":
        run_uv(["ruff", "check", "."], python=TEST_PYTHONS[0])
        run_uv(["ruff", "format", "--check", "."], python=TEST_PYTHONS[0])
        run_uv(["ty", "check", "src"], python=TEST_PYTHONS[0])
    elif cmd[0] == "test":
        for python in TEST_PYTHONS:
            run_uv(["pytest", "-q"], python=python)
    else:
        print("Usage: tools/dev.py [lint|test]")


if __name__ == "__main__":
    main()
