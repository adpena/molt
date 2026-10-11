#!/usr/bin/env python3
"""Inspect, reconcile and retire memory-guard custody records.

Dry run is the default. ``--apply`` marks a record ``custody_reconciled`` when
one native process snapshot proves its guard, child and child process group
gone. It then moves every resolved record out of the active directory into
its bounded retired history, and hands a dead guard's leased or indeterminate
scratch to the scratch reclaim authority. ``--release MARKER`` resolves a
record whose evidence is inconclusive, after the operator confirms that no
process of that run is alive; live evidence still refuses it.
"""

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
    record_custody_sweep,
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
        help="write receipts and retire resolved records; default is dry-run",
    )
    parser.add_argument(
        "--release",
        type=Path,
        action="append",
        default=[],
        metavar="MARKER",
        help=(
            "operator attestation: resolve this inconclusive record "
            "(repeatable); a record with a live process is still refused"
        ),
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
            active_dir,
            sample_processes,
            apply=args.apply,
            release=tuple(path.expanduser() for path in args.release),
        )
        if args.apply and not args.release:
            record_custody_sweep(report)
    except (ActiveCustodyError, ProcessSnapshotError, OSError) as exc:
        print(
            f"memory_guard_custody: custody unavailable; no markers changed: {exc}",
            file=sys.stderr,
        )
        return 2
    if args.json:
        print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
        return 0
    mode = "apply" if args.apply else "dry-run"
    print(
        f"memory_guard_custody [{mode}] active_dir={report.active_dir} "
        f"snapshot_processes={report.snapshot_processes} "
        f"terminalized={report.terminalized} retired={report.retired} "
        f"preserved={report.preserved} remaining={report.remaining}"
    )
    for decision in report.decisions:
        if decision.retired_to is not None:
            continue
        if decision.disposition == "already_terminal" and decision.retirement is None:
            continue
        detail = decision.reason
        if decision.retirement is not None:
            detail += f"; stays active: {decision.retirement}"
        print(f"  {decision.disposition}: {decision.marker} ({detail})")
    for retention in report.scratch_retention:
        errors = retention.get("errors")
        for error in errors if isinstance(errors, list) else ():
            print(f"  scratch error: {error}")
    steps = report.next_steps()
    if steps:
        print("operator action required:")
        for step in steps:
            print(f"  {step}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
