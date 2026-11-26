#!/usr/bin/env python3
"""Run published checkout collection, including --pressure-only.

The launcher checks the pinned worker source for that option before
dispatching a pressure-only run to a possibly older controller.
"""

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

from molt.custody_layout import git_directories  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402
from tools.checkout_maintenance_launch import maintenance_arguments  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    args = maintenance_arguments(argv, checkout=True)
    from tools.disk_guard import collect_due

    auxiliary_results = []

    def auxiliary() -> None:
        from tools.controller_source import controller_common_dir
        from tools.molt_dev_storage import trim

        directories = git_directories(_REPO_ROOT)
        common = (
            directories[1]
            if directories is not None
            else controller_common_dir(_REPO_ROOT)
        )
        auxiliary_results.extend(
            trim(
                common.parent,
                ProcessEnvironment(os.environ),
                apply=True,
                policy_source=_REPO_ROOT,
            )
        )

    results = collect_due(
        root=args.root,
        env=ProcessEnvironment(os.environ),
        auxiliary=auxiliary,
        pressure_only=args.pressure_only,
    )
    print(
        json.dumps(
            {
                "schema": "molt.checkout-maintenance.v1",
                "root": str(args.root),
                "policy_cwd": str(args.policy_cwd),
                "results": [result.to_dict() for result in results],
                "storage": auxiliary_results,
            }
        ),
        flush=True,
    )
    return int(
        any(result.errors for result in results)
        or any(row.get("error") for row in auxiliary_results)
    )


if __name__ == "__main__":
    raise SystemExit(main())
