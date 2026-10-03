from __future__ import annotations

import importlib.util
import inspect
import sys
import types
from pathlib import Path

import pytest


@pytest.fixture
def molt_inspect(monkeypatch):
    intrinsic = types.ModuleType("_intrinsics")

    def signature_data(value):
        # This boundary must receive an unbound function. The compiled runtime
        # owns its payload; explicit signatures below need no fabricated one.
        assert not inspect.ismethod(value)
        return None

    predicates = {
        "molt_is_bound_method": inspect.ismethod,
        "molt_inspect_signature_data": signature_data,
    }
    intrinsic.require_intrinsic = lambda name: predicates.get(name, lambda *args: None)
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsic)
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib/inspect.py"
    spec = importlib.util.spec_from_file_location("molt_inspect_binding_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_managed_receiver_binds_before_forwarded_signature(molt_inspect):
    class Owner:
        def method(self):
            pass

    for api in (inspect, molt_inspect):
        parameter = api.Parameter
        Owner.method.__signature__ = api.Signature([
            parameter("receiver", parameter.POSITIONAL_ONLY),
            parameter("value", parameter.KEYWORD_ONLY, default=7),
        ])
        assert str(api.signature(Owner().method)) == "(*, value=7)"
        Owner.method.__signature__ = api.Signature([
            parameter("arguments", parameter.VAR_POSITIONAL),
        ])
        assert str(api.signature(Owner().method)) == "(*arguments)"
        for parameters in (
            [],
            [parameter("value", parameter.KEYWORD_ONLY)],
            [parameter("keywords", parameter.VAR_KEYWORD)],
        ):
            Owner.method.__signature__ = api.Signature(parameters)
            with pytest.raises(ValueError, match="invalid method signature"):
                api.signature(Owner().method)


def test_method_shaped_attributes_do_not_admit_a_method(molt_inspect):
    class Impostor:
        __func__ = lambda self: None
        __self__ = object()

    for api in (inspect, molt_inspect):
        with pytest.raises(TypeError):
            api.signature(Impostor())


def test_callable_signature_lookup_preserves_exception(molt_inspect):
    class FailingSignature:
        @property
        def __signature__(self):
            raise RuntimeError("original lookup failure")

        def __call__(self):
            pass

    class Callable:
        __call__ = FailingSignature()

    for api in (inspect, molt_inspect):
        with pytest.raises(RuntimeError, match="original lookup failure"):
            api.signature(Callable())
