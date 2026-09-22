#!/usr/bin/env python3
"""Prewarm or inspect the content-addressed native proof supervisor cache.

Every proof run needs the native supervisor binary. It is keyed by its exact
sources plus rustc identity (tools/proof_queue_pkg/supervisor_custody.py) and
published once into the shared custody-external cache; proof runs then hit
that cache instead of paying a cold cargo release build inside their own timed
window. `prewarm` is the prepare step CI and local proof families run before
any queue-driven test; `identity` prints the current key.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from tools.proof_queue_pkg import supervisor_custody

ROOT = Path(__file__).resolve().parents[1]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    subparsers = parser.add_subparsers(dest="command", required=True)
    prewarm = subparsers.add_parser(
        "prewarm", help="build on miss and publish into the cache"
    )
    prewarm.add_argument("--repo-root", type=Path, default=ROOT)
    prewarm.add_argument("--json", action="store_true")
    identity = subparsers.add_parser(
        "identity", help="print the supervisor source identity"
    )
    identity.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    env = dict(os.environ)
    if args.command == "identity":
        record = supervisor_custody.supervisor_source_identity(env)
        if args.json:
            print(json.dumps(record, indent=2, sort_keys=True))
        else:
            print(record["identity"])
        return 0
    telemetry = supervisor_custody.prewarm_proof_supervisor(
        cwd=args.repo_root.resolve(), env=env
    )
    if args.json:
        print(json.dumps(telemetry, indent=2, sort_keys=True))
    else:
        print(
            f"proof-supervisor cache {telemetry['cache']}: "
            f"{telemetry['source_identity']} ({telemetry['build_s']:.1f}s) -> {telemetry['cache_dir']}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
