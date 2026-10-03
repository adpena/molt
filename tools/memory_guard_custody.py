#!/usr/bin/env python3
"""Inspect or explicitly reconcile stale active-marker evidence without pruning."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports

bind_repository_imports(__file__)

from tools.memory_guard_core.active_custody import (  # noqa: E402
    ActiveCustodyError,
    reconcile_active_guard_markers,
)
from molt.memory_guard_paths import active_guard_marker_dir  # noqa: E402
from tools.memory_guard_core.process_model import (  # noqa: E402
    ProcessSnapshotError,
    sample_processes,
)

ROOT = Path(__file__).resolve().parents[1]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--active-dir", type=Path, help="canonical active-marker directory"
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="terminalize only birth-identified stale records; default is dry-run",
    )
    parser.add_argument(
        "--json", action="store_true", help="emit machine-readable evidence"
    )
    args = parser.parse_args(argv)
    active_dir = (
        args.active_dir.expanduser().absolute()
        if args.active_dir is not None
        else active_guard_marker_dir(ROOT, os.environ)
    )
    try:
        report = reconcile_active_guard_markers(
            active_dir, sample_processes, apply=args.apply
        )
    except (ActiveCustodyError, ProcessSnapshotError, OSError) as exc:
        print(
            f"memory_guard_custody: custody unavailable; no markers changed: {exc}",
            file=sys.stderr,
        )
        return 2
    if args.json:
        print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
    else:
        mode = "apply" if args.apply else "dry-run"
        print(
            f"memory_guard_custody [{mode}] active_dir={report.active_dir} "
            f"snapshot_processes={report.snapshot_processes} "
            f"terminalized={report.terminalized} preserved={report.preserved}"
        )
        for decision in report.decisions:
            if decision.disposition != "already_terminal":
                verb = "terminalized" if decision.applied else decision.disposition
                print(f"  {verb}: {decision.marker} ({decision.reason})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
