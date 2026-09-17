"""The mixin protocol must preserve callable wrappers, not generator bodies."""

import ast
import contextlib
import importlib
import inspect
from contextlib import contextmanager as scope
from typing import AsyncIterator, Iterator

import pytest

from tools import gen_protocol
from molt.frontend.sema import FunctionKind


class DefaultMethods:
    def defaults(
        self,
        required: int,
        positional: object = object(),
        /,
        kind: FunctionKind = FunctionKind.SYNC,
        *,
        required_keyword: int,
        optional_keyword: object = object(),
    ) -> None:
        pass

    async def async_defaults(self, kind: FunctionKind = FunctionKind.SYNC) -> None:
        pass


@pytest.mark.parametrize("name", ["defaults", "async_defaults"])
def test_protocol_defaults_preserve_call_shape_without_runtime_dependencies(name):
    implementation = vars(DefaultMethods)[name]
    stub = gen_protocol._render_method_stub(name, implementation)
    assert stub is not None
    rendered = gen_protocol.render_protocol_file(
        [], [(name, stub)], types_module_exports=set()
    )
    namespace = {}
    exec(compile(rendered, "<generated-protocol>", "exec"), namespace)
    assert "FunctionKind" not in namespace
    projected = inspect.signature(getattr(namespace["_GeneratorProtocol"], name))
    original = inspect.signature(implementation)
    assert projected.parameters.keys() == original.parameters.keys()
    for key, actual in projected.parameters.items():
        expected = original.parameters[key]
        assert actual.kind == expected.kind
        assert actual.default is (
            inspect.Parameter.empty
            if expected.default is inspect.Parameter.empty
            else Ellipsis
        )
    assert ("async def" in stub) == inspect.iscoroutinefunction(implementation)


def test_live_generated_protocol_imports():
    module = importlib.import_module("molt.frontend._protocol")
    assert module._GeneratorProtocol.start_function


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
