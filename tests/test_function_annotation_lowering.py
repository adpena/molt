from __future__ import annotations

import ast

import pytest

from molt.frontend import SimpleTIRGenerator, compile_to_tir
from tools.check_ir_structure import verify_frontend_tir


def _function_names(source: str) -> list[str]:
    ir = compile_to_tir(source)
    return [func["name"] for func in ir["functions"]]


def test_eager_function_annotations_do_not_emit_dead_annotate_function() -> None:
    source = """
def f(value: int) -> str:
    return "ok"
"""

    names = _function_names(source)

    assert not any("__annotate__" in name for name in names)


def test_future_function_annotations_do_not_emit_dead_annotate_function() -> None:
    source = """
from __future__ import annotations

def f(value: int) -> str:
    return "ok"
"""

    names = _function_names(source)

    assert not any("__annotate__" in name for name in names)


def test_eager_function_annotations_still_materialize_annotations_dict() -> None:
    source = """
from __future__ import annotations

def f(value: int) -> str:
    return "ok"
"""
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    assert any(
        op.get("kind") == "set_attr_generic_obj"
        and op.get("s_value") == "__annotations__"
        for op in main_ops
    )


def _compile_for_python_314(source: str) -> dict[str, object]:
    generator = SimpleTIRGenerator(target_python=(3, 14))
    generator.visit(ast.parse(source))
    return generator.to_json()


def test_frontend_rejects_unsupported_target_python_feature_version() -> None:
    with pytest.raises(ValueError, match="supported feature versions"):
        SimpleTIRGenerator(target_python=(3, 15))


def test_python_314_class_annotations_have_one_lazy_execution_authority() -> None:
    ir = _compile_for_python_314(
        """
class C:
    value = 7
    observed: marker()
    captured: value
"""
    )
    verification = verify_frontend_tir(ir)
    assert verification.ok, verification.errors
    functions = {function["name"]: function for function in ir["functions"]}
    annotate_name = next(name for name in functions if "__annotate__" in name)
    annotate_ops = functions[annotate_name]["ops"]
    main_ops = functions["molt_main"]["ops"]

    child_defs = {op["out"]: op for op in annotate_ops if "out" in op}
    (namespace_lookup,) = [
        op
        for op in annotate_ops
        if op.get("s_value") == "molt_namespace_get"
        and child_defs[op["args"][1]].get("s_value") == "marker"
    ]
    (global_lookup,) = [
        op
        for op in annotate_ops
        if op["kind"] == "module_get_global"
        and child_defs[op["args"][1]].get("s_value") == "marker"
    ]

    def identity_source(value):
        while child_defs[value]["kind"] in {"identity_alias", "binding_alias"}:
            value = child_defs[value]["args"][0]
        return value

    (callee,) = [
        op
        for op in annotate_ops
        if op["kind"] == "phi"
        and [identity_source(value) for value in op["args"]]
        == [global_lookup["out"], namespace_lookup["out"]]
    ]
    (marker_call,) = [
        op
        for op in annotate_ops
        if op["kind"] == "call_func" and op["args"] == [callee["out"]]
    ]
    assert any(
        op["kind"] == "store_index"
        and child_defs[op["args"][1]].get("s_value") == "observed"
        and op["args"][2] == marker_call["out"]
        for op in annotate_ops
    )
    assert not any(op.get("s_value") == "marker" for op in main_ops)

    class_def = next(op for op in main_ops if op.get("kind") == "class_def")
    class_arg_values = {
        op["out"]: op.get("s_value")
        for op in main_ops
        if op.get("kind") == "const_str" and "out" in op
    }
    class_attr_names = {
        class_arg_values[arg] for arg in class_def["args"] if arg in class_arg_values
    }
    assert "__annotate__" in class_attr_names
    assert "__annotations__" not in class_attr_names
    assert not any(
        str(op.get("s_value", "")).startswith("__molt_annotations_exec_C_")
        for op in main_ops
    )
    definitions = {op["out"]: op for op in main_ops if "out" in op}
    constructor = next(
        op
        for op in main_ops
        if op.get("kind") == "func_new_closure" and op.get("s_value") == annotate_name
    )
    captures = definitions[constructor["args"][0]]["args"]
    assert any(
        class_arg_values.get(value) == "__annotate__"
        and class_def["args"][index + 1] == constructor["out"]
        for index, value in enumerate(class_def["args"][:-1])
    )
    reads = [op for op in annotate_ops if op.get("kind") == "dict_get"]
    assert len(reads) == 2
    assert len({read["args"][0] for read in reads}) == 1
    loaded_map = child_defs[reads[0]["args"][0]]
    assert loaded_map["kind"] == "call" and loaded_map["s_value"] == "molt_cell_get"
    (loaded_cell_name,) = loaded_map["args"]
    capture = child_defs[loaded_cell_name]
    assert capture["kind"] == "index" and capture["args"][0] == "__molt_closure__"
    capture_slot = child_defs[capture["args"][1]]["value"]
    execution_cell = definitions[captures[capture_slot]]
    assert execution_cell["kind"] == "call"
    assert execution_cell["s_value"] == "molt_cell_new"
    (execution_map,) = execution_cell["args"]
    assert definitions[execution_map]["kind"] == "dict_new"
    marks = [
        op
        for op in main_ops
        if op.get("kind") == "store_index" and op["args"][0] == execution_map
    ]
    assert len(marks) == 2
    assert (
        {definitions[mark["args"][1]]["value"] for mark in marks}
        == {child_defs[read["args"][1]]["value"] for read in reads}
        == {0, 1}
    )
    for mark in marks:
        flag = definitions[mark["args"][2]]
        assert flag["kind"] == "const_bool"
        assert type(flag["value"]) is int and flag["value"] == 1
    assert not any(execution_map == value for value in captures)


def test_python_314_module_annotation_execution_state_is_globally_resolvable() -> None:
    ir = _compile_for_python_314("observed: marker()\n")
    functions = {function["name"]: function for function in ir["functions"]}
    main_ops = functions["molt_main"]["ops"]
    annotate = next(
        function
        for name, function in functions.items()
        if name != "molt_main" and "__annotate__" in name
    )
    exec_map_keys = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and str(op.get("s_value", "")).startswith("__molt_annotations_exec___main___")
    }

    assert exec_map_keys
    assert any(
        op.get("kind") == "module_set_attr"
        and len(op.get("args", ())) >= 2
        and op["args"][1] in exec_map_keys
        for op in main_ops
    )
    assert any(op.get("kind") == "module_get_global" for op in annotate["ops"])


def test_python_314_type_parameter_annotations_capture_parent_frame_values() -> None:
    ir = _compile_for_python_314(
        """
class Box[T]:
    item: T
"""
    )
    annotate = next(
        function for function in ir["functions"] if "__annotate__" in function["name"]
    )

    assert annotate["params"] == ["__molt_closure__", "format"]
    assert any(
        op.get("kind") == "func_new_closure" and op.get("s_value") == annotate["name"]
        for function in ir["functions"]
        for op in function["ops"]
    )
    verification = verify_frontend_tir(ir)
    assert verification.ok, verification.errors
