"""Protocol facts share one source snapshot and preserve the callable contract."""

import ast
import contextlib
import importlib
import inspect
from contextlib import contextmanager as scope
from pathlib import Path
from typing import AsyncIterator, Iterator

import pytest

from tools import gen_protocol, python_source_index
from tools.python_source_index import PythonSourceError, PythonSourceIndex
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


def _surface(
    klass: type,
    *,
    source_index: PythonSourceIndex | None = None,
    curated: dict[str, str] | None = None,
) -> gen_protocol.ProtocolSurface:
    return gen_protocol._collect_surface(
        gen_protocol._surface_classes(klass),
        gen_protocol._builtin_names(),
        curated={} if curated is None else curated,
        source_index=PythonSourceIndex() if source_index is None else source_index,
    )


@pytest.mark.parametrize("name", ["defaults", "async_defaults"])
def test_protocol_defaults_preserve_call_shape_without_runtime_dependencies(name):
    implementation = vars(DefaultMethods)[name]
    method = next(
        method for method in _surface(DefaultMethods).methods if method.name == name
    )
    stub = gen_protocol._render_method_stub(method)
    rendered = gen_protocol.render_protocol_file(
        [], (method,), types_module_exports=set()
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
    method = next(
        method for method in _surface(WrappedMethods).methods if method.name == name
    )
    rendered = gen_protocol.render_protocol_file(
        [], (method,), types_module_exports=set()
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
    methods = {method.name: method for method in _surface(generator).methods}
    assert "contextmanager" in methods["_comprehension_scope"].decorators
    assert "contextmanager" in methods["_suppress_check_exception"].decorators


def test_source_file_is_read_and_parsed_once_without_stub_reparsing(monkeypatch):
    class Base:
        def shared(self, value: int = 1) -> None:
            self.base: int = value

    class Derived(Base):
        # A repeated MRO binding still has just one method-body analysis.
        shared = Base.shared

        def own(self, value: str = "value") -> None:
            self.derived: str = value

    reads: list[Path] = []
    parses: list[str] = []
    bodies: list[ast.FunctionDef | ast.AsyncFunctionDef] = []
    original_open = python_source_index.tokenize.open
    original_parse = ast.parse
    original_body = gen_protocol._method_body

    def counted_open(path):
        reads.append(Path(path))
        return original_open(path)

    def counted_parse(source, filename="<unknown>", mode="exec", **kwargs):
        if mode == "exec":
            parses.append(filename)
        return original_parse(source, filename=filename, mode=mode, **kwargs)

    def counted_body(node):
        bodies.append(node)
        return original_body(node)

    monkeypatch.setattr(python_source_index.tokenize, "open", counted_open)
    monkeypatch.setattr(ast, "parse", counted_parse)
    monkeypatch.setattr(gen_protocol, "_method_body", counted_body)

    index = PythonSourceIndex()
    surface = _surface(Derived, source_index=index)
    assert {method.name for method in surface.methods} == {"own", "shared"}
    assert dict(surface.attrs) == {"base": "int", "derived": "str"}
    assert index.function(Base.shared) is index.function(Derived.shared)
    for _ in range(2):
        gen_protocol.render_protocol_file(
            [], surface.methods, types_module_exports=set()
        )
    assert reads == [Path(__file__).resolve()]
    assert parses == [str(Path(__file__).resolve())]
    assert len(bodies) == len({id(node) for node in bodies}) == 2

    _surface(Derived, source_index=PythonSourceIndex())
    assert reads == [Path(__file__).resolve()] * 2
    assert parses == [str(Path(__file__).resolve())] * 2
    assert len(bodies) == 4


@pytest.mark.parametrize("name", ["defaults", "async_defaults"])
def test_collecting_and_rendering_never_mutate_the_source_ast(name):
    index = PythonSourceIndex()
    source = index.function(vars(DefaultMethods)[name])
    assert source is not None
    before = ast.dump(source.node, include_attributes=True)
    method = next(
        method
        for method in _surface(DefaultMethods, source_index=index).methods
        if method.name == name
    )
    assert method.node is source.node
    first = gen_protocol._render_method_stub(method)
    assert gen_protocol._render_method_stub(method) == first
    rendered = gen_protocol.render_protocol_file(
        [], (method,), types_module_exports=set()
    )
    assert (
        gen_protocol.render_protocol_file([], (method,), types_module_exports=set())
        == rendered
    )
    assert ast.dump(source.node, include_attributes=True) == before


def test_source_identity_distinguishes_same_named_decorated_methods():
    class First:
        @scope
        def shared(self) -> Iterator[str]:
            yield "first"

    class Second:
        @contextlib.contextmanager
        def shared(self) -> Iterator[str]:
            yield "second"

    index = PythonSourceIndex()
    first = index.function(First.shared)
    second = index.function(Second.shared)
    assert first is not None and second is not None
    assert first.path == second.path
    assert first.node is not second.node
    assert first.function is inspect.unwrap(First.shared)
    assert second.function is inspect.unwrap(Second.shared)
    assert first.node.name == second.node.name == "shared"
    assert first.node.decorator_list[0].lineno == first.function.__code__.co_firstlineno
    assert (
        second.node.decorator_list[0].lineno == second.function.__code__.co_firstlineno
    )
    for source, expected in ((first, "first"), (second, "second")):
        statement = source.node.body[0]
        assert isinstance(statement, ast.Expr)
        assert isinstance(statement.value, ast.Yield)
        assert isinstance(statement.value.value, ast.Constant)
        assert statement.value.value.value == expected
    assert index.function(First().shared) is first


@pytest.mark.parametrize("name", ["static_scoped", "class_scoped"])
def test_source_identity_unifies_descriptors_and_bound_wrappers(name):
    index = PythonSourceIndex()
    descriptor = vars(WrappedMethods)[name]
    source = index.function(descriptor)
    assert source is not None
    assert source is index.function(getattr(WrappedMethods, name))
    assert source.function is inspect.unwrap(descriptor.__func__)
    assert source.node.name == name


def _source_namespace(path: Path, text: str) -> dict[str, object]:
    path.write_text(text, encoding="utf-8")
    namespace: dict[str, object] = {"__name__": "protocol_source_fixture"}
    exec(compile(text, str(path), "exec"), namespace)
    return namespace


def test_source_snapshot_lifetime_is_one_index(tmp_path):
    path = tmp_path / "methods.py"
    first = _source_namespace(path, "def method(self):\n    self.before = 1\n")[
        "method"
    ]
    index = PythonSourceIndex()
    snapshot = index.function(first)
    assert snapshot is not None
    before = ast.dump(snapshot.node, include_attributes=True)

    second = _source_namespace(path, "def method(self):\n    self.after = 2\n")[
        "method"
    ]
    assert index.function(first) is snapshot
    same_snapshot = index.function(second)
    assert same_snapshot is not None and same_snapshot.node is snapshot.node
    assert ast.dump(snapshot.node, include_attributes=True) == before

    refreshed = PythonSourceIndex().function(second)
    assert refreshed is not None and refreshed.node is not snapshot.node
    assert gen_protocol._method_body(snapshot.node).attrs == {"before"}
    assert gen_protocol._method_body(refreshed.node).attrs == {"after"}


@pytest.mark.parametrize(
    "failure", ["missing_file", "invalid_source", "mismatched_name", "mismatched_line"]
)
def test_source_failure_cannot_emit_a_guessed_protocol(tmp_path, failure):
    path = tmp_path / "methods.py"
    original = "def method(self, required: int) -> None:\n    self.value = required\n"
    function = _source_namespace(path, original)["method"]
    if failure == "missing_file":
        path.unlink()
    elif failure == "invalid_source":
        path.write_text("def method(\n", encoding="utf-8")
    elif failure == "mismatched_name":
        path.write_text(original.replace("def method", "def renamed"), encoding="utf-8")
    else:
        path.write_text("\n" + original, encoding="utf-8")
    with pytest.raises(PythonSourceError):
        PythonSourceIndex().function(function)
    surface_class = type("Unavailable", (), {"method": function})
    with pytest.raises(
        gen_protocol.ProtocolGenError, match="real method/attribute surface"
    ):
        _surface(surface_class)


def test_generator_formats_its_excluded_output_path():
    assert (
        gen_protocol._format_generated_text(gen_protocol.OUT_PROTOCOL, "value=1\n")
        == "value = 1\n"
    )


def test_attribute_union_preserves_mro_precedence_and_root_scope():
    class Base:
        class_choice: int
        base_annotation_only: bytes

        def update(self, value: int = 1) -> int:
            self.base_only = value
            self.store_choice: int = value
            self.class_choice: float = value
            return value

    class Derived(Base):
        class_choice: str

        def update(self, value: str = "new") -> str:
            self.derived_only = value
            self.store_choice: str = value
            self.curated_only = value
            self.unknown = value
            return value

        def rooted(self, /) -> None:
            self.posonly: bytes = b"value"
            self.annotation_only: list[int]
            self.callback = lambda self: [None for self.nested_lambda in ()]

            def helper(self):
                self.nested_function: int = 1

            async def async_helper(self):
                self.nested_async: int = 1

            class Helper:
                def __init__(self):
                    self.nested_class: int = 1

    surface = _surface(
        Derived,
        curated={
            "class_choice": "complex",
            "store_choice": "float",
            "curated_only": "object",
            "not_on_surface": "bool",
        },
    )
    assert dict(surface.attrs) == {
        "annotation_only": "list[int]",
        "base_annotation_only": "bytes",
        "base_only": "Any",
        "callback": "Any",
        "class_choice": "str",
        "curated_only": "object",
        "derived_only": "Any",
        "posonly": "bytes",
        "store_choice": "str",
        "unknown": "Any",
    }
    method = next(method for method in surface.methods if method.name == "update")
    assert method.annotation_texts == ("str", "str")
    assert "value: str=..." in gen_protocol._render_method_stub(method)
