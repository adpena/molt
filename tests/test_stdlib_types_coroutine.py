from __future__ import annotations

import importlib.util
import inspect
import sys
import types
from collections.abc import Generator
from pathlib import Path

import pytest


@pytest.fixture
def molt_types(monkeypatch):
    bootstrap = {
        name: getattr(types, name)
        for name in ("FunctionType", "CodeType", "GeneratorType", "CoroutineType")
    }
    bootstrap["coroutine"] = types.coroutine
    intrinsic = types.ModuleType("_intrinsics")
    intrinsic.require_intrinsic = lambda name: lambda: bootstrap
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsic)
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib/types.py"
    spec = importlib.util.spec_from_file_location("molt_types_coroutine_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, spec.name, module)
    spec.loader.exec_module(module)
    return module


def test_generator_decoration_preserves_shared_code_and_execution_kind(molt_types):
    def generator():
        yield 7

    sibling = types.FunctionType(generator.__code__, generator.__globals__)
    original = generator.__code__
    assert molt_types.coroutine(generator) is generator
    assert generator.__code__ is not original
    assert sibling.__code__ is original
    assert generator.__code__.co_flags & 0x100
    assert not sibling.__code__.co_flags & 0x100
    assert inspect.isgeneratorfunction(generator)
    assert not inspect.iscoroutinefunction(generator)
    assert molt_types.coroutine(generator) is generator


def test_general_callable_wrapper_preserves_results_and_delegation(molt_types):
    class GeneratorLike(Generator):
        def __init__(self):
            self.calls = []

        def send(self, value):
            self.calls.append(("send", value))
            return 13

        def throw(self, *args):
            self.calls.append(("throw", args))
            return 17

        def close(self):
            self.calls.append(("close",))

    gen = GeneratorLike()

    class Factory:
        def __call__(self, *args, **kwargs):
            assert args == (3,)
            assert kwargs == {"key": 5}
            return gen

    wrapped = molt_types.coroutine(Factory())(3, key=5)
    assert wrapped.__name__ is None
    assert wrapped.__qualname__ is None
    assert inspect.isawaitable(wrapped)
    assert not inspect.iscoroutine(wrapped)
    assert iter(wrapped) is wrapped
    assert wrapped.__await__() is wrapped
    assert wrapped.send(19) == 13
    assert wrapped.throw(ValueError, None, None) == 17
    assert gen.calls == [("send", 19), ("throw", (ValueError, None, None))]
    wrapped.close()
    assert gen.calls[-1] == ("close",)
    assert molt_types.coroutine(lambda value: value)(23) == 23
    with pytest.raises(TypeError, match="expects a callable"):
        molt_types.coroutine(3)


def test_generator_wrapper_captures_names_and_preserves_iterator_identity(molt_types):
    def generator():
        yield 41

    gen = generator()
    wrapped = molt_types.coroutine(lambda: gen)()
    try:
        assert wrapped.__name__ == gen.__name__
        assert wrapped.__qualname__ == gen.__qualname__
        assert iter(wrapped) is gen
        assert wrapped.__await__() is gen
        assert next(wrapped) == 41
    finally:
        gen.close()


def test_coroutine_and_generator_results_keep_identity(molt_types):
    async def coroutine():
        return 29

    assert molt_types.coroutine(coroutine) is coroutine
    coro = coroutine()
    try:
        assert molt_types.coroutine(lambda: coro)() is coro
    finally:
        coro.close()

    @types.coroutine
    def iterable():
        yield 31

    gen = iterable()
    try:
        assert molt_types.coroutine(lambda: gen)() is gen
    finally:
        gen.close()
