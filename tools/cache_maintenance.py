#!/usr/bin/env python3
"""Cache-policy worker for the existing retained maintenance supervisor."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.checkout_maintenance import maintenance_arguments  # noqa: E402
from molt.cache_eviction import automatic_retention  # noqa: E402
from molt.default_paths import CLI_CACHE_RETENTION_INTERVAL_SECONDS  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    args = maintenance_arguments(argv)
    result = automatic_retention(
        ProcessEnvironment(os.environ),
        cache_root=args.root,
        min_interval_seconds=CLI_CACHE_RETENTION_INTERVAL_SECONDS,
        wait_for_lock=True,
    )
    print(
        json.dumps(
            {
                "schema": "molt.cache-maintenance.v1",
                "root": str(args.root),
                "policy_cwd": str(args.policy_cwd),
                "result": result,
            }
        ),
        flush=True,
    )
    return int(
        result is not None
        and (
            result.get("status") == "failed"
            or bool(result.get("entries_failed"))
            or bool(result.get("source_record_maintenance_failed"))
        )
    )


if __name__ == "__main__":
    raise SystemExit(main())
