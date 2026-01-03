#!/usr/bin/env python3
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(*args: str) -> None:
    subprocess.check_call(args, cwd=ROOT)


def main() -> None:
    cmd = sys.argv[1:] or ["help"]
    if cmd[0] == "lint":
        run("ruff", "check", ".")
        run("ruff", "format", "--check", ".")
        run("ty", "check", "src")
    elif cmd[0] == "test":
        run("pytest", "-q")
    else:
        print("Usage: tools/dev.py [lint|test]")


if __name__ == "__main__":
    main()
