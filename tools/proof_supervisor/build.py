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

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.proof_queue_pkg import supervisor_generation  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--target")
    args = parser.parse_args()

    env = dict(os.environ)
    with supervisor_generation._provision_guard_scope(env):
        inputs, identities = supervisor_generation._build_inputs(
            env, profile="release" if args.release else "debug", target=args.target
        )
        binary, completed = supervisor_generation.build_cargo(inputs=inputs, env=env)
        if completed.stdout:
            print(completed.stdout, end="")
        if completed.stderr:
            print(completed.stderr, file=sys.stderr, end="")
        if completed.returncode:
            return completed.returncode
        supervisor_generation._verify_inputs(inputs, identities, env)
    print(binary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
