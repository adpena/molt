"""Class definitions share source-point creation and explicit lexical owners."""

import ast

import pytest

from molt.compiler_analysis.python_lexical_scope import PythonDependencyAuthority
from molt.frontend import SimpleTIRGenerator
from tools.check_ir_structure import verify_frontend_tir


def _compile(source, target=(3, 12)):
    generator = SimpleTIRGenerator(target_python=target)
    generator.visit(ast.parse(source))
    ir = generator.to_json()
    result = verify_frontend_tir(ir)
    assert result.ok, result.errors
    return generator, ir


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "kind", ["method", "generator", "async", "async_generator", "lambda", "genexpr"]
)
def test_class_owned_cell_reaches_every_function_like_region(target, kind):
    bodies = {
        "method": "if flag:\n    def method(self): return __class__",
        "generator": "for item in (0, 1):\n    def method(self): yield __class__",
        "async": "if flag:\n    async def method(self): return __class__",
        "async_generator": "if flag:\n    async def method(self): yield __class__",
        "lambda": "method = lambda self: __class__",
        "genexpr": "values = (__class__ for item in (0,))",
    }
    source = "flag = True\nclass Owner:\n" + "".join(
        "    " + line + "\n" for line in bodies[kind].splitlines()
    )
    _generator, ir = _compile(source, target)
    assert any(
        op.get("s_value") == "__classcell__"
        for function in ir["functions"]
        for op in function["ops"]
    )


def test_repeated_method_definitions_create_distinct_functions_in_source_order():
    generator, ir = _compile(
        "class Owner:\n"
        "    def method(self): return 1\n"
        "    first = method\n"
        "    def method(self): return 2\n"
        "    second = method\n"
    )
    methods = [fn for fn in ir["functions"] if "Owner_method" in fn["name"]]
    assert len(methods) == 2
    assert len({fn["name"] for fn in methods}) == 2
    attrs = generator.classes["Owner"]["methods"]
    assert attrs["method"]["func"].type_hint.endswith(methods[-1]["name"])


def test_method_decorator_expression_precedes_default_evaluation():
    _generator, ir = _compile(
        "def mark(label): return lambda value: value\n"
        "class Owner:\n"
        "    @mark('decorator-evaluated')\n"
        "    def method(self, value=mark('default-evaluated')): return value\n"
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    labels = [op.get("s_value") for op in main if op["kind"] == "const_str"]
    assert labels.index("decorator-evaluated") < labels.index("default-evaluated")


@pytest.mark.parametrize("mutation", ["method = 7", "del method"])
def test_method_rebinding_retires_compiler_method_identity(mutation):
    generator, _ir = _compile(
        "class Owner:\n    def method(self): return 1\n    " + mutation + "\n"
    )
    assert "method" not in generator.classes["Owner"]["methods"]


@pytest.mark.parametrize(
    ("source", "required"),
    [
        ("class C:\n    f = lambda self: __class__\n", True),
        ("class C:\n    def f(self, super): return super\n", True),
        (
            "class C:\n    def f(self):\n        global __class__\n        return super()\n",
            False,
        ),
        ("class C:\n    def f(self, __class__): return super()\n", False),
        (
            "class C:\n    def f(self):\n        class Inner:\n            def f(self): return __class__\n        return Inner\n",
            False,
        ),
        (
            "class C:\n    def f(self):\n        class Inner:\n            def f(self, owner=__class__): return __class__, owner\n        return Inner\n",
            True,
        ),
    ],
)
def test_class_cell_requirement_is_a_lexical_authority_fact(source, required):
    owner = ast.parse(source).body[0]
    authority = PythonDependencyAuthority(
        eager_annotations=True, future_annotations=False
    )
    assert authority.summary(owner).class_cell_required is required


def test_nested_class_separates_default_outer_cell_from_method_inner_cell():
    _compile(
        "class Outer:\n"
        "    def factory(self):\n"
        "        class Inner:\n"
        "            def method(self, owner=__class__): return __class__, owner\n"
        "        return Inner\n"
    )


@pytest.mark.parametrize(
    ("expression", "captured"),
    [
        ("[x for y in values]", set()),
        ("{x for y in values}", set()),
        ("{y: x for y in values}", set()),
        ("[lambda: x for y in values]", {"x"}),
        ("[lambda: x for x in values]", set()),
        ("[x for x in (lambda: x)()]", {"x"}),
        ("[[lambda: x for x in values] for y in values]", set()),
        ("[[lambda: x for y in values] for z in values]", {"x"}),
        ("(x for y in values)", {"x"}),
        ("(x for x in values)", set()),
    ],
)
def test_only_real_code_object_dependencies_require_enclosing_cells(
    expression, captured
):
    generator = SimpleTIRGenerator(target_python=(3, 12))
    body = ast.parse(expression).body
    assert generator._collect_scope_cell_vars(body, {"x"}) == captured


@pytest.mark.parametrize(
    ("expression", "captured"),
    [
        ("[[x for y in values] for x in values]", []),
        ("[[lambda: x for y in values] for x in values]", ["x"]),
        ("[[lambda: x for x in values] for x in values]", []),
        ("[[x for x in (lambda: x)()] for x in values]", ["x"]),
        ("[(x for y in values) for x in values]", ["x"]),
        ("[(x for x in values) for x in values]", []),
        ("[lambda x=x: x for x in values]", []),
        ("[lambda: x for x in values]", ["x"]),
    ],
)
def test_comprehension_cells_exclude_nested_eager_reads_and_shadowed_captures(
    expression, captured
):
    generator = SimpleTIRGenerator(target_python=(3, 12))
    node = ast.parse(expression, mode="eval").body
    assert generator._collect_comprehension_cell_vars(node) == captured
