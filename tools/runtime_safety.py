#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNTIME_DIR = ROOT / "runtime/molt-runtime"

SANITIZERS = {
    "asan": "address",
    "tsan": "thread",
    "ubsan": "undefined",
}


def _run(
    cmd: list[str], env: dict[str, str] | None = None, cwd: Path | None = None
) -> None:
    subprocess.check_call(cmd, cwd=cwd or ROOT, env=env or os.environ.copy())


def run_sanitizer(kind: str) -> None:
    sanitizer = SANITIZERS[kind]
    env = os.environ.copy()
    env["RUSTFLAGS"] = f"-Z sanitizer={sanitizer}"
    env["RUSTDOCFLAGS"] = env["RUSTFLAGS"]
    _run(
        [
            "cargo",
            "+nightly",
            "test",
            "-p",
            "molt-runtime",
            "--all-targets",
        ],
        env=env,
    )


def run_miri() -> None:
    env = os.environ.copy()
    miriflags = env.get("MIRIFLAGS", "")
    if "-Zmiri-disable-isolation" not in miriflags:
        env["MIRIFLAGS"] = f"{miriflags} -Zmiri-disable-isolation".strip()
    _run(["cargo", "+nightly", "miri", "test", "-p", "molt-runtime"], env=env)


def run_fuzz(target: str) -> None:
    _run(["cargo", "+nightly", "fuzz", "run", target], cwd=RUNTIME_DIR)


def main() -> int:
    parser = argparse.ArgumentParser(description="Runtime safety entrypoints")
    sub = parser.add_subparsers(dest="command", required=True)

    for name in SANITIZERS:
        sub.add_parser(name, help=f"run {name} sanitizer")

    sub.add_parser("miri", help="run miri checks")

    fuzz = sub.add_parser("fuzz", help="run cargo-fuzz target")
    fuzz.add_argument("--target", default="string_ops", help="fuzz target name")

    args = parser.parse_args()

    if args.command in SANITIZERS:
        run_sanitizer(args.command)
    elif args.command == "miri":
        run_miri()
    elif args.command == "fuzz":
        run_fuzz(args.target)
    else:
        parser.print_help()
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
