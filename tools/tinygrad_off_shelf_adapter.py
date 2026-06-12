#!/usr/bin/env python3
"""Run small off-the-shelf tinygrad API workloads from an upstream checkout.

The adapter intentionally imports `tinygrad` as a normal package after adding the
provided suite root to `sys.path`. It must not vendor, patch, or translate
tinygrad sources; Molt's benchmark lane compiles this adapter while resolving the
`tinygrad` package from the pinned upstream checkout.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
import time
from pathlib import Path
from typing import Any, Callable


WorkloadFn = Callable[[Any, int], dict[str, Any]]


def _as_nested_list(value: Any) -> Any:
    if hasattr(value, "tolist"):
        return value.tolist()
    if hasattr(value, "numpy"):
        value = value.numpy()
        if hasattr(value, "tolist"):
            return value.tolist()
    return value


def _flatten_numbers(value: Any) -> list[float]:
    if isinstance(value, (int, float, bool)):
        return [float(value)]
    out: list[float] = []
    for item in value:
        out.extend(_flatten_numbers(item))
    return out


def _assert_close(actual: Any, expected: Any, *, workload: str) -> None:
    actual_values = _flatten_numbers(actual)
    expected_values = _flatten_numbers(expected)
    if len(actual_values) != len(expected_values):
        raise AssertionError(
            f"{workload}: result length {len(actual_values)} != {len(expected_values)}"
        )
    for idx, (got, want) in enumerate(zip(actual_values, expected_values)):
        if not math.isclose(got, want, rel_tol=1e-6, abs_tol=1e-6):
            raise AssertionError(f"{workload}: value {idx} got {got}, expected {want}")


def _realize(value: Any) -> Any:
    if hasattr(value, "realize"):
        return value.realize()
    return value


def _workload_elementwise_chain(tinygrad: Any, iterations: int) -> dict[str, Any]:
    tensor = tinygrad.Tensor
    a = tensor([1.0, 2.0, 3.0, 4.0])
    b = tensor([4.0, 3.0, 2.0, 1.0])
    result = None
    start = time.perf_counter()
    for _ in range(iterations):
        result = _realize((a + b) * a)
    elapsed = time.perf_counter() - start
    values = _as_nested_list(result)
    _assert_close(values, [5.0, 10.0, 15.0, 20.0], workload="elementwise_chain")
    return {"elapsed_s": elapsed, "result": values}


def _workload_matmul_2x2(tinygrad: Any, iterations: int) -> dict[str, Any]:
    tensor = tinygrad.Tensor
    a = tensor([[1.0, 2.0], [3.0, 4.0]])
    b = tensor([[5.0, 6.0], [7.0, 8.0]])
    result = None
    start = time.perf_counter()
    for _ in range(iterations):
        result = _realize(a @ b)
    elapsed = time.perf_counter() - start
    values = _as_nested_list(result)
    _assert_close(values, [[19.0, 22.0], [43.0, 50.0]], workload="matmul_2x2")
    return {"elapsed_s": elapsed, "result": values}


WORKLOADS: dict[str, WorkloadFn] = {
    "elementwise_chain": _workload_elementwise_chain,
    "matmul_2x2": _workload_matmul_2x2,
}


def _import_tinygrad(suite_root: Path | None) -> Any:
    if suite_root is not None:
        sys.path.insert(0, str(suite_root.resolve()))
    import tinygrad  # noqa: PLC0415

    if not hasattr(tinygrad, "Tensor"):
        from tinygrad.tensor import Tensor  # noqa: PLC0415

        tinygrad.Tensor = Tensor
    return tinygrad


def _selected_workloads(name: str) -> list[str]:
    if name == "all":
        return sorted(WORKLOADS)
    if name not in WORKLOADS:
        raise ValueError(f"unknown workload {name!r}; expected one of {sorted(WORKLOADS)}")
    return [name]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run public tinygrad API workloads from an upstream checkout."
    )
    parser.add_argument("--suite-root", type=Path)
    parser.add_argument("--workload", default="all")
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--list-workloads", action="store_true")
    args = parser.parse_args(argv)

    if args.list_workloads:
        payload = {"workloads": sorted(WORKLOADS)}
        print(json.dumps(payload, indent=2, sort_keys=True) if args.json else "\n".join(payload["workloads"]))
        return 0
    if args.iterations <= 0:
        raise ValueError("--iterations must be positive")

    tinygrad = _import_tinygrad(args.suite_root)
    selected = _selected_workloads(args.workload)
    results = {
        name: {"iterations": args.iterations, **WORKLOADS[name](tinygrad, args.iterations)}
        for name in selected
    }
    payload = {
        "status": "ok",
        "suite_root": str(args.suite_root.resolve()) if args.suite_root else None,
        "tinygrad_module": getattr(tinygrad, "__file__", None),
        "workloads": results,
    }
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        for name, entry in results.items():
            print(f"{name}: {entry['elapsed_s']:.6f}s ({entry['iterations']} iterations)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
