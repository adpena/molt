"""Exercise Molt's actual executor facade against a strict runtime boundary.

Worker-count expectations follow the CPython 3.12 constructor policy. These
tests prove ordering and exception preservation; runtime tests own grant-set
aggregation and worker creation.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest

from tests.stdlib_intrinsic_registry import intrinsic_registry


SOURCE = (
    Path(__file__).resolve().parents[1]
    / "src/molt/stdlib/concurrent/futures/__init__.py"
)


def _unused(*args: object) -> object:
    pytest.fail(f"constructor called an unrelated runtime hook: {args!r}")


def _load(monkeypatch: pytest.MonkeyPatch, require, new, cpu_count):
    # Enumerate the real import contract. Missing hooks must fail resolution;
    # unrelated hooks must fail if the constructor unexpectedly invokes them.
    registry = {
        "molt_concurrent_threadpool_new": new,
        "molt_concurrent_threadpool_drop": _unused,
        "molt_concurrent_threadpool_submit": _unused,
        "molt_concurrent_threadpool_shutdown": lambda *args: None,
        "molt_concurrent_future_add_done_callback": _unused,
        "molt_concurrent_future_cancel": _unused,
        "molt_concurrent_future_cancelled": _unused,
        "molt_concurrent_future_done": _unused,
        "molt_concurrent_future_drop": _unused,
        "molt_concurrent_future_exception": _unused,
        "molt_concurrent_future_result": _unused,
        "molt_concurrent_future_running": _unused,
        "molt_concurrent_wait": _unused,
        "molt_concurrent_as_completed": _unused,
    }
    if require is not None:
        registry["molt_capabilities_require_operation"] = require
    name = "_molt_executor_capability_contract"
    spec = importlib.util.spec_from_file_location(name, SOURCE)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, name, module)
    with intrinsic_registry(registry):
        spec.loader.exec_module(module)
    monkeypatch.setattr(module, "_os", SimpleNamespace(cpu_count=cpu_count))
    return module


@pytest.mark.parametrize(
    ("executor_name", "operation"),
    [
        ("ThreadPoolExecutor", "concurrent.threadpool.default"),
        ("ProcessPoolExecutor", "concurrent.processpool.default"),
    ],
)
def test_default_denial_precedes_cpu_query_and_pool_creation(
    monkeypatch: pytest.MonkeyPatch, executor_name: str, operation: str
):
    calls = []
    denial = PermissionError(
        "missing capabilities: os.cpu_count, thread.spawn, thread.shared"
    )

    def require(name):
        calls.append(name)
        raise denial

    module = _load(monkeypatch, require, _unused, _unused)
    with pytest.raises(PermissionError) as caught:
        getattr(module, executor_name)()
    assert caught.value is denial
    assert str(caught.value) == (
        "missing capabilities: os.cpu_count, thread.spawn, thread.shared"
    )
    assert calls == [operation]


@pytest.mark.parametrize(
    ("executor_name", "operation", "cpu", "workers"),
    [
        ("ThreadPoolExecutor", "concurrent.threadpool.default", 3, 7),
        ("ThreadPoolExecutor", "concurrent.threadpool.default", None, 5),
        ("ThreadPoolExecutor", "concurrent.threadpool.default", 128, 32),
        ("ProcessPoolExecutor", "concurrent.processpool.default", 3, 3),
        ("ProcessPoolExecutor", "concurrent.processpool.default", None, 1),
    ],
)
def test_default_preflight_then_cpu_then_one_pool_creation(
    monkeypatch: pytest.MonkeyPatch, executor_name, operation, cpu, workers
):
    calls = []

    def require(name):
        calls.append(("require", name))

    def cpu_count():
        calls.append(("cpu",))
        return cpu

    def new(count):
        calls.append(("new", count))
        return 17

    module = _load(monkeypatch, require, new, cpu_count)
    with getattr(module, executor_name)() as executor:
        assert executor._max_workers == workers
        assert executor._handle == 17
        assert calls == [("require", operation), ("cpu",), ("new", workers)]


@pytest.mark.parametrize("executor_name", ["ThreadPoolExecutor", "ProcessPoolExecutor"])
def test_explicit_workers_preserve_identity_without_cpu_or_default_preflight(
    monkeypatch: pytest.MonkeyPatch, executor_name: str
):
    class Workers(int):
        def __int__(self):
            pytest.fail("explicit max_workers was coerced")

    workers = Workers(2)
    calls = []

    def new(count):
        calls.append(count)
        return 19

    module = _load(monkeypatch, _unused, new, _unused)
    with getattr(module, executor_name)(workers) as executor:
        assert executor._max_workers is workers
        assert calls == [workers] and calls[0] is workers


@pytest.mark.parametrize("executor_name", ["ThreadPoolExecutor", "ProcessPoolExecutor"])
def test_explicit_pool_gate_preserves_complete_runtime_denial(
    monkeypatch: pytest.MonkeyPatch, executor_name: str
):
    denial = PermissionError("missing capabilities: thread.spawn, thread.shared")
    calls = []

    def new(count):
        calls.append(count)
        raise denial

    module = _load(monkeypatch, _unused, new, _unused)
    with pytest.raises(PermissionError) as caught:
        getattr(module, executor_name)(2)
    assert caught.value is denial
    assert calls == [2]


def test_missing_named_operation_intrinsic_fails_strict_import(monkeypatch):
    with pytest.raises(RuntimeError, match="molt_capabilities_require_operation"):
        _load(monkeypatch, None, _unused, _unused)
