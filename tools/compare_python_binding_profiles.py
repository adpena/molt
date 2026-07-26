#!/usr/bin/env python3
"""Compare two canonical Python binding profile receipts fail-closed."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def _nested(row: dict[str, Any], *path: str) -> int:
    value: Any = row
    for key in path:
        value = value[key]
    return int(value)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
    candidate = json.loads(args.candidate.read_text(encoding="utf-8"))
    baseline_samples = baseline["samples"]
    candidate_samples = candidate["samples"]
    if len(baseline_samples) != len(candidate_samples):
        raise RuntimeError("profile sample counts differ")
    for before, after in zip(baseline_samples, candidate_samples, strict=True):
        if before["source_sha256"] != after["source_sha256"]:
            raise RuntimeError("profile workload source hashes differ")
        if before["identity"]["tool_sha256"] != after["identity"]["tool_sha256"]:
            raise RuntimeError("profile tool hashes differ")
        if before["ast_nodes"] != after["ast_nodes"]:
            raise RuntimeError("profile AST sizes differ")
    before = baseline_samples[-1]
    after = candidate_samples[-1]
    metrics = {
        "analysis_median_ns": ("analysis_only", "median_ns"),
        "parse_and_analysis_median_ns": ("parse_and_analysis", "median_ns"),
        "peak_traced_bytes": ("allocation_trace", "peak_traced_bytes"),
        "retained_index_bytes": ("allocation_trace", "retained_index_bytes"),
        "peak_rss_delta_bytes": (
            "isolated_process_memory",
            "peak_rss_delta_from_start_bytes",
        ),
        "states": ("states",),
    }
    comparisons: dict[str, object] = {}
    for name, path in metrics.items():
        before_value = _nested(before, *path)
        after_value = _nested(after, *path)
        comparisons[name] = {
            "baseline": before_value,
            "candidate": after_value,
            "improvement_ratio": round(before_value / after_value, 4),
        }
    payload = {
        "schema_version": 1,
        "tool_sha256": before["identity"]["tool_sha256"],
        "scale": baseline["scaling"]["scales"][-1],
        "source_sha256": before["source_sha256"],
        "baseline_implementation_sha256": before["identity"][
            "implementation_sha256"
        ],
        "candidate_implementation_sha256": after["identity"][
            "implementation_sha256"
        ],
        "baseline_scaling": baseline["scaling"],
        "candidate_scaling": candidate["scaling"],
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
