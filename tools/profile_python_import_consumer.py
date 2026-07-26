#!/usr/bin/env python3
"""Profile canonical binding analysis inside one real pytest import consumer."""

from __future__ import annotations

import argparse
import ast
import json
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any

import pytest

from molt.cli import python_import_resolution


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pytest_target")
    parser.add_argument("--min-seconds", type=float, default=0.05)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    if args.min_seconds < 0:
        parser.error("--min-seconds must be non-negative")

    original = python_import_resolution.analyze_python_bindings
    analyses: list[dict[str, Any]] = []

    def measured(tree: ast.Module, **kwargs: Any):
        started = time.perf_counter()
        index = original(tree, **kwargs)
        elapsed = time.perf_counter() - started
        if elapsed >= args.min_seconds:
            policy = kwargs["policy"]
            analyses.append(
                {
                    "module": policy.module_name,
                    "ast_nodes": sum(1 for _node in ast.walk(tree)),
                    "seconds": round(elapsed, 6),
                    "states": index.state_count,
                    "telemetry": asdict(index.telemetry),
                }
            )
        return index

    python_import_resolution.analyze_python_bindings = measured
    started = time.perf_counter()
    try:
        exit_code = int(pytest.main([args.pytest_target, "-q"]))
    finally:
        python_import_resolution.analyze_python_bindings = original
    payload = {
        "schema_version": 1,
        "pytest_target": args.pytest_target,
        "wall_seconds": round(time.perf_counter() - started, 6),
        "exit_code": exit_code,
        "slow_analyses": sorted(
            analyses, key=lambda row: float(row["seconds"]), reverse=True
        ),
    }
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
