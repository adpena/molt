#!/usr/bin/env python3
"""Compare matched PEP 695 lazy-value profile receipts fail-closed."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    baseline: dict[str, Any] = json.loads(args.baseline.read_text(encoding="utf-8"))
    candidate: dict[str, Any] = json.loads(args.candidate.read_text(encoding="utf-8"))
    before_identity = baseline["identity"]
    after_identity = candidate["identity"]
    for key in (
        "tool_sha256",
        "implementation_sha256",
        "python",
        "platform",
        "machine",
    ):
        if before_identity[key] != after_identity[key]:
            raise RuntimeError(f"profile identity mismatch: {key}")
    before_metrics = baseline["metrics"]
    after_metrics = candidate["metrics"]
    for key in ("count", "warm_reads"):
        if before_metrics[key] != after_metrics[key]:
            raise RuntimeError(f"profile workload mismatch: {key}")
    operation_names = sorted(set(before_metrics) - {"count", "warm_reads"})
    if set(operation_names) != set(after_metrics) - {"count", "warm_reads"}:
        raise RuntimeError("profile operation sets differ")
    comparisons: dict[str, object] = {}
    for operation_name in operation_names:
        before = before_metrics[operation_name]
        after = after_metrics[operation_name]
        divisor = (
            before_metrics["warm_reads"]
            if operation_name.startswith("warm_")
            else before_metrics["count"]
        )
        comparisons[operation_name] = {
            metric: {
                "unsynchronized_control": before[metric],
                "synchronized": after[metric],
                "delta": after[metric] - before[metric],
                "delta_per_operation": round(
                    (after[metric] - before[metric]) / divisor, 4
                ),
                "synchronized_over_control_ratio": round(
                    after[metric] / before[metric], 4
                )
                if before[metric]
                else None,
            }
            for metric in before
        }
    payload = {
        "schema_version": 1,
        "tool_sha256": before_identity["tool_sha256"],
        "implementation_sha256": before_identity["implementation_sha256"],
        "count": before_metrics["count"],
        "warm_reads": before_metrics["warm_reads"],
        "comparisons": comparisons,
    }
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
