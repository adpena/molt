#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import secrets
import subprocess
import time
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_RANDOM_PLUGIN = "tools.pytest_random_order_plugin"


def _log(msg: str) -> None:
    stamp = datetime.now().isoformat(timespec="seconds")
    print(f"[dev_test_runner {stamp}] {msg}")


def _run(cmd: list[str]) -> None:
    _log(f"run: {' '.join(cmd)}")
    start = time.monotonic()
    subprocess.check_call(cmd, cwd=ROOT, env=os.environ.copy())
    _log(f"done: {' '.join(cmd)} ({time.monotonic() - start:.2f}s)")


def _resolve_random_seed(
    *, random_order: bool, random_seed: str | None, env: dict[str, str] | None = None
) -> str | None:
    current_env = env or os.environ
    env_random_order = current_env.get("MOLT_PYTEST_RANDOM_ORDER", "").strip() == "1"
    env_random_seed = current_env.get("MOLT_PYTEST_RANDOM_SEED", "").strip() or None
    if not (random_order or random_seed or env_random_order or env_random_seed):
        return None
    if random_seed:
        return random_seed
    if env_random_seed:
        return env_random_seed
    return str(secrets.randbelow(2**32))


def _build_pytest_command(
    *, random_order: bool, random_seed: str | None, env: dict[str, str] | None = None
) -> tuple[list[str], str | None]:
    seed = _resolve_random_seed(
        random_order=random_order,
        random_seed=random_seed,
        env=env,
    )
    cmd = ["pytest", "-q"]
    if seed is None:
        return cmd, None
    cmd.extend(
        [
            "-p",
            _RANDOM_PLUGIN,
            "--molt-random-order",
            "--molt-random-seed",
            seed,
        ]
    )
    return cmd, seed


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--verified-subset",
        action="store_true",
        help="Run tools/verified_subset.py after pytest.",
    )
    parser.add_argument(
        "--random-order",
        action="store_true",
        help="Shuffle pytest collection order using a recorded deterministic seed.",
    )
    parser.add_argument(
        "--random-seed",
        help="Explicit seed for --random-order. If omitted, one is generated and logged.",
    )
    args = parser.parse_args()

    pytest_cmd, resolved_seed = _build_pytest_command(
        random_order=args.random_order,
        random_seed=args.random_seed,
    )
    if resolved_seed is not None:
        _log(f"pytest random order enabled (seed={resolved_seed})")
    _run(pytest_cmd)
    if args.verified_subset:
        _run(["python3", "tools/verified_subset.py", "run"])


if __name__ == "__main__":
    main()
