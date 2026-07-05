from __future__ import annotations

import argparse
import importlib
import sys


def handle_queue_command(args: argparse.Namespace) -> int:
    queue_args = list(getattr(args, "queue_args", []) or [])
    if queue_args[:1] == ["--"]:
        queue_args = queue_args[1:]
    if not queue_args:
        queue_args = ["status"]
    try:
        proof_queue = importlib.import_module("tools.proof_queue")
    except ModuleNotFoundError as exc:
        if exc.name not in {"tools", "tools.proof_queue"}:
            raise
        print(
            "molt queue requires a Molt source checkout with tools/proof_queue.py",
            file=sys.stderr,
        )
        return 2
    return int(proof_queue.main(queue_args, prog="molt queue"))
