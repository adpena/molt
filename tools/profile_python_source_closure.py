#!/usr/bin/env python3
"""Measure one closure case per fresh, externally guarded interpreter.

Cold means an absent disk graph cache, not cold OS pages. Warm performs one
untimed closure first. Allocation tracing includes that warmup's retained state;
its timings must not be compared as uninstrumented latency. Imports precede
tracing. The enclosing guard owns whole-invocation process-tree RSS, including
startup and warmup; this tool does not create a second RSS sampler or guard.
Each measured batch includes concurrent pool startup/shutdown, excludes receipt
comparison/publication, and resets the traced peak while retaining warmup state.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import statistics
import sys
from threading import Barrier
import time
import tracemalloc


ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.artifact_publication import atomic_write_json  # noqa: E402
from molt.dx import scratch_dir  # noqa: E402
from molt.cli.python_source_closure import (  # noqa: E402
    LocalPythonSourceClosure,
    local_python_import_closure,
    python_source_closure_cache_path,
)


def _batch(
    root: Path, seed: Path, workers: int
) -> tuple[LocalPythonSourceClosure, ...]:
    if workers == 1:
        return (local_python_import_closure(root, (seed,)),)
    gate = Barrier(workers)

    def collect(_index: int) -> LocalPythonSourceClosure:
        gate.wait()
        return local_python_import_closure(root, (seed,))

    # Pool creation and closure are part of concurrent batch latency. Abort the
    # gate before shutdown if submission fails, so already started workers exit.
    with ThreadPoolExecutor(max_workers=workers) as pool:
        try:
            return tuple(pool.map(collect, range(workers)))
        finally:
            gate.abort()


def profile_closure(
    root: Path,
    seed: Path,
    *,
    cache_state: str,
    instrumentation: str,
    workers: int,
    iterations: int,
) -> dict[str, object]:
    if cache_state not in {"cold", "warm"}:
        raise ValueError("cache_state must be cold or warm")
    if instrumentation not in {"none", "tracemalloc"}:
        raise ValueError("instrumentation must be none or tracemalloc")
    if workers <= 0 or iterations <= 0:
        raise ValueError("workers and iterations must be positive")
    if cache_state == "cold" and iterations != 1:
        raise ValueError("cold requires one batch per fresh interpreter and cache")
    if tracemalloc.is_tracing():
        raise ValueError("profile requires exclusive ownership of allocation tracing")
    root, seed = root.resolve(), seed.resolve()
    cache_path = python_source_closure_cache_path(root)
    cache_existed = cache_path.exists()
    if cache_state == "cold" and cache_existed:
        raise ValueError(
            "cold requires an absent graph cache; select a fresh MOLT_CACHE"
        )

    tracing = instrumentation == "tracemalloc"
    samples: list[dict[str, int | None]] = []
    expected = None
    warmup_peak = None
    if tracing:
        tracemalloc.start()
    try:
        if cache_state == "warm":
            expected = local_python_import_closure(root, (seed,))
            if tracing:
                warmup_peak = tracemalloc.get_traced_memory()[1]
        for _ in range(iterations):
            baseline = None
            if tracing:
                baseline = tracemalloc.get_traced_memory()[0]
                tracemalloc.reset_peak()
            cpu_started = time.process_time_ns()
            started = time.perf_counter_ns()
            results = _batch(root, seed, workers)
            elapsed = time.perf_counter_ns() - started
            cpu_elapsed = time.process_time_ns() - cpu_started
            current, peak = tracemalloc.get_traced_memory() if tracing else (None, None)
            if expected is None:
                expected = results[0]
            if any(result != expected for result in results):
                raise RuntimeError("Python source closure changed during profile")
            samples.append(
                {
                    "wall_ns": elapsed,
                    "process_cpu_ns": cpu_elapsed,
                    "traced_baseline_bytes": baseline,
                    "traced_current_bytes": current,
                    "traced_peak_bytes": peak,
                }
            )
            del results
    finally:
        if tracing:
            tracemalloc.stop()

    assert expected is not None
    return {
        "schema_version": 2,
        "root": str(root),
        "seed": str(seed),
        "cache_state": cache_state,
        "cache_existed_before_warmup": cache_existed,
        "cache_path": str(cache_path),
        "cache_bytes": cache_path.stat().st_size,
        "instrumentation": instrumentation,
        "batch_scope": "closure calls including thread-pool startup/shutdown",
        "allocation_scope": "post-import retained bytes; peak reset before each batch",
        "workers": workers,
        "iterations": iterations,
        "measured_calls": workers * iterations,
        "warmup_calls": int(cache_state == "warm"),
        "closure_count": len(expected.paths),
        "closure_source_bytes": expected.source_bytes,
        "closure_content_digest": expected.content_digest,
        "warmup_traced_peak_bytes": warmup_peak,
        "samples": samples,
        "wall_ns_median": statistics.median(sample["wall_ns"] for sample in samples),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=Path, default=ROOT / "tools" / "wasm_link.py")
    parser.add_argument("--cache-state", choices=("cold", "warm"), required=True)
    parser.add_argument(
        "--instrumentation", choices=("none", "tracemalloc"), required=True
    )
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--iterations", type=int, default=1)
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()
    payload = profile_closure(
        ROOT,
        args.seed,
        cache_state=args.cache_state,
        instrumentation=args.instrumentation,
        workers=args.workers,
        iterations=args.iterations,
    )
    output = args.output or scratch_dir(ROOT, "python_source_closure") / "profile.json"
    atomic_write_json(output.resolve(), payload, sort_keys=True)
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
