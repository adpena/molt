#!/usr/bin/env python3
"""Size and profile the PEP 695 striped lazy-publication lock pool."""

from __future__ import annotations

import argparse
import gc
import hashlib
import json
import platform
import statistics
import threading
import time
import tracemalloc
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from types import ModuleType

from profile_typing_lazy_once import _load_typing


def _pool_allocation(module: ModuleType, stripe_count: int) -> dict[str, int]:
    gc.collect()
    tracemalloc.start()
    before, _ = tracemalloc.get_traced_memory()
    pool = tuple(module._MOLT_RLOCK_NEW() for _ in range(stripe_count))
    current, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    result = {
        "stripe_count": stripe_count,
        "retained_bytes": max(0, current - before),
        "peak_bytes": max(0, peak - before),
    }
    del pool
    return result


def _configure_pool(module: ModuleType, stripe_count: int) -> None:
    if stripe_count <= 0 or stripe_count & (stripe_count - 1):
        raise ValueError("stripe count must be a positive power of two")
    module._LAZY_TYPE_LOCK_MASK = stripe_count - 1
    module._LAZY_TYPE_LOCKS = tuple(
        module._MOLT_RLOCK_NEW() for _ in range(stripe_count)
    )


def _different_alias_sample(
    module: ModuleType, *, thread_count: int, evaluator_delay_seconds: float
) -> dict[str, int]:
    start = threading.Event()
    calls = 0
    calls_lock = threading.Lock()

    def evaluator(_format: int) -> dict[str, object]:
        nonlocal calls
        with calls_lock:
            calls += 1
        time.sleep(evaluator_delay_seconds)
        return {"__value__": int}

    aliases = [
        module._molt_type_alias(f"Alias{index}", evaluator, ())
        for index in range(thread_count)
    ]
    distinct_stripes = len(
        {module._lazy_type_lock_index(id(alias)) for alias in aliases}
    )

    def read(alias: object) -> object:
        start.wait()
        return alias.__value__

    with ThreadPoolExecutor(max_workers=thread_count) as executor:
        futures = [executor.submit(read, alias) for alias in aliases]
        begin = time.perf_counter_ns()
        start.set()
        values = [future.result() for future in futures]
        elapsed = time.perf_counter_ns() - begin
    assert values == [int] * thread_count
    assert calls == thread_count
    return {"elapsed_ns": elapsed, "distinct_stripes": distinct_stripes}


def _same_alias_sample(
    module: ModuleType, *, thread_count: int, evaluator_delay_seconds: float
) -> dict[str, int]:
    start = threading.Event()
    calls = 0
    calls_lock = threading.Lock()

    def evaluator(_format: int) -> dict[str, object]:
        nonlocal calls
        with calls_lock:
            calls += 1
        time.sleep(evaluator_delay_seconds)
        return {"__value__": int}

    alias = module._molt_type_alias("SharedAlias", evaluator, ())

    def read() -> object:
        start.wait()
        return alias.__value__

    with ThreadPoolExecutor(max_workers=thread_count) as executor:
        futures = [executor.submit(read) for _ in range(thread_count)]
        begin = time.perf_counter_ns()
        start.set()
        values = [future.result() for future in futures]
        elapsed = time.perf_counter_ns() - begin
    assert values == [int] * thread_count
    assert calls == 1
    return {"elapsed_ns": elapsed, "evaluator_calls": calls}


def _median(samples: list[dict[str, int]], key: str) -> int:
    return int(statistics.median(sample[key] for sample in samples))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).parents[1])
    parser.add_argument("--output", type=Path)
    parser.add_argument("--repeats", type=int, default=9)
    parser.add_argument("--evaluator-delay-ms", type=float, default=5.0)
    args = parser.parse_args()
    module = _load_typing(args.source_root.resolve())
    source_root = args.source_root.resolve()
    implementation = source_root / "src/molt/stdlib/typing.py"
    tool = Path(__file__).resolve()
    stripe_counts = (1, 2, 4, 8, 16, 32, 64, 128, 256)
    thread_counts = (1, 2, 4, 8)
    delay_seconds = args.evaluator_delay_ms / 1000.0
    rows: list[dict[str, object]] = []
    for stripe_count in stripe_counts:
        _configure_pool(module, stripe_count)
        concurrency: dict[str, object] = {}
        for thread_count in thread_counts:
            samples = [
                _different_alias_sample(
                    module,
                    thread_count=thread_count,
                    evaluator_delay_seconds=delay_seconds,
                )
                for _ in range(args.repeats)
            ]
            concurrency[str(thread_count)] = {
                "elapsed_ns_median": _median(samples, "elapsed_ns"),
                "elapsed_ns_max": max(sample["elapsed_ns"] for sample in samples),
                "distinct_stripes_median": _median(samples, "distinct_stripes"),
                "collision_sample_count": sum(
                    sample["distinct_stripes"] < thread_count for sample in samples
                ),
            }
        rows.append(
            {
                "pool": _pool_allocation(module, stripe_count),
                "different_aliases": concurrency,
            }
        )
    selected_stripe_count = 128
    _configure_pool(module, selected_stripe_count)
    same_alias_samples = [
        _same_alias_sample(
            module, thread_count=8, evaluator_delay_seconds=delay_seconds
        )
        for _ in range(args.repeats)
    ]
    payload = {
        "schema_version": 1,
        "identity": {
            "source_root": str(source_root),
            "implementation_sha256": hashlib.sha256(
                implementation.read_bytes()
            ).hexdigest(),
            "tool_sha256": hashlib.sha256(tool.read_bytes()).hexdigest(),
            "python": platform.python_version(),
            "platform": platform.platform(),
            "machine": platform.machine(),
        },
        "repeats": args.repeats,
        "evaluator_delay_ns": int(delay_seconds * 1_000_000_000),
        "selected_stripe_count": selected_stripe_count,
        "stripe_rows": rows,
        "same_alias_8_threads": {
            "elapsed_ns_median": _median(same_alias_samples, "elapsed_ns"),
            "elapsed_ns_max": max(
                sample["elapsed_ns"] for sample in same_alias_samples
            ),
            "evaluator_calls_per_sample": sorted(
                {sample["evaluator_calls"] for sample in same_alias_samples}
            ),
        },
    }
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
