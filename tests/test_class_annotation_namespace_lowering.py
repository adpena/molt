"""Every class evaluator captures namespace custody instead of a class spelling."""

import ast

import pytest

from molt.frontend import SimpleTIRGenerator
from molt.compat import CompatibilityError
from molt.compiler_analysis.python_lexical_scope import class_annotation_syntax_error
from tools.check_ir_structure import verify_frontend_tir


def _compile(source, target=(3, 14)):
    generator = SimpleTIRGenerator(target_python=target)
    generator.visit(ast.parse(source))
    result = generator.to_json()
    verification = verify_frontend_tir(result)
    assert verification.ok, verification.errors
    return result


@pytest.mark.parametrize("header", ["class Subject:", "class Subject(metaclass=Meta):"])
@pytest.mark.parametrize(
    "body",
    [
        "    type Alias = injected\n",
        "    type Alias[T: injected] = T\n",
        "    type Alias[T: (injected, int)] = T\n",
        "    observed: injected\n",
        "    def method(value: injected): pass\n",
        "    def method(value: injected): yield value\n",
        "    async def method(value: injected): return value\n",
        "    async def method(value: injected): yield value\n",
    ],
)
def test_all_class_evaluators_reload_explicit_namespace_capture(header, body):
    ir = _compile(header + "\n" + body)
    evaluators = [fn for fn in ir["functions"] if "__annotate__" in fn["name"]]
    assert evaluators
    probes = []
    for function in evaluators:
        ops = function["ops"]
        strings = {
            op["out"]: op.get("s_value") for op in ops if op["kind"] == "const_str"
        }
        definitions = {op["out"]: op for op in ops if "out" in op}
        for op in ops:
            if op.get("s_value") != "molt_namespace_get":
                continue
            if strings.get(op["args"][1]) != "injected":
                continue
            namespace = definitions[op["args"][0]]
            assert namespace["kind"] == "call"
            assert namespace["s_value"] == "molt_cell_get"
            assert len(namespace["args"]) == 1
            cell = definitions[namespace["args"][0]]
            assert cell["kind"] == "index"
            assert cell["args"][0] == "__molt_closure__"
            probes.append(op)
        assert not any(
            op.get("kind") == "module_get_global"
            and strings.get(op["args"][1]) == "Subject"
            for op in ops
        )
    assert probes
    assert any(
        op.get("s_value") == "__classdictcell__"
        for function in ir["functions"]
        for op in function["ops"]
    )


@pytest.mark.parametrize("target", [(3, 12), (3, 13)])
def test_eager_method_annotation_is_emitted_in_class_body_namespace(target):
    ir = _compile(
        "class Subject(metaclass=Meta):\n    def method(value: injected): pass\n",
        target,
    )
    main = next(fn for fn in ir["functions"] if fn["name"] == "molt_main")
    assert any(op.get("s_value") == "molt_namespace_get" for op in main["ops"])
    assert not any("__annotate__" in fn["name"] for fn in ir["functions"])


def test_alias_lambda_transports_lexical_capture_across_annotation_frame():
    ir = _compile(
        "def outer():\n"
        "    value = 'outer'\n"
        "    class Subject:\n"
        "        value = 'class'\n"
        "        type Alias = lambda: value\n"
        "    return Subject\n"
    )
    evaluators = [fn for fn in ir["functions"] if "__annotate__" in fn["name"]]
    assert evaluators
    assert all("__molt_closure__" in fn["params"] for fn in evaluators)
    assert not any(
        op.get("s_value") == "molt_namespace_get"
        for fn in evaluators
        for op in fn["ops"]
    )
    lambdas = [fn for fn in ir["functions"] if "lambda" in fn["name"]]
    assert lambdas and all("__molt_closure__" in fn["params"] for fn in lambdas)


def test_conditional_annotation_items_reload_shared_lexical_cells():
    ir = _compile(
        "def outer(flag):\n"
        "    value = 'outer'\n"
        "    class Subject:\n"
        "        if flag:\n"
        "            first: lambda: value\n"
        "        else:\n"
        "            second: lambda: value\n"
        "    return Subject\n"
    )
    outer = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____outer")
    definitions = {op["out"]: op for op in outer if "out" in op}
    constructor = next(
        op
        for op in outer
        if op.get("kind") == "func_new_closure"
        and "__annotate__" in op.get("s_value", "")
    )
    captures = definitions[constructor["args"][0]]["args"]
    evaluator = next(
        fn for fn in ir["functions"] if fn["name"] == constructor["s_value"]
    )
    evaluator_ops = evaluator["ops"]
    evaluator_defs = {op["out"]: op for op in evaluator_ops if "out" in op}
    reads = [op for op in evaluator_ops if op["kind"] == "dict_get"]
    assert len(reads) == 2
    assert len({read["args"][0] for read in reads}) == 1
    loaded_map = evaluator_defs[reads[0]["args"][0]]
    assert loaded_map["kind"] == "call"
    assert loaded_map["s_value"] == "molt_cell_get"
    (loaded_cell_name,) = loaded_map["args"]

    def enclosing_cell(cell_name):
        extraction = evaluator_defs[cell_name]
        assert extraction["kind"] == "index"
        assert extraction["args"][0] == "__molt_closure__"
        slot = evaluator_defs[extraction["args"][1]]
        assert slot["kind"] == "const"
        return captures[slot["value"]]

    execution_cell = definitions[enclosing_cell(loaded_cell_name)]
    assert execution_cell["kind"] == "call"
    assert execution_cell["s_value"] == "molt_cell_new"
    (execution_map,) = execution_cell["args"]
    assert definitions[execution_map]["kind"] == "dict_new"
    assert outer.index(definitions[execution_map]) < next(
        index for index, op in enumerate(outer) if op.get("kind") == "if"
    )
    marks = [
        op
        for op in outer
        if op.get("kind") == "store_index" and op["args"][0] == execution_map
    ]
    assert len(marks) == 2
    assert (
        {definitions[mark["args"][1]]["value"] for mark in marks}
        == {evaluator_defs[read["args"][1]]["value"] for read in reads}
        == {0, 1}
    )
    for mark in marks:
        flag = definitions[mark["args"][2]]
        assert flag["kind"] == "const_bool"
        assert type(flag["value"]) is int and flag["value"] == 1

    # Both conditional lambda bodies receive the same enclosing value cell;
    # neither captures a snapshot nor a cell manufactured in a sibling branch.
    lambdas = [
        op
        for op in evaluator_ops
        if op["kind"] == "func_new_closure" and "lambda" in op["s_value"]
    ]
    assert len(lambdas) == 2
    lexical_cells = []
    for lambda_constructor in lambdas:
        closure = evaluator_defs[lambda_constructor["args"][0]]
        assert closure["kind"] == "tuple_new"
        (cell_name,) = closure["args"]
        lexical_cells.append(enclosing_cell(cell_name))
    assert lexical_cells[0] == lexical_cells[1]
    value_cell = definitions[lexical_cells[0]]
    assert value_cell["kind"] == "call" and value_cell["s_value"] == "molt_cell_new"

    def initial_value(value):
        while definitions[value]["kind"] in {"binding_alias", "identity_alias"}:
            value = definitions[value]["args"][0]
        return definitions[value]

    cell_values = [*value_cell["args"]]
    cell_values.extend(
        op["args"][1]
        for op in outer
        if op.get("s_value") == "molt_cell_set" and op["args"][0] == lexical_cells[0]
    )
    assert any(initial_value(value).get("s_value") == "outer" for value in cell_values)
    assert not any(
        str(op.get("s_value", "")).startswith("__molt_annotations_exec_")
        for op in outer
    )


def test_sibling_conditional_lambdas_reload_enclosing_closure_cell():
    _compile(
        "def outer():\n"
        "    value = 'outer'\n"
        "    def middle(flag):\n"
        "        if flag:\n"
        "            callback = lambda: value\n"
        "        else:\n"
        "            callback = lambda: value\n"
        "        return callback\n"
        "    return middle\n"
    )


def test_alias_comprehension_only_probes_namespace_for_outermost_iterable():
    ir = _compile(
        "value = 'global'\n"
        "class Subject:\n"
        "    value = 'class'\n"
        "    items = (0,)\n"
        "    type Alias = [value for item in items]\n"
    )
    names = []
    for function in ir["functions"]:
        if "__annotate__" not in function["name"]:
            continue
        strings = {
            op["out"]: op.get("s_value")
            for op in function["ops"]
            if op["kind"] == "const_str"
        }
        names.extend(
            strings[op["args"][1]]
            for op in function["ops"]
            if op.get("s_value") == "molt_namespace_get"
        )
    assert "items" in names
    assert "value" not in names


@pytest.mark.parametrize("future", [False, True])
def test_class_annotations_mapping_exists_before_first_body_statement(future):
    prefix = "from __future__ import annotations\n" if future else ""
    ir = _compile(
        prefix
        + "class Subject:\n    before = __annotations__\n    value: int = 1\n    after = __annotations__\n",
        (3, 12),
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    strings = {op["out"]: op.get("s_value") for op in main if op["kind"] == "const_str"}
    class_op = next(op for op in main if op["kind"] == "class_def")
    args = class_op["args"]
    attrs = {
        strings[arg]: args[index + 1]
        for index, arg in enumerate(args[:-1])
        if strings.get(arg) in {"before", "after", "__annotations__"}
    }
    assert attrs["before"] == attrs["after"] == attrs["__annotations__"]
    assert any(
        op["kind"] == "store_index" and op["args"][0] == attrs["before"] for op in main
    )


def test_class_annotation_value_store_precedes_annotation_callback_and_mapping_store():
    ir = _compile(
        "class Subject(metaclass=Meta):\n    value: print('annotation') = print('rhs')\n",
        (3, 12),
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    strings = {op["out"]: op.get("s_value") for op in main if op["kind"] == "const_str"}
    rhs = next(index for index, op in enumerate(main) if op.get("s_value") == "rhs")
    annotation = next(
        index for index, op in enumerate(main) if op.get("s_value") == "annotation"
    )
    stores = [
        index
        for index, op in enumerate(main)
        if op["kind"] == "store_index" and strings.get(op["args"][1]) == "value"
    ]
    assert len(stores) == 2
    assert rhs < stores[0] < annotation < stores[1]


def test_nested_class_annotations_do_not_publish_module_bindings():
    ir = _compile(
        "class Subject:\n    for index in (0,):\n        value: int = index\n", (3, 12)
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    strings = {op["out"]: op.get("s_value") for op in main if op["kind"] == "const_str"}
    assert not any(
        op["kind"] == "module_set_attr"
        and strings.get(op["args"][1]) in {"value", "__annotations__"}
        for op in main
    )


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    ("source", "forbidden"),
    [
        ("class C:\n    type Alias = lambda: x\n", True),
        ("class C:\n    type Alias = [x for x in ()]\n", True),
        ("class C:\n    type Alias = (x for x in ())\n", True),
        ("class C:\n    def method[T](x: (lambda: T)): pass\n", True),
        ("class C:\n    def method(x: (lambda: x)): pass\n", False),
        ("class C:\n    x: (lambda: x)\n", False),
        ("class C:\n    type Alias[T: (lambda: x)] = T\n", True),
        ("class C:\n    def method[T: (lambda: x)](): pass\n", True),
        ("class Outer:\n    class Inner[T]((lambda: object)()): pass\n", True),
        ("class C:\n    def method(self):\n        type Alias = lambda: x\n", False),
        (
            "from __future__ import annotations\nclass C:\n    def method[T](x: (lambda: T)): pass\n",
            False,
        ),
        ("if False:\n    class C:\n        type Alias = lambda: x\n", True),
    ],
)
def test_class_annotation_syntax_uses_target_and_lexical_region(
    source, forbidden, target
):
    error = class_annotation_syntax_error(
        ast.parse(source),
        target_python=target,
        future_annotations=source.startswith("from __future__"),
    )
    rejected = forbidden and target < (3, 13)
    assert (error is not None) is rejected
    if rejected:
        with pytest.raises(
            CompatibilityError, match="annotation scope within class scope"
        ):
            _compile(source, target)


def test_dead_annotation_still_sets_up_class_mapping():
    ir = _compile(
        "class Subject:\n    before = __annotations__\n    if False:\n        value: int\n",
        (3, 12),
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    strings = {op["out"]: op.get("s_value") for op in main if op["kind"] == "const_str"}
    assert any(
        op["kind"] == "store_index" and strings.get(op["args"][1]) == "__annotations__"
        for op in main
    )


@pytest.mark.parametrize("header", ["class Subject:", "class Subject(metaclass=type):"])
def test_class_constructor_owns_cell_fill_and_descriptor_callbacks(header):
    ir = _compile(
        header + "\n    def defining_class(self):\n        return __class__\n"
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    strings = {op["out"]: op.get("s_value") for op in main if "out" in op}
    cells = set()
    for op in main:
        args = op.get("args", [])
        if op["kind"] == "store_index" and strings.get(args[1]) == "__classcell__":
            cells.add(args[2])
        if op["kind"] == "class_def":
            for index, value in enumerate(args[:-1]):
                if strings.get(value) == "__classcell__":
                    cells.add(args[index + 1])
    assert cells, "every construction path must publish the original method cell"
    definitions = {op["out"]: op for op in main if "out" in op}
    assert all(
        definitions[cell]["kind"] == "call"
        and definitions[cell]["s_value"] == "molt_cell_new"
        for cell in cells
    )
    assert not any(
        (op["kind"] == "store_index" or op.get("s_value") == "molt_cell_set")
        and op["args"][0] in cells
        for op in main
    ), "only the runtime constructor may fill method cells"
    assert not any(
        op["kind"] == "class_apply_set_name"
        for function in ir["functions"]
        for op in function["ops"]
    ), "the frontend must not replay metaclass-owned descriptor callbacks"


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "body",
    [
        "if flag:\n    type Alias = value\n",
        "if flag:\n    type Alias[T: value] = T\nelse:\n    type Alias = value\n",
        "for item in (0, 1):\n    type Alias = value\n",
        "try:\n    if flag:\n        type Alias = value\nfinally:\n    type Last = value\n",
    ],
)
def test_class_namespace_cell_dominates_conditional_evaluator_creation(target, body):
    source = (
        "def outer(flag):\n"
        "    value = 'outer'\n"
        "    class Subject:\n"
        + "".join("        " + line + "\n" for line in body.splitlines())
        + "    return Subject\n"
    )
    # Includes the false/empty path: an unconditional classdictcell publication
    # must never reference a cell allocated only in a taken branch/iteration.
    ir = _compile(source, target)
    outer = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____outer")
    strings = {
        op["out"]: op.get("s_value") for op in outer if op["kind"] == "const_str"
    }
    publications = [
        op
        for op in outer
        if op["kind"] == "store_index"
        and strings.get(op["args"][1]) == "__classdictcell__"
    ]
    assert len(publications) == 1
    definitions = {op["out"]: op for op in outer if "out" in op}
    cell = definitions[publications[0]["args"][2]]
    assert cell["kind"] == "call"
    assert cell["s_value"] == "molt_cell_new"
    assert cell["args"] == [publications[0]["args"][0]]


def test_deferred_method_annotation_namespace_dominates_both_definition_branches():
    _compile(
        "def outer(flag):\n"
        "    class Subject:\n"
        "        if flag:\n"
        "            def method(value: injected): pass\n"
        "        else:\n"
        "            def method(value: injected): pass\n"
        "    return Subject\n"
    )


@pytest.mark.parametrize(
    ("source", "target"),
    [
        ("class C:\n    def method(value: injected): pass\n", (3, 12)),
        (
            "from __future__ import annotations\nclass C:\n    value: injected\n",
            (3, 14),
        ),
        ("class C:\n    type Alias = lambda: value\n", (3, 13)),
    ],
)
def test_class_without_deferred_namespace_reader_has_no_namespace_cell(source, target):
    ir = _compile(source, target)
    assert not any(
        op.get("s_value") == "__classdictcell__"
        for function in ir["functions"]
        for op in function["ops"]
    )


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_conditional_nested_generic_bound_captures_outer_class_owner(target):
    _compile(
        "def outer(flag):\n"
        "    class Outer:\n"
        "        marker = int\n"
        "        if flag:\n"
        "            class Inner[T: marker]: pass\n"
        "    return Outer\n",
        target,
    )
