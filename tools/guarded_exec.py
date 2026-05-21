#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys

try:
    from tools import harness_memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct script execution
    import harness_memory_guard  # type: ignore


ROOT = Path(__file__).resolve().parents[1]


def _timeout_from_env(name: str | None) -> float | None:
    if not name:
        return None
    raw = os.environ.get(name, "").strip()
    if not raw:
        return None
    try:
        parsed = float(raw)
    except ValueError:
        return None
    return parsed if parsed > 0 else None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a command under Molt's canonical harness memory guard."
    )
    parser.add_argument("--prefix", default="MOLT")
    parser.add_argument("--cwd", type=Path, default=ROOT)
    parser.add_argument("--timeout", type=float, default=None)
    parser.add_argument("--timeout-env", default=None)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("command is required after --")

    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=ROOT)
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        args.prefix,
        env,
        repo_root=ROOT,
    )
    timeout = args.timeout
    if timeout is None:
        timeout = _timeout_from_env(args.timeout_env)
    result = context.run(
        command,
        cwd=args.cwd,
        env=env,
        capture_output=False,
        timeout=timeout,
    )
    if result.stderr:
        sys.stderr.write(result.stderr)
    return int(result.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
