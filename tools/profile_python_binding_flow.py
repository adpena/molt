#!/usr/bin/env python3
"""Profile the canonical Python binding/capability analysis hot path."""

from __future__ import annotations

import argparse
import ast
import gc
import json
import statistics
import time
import tracemalloc
from pathlib import Path

from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_bindings,
    analyze_python_source_bindings,
    python_source_digest,
)


def _representative_source(import_count: int) -> str:
    lines = ["import importlib as loader", "from importlib import import_module"]
    for index in range(import_count):
        lines.extend(
            (
                f"target_{index} = loader.import_module",
                f"if flag_{index}:",
                f"    target_{index} = replacement_{index}",
                f"target_{index}('package.module_{index}')",
            )
        )
    return "\n".join(lines) + "\n"


def _percentile(samples: list[int], percentile: float) -> int:
    ordered = sorted(samples)
    index = min(len(ordered) - 1, round((len(ordered) - 1) * percentile))
    return ordered[index]


def _measure(import_count: int, iterations: int) -> dict[str, object]:
    source = _representative_source(import_count)
    digest = python_source_digest(source)
    tree = ast.parse(source, feature_version=(3, 12))
    policy = PythonBindingPolicy()

    # Warm Python's own allocators and validate the representative shape first.
    reference = analyze_python_bindings(tree, source_digest=digest, policy=policy)
    assert len(reference.calls) == import_count

    cold_ns: list[int] = []
    for _iteration in range(iterations):
        start = time.perf_counter_ns()
        index = analyze_python_bindings(tree, source_digest=digest, policy=policy)
        cold_ns.append(time.perf_counter_ns() - start)
        assert len(index.calls) == import_count

    gc.collect()
    tracemalloc.start(8)
    try:
        allocation_start = time.perf_counter_ns()
        allocation_index = analyze_python_bindings(
            tree, source_digest=digest, policy=policy
        )
        allocation_profile_ns = time.perf_counter_ns() - allocation_start
        assert len(allocation_index.calls) == import_count
        retained_bytes, peak_bytes = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()

    cached = analyze_python_source_bindings(source, policy=policy)
    cached_ns: list[int] = []
    for _iteration in range(iterations * 10):
        start = time.perf_counter_ns()
        hit = analyze_python_source_bindings(source, policy=policy)
        cached_ns.append(time.perf_counter_ns() - start)
        assert hit is cached

    median = int(statistics.median(cold_ns))
    node_count = sum(1 for _node in ast.walk(tree))
    return {
        "schema_version": 1,
        "source_bytes": len(source.encode("utf-8")),
        "ast_nodes": node_count,
        "import_calls": import_count,
        "states": len(reference.states),
        "scopes": len(reference.scopes),
        "iterations": iterations,
        "cold": {
            "median_ns": median,
            "p95_ns": _percentile(cold_ns, 0.95),
            "nodes_per_second": round(node_count * 1_000_000_000 / median),
            "peak_traced_bytes": peak_bytes,
            "peak_bytes_per_ast_node": round(peak_bytes / node_count, 2),
            "retained_index_bytes": retained_bytes,
            "retained_bytes_per_ast_node": round(retained_bytes / node_count, 2),
            "allocation_profile_ns": allocation_profile_ns,
        },
        "cache_hit": {
            "median_ns": int(statistics.median(cached_ns)),
            "p95_ns": _percentile(cached_ns, 0.95),
            "same_immutable_index": True,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imports", type=int, default=500)
    parser.add_argument("--iterations", type=int, default=7)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    if args.imports <= 0 or args.iterations <= 0:
        parser.error("--imports and --iterations must be positive")
    payload = _measure(args.imports, args.iterations)
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
