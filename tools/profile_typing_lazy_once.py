#!/usr/bin/env python3
"""Benchmark PEP 695 lazy-value publication without importing Molt's runtime."""

from __future__ import annotations

import argparse
import builtins
from contextlib import contextmanager
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
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tests.stdlib_intrinsic_registry import intrinsic_registry  # noqa: E402


def _host_collection_runtime_types() -> dict[str, type]:
    """Actual host runtime input for the selected collections.abc source."""
    return {
        "bytes_iterator": type(iter(b"")),
        "bytearray_iterator": type(iter(bytearray())),
        "dict_keyiterator": type(iter({}.keys())),
        "dict_valueiterator": type(iter({}.values())),
        "dict_itemiterator": type(iter({}.items())),
        "list_iterator": type(iter([])),
        "list_reverseiterator": type(reversed([])),
        "range_iterator": type(iter(range(0))),
        "longrange_iterator": type(iter(range(1 << 65, (1 << 65) + 1))),
        "set_iterator": type(iter(set())),
        "str_iterator": type(iter("")),
        "tuple_iterator": type(iter(())),
        "zip_iterator": type(iter(zip())),
        "dict_keys": type({}.keys()),
        "dict_values": type({}.values()),
        "dict_items": type({}.items()),
        "mappingproxy": type(type.__dict__),
        "framelocalsproxy": type(sys._getframe().f_locals),
        "generator": types.GeneratorType,
        "coroutine": types.CoroutineType,
        "async_generator": types.AsyncGeneratorType,
    }


def typing_source_inputs(source_root: Path) -> dict[str, str]:
    """Bind selected source and the actual host fixture/resolver reader authority."""
    paths = {
        "typing": source_root / "src/molt/stdlib/typing.py",
        "collections_abc": source_root / "src/molt/stdlib/_collections_abc.py",
        "resolver": _REPO_ROOT / "src/molt/stdlib/_intrinsics.py",
        "registry": _REPO_ROOT / "tests/stdlib_intrinsic_registry.py",
        "loader": Path(__file__).resolve(),
    }
    return {
        f"{name}:{path}": hashlib.sha256(path.read_bytes()).hexdigest()
        for name, path in paths.items()
    }


@contextmanager
def typing_source_modules(
    source_root: Path,
) -> Iterator[tuple[types.ModuleType, types.ModuleType]]:
    """Execute both genuine sources with private imports and the strict resolver.

    The host typing/collections modules remain untouched for independent oracles.
    This one loader is shared by semantic tests and both profiling tools.
    """
    import _thread

    def rlock_acquire(lock: object, blocking: bool, timeout: float) -> bool:
        acquire = getattr(lock, "acquire")
        return bool(
            acquire(blocking) if timeout == -1.0 else acquire(blocking, timeout)
        )

    behavior = {
        "molt_typing_cast": lambda _tp, value: value,
        "molt_generic_alias_new": types.GenericAlias,
        "molt_typing_type_param": lambda factory, name: factory(name),
        "molt_rlock_new": _thread.RLock,
        "molt_rlock_acquire": rlock_acquire,
        "molt_rlock_release": lambda lock: lock.release(),
        "molt_protocol_check": lambda *_args: None,
        "molt_protocol_get_structural_members": lambda *_args: None,
        "molt_protocol_register": lambda *_args: None,
        "molt_collections_abc_runtime_types": _host_collection_runtime_types,
    }
    specs = [
        importlib.util.spec_from_file_location(
            name, source_root / "src/molt/stdlib" / filename
        )
        for name, filename in (
            ("_collections_abc", "_collections_abc.py"),
            ("typing", "typing.py"),
        )
    ]
    collections_spec, typing_spec = specs
    if collections_spec is None or collections_spec.loader is None:
        raise RuntimeError(
            f"cannot load selected collections source from {source_root}"
        )
    if typing_spec is None or typing_spec.loader is None:
        raise RuntimeError(f"cannot load selected typing source from {source_root}")
    collections_module = importlib.util.module_from_spec(collections_spec)
    typing_module = importlib.util.module_from_spec(typing_spec)
    with intrinsic_registry(behavior) as resolver:
        selected = {
            "_intrinsics": resolver,
            "_collections_abc": collections_module,
            "typing": typing_module,
        }
        original_import = builtins.__import__

        def source_import(name, globals=None, locals=None, fromlist=(), level=0):
            if level == 0 and name in selected:
                return selected[name]
            return original_import(name, globals, locals, fromlist, level)

        for module in (collections_module, typing_module):
            module.__dict__["__builtins__"] = {
                **vars(builtins),
                "__import__": source_import,
            }
        collections_spec.loader.exec_module(collections_module)
        typing_spec.loader.exec_module(typing_module)
        yield typing_module, collections_module


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
    with typing_source_modules(source_root) as (module, _collections):
        if args.synchronization == "unsynchronized-control":
            _disable_synchronization_control(module)
        result = {
            "schema_version": 1,
            "identity": {
                "source_inputs_sha256": typing_source_inputs(source_root),
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
