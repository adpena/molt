from __future__ import annotations

import types
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import Event, Lock

import pytest


@pytest.fixture
def molt_typing():
    from tools.profile_typing_lazy_once import typing_source_modules

    with typing_source_modules(Path(__file__).parents[1]) as (module, _collections):
        yield module


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


def test_applied_type_alias_parameters_are_residual_not_declared(molt_typing) -> None:
    parameter = molt_typing.TypeVar("T")
    alias = molt_typing._molt_type_alias(
        "Pair",
        lambda _format: {"__value__": tuple},
        (parameter,),
    )

    assert alias.__type_params__ == (parameter,)
    assert alias[int].__parameters__ == ()
    assert alias[parameter].__parameters__ == (parameter,)


@pytest.mark.parametrize("shift", [0, 4, 12, 20, 32, 40, 48, 56])
def test_lazy_type_lock_mix_distributes_address_and_handle_families(
    molt_typing, shift: int
) -> None:
    indexes = {
        molt_typing._lazy_type_lock_index(identity << shift) for identity in range(4096)
    }
    assert indexes == set(range(molt_typing._LAZY_TYPE_LOCK_MASK + 1))


@pytest.mark.parametrize("factory_name", ["TypeVar", "ParamSpec", "TypeVarTuple"])
def test_type_parameter_producer_supplies_real_generic_alias_protocol(
    molt_typing, factory_name
) -> None:
    # The host's real GenericAlias is an independent consumer of Molt's Python
    # parameter producer. It cannot pass through a Rust class-name heuristic.
    parameter = getattr(molt_typing, factory_name)("T")
    parameter_class = type(parameter)
    original_name = parameter_class.__name__
    original_qualname = parameter_class.__qualname__
    parameter_class.__name__ = parameter_class.__qualname__ = "RenamedParameter"
    try:
        alias = types.GenericAlias(tuple, (parameter, parameter))
        assert len(alias.__parameters__) == 1
        assert alias.__parameters__[0] is parameter
        if factory_name == "ParamSpec":
            assert molt_typing._collections_abc_module._is_param_expr(parameter)
    finally:
        # Special type attributes need restoration through their descriptors;
        # they need not be present in the class dictionary on every CPython.
        parameter_class.__name__ = original_name
        parameter_class.__qualname__ = original_qualname


@pytest.mark.parametrize(
    ("factory_name", "arguments"),
    [("TypeVar", (int, None)), ("ParamSpec", ([int, str], Ellipsis))],
)
def test_type_parameter_substitution_protocol_has_real_behavior(
    molt_typing, factory_name, arguments
) -> None:
    import typing as host_typing

    expected_parameter = getattr(host_typing, factory_name)("T")
    actual_parameter = getattr(molt_typing, factory_name)("T")
    for argument in arguments:
        expected = expected_parameter.__typing_subst__(argument)
        actual = actual_parameter.__typing_subst__(argument)
        assert actual == expected


def test_typevartuple_substitution_protocol_preserves_bare_rejection(molt_typing):
    import typing as host_typing

    with pytest.raises(TypeError) as expected:
        host_typing.TypeVarTuple("Ts").__typing_subst__(int)
    with pytest.raises(TypeError) as actual:
        molt_typing.TypeVarTuple("Ts").__typing_subst__(int)
    assert str(actual.value) == str(expected.value)


def test_callable_and_paramspec_share_actual_selected_type_identity(molt_typing):
    import _collections_abc as host_collections
    import typing as host_typing

    selected_collections = molt_typing._collections_abc_module
    assert selected_collections is not host_collections
    assert molt_typing is not host_typing
    actual_p = molt_typing.ParamSpec("P")
    actual_q = molt_typing.ParamSpec("Q")
    actual_concat = molt_typing.Concatenate[int, actual_q]
    expected_p = host_typing.ParamSpec("P")
    expected_q = host_typing.ParamSpec("Q")
    expected_concat = host_typing.Concatenate[int, expected_q]
    for actual_arg, expected_arg in (
        (actual_q, expected_q),
        (actual_concat, expected_concat),
    ):
        assert actual_p.__typing_subst__(actual_arg) is actual_arg
        assert expected_p.__typing_subst__(expected_arg) is expected_arg
        actual = selected_collections.Callable[actual_arg, str]
        expected = host_collections.Callable[expected_arg, str]
        assert actual.__args__[0] is actual_arg
        assert expected.__args__[0] is expected_arg
        assert len(actual.__parameters__) == len(expected.__parameters__)
    renamed = type("ParamSpec", (), {"__module__": "typing"})()
    assert not selected_collections._is_param_expr(renamed)
    assert not selected_collections._is_param_expr(host_typing.ParamSpec("Foreign"))


def test_typing_collection_uses_python_equality_and_iterable_metadata(molt_typing):
    import typing as host_typing

    host_collect = getattr(host_typing, "_collect_type_parameters", None)
    if host_collect is None:
        host_collect = host_typing._collect_parameters

    class EqualParameter:
        def __eq__(self, other):
            return isinstance(other, EqualParameter)

    first, second = EqualParameter(), EqualParameter()

    class Provider:
        __parameters__ = [first, second]

    actual = molt_typing._collect_type_parameters((Provider(),))
    expected = host_collect((Provider(),))
    assert len(actual) == len(expected) == 1
    assert actual[0] is expected[0] is first


def test_typing_residual_collection_preserves_default_laziness(molt_typing):
    if not molt_typing._SUPPORTS_TYPEVAR_DEFAULTS:
        assert not hasattr(molt_typing.TypeVar("T"), "has_default")
        return
    import typing as host_typing

    calls = []
    defaulted = molt_typing._TypeVar("D", False, False, None, (), pep695=True)
    molt_typing._molt_type_param_set_evaluators(
        defaulted,
        None,
        None,
        lambda _format: calls.append("default") or {"__default__": int},
    )
    required = molt_typing.TypeVar("R")
    host_defaulted = host_typing.TypeVar("D", default=int)
    host_required = host_typing.TypeVar("R")
    assert molt_typing.Tuple[defaulted, required].__parameters__ == (
        defaulted,
        required,
    )
    assert host_typing.Tuple[host_defaulted, host_required].__parameters__ == (
        host_defaulted,
        host_required,
    )
    with pytest.raises(TypeError) as actual:
        molt_typing.Generic[defaulted, required]
    with pytest.raises(TypeError) as expected:
        host_typing.Generic[host_defaulted, host_required]
    assert str(actual.value) == str(expected.value)
    assert calls == []


def test_typing_tuple_collection_has_independent_default_order_boundaries(molt_typing):
    import typing as host_typing

    if not molt_typing._SUPPORTS_TYPEVAR_DEFAULTS:
        assert molt_typing._collect_type_parameters(((molt_typing.TypeVar("T"),),))
        return
    actual_d = molt_typing.TypeVar("D", default=int)
    actual_r = molt_typing.TypeVar("R")
    host_d = host_typing.TypeVar("D", default=int)
    host_r = host_typing.TypeVar("R")
    actual = molt_typing._collect_type_parameters(((actual_d, actual_r),))
    expected = host_typing._collect_type_parameters(((host_d, host_r),))
    assert actual == (actual_d, actual_r)
    assert expected == (host_d, host_r)

    class Unpacked:
        __typing_is_unpacked_typevartuple__ = True
        __parameters__ = ()

    with pytest.raises(TypeError) as actual_error:
        molt_typing._collect_type_parameters((Unpacked(), actual_d))
    with pytest.raises(TypeError) as expected_error:
        host_typing._collect_type_parameters((Unpacked(), host_d))
    assert str(actual_error.value) == str(expected_error.value)
