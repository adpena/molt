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
import os
import sys
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable, Iterator


WorkloadFn = Callable[[Any, int], dict[str, Any]]
STATIC_EXEC_REGISTRY_MODULE = "_molt_tinygrad_upat_static_exec_registry"
STATIC_EXEC_REGISTRY_ROOT_ENV = "MOLT_TINYGRAD_UPAT_STATIC_EXEC_ROOT"


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


def _workload_attention_core(tinygrad: Any, iterations: int) -> dict[str, Any]:
    tensor = tinygrad.Tensor
    q = tensor([[[[1.0, 0.0], [0.0, 1.0]]]])
    k = tensor([[[[1.0, 0.0], [0.0, 1.0]]]])
    v = tensor([[[[10.0, 1.0], [2.0, 20.0]]]])
    mask = tensor([[[[0.0, -1.0e9], [-1.0e9, 0.0]]]])
    result = None
    start = time.perf_counter()
    for _ in range(iterations):
        result = _realize(
            q.scaled_dot_product_attention(k, v, attn_mask=mask, is_causal=False)
        )
    elapsed = time.perf_counter() - start
    values = _as_nested_list(result)
    _assert_close(
        values,
        [[[[10.0, 1.0], [2.0, 20.0]]]],
        workload="attention_core",
    )
    return {"elapsed_s": elapsed, "result": values}


def _workload_where_promotion(tinygrad: Any, iterations: int) -> dict[str, Any]:
    tensor = tinygrad.Tensor
    cond = tensor([1, 0, 1, 0])
    branch = tensor([1.5, 2.5, 3.5, 4.5])
    result = None
    start = time.perf_counter()
    for _ in range(iterations):
        result = _realize(cond.where(5, branch))
    elapsed = time.perf_counter() - start
    values = _as_nested_list(result)
    _assert_close(values, [5.0, 2.5, 5.0, 4.5], workload="where_promotion")
    return {"elapsed_s": elapsed, "result": values}


def _workload_movement_views(tinygrad: Any, iterations: int) -> dict[str, Any]:
    tensor = tinygrad.Tensor
    base = tensor([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])
    result = None
    start = time.perf_counter()
    for _ in range(iterations):
        result = _realize(
            base.pad((1, 0, 0, 1)).shrink(((1, 3), (1, 3))).flip(1).contiguous()
        )
    elapsed = time.perf_counter() - start
    values = _as_nested_list(result)
    _assert_close(values, [[5.0, 4.0], [0.0, 0.0]], workload="movement_views")
    return {"elapsed_s": elapsed, "result": values}


WORKLOADS: dict[str, WorkloadFn] = {
    "attention_core": _workload_attention_core,
    "elementwise_chain": _workload_elementwise_chain,
    "matmul_2x2": _workload_matmul_2x2,
    "movement_views": _workload_movement_views,
    "where_promotion": _workload_where_promotion,
}


@contextmanager
def _suppress_bytecode_writes() -> Iterator[None]:
    # Off-the-shelf custody requires the upstream checkout to remain byte-clean:
    # adapter probes must not leave CPython cache files beside pinned sources.
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        yield
    finally:
        sys.dont_write_bytecode = previous


@contextmanager
def _suite_root_import_path(suite_root: Path | None) -> Iterator[None]:
    if suite_root is None:
        yield
        return
    path_entry = str(suite_root.resolve())
    sys.path.insert(0, path_entry)
    try:
        yield
    finally:
        if sys.path and sys.path[0] == path_entry:
            del sys.path[0]
        else:
            try:
                sys.path.remove(path_entry)
            except ValueError as exc:
                raise RuntimeError(
                    f"tinygrad suite root escaped sys.path cleanup: {path_entry}"
                ) from exc


def _import_tinygrad() -> Any:
    import tinygrad  # noqa: PLC0415

    if not hasattr(tinygrad, "Tensor"):
        from tinygrad.tensor import Tensor  # noqa: PLC0415

        tinygrad.Tensor = Tensor
    return tinygrad


def _install_tinygrad_upat_static_exec_registry(tinygrad: Any) -> bool:
    registry_root = os.environ.get(STATIC_EXEC_REGISTRY_ROOT_ENV, "").strip()
    if not registry_root:
        return False
    root_path = str(Path(registry_root).resolve())
    sys.path.insert(0, root_path)
    try:
        import _molt_tinygrad_upat_static_exec_registry as registry  # noqa: PLC0415
    except Exception as exc:
        raise RuntimeError(
            "MOLT_COMPAT_ERROR: configured tinygrad UPat static exec registry "
            f"{STATIC_EXEC_REGISTRY_MODULE!r} could not be imported from {root_path!r}"
        ) from exc
    finally:
        try:
            sys.path.remove(root_path)
        except ValueError:
            pass
    exec_static = getattr(registry, "exec_static", None)
    if exec_static is None:
        raise RuntimeError(
            "MOLT_COMPAT_ERROR: tinygrad UPat static exec registry does not "
            "export exec_static"
        )
    from tinygrad.uop import upat  # noqa: PLC0415

    upat.exec = exec_static
    setattr(tinygrad, "_molt_upat_static_exec_registry", registry)
    return True


def _selected_workloads(name: str) -> list[str]:
    if name == "all":
        return sorted(WORKLOADS)
    if name not in WORKLOADS:
        raise ValueError(
            f"unknown workload {name!r}; expected one of {sorted(WORKLOADS)}"
        )
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
        print(
            json.dumps(payload, indent=2, sort_keys=True)
            if args.json
            else "\n".join(payload["workloads"])
        )
        return 0
    if args.iterations <= 0:
        raise ValueError("--iterations must be positive")

    with _suppress_bytecode_writes(), _suite_root_import_path(args.suite_root):
        tinygrad = _import_tinygrad()
        static_exec_registry = _install_tinygrad_upat_static_exec_registry(tinygrad)
        selected = _selected_workloads(args.workload)
        results = {
            name: {
                "iterations": args.iterations,
                **WORKLOADS[name](tinygrad, args.iterations),
            }
            for name in selected
        }
    payload = {
        "status": "ok",
        "suite_root": str(args.suite_root.resolve()) if args.suite_root else None,
        "static_exec_registry": static_exec_registry,
        "tinygrad_module": getattr(tinygrad, "__file__", None),
        "workloads": results,
    }
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        for name, entry in results.items():
            print(
                f"{name}: {entry['elapsed_s']:.6f}s ({entry['iterations']} iterations)"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
