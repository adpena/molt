"""Every class evaluator captures namespace custody instead of a class spelling."""

import ast
import sys

import pytest

from molt.frontend import SimpleTIRGenerator
from molt.compat import CompatibilityError
from molt.frontend.diagnostics import FrontendDiagnostic, FrontendRejection
from molt.frontend._types import CodeSlotDeclaration
from molt.compiler_analysis.python_lexical_scope import python_scope_syntax_error
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
    # The enclosing value's cell lives in its frame home; its operand is the
    # initial value.
    assert value_cell["kind"] == "frame_home_cell"

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
    error = python_scope_syntax_error(
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


@pytest.mark.parametrize("parameters", ["", "[T]"])
@pytest.mark.parametrize(
    ("prefix", "finish"),
    [
        ("", "return Alias"),
        ("", "yield Alias"),
        ("async ", "return Alias"),
        ("async ", "yield Alias"),
    ],
)
def test_method_type_parameter_transport_covers_every_callable_frame(
    parameters, prefix, finish
):
    _compile(
        "class Outer[T]:\n"
        f"    {prefix}def method{parameters}(self):\n"
        "        type Alias = T\n"
        f"        {finish}\n"
    )


@pytest.mark.parametrize(
    "source",
    [
        "def factory[T]():\n    type Alias = T\n    return Alias\n",
        "class Outer[T]:\n    type Alias[T] = T\n",
        "class Outer[T]:\n    type Alias[U: T] = U\n",
        "class Outer[T]:\n    type Alias[U: (T, int)] = U\n",
        "class ForwardBound[T: U, U]: pass\n",
        "class ForwardConstraints[T: (U, int), U]: pass\n",
        "class Outer[T]:\n    class Inner[T]:\n        value: T\n",
        "class Outer[T]:\n    values = [T for _ in (0,)]\n",
        "def factory(flag):\n"
        "    class Outer[T]:\n"
        "        if flag:\n"
        "            type First = T\n"
        "        else:\n"
        "            type Second = T\n"
        "        type Last = T\n"
        "    return Outer\n",
        "T = int\nclass Outer[T]:\n    T = str\n    value: T\n    type Alias = T\n",
    ],
)
def test_type_parameter_capture_cells_dominate_nested_and_conditional_consumers(source):
    # _compile invokes the independent executable-IR dominance verifier.
    _compile(source)


def test_type_parameter_body_and_annotation_share_one_declaration_cell():
    ir = _compile("def shared[T](value: T) -> T:\n    return T\n")
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    definitions = {op["out"]: op for op in main if "out" in op}
    (attachment,) = [op for op in main if op.get("s_value") == "__type_params__"]
    (parameter,) = definitions[attachment["args"][-1]]["args"]
    (cell,) = [
        op
        for op in main
        if op.get("s_value") == "molt_cell_new" and op["args"] == [parameter]
    ]
    consumers = [
        op
        for op in main
        if op["kind"] == "func_new_closure"
        and cell["out"] in definitions[op["args"][0]]["args"]
    ]
    assert len(consumers) == 2, (
        "body and deferred annotations must share the lexical cell"
    )


def test_direct_only_class_type_parameter_keeps_uncaptured_value_path():
    ir = _compile("class Plain[T]:\n    direct = T\n")
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    definitions = {op["out"]: op for op in main if "out" in op}
    (attachment,) = [op for op in main if op.get("s_value") == "__type_params__"]
    (parameter,) = definitions[attachment["args"][-1]]["args"]
    assert not any(
        op.get("s_value") == "molt_cell_new" and op["args"] == [parameter]
        for op in main
    )


def test_type_parameter_cell_read_stays_in_namespace_miss_arm():
    ir = _compile("class Box[T]:\n    value: T\n")
    evaluator = next(fn for fn in ir["functions"] if "__annotate__" in fn["name"])
    ops = evaluator["ops"]
    definitions = {op["out"]: op for op in ops if "out" in op}
    (probe,) = [
        op
        for op in ops
        if op.get("s_value") == "molt_namespace_get"
        and definitions[op["args"][1]].get("s_value") == "T"
    ]
    absent = next(op for op in ops if op["kind"] == "is" and probe["out"] in op["args"])
    branch = next(
        i
        for i, op in enumerate(ops)
        if op["kind"] == "if" and op["args"] == [absent["out"]]
    )
    other_arm = next(i for i in range(branch + 1, len(ops)) if ops[i]["kind"] == "else")
    (fallback,) = [
        op for op in ops[branch + 1 : other_arm] if op.get("s_value") == "molt_cell_get"
    ]
    capture = definitions[fallback["args"][0]]
    assert capture["kind"] == "index"
    assert capture["args"][0] == "__molt_closure__"
    assert ops.index(probe) < branch < ops.index(fallback) < other_arm


def test_declared_type_parameter_capture_rejects_missing_transport(monkeypatch):
    generator = SimpleTIRGenerator(target_python=(3, 14))
    available = generator._free_var_cell_available
    monkeypatch.setattr(
        generator,
        "_free_var_cell_available",
        lambda name: name != "T" and available(name),
    )
    with pytest.raises(
        CompatibilityError, match="Free variable 'T' has no closure transport"
    ) as error:
        generator.visit(ast.parse("class Box[T]:\n    value: T\n"))
    _assert_internal_rejection(
        error.value, "Free variable 'T' has no closure transport"
    )
    assert generator.current_func_name == "molt_main"
    assert generator.current_ops is generator.funcs_map["molt_main"]["ops"]
    assert generator._class_ns_stack == []
    assert generator._class_body_depth == 0
    assert generator.qualname_stack == []
    assert generator.type_parameter_bindings == {}
    assert generator.class_annotation_items == []


@pytest.mark.skipif(
    sys.version_info < (3, 13),
    reason="type parameter default syntax requires host 3.13+",
)
@pytest.mark.parametrize("target", [(3, 13), (3, 14)])
@pytest.mark.parametrize(
    "source",
    [
        "class Outer[T]:\n    type Alias[U = T] = U\n",
        "class ForwardDefault[T = U, U = int]: pass\n",
    ],
)
def test_type_parameter_default_evaluator_captures_its_own_frame(target, source):
    _compile(source, target)


@pytest.mark.parametrize(
    "source",
    [
        "T = int\nclass Outer[T]:\n    global T\n    value: T\n",
        "def outer():\n"
        "    T = int\n"
        "    class Subject[U]:\n"
        "        nonlocal T\n"
        "        value: T\n"
        "    return Subject\n",
    ],
)
def test_type_parameter_overlay_preserves_explicit_global_and_nonlocal_storage(source):
    _compile(source)


def test_type_parameter_capture_rejects_missing_planned_cell(monkeypatch):
    generator = SimpleTIRGenerator(target_python=(3, 14))
    monkeypatch.setattr(
        generator, "_type_parameter_capture_names", lambda owner: frozenset()
    )
    with pytest.raises(
        CompatibilityError, match="Type parameter 'T' has no planned closure cell"
    ) as error:
        generator.visit(ast.parse("class Box[T]:\n    value: T\n"))
    _assert_internal_rejection(
        error.value, "Type parameter 'T' has no planned closure cell"
    )
    assert generator.type_parameter_bindings == {}
    assert generator._class_ns_stack == []


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_generic_class_local_name_controls_before_after_and_method_annotation(target):
    _compile(
        "T = 'global'\n"
        "class C[T]:\n"
        "    before = T\n"
        "    T = 'class'\n"
        "    after = T\n"
        "    def method(self, value: T): pass\n",
        target,
    )


def _assert_internal_rejection(error, message):
    assert error.issue.diagnostic_code == "MOLT-FE008"
    assert "frontend lowering invariant failed" in error.issue.detail
    assert isinstance(error.__cause__, FrontendRejection)
    assert error.__cause__.args == (FrontendDiagnostic.INTERNAL_INVARIANT, message)


@pytest.mark.parametrize("method", [False, True])
@pytest.mark.parametrize(
    "definition",
    [
        "def child(self):\n    broken\n",
        "def child(self):\n    broken\n    yield 1\n",
        "async def child(self):\n    broken\n",
        "async def child(self):\n    broken\n    yield 1\n",
        "def child(self):\n    return lambda: broken\n",
        "def child(self):\n    return lambda: (yield broken)\n",
        "def child(self):\n    return (broken for value in [1])\n",
    ],
)
def test_rejected_function_family_restores_enclosing_frame(
    monkeypatch, method, definition
):
    generator = SimpleTIRGenerator(target_python=(3, 14))
    original_visit = generator.visit_Name
    original_ops = generator.current_ops
    original_scopes = generator._class_ns_stack
    original_bindings = generator.type_parameter_bindings
    original_annotations = generator.class_annotation_items
    rejection = FrontendRejection(
        FrontendDiagnostic.INTERNAL_INVARIANT, "injected child rejection"
    )

    def reject(node):
        if node.id == "broken":
            raise rejection
        return original_visit(node)

    monkeypatch.setattr(generator, "visit_Name", reject)
    source = definition
    if method:
        source = "class Owner[T]:\n" + "".join(
            "    " + line for line in definition.splitlines(keepends=True)
        )
    with pytest.raises(CompatibilityError, match="injected child rejection") as error:
        generator.visit(ast.parse(source))
    _assert_internal_rejection(error.value, "injected child rejection")
    assert error.value.__cause__ is rejection
    assert generator.current_func_name == "molt_main"
    assert generator.current_ops is original_ops
    assert generator._class_ns_stack is original_scopes
    assert original_scopes == []
    assert generator._class_body_depth == 0
    assert generator.qualname_stack == []
    assert generator.type_parameter_bindings is original_bindings
    assert generator.class_annotation_items is original_annotations


def test_failed_function_scope_restores_snapshot_without_finalization(monkeypatch):
    generator = SimpleTIRGenerator()
    original_function = generator.current_func_name
    original_ops = generator.current_ops
    original_state = generator._capture_function_state()
    failure = RuntimeError("original lowering failure")
    finalized = []
    monkeypatch.setattr(
        generator, "_emit_function_exception_handler", lambda: finalized.append(True)
    )
    with pytest.raises(RuntimeError) as error:
        with generator._function_state_scope():
            generator.start_function(
                "rejected_child", code_slots=CodeSlotDeclaration((), (), (), ())
            )
            raise failure
    assert error.value is failure
    assert finalized == []
    assert generator.current_func_name == original_function
    assert generator.current_ops is original_ops
    restored = generator._capture_function_state()
    assert restored.keys() == original_state.keys()
    assert all(restored[name] is value for name, value in original_state.items())


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    ("parameters", "name"), [("T", "T"), ("**P", "P"), ("*Ts", "Ts")]
)
@pytest.mark.parametrize("owner", ["function", "async_function", "class", "class_body"])
def test_nonlocal_type_parameter_rejection_uses_lexical_binding_kind(
    target, parameters, name, owner
):
    if owner == "class_body":
        source = f"class Owner[{parameters}]:\n    nonlocal {name}\n"
    elif owner == "class":
        source = (
            f"class Owner[{parameters}]:\n"
            "    def inner(self):\n"
            f"        nonlocal {name}\n"
            f"        return {name}\n"
        )
    else:
        prefix = "async " if owner == "async_function" else ""
        source = (
            f"{prefix}def owner[{parameters}]():\n"
            "    def inner():\n"
            f"        nonlocal {name}\n"
            f"        return {name}\n"
            "    return inner\n"
        )
    with pytest.raises(SyntaxError) as cpython:
        compile(source, "<nonlocal-type-parameter>", "exec")
    expected = f"nonlocal binding not allowed for type parameter {name!r}"
    assert cpython.value.msg == expected
    syntax_error = python_scope_syntax_error(
        ast.parse(source), target_python=target, future_annotations=False
    )
    assert syntax_error is not None
    assert isinstance(syntax_error.node, ast.Nonlocal)
    assert syntax_error.node.lineno == cpython.value.lineno
    assert syntax_error.message == expected
    with pytest.raises(CompatibilityError) as molt:
        _compile(source, target)
    assert molt.value.issue.diagnostic_code == FrontendDiagnostic.SYNTAX_FORM.value
    assert isinstance(molt.value.__cause__, FrontendRejection)
    assert molt.value.__cause__.args == (
        FrontendDiagnostic.SYNTAX_FORM,
        expected,
        syntax_error.alternative,
    )


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "source",
    [
        "def owner[T]():\n"
        "    def middle():\n"
        "        def inner():\n"
        "            nonlocal T\n"
        "            return T\n"
        "        return inner\n"
        "    return middle\n",
        "from __future__ import annotations\n"
        "def owner[T]():\n"
        "    def inner():\n"
        "        nonlocal T\n"
        "        return T\n"
        "    return inner\n",
        "def owner[T]():\n"
        "    type Alias[T] = T\n"
        "    def inner():\n"
        "        nonlocal T\n"
        "        return T\n"
        "    return inner\n",
        "def owner[T]():\n"
        "    class Shadow:\n"
        "        T = int\n"
        "        def middle[T]():\n"
        "            def inner():\n"
        "                nonlocal T\n"
        "                return T\n"
        "            return inner\n"
        "    return Shadow\n",
        "if False:\n"
        "    def owner[T]():\n"
        "        def inner():\n"
        "            nonlocal T\n"
        "            return T\n",
    ],
)
def test_nonlocal_type_parameter_validation_precedes_source_pruning(
    monkeypatch, target, source
):
    with pytest.raises(SyntaxError) as cpython:
        compile(source, "<nonlocal-before-pruning>", "exec")
    assert cpython.value.msg == "nonlocal binding not allowed for type parameter 'T'"
    generator = SimpleTIRGenerator(target_python=target)

    def refuse_pruning(node):
        pytest.fail("invalid lexical source reached reachability pruning")

    monkeypatch.setattr(
        generator, "_prune_native_support_module_functions", refuse_pruning
    )
    with pytest.raises(CompatibilityError) as molt:
        generator.visit(ast.parse(source))
    assert molt.value.issue.diagnostic_code == FrontendDiagnostic.SYNTAX_FORM.value
    assert isinstance(molt.value.__cause__, FrontendRejection)
    assert molt.value.__cause__.args[1] == cpython.value.msg


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "source",
    [
        "def owner[T]():\n"
        "    def inner():\n"
        "        nonlocal T\n"
        "        return T\n"
        "    T = int\n"
        "    return inner\n",
        "def owner[T]():\n"
        "    ordinary = int\n"
        "    def inner():\n"
        "        nonlocal ordinary\n"
        "        return ordinary\n"
        "    return inner\n",
        "class Owner[T]:\n"
        "    T = int\n"
        "    def inner(self):\n"
        "        nonlocal T\n"
        "        return T\n",
        "def owner[T]():\n"
        "    class Shadow:\n"
        "        T = int\n"
        "        def inner(self):\n"
        "            nonlocal T\n"
        "            return T\n"
        "    return Shadow\n",
        "def owner():\n"
        "    T = int\n"
        "    type Alias[T] = T\n"
        "    def inner():\n"
        "        nonlocal T\n"
        "        return T\n"
        "    return inner\n",
    ],
)
def test_nonlocal_ordinary_shadow_and_alias_regions_preserve_cpython(source, target):
    # The host compiler independently classifies these source bindings, including
    # CPython's ordinary class shadow of an enclosing type-parameter restriction.
    compile(source, "<ordinary-nonlocal-binding>", "exec")
    assert (
        python_scope_syntax_error(
            ast.parse(source), target_python=target, future_annotations=False
        )
        is None
    )
    _compile(source, target)
