from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys

from molt.cli.project_roots import _find_molt_root


def _queue_repo_root() -> Path:
    return _find_molt_root(Path.cwd()) or Path(__file__).resolve().parents[3]


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
    result = subprocess.run(
        [sys.executable, str(proof_queue), *queue_args],
        cwd=repo_root,
    )
    return int(result.returncode)
