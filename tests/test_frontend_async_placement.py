"""Async constructs compile exactly where CPython's compiler admits them.

The oracle is CPython's own ``compile()``: for each placement the frontend
must accept what CPython accepts and raise the same ``SyntaxError`` message
at the same line and column for what it rejects.
"""

from __future__ import annotations

import ast
from collections import Counter
import inspect
from types import CodeType
from typing import Any

import pytest

from molt.compiler_analysis.python_function_kind import FunctionKind
from molt.frontend import SimpleTIRGenerator, compile_to_tir
from molt.frontend._types import GEN_CONTROL_SIZE
from molt.frontend.sema.funcmeta import stateful_function_frame_plan

PLACEMENTS = {
    "await_at_module": "x = await y\n",
    "await_in_class_body": "class A:\n    z = await y\n",
    "await_in_plain_def": "def f():\n    return await y\n",
    "await_in_lambda": "f = lambda: await y\n",
    "await_in_sync_generator": "def f():\n    yield 1\n    await y\n",
    "await_in_def_nested_in_async": (
        "async def g():\n    def f():\n        return await y\n"
    ),
    "await_in_class_in_async": "async def g():\n    class A:\n        z = await y\n",
    "await_in_lambda_in_async": "async def g():\n    f = lambda: await y\n",
    "await_in_outer_genexp_iterable": "def f(y):\n    return (x for x in await y)\n",
    "async_for_at_module": "async for x in y:\n    pass\n",
    "async_for_in_plain_def": "def f():\n    async for x in y:\n        pass\n",
    "async_for_in_sync_generator": (
        "def f():\n    yield 1\n    async for x in y:\n        pass\n"
    ),
    "async_with_in_plain_def": "def f():\n    async with y:\n        pass\n",
    "async_listcomp_in_plain_def": "def f(y):\n    return [x async for x in y]\n",
    "await_in_async_def": "async def g(y):\n    return await y\n",
    "await_in_async_generator": "async def g(y):\n    yield await y\n",
    "await_in_async_method": (
        "class A:\n    async def m(self, y):\n        return await y\n"
    ),
    "async_for_in_async_def": (
        "async def g(y):\n    async for x in y:\n        pass\n"
    ),
    "await_genexp_in_plain_def": "def f(y):\n    return (await v for v in y)\n",
    "async_for_genexp_in_plain_def": "def f(y):\n    return (x async for x in y)\n",
    "await_genexp_at_module": "g = (await v for v in [])\n",
    "await_genexp_in_class_body": "class A:\n    g = (await v for v in [])\n",
    "await_listcomp_in_async_genexp": (
        "def f(y):\n    return ([await v for v in z] for z in y)\n"
    ),
}


def _outcome(compile_source, source: str) -> object:
    try:
        compile_source(source)
    except SyntaxError as exc:
        return (exc.msg, exc.lineno, exc.offset)
    return "accepted"


@pytest.mark.parametrize("source", PLACEMENTS.values(), ids=PLACEMENTS.keys())
def test_async_construct_placement_matches_cpython(source: str) -> None:
    expected = _outcome(lambda text: compile(text, "<case>", "exec"), source)

    assert _outcome(compile_to_tir, source) == expected


def _cpython_stateful_kinds(code: CodeType) -> Counter[FunctionKind]:
    kinds: Counter[FunctionKind] = Counter()
    if code.co_flags & inspect.CO_ASYNC_GENERATOR:
        kinds[FunctionKind.ASYNC_GENERATOR] += 1
    elif code.co_flags & inspect.CO_COROUTINE:
        kinds[FunctionKind.ASYNC] += 1
    elif code.co_flags & inspect.CO_GENERATOR:
        kinds[FunctionKind.GENERATOR] += 1
    for constant in code.co_consts:
        if isinstance(constant, CodeType):
            kinds.update(_cpython_stateful_kinds(constant))
    return kinds


@pytest.mark.parametrize(
    "expression",
    [
        "(await value if False else value for value in values)",
        "(value for value in await values)",
        "((await value for value in values) for unused in values)",
        "([value async for value in values] for unused in (0,))",
        "(value for value in [value async for value in values])",
        "((value for value in await values) for unused in (0,))",
        "(value async for value in values)",
    ],
    ids=[
        "dead-await-still-classifies-code",
        "outer-iterable-await-belongs-to-enclosing-code",
        "nested-generator-body-stays-deferred",
        "nested-materialized-async-comprehension-is-eager",
        "materialized-outer-iterable-runs-before-generator",
        "nested-generator-outer-iterable-runs-in-parent-generator",
        "async-for-classifies-own-generator",
    ],
)
def test_generator_expression_frame_kinds_match_cpython_code_flags(
    expression: str,
) -> None:
    source = f"async def probe(values):\n    return {expression}\n"
    expected = _cpython_stateful_kinds(compile(source, "<genexpr-kinds>", "exec"))
    generator = SimpleTIRGenerator(module_name="genexpr_kinds")
    generator.visit(ast.parse(source))
    observed = Counter(
        function["stateful_frame_plan"].kind
        for function in generator.funcs_map.values()
        if "stateful_frame_plan" in function
    )
    assert observed == expected


@pytest.mark.parametrize(
    "expression",
    [
        "lambda initial=await pause(): initial",
        "(value for value in await pause())",
        "(await pause() for unused in (0,))",
        "lambda: (await pause() for unused in (0,))",
        "[[value async for value in async_values] for unused in (0,)]",
        "[(value async for value in async_values) for unused in (0,)]",
    ],
    ids=[
        "lambda-default-await-is-eager",
        "generator-outer-iterable-await-is-eager",
        "generator-body-await-is-deferred",
        "lambda-body-is-deferred",
        "nested-materialized-async-comprehension-is-eager",
        "nested-async-generator-is-deferred",
    ],
)
def test_expression_suspension_matches_cpython_evaluation_and_is_cached(
    expression: str,
) -> None:
    marker = object()

    class Pause:
        def __await__(self):
            yield marker
            return ()

    class AsyncValues:
        def __aiter__(self):
            return self

        async def __anext__(self):
            await Pause()
            raise StopAsyncIteration

    namespace: dict[str, Any] = {"pause": Pause, "async_values": AsyncValues()}
    source = f"async def probe():\n    return {expression}\n"
    exec(compile(source, "<expression-suspension>", "exec"), namespace)
    coroutine = namespace["probe"]()
    try:
        try:
            yielded = coroutine.send(None)
        except StopIteration as done:
            suspended = False
            if inspect.isgenerator(done.value):
                done.value.close()
        else:
            assert yielded is marker
            suspended = True
    finally:
        coroutine.close()

    generator = SimpleTIRGenerator(module_name="expression_suspension")
    plan = stateful_function_frame_plan(
        kind=FunctionKind.ASYNC,
        poll_symbol="expression_suspension_poll",
        param_count=0,
        has_closure=False,
        gen_control_size=GEN_CONTROL_SIZE,
    )
    generator.start_function(
        plan.poll_symbol,
        params=["self"],
        compiler_params={"self"},
        stateful_frame_plan=plan,
    )
    node = ast.parse(expression, mode="eval").body
    assert generator._expr_may_yield(node) is suspended
    authority = generator._lexical_dependencies()
    visits = authority.node_visits
    assert generator._expr_may_yield(node) is suspended
    assert authority.node_visits == visits
