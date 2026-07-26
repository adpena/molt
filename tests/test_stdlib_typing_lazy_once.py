from __future__ import annotations

import importlib.util
import sys
import types
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import Event, Lock

import pytest


@pytest.fixture
def molt_typing(monkeypatch: pytest.MonkeyPatch):
    intrinsic_module = types.ModuleType("_intrinsics")

    def rlock_acquire(lock, blocking: bool, timeout: float) -> bool:
        if timeout == -1.0:
            return lock.acquire(blocking)
        return lock.acquire(blocking, timeout)

    def require_intrinsic(name: str):
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
        raise RuntimeError("runtime inactive")

    intrinsic_module.require_intrinsic = require_intrinsic  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsic_module)
    typing_path = Path(__file__).parents[1] / "src/molt/stdlib/typing.py"
    spec = importlib.util.spec_from_file_location("_molt_typing_lazy_once", typing_path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    previous_type_attrs = {
        name: getattr(types, name, None) for name in ("Any", "Iterable")
    }
    previous_presence = {name: hasattr(types, name) for name in previous_type_attrs}
    try:
        spec.loader.exec_module(module)
        yield module
    finally:
        for name, value in previous_type_attrs.items():
            if previous_presence[name]:
                setattr(types, name, value)
            elif hasattr(types, name):
                delattr(types, name)


def test_type_alias_concurrent_first_read_evaluates_once(molt_typing) -> None:
    entered = Event()
    release = Event()
    calls = 0
    value = object()

    def evaluator(_format: int) -> dict[str, object]:
        nonlocal calls
        calls += 1
        entered.set()
        assert release.wait(5)
        return {"__value__": value}

    alias = molt_typing._molt_type_alias("Alias", evaluator, ())
    with ThreadPoolExecutor(max_workers=8) as executor:
        futures = [executor.submit(lambda: alias.__value__) for _ in range(8)]
        assert entered.wait(5)
        release.set()
        assert [future.result() for future in futures] == [value] * 8

    assert calls == 1


def test_type_parameter_concurrent_bound_read_evaluates_once(molt_typing) -> None:
    entered = Event()
    release = Event()
    calls_lock = Lock()
    calls = 0
    value = object()

    def evaluator(_format: int) -> dict[str, object]:
        nonlocal calls
        with calls_lock:
            calls += 1
        entered.set()
        assert release.wait(5)
        return {"__bound__": value}

    parameter = molt_typing._TypeVar("T", False, False, None, (), pep695=True)
    molt_typing._molt_type_param_set_evaluators(parameter, evaluator, None, None)
    with ThreadPoolExecutor(max_workers=8) as executor:
        futures = [executor.submit(lambda: parameter.__bound__) for _ in range(8)]
        assert entered.wait(5)
        release.set()
        assert [future.result() for future in futures] == [value] * 8

    assert calls == 1


def test_lazy_type_value_failure_is_not_cached_and_next_read_retries(
    molt_typing,
) -> None:
    calls = 0

    def evaluator(_format: int) -> dict[str, object]:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise LookupError("not ready")
        return {"__value__": "ready"}

    alias = molt_typing._molt_type_alias("Alias", evaluator, ())
    with pytest.raises(LookupError, match="not ready"):
        _ = alias.__value__
    assert alias.__value__ == "ready"
    assert alias.__value__ == "ready"
    assert calls == 2


def test_recursive_type_alias_evaluation_raises_and_next_read_retries(
    molt_typing,
) -> None:
    recursive = True
    calls = 0
    alias = None

    def evaluator(_format: int) -> dict[str, object]:
        nonlocal calls
        calls += 1
        assert alias is not None
        if recursive:
            return {"__value__": alias.__value__}
        return {"__value__": "recovered"}

    alias = molt_typing._molt_type_alias("Alias", evaluator, ())
    with pytest.raises(RecursionError, match="maximum recursion depth exceeded"):
        _ = alias.__value__
    recursive = False
    recursive_calls = calls
    assert alias.__value__ == "recovered"
    assert alias.__value__ == "recovered"
    assert calls == recursive_calls + 1


def test_applied_type_alias_repr_formats_concrete_argument_sequence(
    molt_typing,
) -> None:
    alias = molt_typing._molt_type_alias(
        "Pair",
        lambda _format: {"__value__": tuple},
        (),
    )

    assert repr(alias[int]) == "Pair[int]"
    assert repr(alias[int, str]) == "Pair[int, str]"


@pytest.mark.parametrize("shift", [0, 4, 12, 20, 32, 40, 48, 56])
def test_lazy_type_lock_mix_distributes_address_and_handle_families(
    molt_typing, shift: int
) -> None:
    indexes = {
        molt_typing._lazy_type_lock_index(identity << shift) for identity in range(4096)
    }
    assert indexes == set(range(molt_typing._LAZY_TYPE_LOCK_MASK + 1))
