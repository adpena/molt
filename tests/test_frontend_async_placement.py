"""Async constructs compile exactly where CPython's compiler admits them.

The oracle is CPython's own ``compile()``: for each placement the frontend
must accept what CPython accepts and raise the same ``SyntaxError`` message
at the same line and column for what it rejects.
"""

from __future__ import annotations

import pytest

from molt.frontend import compile_to_tir

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
