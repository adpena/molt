#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import subprocess
import time
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _log(msg: str) -> None:
    stamp = datetime.now().isoformat(timespec="seconds")
    print(f"[dev_test_runner {stamp}] {msg}")


def _run(cmd: list[str]) -> None:
    _log(f"run: {' '.join(cmd)}")
    start = time.monotonic()
    subprocess.check_call(cmd, cwd=ROOT, env=os.environ.copy())
    _log(f"done: {' '.join(cmd)} ({time.monotonic() - start:.2f}s)")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--verified-subset",
        action="store_true",
        help="Run tools/verified_subset.py after pytest.",
    )
    args = parser.parse_args()

    _run(["pytest", "-q"])
    if args.verified_subset:
        _run(["python3", "tools/verified_subset.py", "run"])


if __name__ == "__main__":
    main()
