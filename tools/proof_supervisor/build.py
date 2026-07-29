#!/usr/bin/env python3
"""Build the standalone native proof supervisor and print its exact path."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parent
REPO_ROOT = ROOT.parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools.command_execution import CommandExecutor  # noqa: E402


_COMMANDS = CommandExecutor.for_file(__file__)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--target")
    args = parser.parse_args()

    command = ["cargo", "build", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")]
    profile = "debug"
    if args.release:
        command.append("--release")
        profile = "release"
    if args.target:
        command.extend(("--target", args.target))
    _COMMANDS.run(command, cwd=ROOT, check=True, text=True)

    target_root = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    if not target_root.is_absolute():
        target_root = ROOT / target_root
    if args.target:
        target_root /= args.target
    target_is_windows = "windows" in args.target if args.target else os.name == "nt"
    suffix = ".exe" if target_is_windows else ""
    binary = (target_root / profile / f"molt-proof-supervisor{suffix}").resolve()
    if not binary.is_file():
        raise SystemExit(f"cargo succeeded without expected binary: {binary}")
    print(binary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
