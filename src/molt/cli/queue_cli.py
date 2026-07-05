from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys

from molt.cli.project_roots import _find_molt_root

PROOF_QUEUE_SIZE_ENV = "MOLT_PROOF_QUEUE_SIZE"


def _queue_repo_root() -> Path:
    return _find_molt_root(Path.cwd()) or Path(__file__).resolve().parents[3]


def _queue_args_define_queue_size(queue_args: list[str]) -> bool:
    for token in queue_args:
        if token == "--":
            return False
        if token == "--queue-size" or token.startswith("--queue-size="):
            return True
    return False


def handle_queue_command(args: argparse.Namespace) -> int:
    repo_root = _queue_repo_root()
    proof_queue = repo_root / "tools" / "proof_queue.py"
    if not proof_queue.exists():
        print(
            "molt queue requires a Molt source checkout containing "
            "tools/proof_queue.py",
            file=sys.stderr,
        )
        return 2
    queue_args = list(getattr(args, "queue_args", []) or [])
    if queue_args[:1] == ["--"]:
        queue_args = queue_args[1:]
    if not queue_args:
        queue_args = ["quickstart"]
    queue_size = getattr(args, "queue_size", None)
    if queue_size is not None and _queue_args_define_queue_size(queue_args):
        print(
            "molt queue: use either top-level --queue-size or proof_queue "
            "run --queue-size, not both",
            file=sys.stderr,
        )
        return 2
    if queue_size is not None:
        env = os.environ.copy()
        env[PROOF_QUEUE_SIZE_ENV] = str(queue_size)
        result = subprocess.run(
            [sys.executable, str(proof_queue), *queue_args],
            cwd=repo_root,
            env=env,
        )
    else:
        result = subprocess.run(
            [sys.executable, str(proof_queue), *queue_args],
            cwd=repo_root,
        )
    return int(result.returncode)
