#!/usr/bin/env python3
"""Benchmark PEP 695 lazy-value publication without importing Molt's runtime."""

from __future__ import annotations

import argparse
import gc
import hashlib
import importlib.util
import json
import platform
import statistics
import sys
import time
import tracemalloc
import types
from collections.abc import Callable
from pathlib import Path
from typing import Any


def _load_typing(source_root: Path) -> types.ModuleType:
    intrinsic_module = types.ModuleType("_intrinsics")

    def rlock_acquire(lock: object, blocking: bool, timeout: float) -> bool:
        acquire = getattr(lock, "acquire")
        if timeout == -1.0:
            return bool(acquire(blocking))
        return bool(acquire(blocking, timeout))

    def require_intrinsic(name: str) -> object:
        if name == "molt_stdlib_probe":
            return None
        if name == "molt_generic_alias_new":
            return lambda origin, args: types.GenericAlias(origin, args)
        if name == "molt_typing_type_param":
            return lambda factory, name: factory(name)
        if name == "molt_rlock_new":
            return __import__("_thread").RLock
        if name == "molt_rlock_acquire":
            return rlock_acquire
        if name == "molt_rlock_release":
            return lambda lock: lock.release()
        if name.startswith("molt_protocol_"):
            return lambda *_args, **_kwargs: None
        raise RuntimeError(f"runtime intrinsic unavailable while profiling: {name}")

    intrinsic_module.require_intrinsic = require_intrinsic  # type: ignore[attr-defined]
    sys.modules["_intrinsics"] = intrinsic_module
    path = source_root / "src/molt/stdlib/typing.py"
    module_name = (
        f"_molt_typing_profile_{hashlib.sha256(path.read_bytes()).hexdigest()}"
    )
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load typing implementation from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _measure(
    operation: Callable[[], object], *, repeats: int
) -> dict[str, int | float]:
    elapsed: list[int] = []
    retained: list[int] = []
    peaks: list[int] = []
    for _ in range(repeats):
        gc.collect()
        tracemalloc.start()
        before, _ = tracemalloc.get_traced_memory()
        start = time.perf_counter_ns()
        result = operation()
        elapsed.append(time.perf_counter_ns() - start)
        current, peak = tracemalloc.get_traced_memory()
        retained.append(max(0, current - before))
        peaks.append(max(0, peak - before))
        tracemalloc.stop()
        del result
    return {
        "median_ns": int(statistics.median(elapsed)),
        "min_ns": min(elapsed),
        "retained_bytes_median": int(statistics.median(retained)),
        "peak_bytes_median": int(statistics.median(peaks)),
    }


def _disable_synchronization_control(module: types.ModuleType) -> None:
    """Install the pre-synchronization behavior on the same implementation."""

    def evaluate_once(
        owner: object,
        cache_name: str,
        evaluator_name: str,
        key: str,
        fallback: object,
    ) -> object:
        evaluator = getattr(owner, evaluator_name)
        if evaluator is None:
            setattr(owner, cache_name, fallback)
            return fallback
        value = getattr(owner, cache_name)
        if value is module._LAZY_TYPE_VALUE_UNSET:
            value = module._evaluate_lazy_type_value(evaluator, key)
            setattr(owner, cache_name, value)
        return value

    module._MOLT_RLOCK_ACQUIRE = lambda *_args: True
    module._MOLT_RLOCK_RELEASE = lambda *_args: None
    module._evaluate_lazy_type_value_once = evaluate_once


def _profile(
    module: types.ModuleType, *, count: int, warm_reads: int, repeats: int
) -> dict[str, object]:
    def evaluator(_format: int) -> dict[str, object]:
        return {"__value__": int, "__bound__": int}

    def regular_typevars() -> object:
        return [module.TypeVar("T") for _ in range(count)]

    def lazy_typevars() -> object:
        values = [
            module._TypeVar("T", False, False, None, (), pep695=True)
            for _ in range(count)
        ]
        for value in values:
            module._molt_type_param_set_evaluators(value, evaluator, None, None)
        return values

    def aliases() -> object:
        return [module._molt_type_alias("Alias", evaluator, ()) for _ in range(count)]

    def first_typevar_reads() -> object:
        values = lazy_typevars()
        return [value.__bound__ for value in values]

    def first_alias_reads() -> object:
        values = aliases()
        return [value.__value__ for value in values]

    typevar = module._TypeVar("T", False, False, None, (), pep695=True)
    module._molt_type_param_set_evaluators(typevar, evaluator, None, None)
    assert typevar.__bound__ is int
    alias = module._molt_type_alias("Alias", evaluator, ())
    assert alias.__value__ is int

    def warm_typevar_reads_operation() -> object:
        value: Any = None
        for _ in range(warm_reads):
            value = typevar.__bound__
        return value

    def warm_alias_reads_operation() -> object:
        value: Any = None
        for _ in range(warm_reads):
            value = alias.__value__
        return value

    return {
        "count": count,
        "warm_reads": warm_reads,
        "regular_typevar_creation": _measure(regular_typevars, repeats=repeats),
        "lazy_typevar_creation_and_install": _measure(lazy_typevars, repeats=repeats),
        "alias_creation": _measure(aliases, repeats=repeats),
        "lazy_typevar_create_and_first_read": _measure(
            first_typevar_reads, repeats=repeats
        ),
        "alias_create_and_first_read": _measure(first_alias_reads, repeats=repeats),
        "warm_typevar_reads": _measure(warm_typevar_reads_operation, repeats=repeats),
        "warm_alias_reads": _measure(warm_alias_reads_operation, repeats=repeats),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, default=Path(__file__).parents[1])
    parser.add_argument("--label", required=True)
    parser.add_argument("--count", type=int, default=20_000)
    parser.add_argument("--warm-reads", type=int, default=500_000)
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument(
        "--synchronization",
        choices=("synchronized", "unsynchronized-control"),
        default="synchronized",
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    source_root = args.source_root.resolve()
    implementation = source_root / "src/molt/stdlib/typing.py"
    tool = Path(__file__).resolve()
    module = _load_typing(source_root)
    if args.synchronization == "unsynchronized-control":
        _disable_synchronization_control(module)
    result = {
        "schema_version": 1,
        "identity": {
            "label": args.label,
            "source_root": str(source_root),
            "implementation": str(implementation),
            "implementation_sha256": hashlib.sha256(
                implementation.read_bytes()
            ).hexdigest(),
            "tool": str(tool),
            "tool_sha256": hashlib.sha256(tool.read_bytes()).hexdigest(),
            "python": platform.python_version(),
            "platform": platform.platform(),
            "machine": platform.machine(),
            "synchronization": args.synchronization,
        },
        "metrics": _profile(
            module,
            count=args.count,
            warm_reads=args.warm_reads,
            repeats=args.repeats,
        ),
    }
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
