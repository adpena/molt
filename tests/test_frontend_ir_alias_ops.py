from __future__ import annotations

import ast
import sys

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator, compile_to_tir


def _map_single(op: MoltOp) -> dict:
    gen = SimpleTIRGenerator()
    return gen.map_ops_to_json([op], run_midend=False)[0]


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


def test_call_bind_lowers_finalizer_fact() -> None:
    op = MoltOp(
        kind="CALL_BIND",
        args=[MoltValue("callee"), MoltValue("callargs")],
        result=MoltValue("out", type_hint="FinalizerClass"),
        metadata={"defines_del": True},
    )
    lowered = _map_single(op)
    assert lowered == {
        "kind": "call_bind",
        "args": ["callee", "callargs"],
        "out": "out",
        "type_hint": "FinalizerClass",
        "defines_del": True,
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


def test_invoke_ffi_native_callable_metadata_lowers_to_schema_fields() -> None:
    op = MoltOp(
        kind="INVOKE_FFI",
        args=[MoltValue("arg0")],
        result=MoltValue("out"),
        metadata={
            "native_callable_export": "scipy.ndimage.distance_transform_edt",
            "native_callable_binding": "direct_symbol",
            "native_callable_symbol": "molt_scipy_ndimage_distance_transform_edt",
            "native_callable_abi": "molt.forward_f32_v1",
        },
    )
    lowered = _map_single(op)
    assert lowered == {
        "kind": "invoke_ffi",
        "args": ["arg0"],
        "out": "out",
        "native_callable_export": "scipy.ndimage.distance_transform_edt",
        "native_callable_binding": "direct_symbol",
        "native_callable_symbol": "molt_scipy_ndimage_distance_transform_edt",
        "native_callable_abi": "molt.forward_f32_v1",
    }


@pytest.mark.parametrize("kind", ["GUARD_TAG", "GUARD_TYPE"])
@pytest.mark.parametrize("result", ["none", "checked"])
def test_runtime_guard_preserves_value_and_expected_tag_operands(
    kind: str, result: str
) -> None:
    op = MoltOp(
        kind=kind,
        args=[MoltValue("value"), MoltValue("tag")],
        result=MoltValue(result),
    )
    lowered = _map_single(op)
    expected = {"kind": kind.lower(), "args": ["value", "tag"]}
    if result != "none":
        expected["out"] = result
    assert lowered == expected


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


def test_borrow_lowers_to_canonical_inc_ref_lane() -> None:
    op = MoltOp(kind="BORROW", args=[MoltValue("value")], result=MoltValue("borrowed"))
    lowered = _map_single(op)
    assert lowered == {
        "kind": "inc_ref",
        "args": ["value"],
        "out": "borrowed",
    }


def test_binding_alias_lowers_to_owned_alias_lane() -> None:
    op = MoltOp(
        kind="BINDING_ALIAS", args=[MoltValue("value")], result=MoltValue("owned")
    )
    lowered = _map_single(op)
    assert lowered == {"kind": "binding_alias", "args": ["value"], "out": "owned"}


def test_plain_local_alias_assignment_emits_owned_binding_alias() -> None:
    raw_ops = _raw_ops("def f(x):\n    y = x\n    return y\n")
    assert any(op.kind == "BINDING_ALIAS" for op in raw_ops)

    lowered_ops = _lowered_ops("def f(x):\n    y = x\n    return y\n")
    assert any(op["kind"] == "binding_alias" for op in lowered_ops)


def _rebinding_store(ops: list[MoltOp], slot: int) -> int:
    """The first store to `slot`'s home after the prologue's parameter store:
    the write that releases the binding it displaces."""
    stores = [
        i
        for i, op in enumerate(ops)
        if op.kind == "FRAME_HOME_STORE" and op.metadata["slot"] == slot
    ]
    return stores[1]


@pytest.mark.parametrize(
    "expression",
    [
        "(value, (value := replacement))",
        "consume(value, (value := replacement))",
        "value + (value := replacement)",
    ],
)
def test_expression_capture_precedes_binding_release(expression: str) -> None:
    ops = _raw_ops(f"def f(value, replacement, consume):\n    return {expression}\n")
    capture = next(i for i, op in enumerate(ops) if op.kind == "BINDING_ALIAS")
    # `value` is the first code slot; its home releases the displaced binding.
    release = _rebinding_store(ops, 0)
    assert capture < release
    assert any(
        op.kind not in {"FRAME_HOME_STORE", "STORE_VAR"}
        and any(
            isinstance(arg, MoltValue) and arg.name == ops[capture].result.name
            for arg in op.args
        )
        for op in ops[release + 1 :]
    ), "the consumer must use the independently captured value"


@pytest.mark.parametrize(
    "expression",
    ["value", "value if condition else replacement", "value or replacement"],
)
def test_return_capture_precedes_finally_rebind(expression: str) -> None:
    ops = _raw_ops(
        "def f(value, replacement, condition):\n"
        "    try:\n"
        f"        return {expression}\n"
        "    finally:\n"
        "        value = replacement\n"
    )
    returned = next(op for op in ops if op.kind == "ret" and op.args)
    result = returned.args[0]
    capture = next(
        i
        for i, op in enumerate(ops)
        if op.kind == "BINDING_ALIAS" and op.result.name == result.name
    )
    assert capture < _rebinding_store(ops, 0)


def test_ordinary_expression_reads_do_not_acquire_blanket_owners() -> None:
    ops = _raw_ops("def f(value, other):\n    return (value, other)\n")
    assert not any(op.kind == "BINDING_ALIAS" for op in ops)


def test_assignment_expression_result_has_an_independent_owner() -> None:
    ops = _raw_function_ops(
        "def f(make):\n    return ((value := make()), (value := make()))\n"
    )
    captures = [op for op in ops if op.kind == "BINDING_ALIAS"]
    assert len(captures) == 2
    returned = next(op for op in ops if op.kind == "ret" and op.args)
    aggregate = next(op for op in ops if op.result.name == returned.args[0].name)
    assert aggregate.kind == "TUPLE_NEW"
    assert [arg.name for arg in aggregate.args] == [op.result.name for op in captures]
    stores = []
    for capture in captures:
        publications = [
            op
            for op in ops
            if op.kind == "FRAME_HOME_STORE" and op.args[0].name == capture.args[0].name
        ]
        assert len(publications) == 1
        store = publications[0]
        assert ops.index(capture) < ops.index(store) < ops.index(aggregate)
        assert store.result.borrows_binding
        assert store.result.name not in {arg.name for arg in aggregate.args}
        stores.append(store)
    assert stores[0].metadata["slot"] == stores[1].metadata["slot"]
    assert ops.index(stores[0]) < ops.index(captures[1])


def test_lambda_return_captures_a_borrowed_parameter() -> None:
    ops = _raw_ops("f = lambda value: value\n")
    returned = next(op for op in ops if op.kind == "ret" and op.args)
    assert any(
        op.kind == "BINDING_ALIAS" and op.result.name == returned.args[0].name
        for op in ops
    )


def test_class_control_flow_type_alias_publishes_through_class_namespace() -> None:
    ops = _raw_ops(
        "class AliasOwner:\n"
        "    ClassValue = int\n"
        "    if True:\n"
        "        type Member[T] = tuple[ClassValue, T]\n",
        module_name="type_alias_class_scope",
    )
    member_keys = {
        op.result.name
        for op in ops
        if op.kind == "CONST_STR" and op.args == ["Member"] and op.result is not None
    }

    assert member_keys
    assert any(
        op.kind == "STORE_INDEX"
        and len(op.args) >= 2
        and isinstance(op.args[1], MoltValue)
        and op.args[1].name in member_keys
        for op in ops
    )


def test_inc_ref_and_dec_ref_lower_to_explicit_ownership_lanes() -> None:
    inc = _map_single(
        MoltOp(kind="INC_REF", args=[MoltValue("value")], result=MoltValue("owned"))
    )
    dec = _map_single(
        MoltOp(kind="DEC_REF", args=[MoltValue("value")], result=MoltValue("released"))
    )
    assert inc == {"kind": "inc_ref", "args": ["value"], "out": "owned"}
    assert dec == {"kind": "dec_ref", "args": ["value"], "out": "released"}


def test_release_lowers_to_canonical_dec_ref_lane() -> None:
    lowered = _map_single(
        MoltOp(kind="RELEASE", args=[MoltValue("value")], result=MoltValue("done"))
    )
    assert lowered == {"kind": "dec_ref", "args": ["value"], "out": "done"}


def _raw_kinds(source: str, **kwargs: object) -> set[str]:
    gen = SimpleTIRGenerator(**kwargs)
    gen.visit(ast.parse(source))
    return {op.kind for data in gen.funcs_map.values() for op in data["ops"]}


def _raw_ops(source: str, **kwargs: object) -> list[MoltOp]:
    gen = SimpleTIRGenerator(**kwargs)
    gen.visit(ast.parse(source))
    return [op for data in gen.funcs_map.values() for op in data["ops"]]


def _raw_function_ops(source: str, **kwargs: object) -> list[MoltOp]:
    gen = SimpleTIRGenerator(module_name="__main__", **kwargs)
    gen.visit(ast.parse(source))
    return gen.funcs_map["__main____f"]["ops"]


def _raw_attribute_loads(ops: list[MoltOp], name: str) -> list[MoltOp]:
    constants = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    return [
        op
        for op in ops
        if (op.kind == "GETATTR_GENERIC_OBJ" and op.args[1] == name)
        or (op.kind == "MODULE_GET_ATTR" and constants.get(op.args[1].name) == name)
    ]


def _lowered_kinds(source: str, **kwargs: object) -> set[str]:
    ir = compile_to_tir(source, **kwargs)
    return {op["kind"] for fn in ir["functions"] for op in fn["ops"]}


def _lowered_ops(source: str, **kwargs: object) -> list[dict]:
    gen = SimpleTIRGenerator(**kwargs)
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    return [op for fn in ir["functions"] for op in fn["ops"]]


def test_dead_static_module_branch_preserves_names_without_executing_import() -> None:
    source = (
        "from __future__ import annotations\n"
        "if False:\n"
        "    from typing import Callable\n"
        "    molt_msgpack_parse_scalar_obj: Callable[[object], object]\n"
        "print('live')\n"
    )
    ops = _raw_ops(
        source,
        module_name="dead_module_metadata_probe",
        target_python=sys.version_info[:2],
    )
    constants = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    producers = {op.result.name: op for op in ops}
    code = next(
        op
        for op in ops
        if op.kind == "CODE_NEW" and constants.get(op.args[1].name) == "<module>"
    )
    names_tuple = producers[code.args[5].name]
    assert names_tuple.kind == "TUPLE_NEW"
    names = tuple(constants[arg.name] for arg in names_tuple.args)
    # co_names follows compiler visitation, before dead-code removal. Use an
    # independent CPython oracle rather than the frontend's own collector.
    reference = compile(source, "<dead-module-metadata>", "exec", dont_inherit=True)
    assert names == reference.co_names
    assert "Callable" in names
    assert "molt_msgpack_parse_scalar_obj" not in constants.values()
    assert "live" in constants.values()
    for dead_name in ("typing", "Callable"):
        values = {value for value, name in constants.items() if name == dead_name}
        assert values
        consumers = [
            op
            for op in ops
            if any(isinstance(arg, MoltValue) and arg.name in values for arg in op.args)
        ]
        assert consumers == [names_tuple], (
            "dead imports may occur only in code metadata"
        )


def test_raw_guard_tag_emitted_for_type_hints() -> None:
    kinds = _raw_kinds(
        "x: int = 1\n", type_hint_policy="check", fallback_policy="bridge"
    )
    assert "GUARD_TAG" in kinds


def test_raw_dict_increment_uses_runtime_admission_for_bound_operands() -> None:
    ops = _raw_function_ops(
        "def f(d, key, step):\n    d[key] = d.get(key, 0) + step\n",
        fallback_policy="bridge",
    )
    increments = [op for op in ops if op.kind == "DICT_STR_INT_INC"]
    assert len(increments) == 1
    increment = increments[0]
    assert len(increment.args) == 3
    assert all(isinstance(arg, MoltValue) for arg in increment.args)
    assert increment.result.type_hint == "bool"
    assert not any(op.kind == "GUARD_DICT_SHAPE" for op in ops)


def test_raw_dict_increment_decline_runs_the_original_statement() -> None:
    ops = _raw_function_ops(
        "def f(d, key, step):\n    d[key] = d.get(key, 0) + step\n",
        fallback_policy="bridge",
    )
    increment = next(op for op in ops if op.kind == "DICT_STR_INT_INC")
    declined = next(
        op for op in ops if op.kind == "NOT" and op.args == [increment.result]
    )
    branch = next(op for op in ops if op.kind == "IF" and op.args == [declined.result])
    start = ops.index(branch)
    depth = 1
    for end in range(start + 1, len(ops)):
        depth += (ops[end].kind == "IF") - (ops[end].kind == "END_IF")
        if depth == 0:
            break
    assert depth == 0
    fallback = ops[start + 1 : end]
    gets = _raw_attribute_loads(fallback, "get")
    assert len(gets) == 1
    call = next(
        op
        for op in fallback
        if op.kind == "CALL_FUNC" and op.args[0].name == gets[0].result.name
    )
    addition = next(
        op
        for op in fallback
        if op.kind == "ADD" and op.args[0].name == call.result.name
    )
    store = next(
        op
        for op in fallback
        if op.kind == "STORE_INDEX" and op.args[2].name == addition.result.name
    )
    assert ops.index(increment) < ops.index(declined) < start
    assert (
        fallback.index(gets[0])
        < fallback.index(call)
        < fallback.index(addition)
        < fallback.index(store)
    )
    assert not any(op.kind == "DICT_STR_INT_INC" for op in fallback)


@pytest.mark.parametrize(
    "arguments,expected", [("1", "CALL_FUNC"), ("value=1", "CALL_INDIRECT")]
)
def test_raw_bridge_attr_call_uses_matching_argument_protocol(
    arguments, expected
) -> None:
    kinds = _raw_kinds(
        f"import unknown_mod\nunknown_mod.foo({arguments})\n", fallback_policy="bridge"
    )
    assert expected in kinds


@pytest.mark.parametrize(
    "arguments,expected", [("1", "call_func"), ("value=1", "call_indirect")]
)
def test_lowered_bridge_attr_call_uses_matching_argument_protocol(
    arguments, expected
) -> None:
    kinds = _lowered_kinds(
        f"import unknown_mod\nunknown_mod.foo({arguments})\n", fallback_policy="bridge"
    )
    assert expected in kinds


def test_lowered_dynamic_noncallable_attr_uses_runtime_callable_check() -> None:
    kinds = _lowered_kinds(
        "import types\nns = types.SimpleNamespace()\nns.fn = 7\nns.fn()\n"
    )
    assert "call_func" in kinds


def test_lowered_explicit_dict_shape_guard_preserves_version_operand() -> None:
    lowered = _map_single(
        MoltOp(
            kind="GUARD_DICT_SHAPE",
            args=[MoltValue("obj"), MoltValue("dict_type"), MoltValue("shape_ver")],
            result=MoltValue("guard"),
        )
    )
    assert lowered == {
        "kind": "guard_dict_shape",
        "args": ["obj", "dict_type", "shape_ver"],
        "out": "guard",
    }


def test_lowered_guard_tag_lane_is_used_for_type_hint_checking() -> None:
    kinds = _lowered_kinds(
        "def f(x: int):\n    return x\n",
        type_hint_policy="check",
        fallback_policy="bridge",
    )
    assert "guard_tag" in kinds


def test_raw_module_call_uses_the_captured_callable_object() -> None:
    ops = _raw_function_ops(
        "def f():\n    import os\n    return os.getcwd()\n", fallback_policy="bridge"
    )
    loads = _raw_attribute_loads(ops, "getcwd")
    assert len(loads) == 1
    calls = [
        op
        for op in ops
        if op.kind == "CALL_FUNC" and op.args[0].name == loads[0].result.name
    ]
    assert len(calls) == 1
    assert calls[0].args == [loads[0].result]
    assert ops.index(loads[0]) < ops.index(calls[0])
    assert not any(op.kind == "INVOKE_FFI" for op in ops)


def test_lowered_module_call_preserves_the_captured_callable_operand() -> None:
    ir = compile_to_tir(
        "def f():\n    import os\n    return os.getcwd()\n", fallback_policy="bridge"
    )
    ops = [op for fn in ir["functions"] for op in fn["ops"]]
    constants = _const_str_map(ops)
    loads = [
        op
        for op in ops
        if (op["kind"] == "get_attr_generic_obj" and op.get("s_value") == "getcwd")
        or (
            op["kind"] == "module_get_attr" and constants.get(op["args"][1]) == "getcwd"
        )
    ]
    assert len(loads) == 1
    calls = [
        op
        for op in ops
        if op["kind"] == "call_func" and op["args"] == [loads[0]["out"]]
    ]
    assert len(calls) == 1
    assert ops.index(loads[0]) < ops.index(calls[0])
    assert not any(op["kind"] == "invoke_ffi" for op in ops)


@pytest.mark.parametrize("keyword", [False, True])
def test_module_call_captures_before_argument_effects_and_preserves_binding(
    keyword: bool,
) -> None:
    arguments = "path=effect()" if keyword else "effect()"
    ops = _raw_function_ops(
        f"def f(effect):\n    import os\n    return os.getcwd({arguments})\n",
        fallback_policy="bridge",
    )
    loads = _raw_attribute_loads(ops, "getcwd")
    assert len(loads) == 1
    expected = "CALL_INDIRECT" if keyword else "CALL_FUNC"
    calls = [
        op
        for op in ops
        if op.kind == expected and op.args[0].name == loads[0].result.name
    ]
    assert len(calls) == 1
    call = calls[0]
    effects = [
        op
        for op in ops[ops.index(loads[0]) + 1 : ops.index(call)]
        if op.kind == "CALL_FUNC" and len(op.args) == 1
    ]
    assert len(effects) == 1
    if keyword:
        pushes = [
            op
            for op in ops
            if op.kind == "CALLARGS_PUSH_KW" and op.args[0].name == call.args[1].name
        ]
        assert len(pushes) == 1
        assert pushes[0].args[2].name == effects[0].result.name
        key = next(op for op in ops if op.result.name == pushes[0].args[1].name)
        assert key.kind == "CONST_STR" and key.args == ["path"]
        assert ops.index(effects[0]) < ops.index(pushes[0]) < ops.index(call)
    else:
        assert call.args[1].name == effects[0].result.name
    assert not any(op.kind == "INVOKE_FFI" for op in ops)


def test_native_callable_export_calls_the_live_callable_object() -> None:
    sources = (
        "def f(data):\n    import nativepkg.ndimage as ndi\n"
        "    return ndi.distance_transform_edt(data)\n",
        "def f(data):\n    from nativepkg.ndimage import distance_transform_edt\n"
        "    return distance_transform_edt(data)\n",
    )
    exports = {
        "nativepkg.ndimage.distance_transform_edt": {
            "module": "nativepkg.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.forward_f32_v1",
            "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
        }
    }
    for source in sources:
        gen = SimpleTIRGenerator(
            known_modules={"nativepkg", "nativepkg.ndimage"},
            direct_call_modules={"__main__"},
            native_callable_exports=exports,
            fallback_policy="bridge",
        )
        gen.visit(ast.parse(source))
        ir = gen.to_json()
        ops = [op for fn in ir["functions"] for op in fn["ops"]]
        assert not any(op["kind"] == "invoke_ffi" for op in ops)
        assert any(
            op["kind"] in {"call_bind", "call_func", "call_indirect", "call_guarded"}
            for op in ops
        )


@pytest.mark.parametrize("condition", ["flag is not None", "flag"])
def test_conditional_native_callable_import_calls_live_global(
    condition: str,
) -> None:
    gen = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": "direct_symbol",
                "abi": "molt.forward_f32_v1",
                "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
            }
        },
        fallback_policy="bridge",
    )
    gen.visit(
        ast.parse(
            f"if {condition}:\n"
            "    from nativepkg.ndimage import distance_transform_edt\n"
            "value = distance_transform_edt(data)\n"
        )
    )
    ops = next(
        fn["ops"] for fn in gen.to_json()["functions"] if fn["name"] == "molt_main"
    )

    # __bool__ can replace the name on the bypass path; even the identity-test
    # condition cannot prevent import STORE release callbacks on the taken path.
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    _assert_live_global_call(ops, "distance_transform_edt", argument="data")


def test_rebound_native_callable_import_still_uses_module_global_call() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": "direct_symbol",
                "abi": "molt.forward_f32_v1",
                "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
            }
        },
        fallback_policy="bridge",
    )
    gen.visit(
        ast.parse(
            "from nativepkg.ndimage import distance_transform_edt\n"
            "distance_transform_edt = other\n"
            "value = distance_transform_edt(data)\n"
        )
    )
    ops = next(
        fn["ops"] for fn in gen.to_json()["functions"] if fn["name"] == "molt_main"
    )

    assert [op for op in ops if op["kind"] == "invoke_ffi"] == []
    _assert_live_global_call(ops, "distance_transform_edt", argument="data")


def test_native_callable_export_rejects_unknown_abi_before_invoke_ffi() -> None:
    with pytest.raises(ValueError, match="abi must be one of"):
        SimpleTIRGenerator(
            known_modules={"nativepkg", "nativepkg.ndimage"},
            direct_call_modules={"__main__"},
            native_callable_exports={
                "nativepkg.ndimage.distance_transform_edt": {
                    "module": "nativepkg.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "direct_symbol",
                    "abi": "molt.forward_f33_v1",
                    "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
                }
            },
            fallback_policy="bridge",
        )


def test_native_callable_fixed_arity_is_enforced_by_the_published_wrapper() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": "direct_symbol",
                "abi": "molt.forward_f32_v1",
                "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
            }
        },
        fallback_policy="bridge",
    )

    gen.visit(
        ast.parse(
            "def f(data, sampling):\n    import nativepkg.ndimage as ndi\n"
            "    return ndi.distance_transform_edt(data, sampling)\n"
        )
    )
    ops = [op for fn in gen.to_json()["functions"] for op in fn["ops"]]
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    assert any(
        op["kind"] in {"call_bind", "call_func", "call_indirect", "call_guarded"}
        for op in ops
    )


def test_native_callable_module_attr_export_uses_normal_dispatch() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": "module_attr",
                "abi": "molt.object_call_v1",
            }
        },
        fallback_policy="bridge",
    )

    gen.visit(
        ast.parse(
            "def f(data):\n    import nativepkg.ndimage as ndi\n"
            "    return ndi.distance_transform_edt(data)\n"
        )
    )
    ir = gen.to_json()
    ops = [op for fn in ir["functions"] for op in fn["ops"]]
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    assert any(
        op["kind"] in {"call_bind", "call_func", "call_indirect", "call_guarded"}
        for op in ops
    )


def test_native_callable_dotted_import_export_uses_normal_dispatch() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": "module_attr",
                "abi": "molt.object_call_v1",
            }
        },
        fallback_policy="bridge",
    )

    gen.visit(
        ast.parse(
            "def f(data):\n    import nativepkg.ndimage\n"
            "    return nativepkg.ndimage.distance_transform_edt(data)\n"
        )
    )
    ir = gen.to_json()
    ops = [op for fn in ir["functions"] for op in fn["ops"]]
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    assert any(
        op["kind"] in {"call_bind", "call_func", "call_indirect", "call_guarded"}
        for op in ops
    )


def test_native_callable_dotted_chain_requires_imported_child_module() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": "module_attr",
                "abi": "molt.object_call_v1",
            }
        },
        fallback_policy="bridge",
    )

    gen.visit(
        ast.parse(
            "def f(data):\n    import nativepkg\n"
            "    return nativepkg.ndimage.distance_transform_edt(data)\n"
        )
    )
    ir = gen.to_json()

    assert not any(
        op["kind"] == "invoke_ffi" for fn in ir["functions"] for op in fn["ops"]
    )
    assert any(op["kind"] == "call_func" for fn in ir["functions"] for op in fn["ops"])


def test_native_callable_module_attr_from_import_uses_captured_callable() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"scipy", "scipy.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "scipy.ndimage.distance_transform_edt": {
                "module": "scipy.ndimage",
                "name": "distance_transform_edt",
                "binding": "module_attr",
                "abi": "molt.object_call_v1",
            }
        },
        fallback_policy="bridge",
    )

    gen.visit(
        ast.parse(
            "def f(inside):\n    from scipy.ndimage import distance_transform_edt\n"
            "    return distance_transform_edt(inside)\n"
        )
    )
    ir = gen.to_json()
    ops = [op for fn in ir["functions"] for op in fn["ops"]]
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    assert any(
        op["kind"] in {"call_bind", "call_func", "call_indirect", "call_guarded"}
        for op in ops
    )


def _molt_main_ops(src: str) -> list[dict]:
    ir = compile_to_tir(src)
    main = next(fn for fn in ir["functions"] if fn.get("name") == "molt_main")
    return main["ops"]


def _const_str_map(ops: list[dict]) -> dict[str, str]:
    return {
        op["out"]: op["s_value"]
        for op in ops
        if op.get("kind") == "const_str" and "out" in op
    }


def _assert_live_global_call(
    ops: list[dict],
    name: str,
    *,
    attributes: tuple[str, ...] = (),
    argument: str | None = None,
) -> None:
    """Prove live callee and positional value custody after import callbacks."""
    consts = _const_str_map(ops)
    reads = [
        op
        for op in ops
        if op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == name
    ]
    assert reads, f"missing live global read for {name}"
    chain = reads
    values = {op["out"] for op in reads}
    for attribute in attributes:
        loads = [
            op
            for op in ops
            if op.get("kind") == "get_attr_generic_obj"
            and op.get("s_value") == attribute
            and (op.get("args") or [None])[0] in values
        ]
        assert loads, f"{attribute} must be loaded from the live receiver"
        chain.extend(loads)
        values = {op["out"] for op in loads}
    calls = [
        op
        for op in ops
        if op.get("kind") == "call_func" and (op.get("args") or [None])[0] in values
    ]
    assert len(calls) == 1
    call = calls[0]
    assert len(call["args"]) == (1 if argument is None else 2)
    if argument is not None:
        argument_reads = {
            op["out"]
            for op in ops
            if op.get("kind") == "module_get_global"
            and consts.get((op.get("args") or [None, None])[1]) == argument
        }
        assert call["args"][1] in argument_reads
    for consumer in [*chain, call]:
        assert set(consumer.get("args", ())) <= _defined_before(
            ops, ops.index(consumer)
        ), "branch-local SSA escaped into live dispatch"


@pytest.mark.parametrize(
    ("source", "name", "attributes"),
    [
        ("import os\nos.getcwd()\n", "os", ("getcwd",)),
        ("from os import getcwd\ngetcwd()\n", "getcwd", ()),
    ],
)
def test_module_import_store_callbacks_require_live_bridge_callee(
    source: str, name: str, attributes: tuple[str, ...]
) -> None:
    ops = _lowered_ops(source, fallback_policy="bridge")
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    _assert_live_global_call(ops, name, attributes=attributes)


@pytest.mark.parametrize("binding", ["direct_symbol", "module_attr"])
@pytest.mark.parametrize(
    ("source", "name", "attributes"),
    [
        (
            "import nativepkg.ndimage as ndi\nndi.distance_transform_edt(data)\n",
            "ndi",
            ("distance_transform_edt",),
        ),
        (
            "from nativepkg.ndimage import distance_transform_edt\n"
            "distance_transform_edt(data)\n",
            "distance_transform_edt",
            (),
        ),
        (
            "import nativepkg.ndimage\n"
            "nativepkg.ndimage.distance_transform_edt(data)\n",
            "nativepkg",
            ("ndimage", "distance_transform_edt"),
        ),
    ],
)
def test_native_export_metadata_does_not_override_callback_exposed_binding(
    binding: str, source: str, name: str, attributes: tuple[str, ...]
) -> None:
    ops = _lowered_ops(
        source,
        known_modules={"nativepkg", "nativepkg.ndimage"},
        direct_call_modules={"__main__"},
        native_callable_exports={
            "nativepkg.ndimage.distance_transform_edt": {
                "module": "nativepkg.ndimage",
                "name": "distance_transform_edt",
                "binding": binding,
                "abi": "molt.forward_f32_v1"
                if binding == "direct_symbol"
                else "molt.object_call_v1",
                **(
                    {"symbol": "molt_nativepkg_ndimage_distance_transform_edt"}
                    if binding == "direct_symbol"
                    else {}
                ),
            }
        },
        fallback_policy="bridge",
    )
    assert not any(op["kind"] == "invoke_ffi" for op in ops)
    _assert_live_global_call(ops, name, attributes=attributes, argument="data")


def _defined_before(ops: list[dict], target_index: int) -> set[str]:
    """SSA values defined by ops strictly before target_index that are NOT
    produced inside an if/end_if region (i.e. defined on the fall-through
    path)."""
    defined: set[str] = set()
    depth = 0
    for op in ops[:target_index]:
        kind = op.get("kind")
        if kind == "if":
            depth += 1
        elif kind == "end_if":
            depth = max(0, depth - 1)
        out = op.get("out")
        if depth == 0 and isinstance(out, str) and out != "none":
            defined.add(out)
    return defined


def test_conditional_reimport_reads_global_not_branch_local() -> None:
    # A name imported unconditionally, then conditionally re-imported, then
    # read AFTER the branch must be read through MODULE_GET_GLOBAL, never
    # through the SSA value produced by the branch-local import, which is
    # undefined on the fall-through path (an uninitialised NaN-box sentinel
    # that reads as None and, when iterated, spins forever). This was the
    # numpy `_core/__init__.py` witness wedge: `import sys` in the except
    # handler, a conditional re-import, then `major, minor, *_ =
    # sys.version_info`.
    ops = _molt_main_ops(
        "import sys\n"
        "if len(sys.argv) > 100000:\n"
        "    import sys\n"
        "print(type(sys).__name__)\n"
    )
    consts = _const_str_map(ops)
    end_if_index = next(i for i, op in enumerate(ops) if op.get("kind") == "end_if")
    post = ops[end_if_index + 1 :]

    # The post-branch read of `sys` is a module_get_global for 'sys'.
    global_reads = [
        op
        for op in post
        if op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == "sys"
    ]
    assert global_reads, (
        "post-branch sys read must route through module_get_global; "
        f"post ops: {[op.get('kind') for op in post]}"
    )

    # The condition can execute callbacks, so the type binding itself is a
    # live namespace read. Prove its real dynamic consumer and argument custody,
    # rather than requiring a name-only TYPE_OF specialization after effects.
    type_reads = [
        op
        for op in post
        if op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == "type"
    ]
    assert type_reads
    type_values = {op["out"] for op in type_reads}
    calls = [
        op
        for op in post
        if op.get("kind") == "call_func"
        and (op.get("args") or [None])[0] in type_values
    ]
    assert calls
    sys_values = {op["out"] for op in global_reads}
    assert len(calls) == 1
    assert len(calls[0]["args"]) == 2
    assert calls[0]["args"][1] in sys_values
    for consumer in calls:
        available = _defined_before(ops, ops.index(consumer))
        assert set(consumer["args"]) <= available, (
            "branch-local SSA escaped into a call"
        )


def test_collect_assigned_names_includes_import_bindings() -> None:
    # Root-cause unit check: the binding collector must treat imports as
    # scope bindings exactly like CPython's symbol table, so visit_If's
    # module-scope flush/evict reconciles a conditionally (re)imported name.
    gen = SimpleTIRGenerator()
    body = ast.parse(
        "import sys\n"
        "import a.b.c\n"
        "import d.e as f\n"
        "from g import h, i as j\n"
        "from k import *\n"
    ).body
    assigned = gen._collect_assigned_names(body)
    assert {"sys", "a", "f", "h", "j"} <= assigned
    # `*` binds no specific name; the dotted `a.b.c` binds only the head.
    assert "b" not in assigned and "c" not in assigned and "*" not in assigned
    ordered = gen._collect_assigned_names_ordered(body)
    assert set(ordered) >= {"sys", "a", "f", "h", "j"}


@pytest.mark.parametrize(
    "name",
    [
        "array",
        "len",
        "list",
        "ValueError",
        "TYPE_CHECKING",
        "NotImplemented",
        "Ellipsis",
    ],
)
def test_import_star_dynamic_binding_precedes_bare_name_fallback(name: str) -> None:
    ops = _molt_main_ops(f"from ext import *\nresult = {name}\n")
    consts = _const_str_map(ops)
    assert any(
        op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == name
        for op in ops
    ), "source-ordered dynamic bindings must be read from the live module namespace"
    assert not any(
        op.get("kind") == "module_import"
        and consts.get((op.get("args") or [None])[0]) == name
        for op in ops
    )


def test_import_star_dynamic_binding_precedes_stdlib_bare_name_fallback() -> None:
    ops = _molt_main_ops("from ext import *\narray.__module__ = 'pkg'\n")
    consts = _const_str_map(ops)
    assert any(
        op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == "array"
        for op in ops
    )
    assert not any(
        op.get("kind") == "module_import"
        and consts.get((op.get("args") or [None])[0]) == "array"
        for op in ops
    )


@pytest.mark.parametrize(
    ("prefix", "expression", "name"),
    [
        ("array = 1\n", "array", "array"),
        ("", "abs(-1)", "abs"),
        ("", "len([])", "len"),
        ("from builtins import abs as original\n", "original(-1)", "original"),
        ("import math\n", "math.sqrt(4)", "math"),
        ("def original():\n    return 1\n", "original()", "original"),
        ("", "__name__", "__name__"),
    ],
)
def test_invalidated_bindings_precede_cached_values_and_direct_call_shortcuts(
    prefix: str, expression: str, name: str
) -> None:
    ops = _molt_main_ops(f"{prefix}from ext import *\nresult = {expression}\n")
    consts = _const_str_map(ops)
    assert any(
        op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == name
        for op in ops
    ), "callee/value resolution must observe the live namespace after import-star"


def test_clean_builtin_read_retains_static_lowering() -> None:
    ops = _molt_main_ops("result = Ellipsis\n")
    assert any(op.get("kind") == "const_ellipsis" for op in ops)
    consts = _const_str_map(ops)
    assert not any(
        op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == "Ellipsis"
        for op in ops
    )


def test_import_star_does_not_replace_lexical_parameter() -> None:
    ir = compile_to_tir("from ext import *\ndef f(array):\n    return array\n")
    functions = [fn for fn in ir["functions"] if fn.get("name") != "molt_main"]
    assert functions
    ops = [op for fn in functions for op in fn["ops"]]
    consts = _const_str_map(ops)
    assert not any(
        op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == "array"
        for op in ops
    )


@pytest.mark.parametrize(
    "source",
    [
        "def f(abs):\n    return abs(-1)\n",
        "def replacement(value):\n    return value\nabs = replacement\nresult = abs(-1)\n",
        "def f(abs):\n    def g():\n        return abs(-1)\n    return g\n",
    ],
)
def test_bound_builtin_calls_do_not_use_name_only_specialization(source: str) -> None:
    ir = compile_to_tir(source)
    assert not any(
        op.get("kind") == "abs" for fn in ir["functions"] for op in fn["ops"]
    )


@pytest.mark.parametrize(
    "condition", ["[print('condition-effect')]", "[missing_condition]"]
)
def test_known_truth_retains_condition_ir_but_not_dead_successor(condition: str):
    source = f"if {condition}:\n    print('live-successor')\nelse:\n    print('dead-successor')\n"
    ops = _molt_main_ops(source)
    strings = set(_const_str_map(ops).values())
    assert "live-successor" in strings
    assert "dead-successor" not in strings
    if "print" in condition:
        assert "condition-effect" in strings
    else:
        assert "missing_condition" in strings
        assert any(op.get("kind") == "module_get_global" for op in ops)


def test_bare_type_checking_name_has_runtime_nameerror_lookup():
    ops = _molt_main_ops("print(TYPE_CHECKING)\n")
    consts = _const_str_map(ops)
    assert any(
        op.get("kind") == "module_get_global"
        and consts.get((op.get("args") or [None, None])[1]) == "TYPE_CHECKING"
        for op in ops
    )


def test_static_false_member_retains_walrus_owner_store():
    ops = _molt_main_ops(
        "import typing\nif (owner_alias := typing).TYPE_CHECKING:\n"
        "    print('dead-successor')\n"
    )
    consts = _const_str_map(ops)
    assert "dead-successor" not in consts.values()
    assert any(
        op.get("kind") == "module_set_attr"
        and consts.get((op.get("args") or [None, None])[1]) == "owner_alias"
        for op in ops
    )


def test_frontend_semantic_token_cannot_escape_after_serializer_rewrites(monkeypatch):
    import pytest

    generator = SimpleTIRGenerator()
    monkeypatch.setattr(
        generator,
        "_fuse_string_split_field_consumers_json",
        lambda ops: [{"kind": "CONST_NOT_IMPLEMENTED", "out": "sentinel"}],
    )
    with pytest.raises(ValueError, match="escaped serialization"):
        generator.map_ops_to_json([], run_midend=False)


@pytest.mark.parametrize("kind", ["guard_tag", "guard_type"])
@pytest.mark.parametrize("tag", [1, 8])
def test_split_scalarization_never_discards_an_undischarged_runtime_guard(
    kind: str, tag: int
) -> None:
    ops = [
        {"kind": "const_str", "out": "sep", "s_value": ","},
        {"kind": "const", "out": "index", "value": 0},
        {"kind": "const", "out": "tag", "value": tag},
        {"kind": "string_split", "args": ["text", "sep"], "out": "fields"},
        {"kind": kind, "args": ["fields", "tag"]},
        {"kind": "index", "args": ["fields", "index"], "out": "field"},
    ]
    assert SimpleTIRGenerator._scalarize_string_split_fields_json(ops) == ops
