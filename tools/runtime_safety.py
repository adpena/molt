#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNTIME_DIR = ROOT / "runtime/molt-runtime"

SANITIZERS = {
    "asan": "address",
    "tsan": "thread",
    "ubsan": "undefined",
}


def _run(
    cmd: list[str],
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    log_path: Path | None = None,
) -> None:
    run_env = env or os.environ.copy()
    if log_path is None:
        subprocess.check_call(cmd, cwd=cwd or ROOT, env=run_env)
        return

    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w", encoding="utf-8") as log:
        process = subprocess.Popen(
            cmd,
            cwd=cwd or ROOT,
            env=run_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        assert process.stdout is not None
        for line in process.stdout:
            sys.stdout.write(line)
            log.write(line)
        retcode = process.wait()
        if retcode != 0:
            raise subprocess.CalledProcessError(retcode, cmd)


def _require_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise SystemExit(f"{name} not found in PATH")


def run_sanitizer(kind: str, log_dir: Path | None) -> None:
    sanitizer = SANITIZERS[kind]
    env = os.environ.copy()
    env["RUSTFLAGS"] = f"-Z sanitizer={sanitizer}"
    env["RUSTDOCFLAGS"] = env["RUSTFLAGS"]
    log_path = log_dir / f"runtime_{kind}.log" if log_dir else None
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
        log_path=log_path,
    )


def run_miri(log_dir: Path | None) -> None:
    env = os.environ.copy()
    miriflags = env.get("MIRIFLAGS", "")
    if "-Zmiri-disable-isolation" not in miriflags:
        env["MIRIFLAGS"] = f"{miriflags} -Zmiri-disable-isolation".strip()
    log_path = log_dir / "runtime_miri.log" if log_dir else None
    _run(
        ["cargo", "+nightly", "miri", "test", "-p", "molt-runtime"],
        env=env,
        log_path=log_path,
    )


def run_fuzz(target: str, runs: int, log_dir: Path | None) -> None:
    cmd = ["cargo", "+nightly", "fuzz", "run", target]
    if runs > 0:
        cmd.extend(["--", f"-runs={runs}"])
    log_path = log_dir / f"runtime_fuzz_{target}.log" if log_dir else None
    _run(cmd, cwd=RUNTIME_DIR, log_path=log_path)


def run_clippy(log_dir: Path | None) -> None:
    log_path = log_dir / "runtime_clippy.log" if log_dir else None
    _run(
        [
            "cargo",
            "clippy",
            "-p",
            "molt-runtime",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        log_path=log_path,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Runtime safety entrypoints")
    sub = parser.add_subparsers(dest="command", required=True)

    for name in SANITIZERS:
        sub.add_parser(name, help=f"run {name} sanitizer")

    sub.add_parser("miri", help="run miri checks")

    fuzz = sub.add_parser("fuzz", help="run cargo-fuzz target")
    fuzz.add_argument("--target", default="string_ops", help="fuzz target name")
    fuzz.add_argument(
        "--runs",
        type=int,
        default=10_000,
        help="number of fuzz iterations (0 for unlimited)",
    )
    sub.add_parser("clippy", help="run clippy on molt-runtime")
    parser.add_argument(
        "--log-dir",
        default=str(ROOT / "logs"),
        help="write command output to log files (disable with --log-dir=)",
    )

    args = parser.parse_args()

    log_dir = None if args.log_dir == "" else Path(args.log_dir)

    _require_tool("cargo")
    if args.command == "fuzz":
        _require_tool("cargo-fuzz")

    if args.command in SANITIZERS:
        run_sanitizer(args.command, log_dir)
    elif args.command == "miri":
        run_miri(log_dir)
    elif args.command == "fuzz":
        run_fuzz(args.target, args.runs, log_dir)
    elif args.command == "clippy":
        run_clippy(log_dir)
    else:
        parser.print_help()
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
