from __future__ import annotations

import ast

import pytest

from molt.compiler_analysis.python_binding_flow import analyze_python_source_bindings
from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator, compile_to_tir


def _raw_ops(source: str) -> list[MoltOp]:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(source))
    return generator.funcs_map["molt_main"]["ops"]


@pytest.mark.parametrize("alias", [False, True])
@pytest.mark.parametrize(
    ("name", "arguments", "opcode"),
    [
        ("bool", "", "CONST_BOOL"),
        ("int", "", "CONST"),
        ("float", "", "CONST_FLOAT"),
        ("complex", "", "COMPLEX_FROM_OBJ"),
        ("str", "", "CONST_STR"),
        ("bytes", "", "CONST_BYTES"),
        ("bytearray", "", "BYTEARRAY_FROM_OBJ"),
        ("list", "", "LIST_NEW"),
        ("tuple", "", "TUPLE_NEW"),
        ("dict", "", "DICT_NEW"),
        ("set", "", "SET_NEW"),
        ("frozenset", "", "FROZENSET_NEW"),
        ("range", "4", "RANGE_NEW"),
        ("len", "b'abc'", "CONST"),
    ],
)
def test_shape_specialization_uses_captured_identity(
    alias: bool, name: str, arguments: str, opcode: str
) -> None:
    source = f"ctor = {name}\n" if alias else ""
    source += f"value = {'ctor' if alias else name}({arguments})\n"
    ops = _raw_ops(source)
    assert any(op.kind == opcode for op in ops)
    assert not any(op.kind in {"CALL_FUNC", "CALL_INDIRECT"} for op in ops)


@pytest.mark.parametrize("name", ["len", "list", "tuple", "range", "int", "str"])
def test_shape_call_captures_live_callee_before_argument_callback(name: str) -> None:
    ops = _raw_ops(f"value = {name}(argument())\n")
    constants = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    callee_loads = [
        (position, op.result)
        for position, op in enumerate(ops)
        if op.kind == "MODULE_GET_GLOBAL" and constants.get(op.args[1].name) == name
    ]
    assert len(callee_loads) == 1
    load_position, callee = callee_loads[0]
    invocation = next(
        position
        for position, op in enumerate(ops)
        if op.kind == "CALL_FUNC" and op.args[0] is callee
    )
    assert any(op.kind == "CALL_FUNC" for op in ops[load_position + 1 : invocation]), (
        "argument callback must occur between callable capture and invocation"
    )


@pytest.mark.parametrize("name", ["len", "range", "list", "tuple", "int"])
def test_shadowed_shape_name_is_not_specialization_authority(name: str) -> None:
    ops = _raw_ops(f"{name} = replacement\nvalue = {name}(3)\n")
    assert any(op.kind == "CALL_FUNC" for op in ops)
    assert not any(op.kind in {"LEN", "RANGE_NEW", "LIST_FROM_RANGE"} for op in ops)


@pytest.mark.parametrize("call", ["len()", "len(1, 2)", "range()", "list(1, 2)"])
def test_invalid_shape_arity_keeps_runtime_binding(call: str) -> None:
    ops = _raw_ops(f"value = {call}\n")
    assert any(op.kind == "CALL_FUNC" for op in ops)


@pytest.mark.parametrize(
    "body",
    [
        "for item in range(3):\n    pass\n",
        "value = [item for item in range(3)]\n",
        "value = ['x' for item in range(3)]\n",
        "value = list(range(3))\n",
    ],
)
def test_range_fusion_obeys_same_identity_authority(body: str) -> None:
    ir = compile_to_tir("range = replacement\n" + body)
    ops = [op for fn in ir["functions"] for op in fn["ops"]]
    assert any(op["kind"] == "call_func" for op in ops)
    assert not any(
        op["kind"] in {"range_new", "list_from_range", "loop_index_start"} for op in ops
    )


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        ("bool", ""),
        ("int", ""),
        ("float", ""),
        ("complex", ""),
        ("str", ""),
        ("bytes", ""),
        ("bytearray", ""),
        ("tuple", ""),
        ("list", ""),
        ("set", ""),
        ("frozenset", ""),
        ("dict", ""),
        ("range", "3"),
        ("len", "()"),
    ],
)
def test_callable_body_uses_its_activation_builtin_namespace(
    name: str,
    arguments: str,
) -> None:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(f"def value():\n    return {name}({arguments})\n"))
    ops = generator.funcs_map["__main____value"]["ops"]
    constants = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    callee = next(
        op.result
        for op in ops
        if op.kind == "MODULE_GET_GLOBAL" and constants.get(op.args[1].name) == name
    )
    call = next(op for op in ops if op.kind == "CALL_FUNC" and op.args[0] is callee)
    assert call.result.type_hint == "Any"


@pytest.mark.parametrize(
    "source",
    [
        "alias = float\ndef value():\n    return alias(1)\n",
        "from builtins import float as alias\ndef value():\n    return alias(1)\n",
        "import builtins as core\ndef value():\n    return core.float(1)\n",
        "def value():\n    from builtins import float as alias\n    return alias(1)\n",
    ],
)
def test_rebound_global_or_import_alias_cannot_bypass_live_dispatch(
    source: str,
) -> None:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(source))
    ops = generator.funcs_map["__main____value"]["ops"]
    assert not any(op.kind == "FLOAT_FROM_OBJ" for op in ops)
    assert any(op.kind == "CALL_FUNC" for op in ops)


def test_alias_acquisition_reads_live_builtin_object() -> None:
    ops = _raw_ops("alias = len\n")
    assert any(op.kind == "MODULE_GET_GLOBAL" for op in ops)
    assert not any(op.kind == "BUILTIN_FUNC" for op in ops)


@pytest.mark.parametrize(
    "source",
    [
        "globals().pop('key', None)\n",
        "namespace = globals()\nnamespace.pop('key', None)\n",
        "def owner(): pass\nowner.__globals__.pop('key', None)\n",
    ],
)
def test_bootstrap_namespace_aliases_share_exact_receiver_authority(
    source: str,
) -> None:
    assert any(op.kind == "DICT_POP" for op in _raw_ops(source))


def test_deferred_namespace_receiver_keeps_subclass_method_dispatch() -> None:
    ir = compile_to_tir("def owner():\n    return globals().pop('key', None)\n")
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____owner")
    assert not any(op["kind"] == "dict_pop" for op in ops)
    assert any(
        op["kind"] == "get_attr_generic_obj" and op.get("s_value") == "pop"
        for op in ops
    )


def test_immediate_import_alias_with_inert_operand_specializes() -> None:
    ir = compile_to_tir("from builtins import float as make\nvalue = make(1)\n")
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    conversion_ops = [op for op in ops if op.get("source_line") == 2]
    assert any(op["kind"] == "float_from_obj" for op in conversion_ops)
    # Import protocol calls belong to line 1, not this constructor invocation.
    assert not any(op["kind"] == "call_func" for op in conversion_ops)


def test_exact_result_projection_never_recovers_unknown_from_frontend_hints() -> None:
    generator = SimpleTIRGenerator()
    generator.locals["value"] = MoltValue("value", type_hint="list")
    generator.boxed_local_hints["value"] = "list"
    assert generator._builtin_exact_type_from_expr(ast.Name(id="value")) is None
    assert generator._builtin_exact_type_from_expr(ast.Constant(value=1j)) == "complex"


@pytest.mark.parametrize(
    ("hint", "method", "arguments", "specialized"),
    [
        ("list", "append", "1", "LIST_APPEND"),
        ("dict", "get", "1", "DICT_GET"),
        ("set", "add", "1", "SET_ADD"),
        ("tuple", "count", "1", "TUPLE_COUNT"),
        ("str", "split", "'x'", "STRING_SPLIT"),
        ("bytes", "split", "b'x'", "BYTES_SPLIT"),
        ("bytearray", "split", "b'x'", "BYTEARRAY_SPLIT"),
    ],
)
def test_stale_receiver_hints_cannot_authorize_any_builtin_method_family(
    hint: str,
    method: str,
    arguments: str,
    specialized: str,
) -> None:
    source = f"value.{method}({arguments})"
    generator = SimpleTIRGenerator()
    generator.python_binding_index = analyze_python_source_bindings(source)
    generator.locals["value"] = MoltValue("value", type_hint=hint)
    generator.visit(ast.parse(source, mode="eval").body)
    ops = generator.funcs_map["molt_main"]["ops"]
    assert any(op.kind == "CALL_FUNC" for op in ops)
    assert not any(op.kind == specialized for op in ops)

    reference = SimpleTIRGenerator()
    reference.python_binding_index = analyze_python_source_bindings(f"value.{method}")
    reference.locals["value"] = MoltValue("value", type_hint=hint)
    result = reference.visit(ast.parse(f"value.{method}", mode="eval").body)
    assert not result.type_hint.startswith("BoundMethod:")


@pytest.mark.parametrize("policy", ["ignore", "check"])
def test_python_method_annotation_does_not_certify_its_return_lane(policy: str) -> None:
    generator = SimpleTIRGenerator(type_hint_policy=policy)
    generator.visit(
        ast.parse(
            "class Owner:\n"
            "    def value(self) -> list:\n"
            "        if self:\n"
            "            return 17\n"
            "        return 23\n"
            "owner = Owner()\n"
            "result = owner.value()\n"
            "result.append(1)\n"
        )
    )
    ops = generator.funcs_map["molt_main"]["ops"]
    assert not any(op.kind == "LIST_APPEND" for op in ops)
    assert not any(op.kind == "CALL" and op.result.type_hint == "list" for op in ops)


def test_known_constructor_result_survives_callbackful_invocation() -> None:
    ops = _raw_ops("value = list(source)\n")
    call = next(op for op in ops if op.kind == "CALL_FUNC")
    assert call.result.type_hint == "list"


@pytest.mark.parametrize(
    ("expression", "kind"),
    [("dict(item=1)", "dict"), ("int('10', base=2)", "int"), ("list(*args)", "list")],
)
def test_generic_binding_preserves_independent_normal_result_kind(
    expression: str, kind: str
) -> None:
    ops = _raw_ops(f"value = {expression}\n")
    call = next(op for op in ops if op.kind == "CALL_INDIRECT")
    assert call.result.type_hint == kind


@pytest.mark.parametrize("name", ["str", "bytes"])
def test_protocol_constructor_result_is_not_assumed_exact(name: str) -> None:
    # CPython accepts a strict subclass returned by __str__/__bytes__. Such a
    # result can override subsequent operations and have observable cleanup.
    ops = _raw_ops(f"value = {name}(source)\n")
    call = next(op for op in ops if op.kind == "CALL_FUNC")
    assert call.result.type_hint == "Any"


def test_inner_string_conversion_precedes_later_base_evaluation() -> None:
    ops = _raw_ops("value = int(str(1.5), base())\n")
    assert not any(op.kind == "INT_FROM_STR_OF_OBJ" for op in ops)
    conversion = next(
        position for position, op in enumerate(ops) if op.kind == "STR_FROM_OBJ"
    )
    assert any(op.kind == "CALL_FUNC" for op in ops[conversion + 1 :])


def test_constant_list_length_keeps_container_evaluation() -> None:
    ops = _raw_ops("value = len([1, 2])\n")
    assert any(op.kind == "LIST_NEW" for op in ops)
    assert any(op.kind == "LEN" for op in ops)


@pytest.mark.parametrize(
    ("literal", "kind"), [("[]", "list"), ("{}", "dict"), ("()", "tuple")]
)
@pytest.mark.parametrize("callback", [False, True])
def test_receiver_exactness_comes_only_from_source_point_result(
    literal: str, kind: str, callback: bool
) -> None:
    source = f"x = {literal}\n" + ("mutate()\n" if callback else "") + "value = x\n"
    tree = ast.parse(source)
    generator = SimpleTIRGenerator()
    generator.python_binding_index = analyze_python_source_bindings(source)
    receiver = tree.body[-1].value
    assert (generator._builtin_exact_type_from_expr(receiver) == kind) is not callback


def test_frontend_iteration_hint_projects_canonical_clause_fact() -> None:
    source = "for item in ['value']:\n    pass\n"
    tree = ast.parse(source)
    loop = tree.body[0]
    assert isinstance(loop, ast.For)
    generator = SimpleTIRGenerator()
    generator.python_binding_index = analyze_python_source_bindings(source)
    iterable = MoltValue("stale", type_hint="bytes")

    assert generator._iteration_element_hint(loop, iterable) == "str"


def test_unknown_canonical_iteration_fact_does_not_fall_back_to_frontend_hint() -> None:
    source = "for item in source:\n    pass\n"
    tree = ast.parse(source)
    loop = tree.body[0]
    assert isinstance(loop, ast.For)
    generator = SimpleTIRGenerator()
    generator.python_binding_index = analyze_python_source_bindings(source)
    iterable = MoltValue("source", type_hint="list")
    generator.container_elem_hints[iterable.name] = "int"

    assert generator._iteration_element_hint(loop, iterable) is None
