from __future__ import annotations

import importlib.util
import inspect
import sys
import types
from functools import partial
from pathlib import Path

from molt._intrinsic_symbols import INTRINSIC_SYMBOL_NAMES


def test_coroutine_mark_is_an_identity_protocol_separate_from_code_kind(monkeypatch):
    intrinsic = types.ModuleType("_intrinsics")
    predicates = {
        "molt_is_bound_method": inspect.ismethod,
        "molt_inspect_iscoroutinefunction": inspect.iscoroutinefunction,
        "molt_inspect_isgeneratorfunction": inspect.isgeneratorfunction,
        "molt_inspect_isasyncgenfunction": inspect.isasyncgenfunction,
    }
    # A host stub must not hide missing registration in the compiled loader.
    assert set(predicates) <= INTRINSIC_SYMBOL_NAMES.keys()
    intrinsic.require_intrinsic = lambda name: predicates.get(name, lambda *args: None)
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsic)
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib/inspect.py"
    spec = importlib.util.spec_from_file_location("molt_inspect_mark_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    def direct():
        return 7

    sibling = types.FunctionType(direct.__code__, direct.__globals__)
    direct.__molt_is_coroutine__ = True
    direct._is_coroutine_marker = object()
    assert not module.iscoroutinefunction(direct)
    assert module.markcoroutinefunction(direct) is direct
    assert module.iscoroutinefunction(direct)
    assert module.iscoroutinefunction(partial(direct))
    assert not module.iscoroutinefunction(sibling)
    assert direct() == sibling() == 7
    assert not module.isgeneratorfunction(direct)
    assert not module.isasyncgenfunction(direct)

    class Owner:
        def method(self):
            return 9

    owner = Owner()
    assert module.markcoroutinefunction(owner.method) is Owner.method
    assert module.iscoroutinefunction(owner.method)
    assert module.iscoroutinefunction(partial(owner.method))
    assert owner.method() == 9

    @types.coroutine
    def iterable_coroutine():
        yield 1

    assert module.isgeneratorfunction(iterable_coroutine)
    assert not module.iscoroutinefunction(iterable_coroutine)
