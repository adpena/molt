#!/usr/bin/env python3
"""Run small off-the-shelf NumPy API probes with strict module-origin custody.

The adapter is intentionally a workload driver, not a NumPy fork, shim, or
translation layer. The CPython lane can use an isolated pinned package install
as a baseline. The Molt lane must resolve NumPy through Molt package admission
and fail loudly if the native-extension/source-recompile path is not ready.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
import time
from pathlib import Path
from typing import Any, Callable


WorkloadFn = Callable[[Any, int], dict[str, Any]]

_EXTENSION_SUFFIXES = (".abi3.so", ".so", ".pyd", ".dylib", ".dll")


def _path_is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _relative_posix(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def _json_safe(value: Any) -> Any:
    if hasattr(value, "tolist"):
        return value.tolist()
    if isinstance(value, tuple):
        return [_json_safe(item) for item in value]
    if isinstance(value, list):
        return [_json_safe(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _json_safe(item) for key, item in value.items()}
    if hasattr(value, "item"):
        return value.item()
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
        if not math.isclose(got, want, rel_tol=1e-9, abs_tol=1e-9):
            raise AssertionError(f"{workload}: value {idx} got {got}, expected {want}")


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _module_origin_path(module: Any) -> Path | None:
    module_file = getattr(module, "__file__", None)
    if isinstance(module_file, str) and module_file:
        return Path(module_file).resolve()
    spec = getattr(module, "__spec__", None)
    origin = getattr(spec, "origin", None)
    if not isinstance(origin, str) or not origin:
        return None
    if origin in {"built-in", "frozen", "namespace"} or origin.startswith("<"):
        return None
    return Path(origin).resolve()


def _audit_loaded_numpy_modules(
    *,
    require_module_under: Path | None,
) -> dict[str, Any]:
    root = require_module_under.resolve() if require_module_under is not None else None
    modules: list[dict[str, Any]] = []
    violations: list[str] = []
    extension_modules: list[dict[str, str]] = []
    for name, module in sorted(sys.modules.items()):
        if name != "numpy" and not name.startswith("numpy."):
            continue
        origin = _module_origin_path(module)
        if origin is None:
            modules.append(
                {
                    "name": name,
                    "origin": None,
                    "under_required_root": None if root is None else True,
                }
            )
            continue
        under_required_root = None if root is None else _path_is_relative_to(origin, root)
        entry: dict[str, Any] = {
            "name": name,
            "origin": str(origin),
            "under_required_root": under_required_root,
        }
        if origin.suffix in _EXTENSION_SUFFIXES and origin.is_file():
            sha256 = _sha256_file(origin)
            entry["sha256"] = sha256
            extension_modules.append(
                {"name": name, "origin": str(origin), "sha256": sha256}
            )
        modules.append(entry)
        if root is not None and not under_required_root:
            violations.append(f"{name}: {origin} is outside required root {root}")

    if violations:
        raise RuntimeError(
            "loaded NumPy module origin custody violations:\n"
            + "\n".join(violations)
        )
    return {
        "required_root": str(root) if root is not None else None,
        "modules": modules,
        "extension_modules": extension_modules,
    }


def _audit_source_tree(suite_root: Path) -> dict[str, Any]:
    root = suite_root.resolve()
    required = [
        root / "pyproject.toml",
        root / "numpy" / "__init__.py",
        root / "numpy" / "_core",
    ]
    missing = [_relative_posix(path, root) for path in required if not path.exists()]
    if missing:
        raise FileNotFoundError(
            "NumPy source checkout is missing required paths: " + ", ".join(missing)
        )
    return {
        "suite_root": str(root),
        "required_paths": [_relative_posix(path, root) for path in required],
    }


def _import_numpy(
    suite_root: Path | None,
    *,
    require_module_under: Path | None,
    require_version: str | None,
) -> Any:
    if suite_root is not None:
        sys.path.insert(0, str(suite_root.resolve()))
    import numpy as np  # noqa: PLC0415

    module_file_raw = getattr(np, "__file__", None)
    if not isinstance(module_file_raw, str) or not module_file_raw:
        raise RuntimeError("imported numpy module has no __file__; refusing custody")
    module_file = Path(module_file_raw).resolve()
    if require_module_under is not None:
        root = require_module_under.resolve()
        if not _path_is_relative_to(module_file, root):
            raise RuntimeError(
                f"imported numpy module {module_file} is outside required root {root}"
            )
    if require_version is not None and getattr(np, "__version__", None) != require_version:
        raise RuntimeError(
            f"imported numpy version {getattr(np, '__version__', None)!r} "
            f"!= required {require_version!r}"
        )
    return np


def _workload_array_dtype_shape_tolist(np: Any, iterations: int) -> dict[str, Any]:
    start = time.perf_counter()
    arr = None
    for _ in range(iterations):
        arr = np.array([[1, 2, 3], [4, 5, 6]], dtype=np.int64)
        values = arr.tolist()
    elapsed = time.perf_counter() - start
    assert arr is not None
    if tuple(arr.shape) != (2, 3):
        raise AssertionError(f"array_dtype_shape_tolist: shape {arr.shape!r}")
    if str(arr.dtype) != "int64":
        raise AssertionError(f"array_dtype_shape_tolist: dtype {arr.dtype!r}")
    if values != [[1, 2, 3], [4, 5, 6]]:
        raise AssertionError(f"array_dtype_shape_tolist: values {values!r}")
    return {
        "elapsed_s": elapsed,
        "dtype": str(arr.dtype),
        "shape": list(arr.shape),
        "result": values,
    }


def _workload_sum_reshape(np: Any, iterations: int) -> dict[str, Any]:
    start = time.perf_counter()
    result = None
    for _ in range(iterations):
        arr = np.arange(12, dtype=np.float64).reshape(3, 4)
        result = arr.sum(axis=1)
    elapsed = time.perf_counter() - start
    values = _json_safe(result)
    _assert_close(values, [6.0, 22.0, 38.0], workload="sum_reshape")
    return {"elapsed_s": elapsed, "result": values}


def _workload_matmul_2x2(np: Any, iterations: int) -> dict[str, Any]:
    a = np.array([[1.0, 2.0], [3.0, 4.0]], dtype=np.float64)
    b = np.array([[5.0, 6.0], [7.0, 8.0]], dtype=np.float64)
    start = time.perf_counter()
    result = None
    for _ in range(iterations):
        result = a @ b
    elapsed = time.perf_counter() - start
    values = _json_safe(result)
    _assert_close(values, [[19.0, 22.0], [43.0, 50.0]], workload="matmul_2x2")
    return {"elapsed_s": elapsed, "result": values}


def _workload_broadcast_where(np: Any, iterations: int) -> dict[str, Any]:
    cond = np.array([[True, False], [False, True]])
    left = np.array([10.0, 20.0])
    right = np.array([[1.0], [2.0]])
    start = time.perf_counter()
    result = None
    for _ in range(iterations):
        result = np.where(cond, left, right)
    elapsed = time.perf_counter() - start
    values = _json_safe(result)
    _assert_close(values, [[10.0, 1.0], [2.0, 20.0]], workload="broadcast_where")
    return {"elapsed_s": elapsed, "result": values}


WORKLOADS: dict[str, WorkloadFn] = {
    "array_dtype_shape_tolist": _workload_array_dtype_shape_tolist,
    "broadcast_where": _workload_broadcast_where,
    "matmul_2x2": _workload_matmul_2x2,
    "sum_reshape": _workload_sum_reshape,
}


def _selected_workloads(name: str) -> list[str]:
    if name == "all":
        return sorted(WORKLOADS)
    if name not in WORKLOADS:
        raise ValueError(f"unknown workload {name!r}; expected one of {sorted(WORKLOADS)}")
    return [name]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run public NumPy API workloads with strict module custody."
    )
    parser.add_argument("--suite-root", type=Path)
    parser.add_argument("--source-tree-audit", action="store_true")
    parser.add_argument("--workload", default="all")
    parser.add_argument("--iterations", type=int, default=5)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--list-workloads", action="store_true")
    parser.add_argument("--require-module-under", type=Path)
    parser.add_argument("--require-version")
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

    source_audit = None
    if args.source_tree_audit:
        if args.suite_root is None:
            raise ValueError("--source-tree-audit requires --suite-root")
        source_audit = _audit_source_tree(args.suite_root)
        if args.workload == "none":
            payload = {"status": "ok", "source_tree": source_audit}
            print(json.dumps(payload, indent=2, sort_keys=True) if args.json else "source tree ok")
            return 0

    np = _import_numpy(
        args.suite_root if not args.source_tree_audit else None,
        require_module_under=args.require_module_under,
        require_version=args.require_version,
    )
    pre_workload_module_audit = _audit_loaded_numpy_modules(
        require_module_under=args.require_module_under
    )
    selected = _selected_workloads(args.workload)
    results = {
        name: {"iterations": args.iterations, **WORKLOADS[name](np, args.iterations)}
        for name in selected
    }
    module_audit = _audit_loaded_numpy_modules(
        require_module_under=args.require_module_under
    )
    payload = {
        "status": "ok",
        "suite_root": str(args.suite_root.resolve()) if args.suite_root else None,
        "source_tree": source_audit,
        "numpy_module": getattr(np, "__file__", None),
        "numpy_modules_imported": pre_workload_module_audit,
        "numpy_modules": module_audit,
        "numpy_version": getattr(np, "__version__", None),
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
