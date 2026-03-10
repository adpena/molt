from __future__ import annotations

import ast

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator, compile_to_tir


def _map_single(op: MoltOp) -> dict:
    gen = SimpleTIRGenerator()
    return gen.map_ops_to_json([op])[0]


def test_call_indirect_lowers_to_call_indirect_lane() -> None:
    op = MoltOp(
        kind="CALL_INDIRECT",
        args=[MoltValue("callee"), MoltValue("callargs")],
        result=MoltValue("out"),
    )
    lowered = _map_single(op)
    assert lowered == {
        "kind": "call_indirect",
        "args": ["callee", "callargs"],
        "out": "out",
    }


def test_invoke_ffi_lowers_to_invoke_ffi_lane() -> None:
    op = MoltOp(
        kind="INVOKE_FFI",
        args=[MoltValue("callee"), MoltValue("arg0"), MoltValue("arg1")],
        result=MoltValue("out"),
    )
    lowered = _map_single(op)
    assert lowered == {
        "kind": "invoke_ffi",
        "args": ["callee", "arg0", "arg1"],
        "out": "out",
    }


def test_invoke_ffi_bridge_lane_marker_lowers_to_s_value() -> None:
    op = MoltOp(
        kind="INVOKE_FFI",
        args=[MoltValue("callee"), MoltValue("arg0")],
        result=MoltValue("out"),
        metadata={"ffi_lane": "bridge"},
    )
    lowered = _map_single(op)
    assert lowered == {
        "kind": "invoke_ffi",
        "args": ["callee", "arg0"],
        "out": "out",
        "s_value": "bridge",
    }


def test_guard_tag_lowers_to_guard_tag_lane() -> None:
    op = MoltOp(
        kind="GUARD_TAG",
        args=[MoltValue("value"), MoltValue("tag")],
        result=MoltValue("none"),
    )
    lowered = _map_single(op)
    assert lowered == {"kind": "guard_tag", "args": ["value", "tag"]}


def test_guard_dict_shape_lowers_to_guard_dict_shape_lane() -> None:
    op = MoltOp(
        kind="GUARD_DICT_SHAPE",
        args=[MoltValue("obj"), MoltValue("dict_type"), MoltValue("version")],
        result=MoltValue("guard"),
    )
    lowered = _map_single(op)
    assert lowered == {
        "kind": "guard_dict_shape",
        "args": ["obj", "dict_type", "version"],
        "out": "guard",
    }


def test_box_lowers_to_explicit_box_lane() -> None:
    op = MoltOp(kind="BOX", args=[MoltValue("value")], result=MoltValue("boxed"))
    lowered = _map_single(op)
    assert lowered == {
        "kind": "box",
        "args": ["value"],
        "out": "boxed",
    }


def test_unbox_cast_widen_lower_to_explicit_conversion_lanes() -> None:
    unbox = _map_single(
        MoltOp(kind="UNBOX", args=[MoltValue("boxed")], result=MoltValue("value"))
    )
    cast = _map_single(
        MoltOp(kind="CAST", args=[MoltValue("value")], result=MoltValue("casted"))
    )
    widen = _map_single(
        MoltOp(kind="WIDEN", args=[MoltValue("value")], result=MoltValue("wide"))
    )
    assert unbox == {"kind": "unbox", "args": ["boxed"], "out": "value"}
    assert cast == {"kind": "cast", "args": ["value"], "out": "casted"}
    assert widen == {"kind": "widen", "args": ["value"], "out": "wide"}


def test_borrow_lowers_to_explicit_borrow_lane() -> None:
    op = MoltOp(kind="BORROW", args=[MoltValue("value")], result=MoltValue("borrowed"))
    lowered = _map_single(op)
    assert lowered == {
        "kind": "borrow",
        "args": ["value"],
        "out": "borrowed",
    }


def test_inc_ref_and_dec_ref_lower_to_explicit_ownership_lanes() -> None:
    inc = _map_single(
        MoltOp(kind="INC_REF", args=[MoltValue("value")], result=MoltValue("owned"))
    )
    dec = _map_single(
        MoltOp(kind="DEC_REF", args=[MoltValue("value")], result=MoltValue("released"))
    )
    assert inc == {"kind": "inc_ref", "args": ["value"], "out": "owned"}
    assert dec == {"kind": "dec_ref", "args": ["value"], "out": "released"}


def test_release_lowers_to_explicit_release_lane() -> None:
    lowered = _map_single(
        MoltOp(kind="RELEASE", args=[MoltValue("value")], result=MoltValue("done"))
    )
    assert lowered == {"kind": "release", "args": ["value"], "out": "done"}


def _raw_kinds(source: str, **kwargs: object) -> set[str]:
    gen = SimpleTIRGenerator(**kwargs)
    gen.visit(ast.parse(source))
    return {op.kind for data in gen.funcs_map.values() for op in data["ops"]}


def _lowered_kinds(source: str, **kwargs: object) -> set[str]:
    ir = compile_to_tir(source, **kwargs)
    return {op["kind"] for fn in ir["functions"] for op in fn["ops"]}


def _lowered_ops(source: str, **kwargs: object) -> list[dict]:
    ir = compile_to_tir(source, **kwargs)
    return [op for fn in ir["functions"] for op in fn["ops"]]


def test_raw_guard_tag_emitted_for_type_hints() -> None:
    kinds = _raw_kinds(
        "x: int = 1\n", type_hint_policy="check", fallback_policy="bridge"
    )
    assert "GUARD_TAG" in kinds


def test_raw_guard_dict_shape_emitted_for_dict_increment() -> None:
    kinds = _raw_kinds('d = {}\nd["k"] = d.get("k", 0) + 1\n', fallback_policy="bridge")
    assert "GUARD_DICT_SHAPE" in kinds


def test_raw_call_indirect_emitted_for_bridge_attr_call() -> None:
    kinds = _raw_kinds(
        "import unknown_mod\nunknown_mod.foo(1)\n", fallback_policy="bridge"
    )
    assert "CALL_INDIRECT" in kinds


def test_lowered_call_indirect_lane_is_used_for_bridge_attr_call() -> None:
    kinds = _lowered_kinds(
        "import unknown_mod\nunknown_mod.foo(1)\n", fallback_policy="bridge"
    )
    assert "call_indirect" in kinds


def test_lowered_call_indirect_lane_is_used_for_dynamic_noncallable_attr_call() -> None:
    kinds = _lowered_kinds(
        "import types\nns = types.SimpleNamespace()\nns.fn = 7\nns.fn()\n"
    )
    assert "call_indirect" in kinds


def test_lowered_guard_dict_shape_lane_is_used_for_dict_increment() -> None:
    kinds = _lowered_kinds(
        'd = {}\nd["k"] = d.get("k", 0) + 1\n', fallback_policy="bridge"
    )
    assert "guard_dict_shape" in kinds


def test_lowered_guard_tag_lane_is_used_for_type_hint_checking() -> None:
    kinds = _lowered_kinds(
        "def f(x: int):\n    return x\n",
        type_hint_policy="check",
        fallback_policy="bridge",
    )
    assert "guard_tag" in kinds


def test_raw_invoke_ffi_emitted_for_non_allowlisted_direct_module_call() -> None:
    kinds = _raw_kinds("import os\nos.getcwd()\n", fallback_policy="bridge")
    assert "INVOKE_FFI" in kinds


def test_lowered_invoke_ffi_lane_is_used_for_non_allowlisted_direct_module_call() -> (
    None
):
    kinds = _lowered_kinds("import os\nos.getcwd()\n", fallback_policy="bridge")
    assert "invoke_ffi" in kinds


def test_invoke_ffi_bridge_lane_marker_is_emitted_for_non_allowlisted_module_call() -> (
    None
):
    ops = _lowered_ops("import os\nos.getcwd()\n", fallback_policy="bridge")
    invoke_ops = [op for op in ops if op["kind"] == "invoke_ffi"]
    assert invoke_ops
    assert any(op.get("s_value") == "bridge" for op in invoke_ops)
