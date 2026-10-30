"""``molt_dev.py storage``: the storage CLI and compile-cache pruning.

The storage library (tools/molt_dev_storage.py) is imported by checkout
maintenance, which ``molt.dx`` schedules, so it stays free of the compiler's
cache stores. Compile-cache pruning reads those stores (molt.cache_eviction
imports molt.cli), so it lives here, with the command that runs it.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping
import json
import os
from pathlib import Path

from molt.dx import storage_path
from tools.molt_dev_storage import inventory, migrate, trim


def trim_compile_cache(
    repo: Path, environment: Mapping[str, str], *, apply: bool
) -> list[dict[str, object]]:
    """Run the compile cache's own size and age retention."""
    from molt.cache_eviction import _prune
    from molt.default_paths import resolve_budget

    max_gb, max_days = resolve_budget(None, None, environment)
    return [
        dict(
            _prune(
                storage_path(repo, "compile-cache", environment),
                max_bytes=int(max_gb * 1024**3),
                max_age_days=max_days,
                dry_run=not apply,
                environ=environment,
            )
        )
    ]


def cmd_storage(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    if args.storage_command == "inventory":
        result = inventory(repo, os.environ)
    elif args.storage_command == "trim":
        result = trim_compile_cache(repo, os.environ, apply=args.apply)
        result += trim(
            repo,
            os.environ,
            apply=args.apply,
            legacy_temp=args.legacy_temp_root,
            include_shared_preimages=True,
        )
    else:
        result = migrate(
            repo, os.environ, apply=args.apply, legacy_temp=args.legacy_temp_root
        )
    print(
        json.dumps(
            {
                "schema": "molt.storage.v1",
                "mode": "apply" if getattr(args, "apply", False) else "dry-run",
                "entries": result,
            },
            indent=2,
        )
    )
    return 0


def add_arguments(subparsers: argparse._SubParsersAction) -> None:
    parser = subparsers.add_parser(
        "storage", help="typed storage inventory and explicit migration"
    )
    commands = parser.add_subparsers(dest="storage_command", required=True)
    for name in ("inventory", "migrate", "trim"):
        command = commands.add_parser(name)
        command.add_argument("--repo", default=".")
        if name != "inventory":
            command.add_argument("--legacy-temp-root", type=Path)
            modes = command.add_mutually_exclusive_group()
            modes.add_argument(
                "--apply",
                action="store_true",
                help="explicitly execute the reviewed migration",
            )
            modes.add_argument(
                "--dry-run", action="store_true", help="read-only plan (default)"
            )
        command.set_defaults(func=cmd_storage)
