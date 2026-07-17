#!/usr/bin/env python3
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import statistics
import sys
import time
import tracemalloc


ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.cli.python_source_closure import local_python_import_closure  # noqa: E402


DEFAULT_OUTPUT = ROOT / "tmp" / "python_source_closure" / "profile.json"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=Path, default=ROOT / "tools" / "wasm_link.py")
    parser.add_argument("--iterations", type=int, default=10)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if args.iterations <= 0:
        raise SystemExit("--iterations must be positive")
    seed = args.seed.resolve()
    warm = local_python_import_closure(ROOT, (seed,))
    elapsed_ns: list[int] = []
    peak_bytes: list[int] = []
    for _ in range(args.iterations):
        tracemalloc.start()
        started = time.perf_counter_ns()
        closure = local_python_import_closure(ROOT, (seed,))
        elapsed_ns.append(time.perf_counter_ns() - started)
        _current, peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        if closure != warm:
            raise RuntimeError("Python source closure changed during profile")
        peak_bytes.append(peak)

    concurrent_started = time.perf_counter_ns()
    with ThreadPoolExecutor(max_workers=8) as pool:
        concurrent = tuple(
            pool.map(
                lambda _index: local_python_import_closure(ROOT, (seed,)),
                range(32),
            )
        )
    concurrent_ns = time.perf_counter_ns() - concurrent_started
    if any(result != warm for result in concurrent):
        raise RuntimeError("concurrent Python source closure results diverged")

    ordered = sorted(elapsed_ns)
    p95_index = min(len(ordered) - 1, max(0, (len(ordered) * 95 + 99) // 100 - 1))
    cache_path = ROOT / ".molt_cache" / "python_source_closure_graph.json"
    payload = {
        "cache_bytes": cache_path.stat().st_size,
        "closure_count": len(warm),
        "closure_source_bytes": sum(path.stat().st_size for path in warm),
        "concurrent_32_wall_ms": concurrent_ns / 1_000_000,
        "concurrent_workers": 8,
        "iterations": args.iterations,
        "schema_version": 1,
        "tracemalloc_peak_bytes_max": max(peak_bytes),
        "warm_wall_ms_median": statistics.median(elapsed_ns) / 1_000_000,
        "warm_wall_ms_p95": ordered[p95_index] / 1_000_000,
    }
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
