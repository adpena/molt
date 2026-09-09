"""The mixin protocol must preserve callable wrappers, not generator bodies."""

import ast
import contextlib
from contextlib import contextmanager as scope
from typing import AsyncIterator, Iterator

import pytest

from tools import gen_protocol


class WrappedMethods:
    @scope
    def scoped(self) -> Iterator[None]:
        yield

    @contextlib.asynccontextmanager
    async def async_scoped(self) -> AsyncIterator[None]:
        yield

    @staticmethod
    @scope
    def static_scoped() -> Iterator[None]:
        yield

    @classmethod
    @scope
    def class_scoped(cls) -> Iterator[None]:
        yield


@pytest.mark.parametrize(
    ("name", "decorators"),
    [
        ("scoped", ["contextmanager"]),
        ("async_scoped", ["asynccontextmanager"]),
        ("static_scoped", ["staticmethod", "contextmanager"]),
        ("class_scoped", ["classmethod", "contextmanager"]),
    ],
)
def test_wrapped_method_contract_and_imports(name, decorators):
    stub = gen_protocol._render_method_stub(name, vars(WrappedMethods)[name])
    assert stub is not None
    rendered = gen_protocol.render_protocol_file(
        [], [(name, stub)], types_module_exports=set()
    )
    tree = ast.parse(rendered)
    method = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    )
    assert [ast.unparse(node) for node in method.decorator_list] == decorators
    imports = {
        alias.name
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom)
        for alias in node.names
    }
    assert set(decorators) - {"staticmethod", "classmethod"} <= imports
    assert ("AsyncIterator" if name == "async_scoped" else "Iterator") in imports


def test_live_contextmanager_family_is_preserved():
    generator = gen_protocol._load_generator()
    methods = dict(
        gen_protocol._collect_methods(
            gen_protocol._surface_classes(generator), gen_protocol._builtin_names()
        )
    )
    assert "@contextmanager" in methods["_comprehension_scope"]
    assert "@contextmanager" in methods["_suppress_check_exception"]
