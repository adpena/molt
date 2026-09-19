from __future__ import annotations

import ast
from pathlib import Path
from textwrap import indent

import pytest

from molt._wasm_abi_generated import wasm_runtime_callable_arity
from molt.compat import CompatibilityError
from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator, compile_to_tir
from molt.frontend._types import BUILTIN_FUNC_SPECS, _builtin_func_abi_arity


def _op_field(op: dict[str, object] | MoltOp, field: str) -> object:
    if isinstance(op, dict):
        return op.get(field)
    if field == "kind":
        return op.kind.lower()
    if field == "args":
        return op.args
    if field == "out":
        return op.result.name
    if field == "s_value":
        if op.kind in {"BUILTIN_FUNC", "CONST_STR"} and op.args:
            return op.args[0]
        return None
    return None


def _value_name(value: object) -> object:
    if isinstance(value, MoltValue):
        return value.name
    return value


@pytest.mark.parametrize("annotation", ["list", "tuple"])
def test_tuple_conversion_does_not_treat_annotations_as_exact(annotation: str) -> None:
    ir = compile_to_tir(f"def f(value: {annotation}):\n    return tuple(value)\n")
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____f")
    assert any(op["kind"] == "call_func" for op in ops)
    assert not any(op["kind"] == "tuple_from_list" for op in ops)


@pytest.mark.parametrize(
    "name",
    [
        "list",
        "tuple",
        "dict",
        "set",
        "frozenset",
        "float",
        "complex",
        "int",
        "pow",
        "round",
        "bytes",
        "bytearray",
        "memoryview",
        "str",
        "bool",
        "len",
        "map",
        "zip",
        "sorted",
        "min",
        "max",
        "any",
        "all",
        "sum",
    ],
)
@pytest.mark.parametrize("expansion", ["*values", "**values"])
def test_builtin_expansion_uses_shared_call_assembly(name: str, expansion: str) -> None:
    ir = compile_to_tir(f"def f(values):\n    return {name}({expansion})\n")
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____f")
    expansion_kind = (
        "callargs_expand_kwstar"
        if expansion.startswith("**")
        else "callargs_expand_star"
    )
    assert sum(op["kind"] == expansion_kind for op in ops) == 1
    assert any(op["kind"] in {"call_bind", "call_indirect"} for op in ops)


def test_frontend_builtin_func_specs_are_wasm_manifest_backed() -> None:
    for func_id, spec in BUILTIN_FUNC_SPECS.items():
        manifest_arity = wasm_runtime_callable_arity(spec.runtime)
        assert manifest_arity is not None, func_id
        assert _builtin_func_abi_arity(spec) == manifest_arity


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("chunked", [False, True])
@pytest.mark.parametrize("module_name", ["globals_callable", "imported_globals"])
def test_globals_callable_uses_canonical_builtin_without_local_wrappers(
    target_python: tuple[int, int], chunked: bool, module_name: str
) -> None:
    from molt.cli.module_cache import _module_lowering_local_reference_issue

    generator = SimpleTIRGenerator(
        module_name=module_name,
        known_modules={"builtins", "inspect", "types", "globals_callable_support"},
        target_python=target_python,
        module_chunking=chunked,
        module_chunk_max_ops=1,
    )
    source = (
        Path(__file__).parent / "differential/basic/globals_callable.py"
    ).read_text(encoding="utf-8")
    generator.visit(ast.parse(source))
    functions = generator.to_json()["functions"]
    assert _module_lowering_local_reference_issue(module_name, functions) is None
    assert not any("__molt_globals_builtin__" in fn["name"] for fn in functions)
    assert any(
        op["kind"] == "builtin_func" and op.get("s_value") == "molt_globals_builtin"
        for fn in functions
        for op in fn["ops"]
    )
    if chunked:
        assert len(generator.module_chunk_symbols) > 1


def test_code_slots_split_lexical_module_bootstrap_from_active_globals() -> None:
    ir = compile_to_tir("def target():\n    return globals()\n")
    main_ops = next(
        function["ops"]
        for function in ir["functions"]
        if function["name"] == "molt_main"
    )
    producers = {op.get("out"): op for op in main_ops if isinstance(op.get("out"), str)}
    slots = [op for op in main_ops if op["kind"] == "code_slot_set"]

    assert slots
    assert all(producers[slot["args"][0]]["kind"] == "code_new" for slot in slots)
    globals_ops = [producers[slot["args"][1]] for slot in slots]
    lexical_globals = [
        op
        for op in globals_ops
        if op["kind"] == "module_get_attr"
        and producers[op["args"][1]].get("s_value") == "__dict__"
    ]
    active_globals = [
        op
        for op in globals_ops
        if op["kind"] == "call" and op.get("s_value") == "molt_globals_builtin"
    ]

    assert len(lexical_globals) == 1
    assert active_globals
    trace_enter = next(
        index for index, op in enumerate(main_ops) if op["kind"] == "trace_enter_slot"
    )
    lexical_slot = next(
        slot for slot in slots if producers[slot["args"][1]] in lexical_globals
    )
    assert main_ops.index(lexical_slot) < trace_enter
    assert all(
        main_ops.index(slot) > trace_enter
        for slot in slots
        if producers[slot["args"][1]] in active_globals
    )

    target_ops = next(
        function["ops"]
        for function in ir["functions"]
        if function["name"] == "__main____target"
    )
    _positional_call(
        target_ops, _module_attr_accesses(target_ops, "module_get_global", "globals"), 0
    )


def test_function_module_binding_reads_use_active_global_lookup() -> None:
    ir = compile_to_tir(
        "def target():\n    return 1\n\ndef probe():\n    return target\n"
    )
    ops = next(
        function["ops"]
        for function in ir["functions"]
        if function["name"] == "__main____probe"
    )

    assert _module_attr_accesses(ops, "module_get_global", "target")
    assert not _module_attr_accesses(ops, "module_get_attr", "target")


def test_active_global_read_does_not_inherit_lexical_module_type() -> None:
    generator = SimpleTIRGenerator(module_name="namespace_probe")
    generator.current_func_name = "namespace_probe__function"
    generator._module_attr_type_hints["marker"] = "int"
    value = generator._emit_module_attr_get("marker")
    assert value.type_hint == "Any"
    producer = next(op for op in generator.current_ops if op.result is value)
    assert producer.kind == "MODULE_GET_GLOBAL"


@pytest.mark.parametrize("module_binding", [False, True])
def test_dynamic_function_hint_uses_shared_guarded_context_dispatch(
    module_binding: bool,
) -> None:
    generator = SimpleTIRGenerator()
    symbol = "__main____target"
    callee = MoltValue("callee", type_hint=f"Func:{symbol}")
    generator.func_symbol_names[symbol] = "target"
    if module_binding:
        generator.globals["target"] = callee
    generator._emit_dynamic_call(ast.parse("target()", mode="eval").body, callee)
    (call,) = (op for op in generator.current_ops if op.kind == "CALL_GUARDED")
    assert call.kind == "CALL_GUARDED"
    assert call.args == [callee]
    assert call.metadata == {"target": symbol}
    assert not any(
        op.kind in {"IS", "CALL", "INVOKE_FFI"} for op in generator.current_ops
    )


@pytest.mark.parametrize(
    "hint, expected_kind",
    [("Func:target", "CALL_GUARDED"), ("BoundMethod:Owner:method", "CALL_FUNC")],
)
def test_dynamic_callable_result_does_not_inherit_lexical_return_hint(
    hint: str, expected_kind: str
) -> None:
    generator = SimpleTIRGenerator()
    generator.funcs_map["target"] = {"return_hint": "int"}
    generator.classes["Owner"] = {"methods": {"method": {"return_hint": "int"}}}
    callee = MoltValue("actual_callable", type_hint=hint)
    result = generator._emit_dynamic_call(
        ast.parse("callee()", mode="eval").body, callee
    )
    assert result.type_hint == "Any"
    call = next(op for op in generator.current_ops if op.result is result)
    assert call.args == [callee]
    assert call.kind == expected_kind
    assert not any(op.kind.startswith("CALLARGS_") for op in generator.current_ops)


def test_function_import_transaction_uses_active_globals() -> None:
    generator = SimpleTIRGenerator(known_modules={"json"}, stdlib_allowlist={"json"})
    generator.visit(ast.parse("def probe():\n    import json\n    return json\n"))
    ops = next(
        function["ops"]
        for function in generator.to_json()["functions"]
        if function["name"] == "__main____probe"
    )
    active_globals = {
        op["out"]
        for op in ops
        if op.get("kind") == "call" and op.get("s_value") == "molt_globals_builtin"
    }
    transaction_funcs = {
        op["out"]
        for op in ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_transaction"
    }
    transactions = [
        op
        for op in ops
        if op.get("kind") == "call_func"
        and isinstance(op.get("args"), list)
        and len(op["args"]) == 6
        and op["args"][0] in transaction_funcs
    ]

    assert len(transactions) == 1
    assert transactions[0]["args"][2] in active_globals


@pytest.mark.parametrize("name", ["globals", "vars", "dir"])
def test_frame_builtin_references_share_named_callable_materialization(
    name: str,
) -> None:
    generator = SimpleTIRGenerator(module_name="frame_builtin")
    value = generator.visit(ast.Name(id=name, ctx=ast.Load()))
    op = next(op for op in generator.current_ops if op.result == value)
    assert op.kind == "BUILTIN_FUNC"
    assert op.args[:2] == [
        BUILTIN_FUNC_SPECS[name].runtime,
        _builtin_func_abi_arity(BUILTIN_FUNC_SPECS[name]),
    ]
    assert op.metadata["builtin_name"] == name
    assert not any(op.kind == "FUNC_NEW" for op in generator.current_ops)


def test_former_globals_wrapper_name_is_an_ordinary_user_frame() -> None:
    generator = SimpleTIRGenerator(module_name="user_globals")
    generator.visit(
        ast.parse("def __molt_globals_builtin__():\n    return globals()\n")
    )
    name = next(
        name for name in generator.funcs_map if "__molt_globals_builtin__" in name
    )
    assert generator._function_needs_frame_trace(name)


@pytest.mark.parametrize("name", ["print", "len", "abs", "sorted", "sum"])
def test_deferred_builtin_lookup_preserves_mutable_global_binding(name: str) -> None:
    ir = compile_to_tir(
        "def f(value):\n    for i in range(2):\n        value = value\n"
        f"    return {name}(value)\n"
    )
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____f")
    lookups = _module_attr_accesses(ops, "module_get_global", name)
    call = _positional_call(ops, lookups, 1)
    assert call["args"][1] in _local_reads(ops, "value")
    assert not any(
        op["kind"] == "builtin_func"
        and op.get("s_value") == BUILTIN_FUNC_SPECS[name].runtime
        for op in ops
    )


def _first_builtin_call_kind(source: str, runtime_name: str) -> str:
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    for idx, op in enumerate(main_ops):
        if op.get("kind") != "builtin_func" or op.get("s_value") != runtime_name:
            continue
        func_var = op.get("out")
        for call_op in main_ops[idx + 1 :]:
            if call_op.get("kind") not in {"call_func", "call_bind"}:
                continue
            args = call_op.get("args") or []
            if args and args[0] == func_var:
                return call_op["kind"]
    raise AssertionError(f"Did not find call for builtin {runtime_name}")


def _module_import_targets(main_ops: list[dict[str, object]]) -> set[str]:
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    targets: set[str] = set()
    for op in main_ops:
        if op.get("kind") != "module_import":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 1:
            continue
        name_var = args[0]
        if not isinstance(name_var, str):
            continue
        target = const_str.get(name_var)
        if isinstance(target, str):
            targets.add(target)
    return targets


def _importlib_transaction_targets(main_ops: list[dict[str, object]]) -> set[str]:
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    transaction_funcs = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_transaction"
        and isinstance(op.get("out"), str)
    }

    targets: set[str] = set()
    for op in main_ops:
        if op.get("kind") != "call_func":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 6:
            continue
        callee, name_var, _globals_var, _locals_var, _fromlist_var, _level_var = args
        if callee not in transaction_funcs:
            continue
        target = const_str.get(name_var)
        if isinstance(target, str):
            targets.add(target)
    return targets


def _importlib_import_module_targets(main_ops: list[dict[str, object]]) -> set[str]:
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    import_module_funcs = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_module"
        and isinstance(op.get("out"), str)
    }

    targets: set[str] = set()
    for op in main_ops:
        if op.get("kind") != "call_func":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 3:
            continue
        callee, name_var, _package_var = args
        if callee not in import_module_funcs:
            continue
        target = const_str.get(name_var)
        if isinstance(target, str):
            targets.add(target)
    return targets


def _import_transaction_details(
    main_ops: list[dict[str, object]],
) -> list[tuple[str, tuple[str, ...], int]]:
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    const_int = {
        op["out"]: op["value"]
        for op in main_ops
        if op.get("kind") == "const"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("value"), int)
    }
    tuple_items = {
        op["out"]: tuple(
            const_str[arg]
            for arg in (op.get("args") or [])
            if isinstance(arg, str) and arg in const_str
        )
        for op in main_ops
        if op.get("kind") == "tuple_new" and isinstance(op.get("out"), str)
    }
    transaction_funcs = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_transaction"
        and isinstance(op.get("out"), str)
    }

    details: list[tuple[str, tuple[str, ...], int]] = []
    for op in main_ops:
        if op.get("kind") != "call_func":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 6:
            continue
        callee, name_var, _globals_var, _locals_var, fromlist_var, level_var = args
        if callee not in transaction_funcs:
            continue
        if not isinstance(name_var, str) or not isinstance(level_var, str):
            continue
        target = const_str.get(name_var)
        level = const_int.get(level_var)
        fromlist = tuple_items.get(fromlist_var)
        if isinstance(target, str) and isinstance(level, int) and fromlist is not None:
            details.append((target, fromlist, level))
    return details


def _module_get_attr_names(main_ops: list[dict[str, object]]) -> set[str]:
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    attrs: set[str] = set()
    for op in main_ops:
        if op.get("kind") != "module_get_attr":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 2:
            continue
        name_var = args[1]
        if isinstance(name_var, str) and isinstance(const_str.get(name_var), str):
            attrs.add(const_str[name_var])
    return attrs


def _module_attr_accesses(
    main_ops: list[dict[str, object]], kind: str, attr_name: str
) -> list[str]:
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    outs: list[str] = []
    for op in main_ops:
        if op.get("kind") != kind:
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) < 2:
            continue
        name_var = args[1]
        out = op.get("out")
        if (
            isinstance(name_var, str)
            and const_str.get(name_var) == attr_name
            and isinstance(out, str)
        ):
            outs.append(out)
    return outs


def _local_import_reads(
    ops: list[dict[str, object]],
    name: str,
    imported_values: set[str],
) -> set[str]:
    """Follow the straight-line fixture's imported value through local storage."""
    reads: set[str] = set()
    stored: object = None
    for op in ops:
        if op.get("var") != name:
            continue
        if op.get("kind") == "store_var":
            stored = op["args"][0]
        elif op.get("kind") == "load_var" and stored in imported_values:
            reads.add(op["out"])
    return reads


def _local_reads(ops: list[dict[str, object]], name: str) -> set[str]:
    return {name} | {
        op["out"]
        for op in ops
        if op.get("kind") == "load_var" and op.get("var") == name
    }


def _positional_call(
    ops: list[dict[str, object]],
    targets: list[str] | set[str],
    arity: int,
    *,
    parameters: list[str] | tuple[str, ...] = (),
) -> dict[str, object]:
    """Require one call of the actual callable, without argument-builder transport."""
    assert targets
    (call,) = (
        op for op in ops if op.get("kind") == "call_func" and op["args"][0] in targets
    )
    assert len(call["args"]) == arity + 1
    if call["args"][0] in parameters:
        callee_index = 0
    else:
        callee_index = next(
            index for index, op in enumerate(ops) if op.get("out") == call["args"][0]
        )
    # Scope this to the source call: other calls in a full module fixture may
    # legitimately need keyword/star argument assembly.
    assert not any(
        op["kind"].startswith("callargs_") for op in ops[callee_index : ops.index(call)]
    )
    return call


def _bound_positional_args(
    ops: list[dict[str, object]], targets: set[str]
) -> list[str]:
    """Retained binders must carry only supplied positionals of the actual callee."""
    assert targets
    (call,) = (
        op for op in ops if op.get("kind") == "call_bind" and op["args"][0] in targets
    )
    assert len(call["args"]) == 2
    builder = call["args"][1]
    assert any(op["kind"] == "callargs_new" and op.get("out") == builder for op in ops)
    pushes = [
        op
        for op in ops[: ops.index(call)]
        if op["kind"].startswith("callargs_")
        and op.get("args")
        and op["args"][0] == builder
    ]
    assert all(op["kind"] == "callargs_push_pos" for op in pushes)
    return [op["args"][1] for op in pushes]


def _importlib_literal_main_ops(source: str) -> list[dict[str, object]]:
    gen = SimpleTIRGenerator(
        known_modules={"importlib", "json"},
        stdlib_allowlist={"importlib", "json"},
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    return next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")


def _importlib_literal_function_ops(
    source: str, func_name: str
) -> list[dict[str, object]]:
    gen = SimpleTIRGenerator(
        known_modules={"importlib", "json"},
        stdlib_allowlist={"importlib", "json"},
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    return next(func["ops"] for func in ir["functions"] if func["name"] == func_name)


def test_unknown_tobytes_with_order_stays_dynamic_method_call() -> None:
    gen = SimpleTIRGenerator(module_name="numpy_format_probe")
    gen.visit(ast.parse("def write(chunk):\n    return chunk.tobytes('C')\n"))
    ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "numpy_format_probe__write"
    )

    (attribute,) = (
        op
        for op in ops
        if op.get("kind") == "get_attr_generic_obj" and op.get("s_value") == "tobytes"
    )
    assert attribute["args"][0] in _local_reads(ops, "chunk")
    call = _positional_call(ops, {attribute["out"]}, 1)
    assert any(
        op.get("kind") == "const_str"
        and op.get("s_value") == "C"
        and op.get("out") == call["args"][1]
        for op in ops
    )
    assert not any(op["kind"].startswith("callargs_") for op in ops)
    assert all(op.get("kind") != "memoryview_tobytes" for op in ops)


def _has_static_call(main_ops: list[dict[str, object]], symbol: str) -> bool:
    return any(
        op.get("kind") == "call" and op.get("s_value") == symbol for op in main_ops
    )


def _has_builtin_func(source: str, runtime_name: str) -> bool:
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    return any(
        op.get("kind") == "builtin_func" and op.get("s_value") == runtime_name
        for op in main_ops
    )


def _has_runtime_intrinsic_lookup_call(source: str, runtime_name: str) -> bool:
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    return _ops_have_runtime_intrinsic_lookup_call([main_ops], runtime_name)


def _ops_have_runtime_intrinsic_lookup_call(
    op_groups: list[list[dict[str, object] | MoltOp]], runtime_name: str
) -> bool:
    const_str = {
        _op_field(op, "out"): _op_field(op, "s_value")
        for ops in op_groups
        for op in ops
        if _op_field(op, "kind") == "const_str"
        and isinstance(_op_field(op, "out"), str)
        and isinstance(_op_field(op, "s_value"), str)
    }
    resolver_vars = {
        _op_field(op, "out")
        for ops in op_groups
        for op in ops
        if _op_field(op, "kind") == "builtin_func"
        and _op_field(op, "s_value") == "molt_require_intrinsic_runtime"
        and isinstance(_op_field(op, "out"), str)
    }
    for ops in op_groups:
        for op in ops:
            if _op_field(op, "kind") != "call_func":
                continue
            args = _op_field(op, "args")
            if not isinstance(args, list) or len(args) < 3:
                continue
            callee_var = _value_name(args[0])
            name_var = _value_name(args[1])
            if callee_var in resolver_vars and const_str.get(name_var) == runtime_name:
                return True
    return False


def test_deferred_builtin_import_alias_keeps_live_callable_dispatch() -> None:
    ir = compile_to_tir(
        "from builtins import float as _float\n"
        "def f(value):\n"
        "    return _float(value)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    # A module alias can change before f runs, and value.__float__ can execute
    # Python. Neither the import spelling nor the annotation owns this call.
    assert any(op.get("kind") == "call_func" for op in func_ops), func_ops
    assert not any(op.get("kind") == "float_from_obj" for op in func_ops)


@pytest.mark.parametrize("arguments, arity", [("", 0), ("i", 1), ("i, i + 1", 2)])
def test_deferred_exception_constructor_calls_actual_class(
    arguments: str, arity: int
) -> None:
    ir = compile_to_tir(f"def f(i):\n    return ValueError({arguments})\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    call = _positional_call(
        func_ops,
        _module_attr_accesses(func_ops, "module_get_global", "ValueError"),
        arity,
    )
    if arity:
        assert call["args"][1] in _local_reads(func_ops, "i")
    if arity == 2:
        addition = next(op for op in func_ops if op.get("out") == call["args"][2])
        assert addition["kind"] == "add"
        assert addition["args"][0] in _local_reads(func_ops, "i")
    assert not any(op.get("kind") == "tuple_new" for op in func_ops)
    assert not any(op["kind"].startswith("exception_new") for op in func_ops)


def test_sync_try_except_uses_split_label_valued_handler_entry() -> None:
    source = (
        "def f(i):\n"
        "    try:\n"
        "        if i:\n"
        "            raise ValueError(i)\n"
        "    except ValueError as e:\n"
        "        return int(str(e))\n"
        "    else:\n"
        "        return 0\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    try_start = next(op for op in func_ops if op.get("kind") == "try_start")
    handler_label = try_start["value"]
    assert isinstance(handler_label, int)
    assert any(
        op.get("kind") == "try_end" and op.get("value") == handler_label
        for op in func_ops
    )

    raise_idx = next(i for i, op in enumerate(func_ops) if op.get("kind") == "raise")
    next_control = next(
        op for op in func_ops[raise_idx + 1 :] if op.get("kind") != "line"
    )
    assert next_control["kind"] == "jump"
    assert next_control["value"] == handler_label

    handler_idx = next(
        i
        for i, op in enumerate(func_ops)
        if op.get("kind") == "label" and op.get("value") == handler_label
    )
    exception_last_idx = next(
        i
        for i, op in enumerate(func_ops[handler_idx + 1 :], start=handler_idx + 1)
        if op.get("kind") == "exception_last_pending"
    )
    normal_label = next(
        op["value"]
        for op in func_ops
        if op.get("kind") == "jump"
        and isinstance(op.get("value"), int)
        and op.get("value") != handler_label
    )
    normal_idx = next(
        i
        for i, op in enumerate(func_ops)
        if op.get("kind") == "label" and op.get("value") == normal_label
    )
    assert handler_idx < exception_last_idx < normal_idx
    match_ops = [op for op in func_ops if op.get("kind") == "exception_match_builtin"]
    assert match_ops
    assert match_ops[0]["s_value"] == "ValueError"
    assert match_ops[0]["value"] == 5
    assert not any(op.get("kind") == "exception_class" for op in func_ops)
    assert not any(op.get("kind") == "context_depth" for op in func_ops)
    assert not any(op.get("kind") == "context_unwind_to" for op in func_ops)


def test_async_with_serializes_metadata_region_as_label_valued_handler_entry() -> None:
    source = "async def f(cm):\n    async with cm:\n        return 1\n"
    ir = compile_to_tir(source)
    poll_ops = next(
        func["ops"] for func in ir["functions"] if func["name"].endswith("__f_poll")
    )
    starts = [op for op in poll_ops if op.get("kind") == "try_start"]
    assert starts
    labels = {op.get("value") for op in starts}
    assert None not in labels
    assert all(isinstance(label, int) for label in labels)
    assert labels.issubset(
        {op.get("value") for op in poll_ops if op.get("kind") == "try_end"}
    )


def test_try_finally_uses_finally_pending_observer_not_handler_match_ref() -> None:
    source = (
        "def f(i):\n"
        "    try:\n"
        "        if i:\n"
        "            raise ValueError(i)\n"
        "    finally:\n"
        "        i = i + 1\n"
        "    return i\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(
        op.get("kind") == "exception_finally_pending_observer" for op in func_ops
    )
    assert not any(op.get("kind") == "exception_last_pending" for op in func_ops)


def test_try_except_finally_splits_handler_match_ref_from_finally_observer() -> None:
    source = (
        "def f(i):\n"
        "    try:\n"
        "        if i:\n"
        "            raise ValueError(i)\n"
        "    except ValueError:\n"
        "        i = 2\n"
        "    finally:\n"
        "        i = i + 1\n"
        "    return i\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "exception_last_pending" for op in func_ops)
    assert any(
        op.get("kind") == "exception_finally_pending_observer" for op in func_ops
    )


def test_module_try_except_assignments_use_module_storage_after_join() -> None:
    source = (
        "try:\n"
        "    raise ModuleNotFoundError('x')\n"
        "except ModuleNotFoundError:\n"
        "    flag = True\n"
        "else:\n"
        "    flag = False\n"
        "print(flag)\n"
    )
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    assert len(_module_attr_accesses(main_ops, "module_set_attr", "flag")) == 2
    flag_loads = _module_attr_accesses(main_ops, "module_get_global", "flag")
    call = _positional_call(
        main_ops, _module_attr_accesses(main_ops, "module_get_global", "print"), 1
    )
    assert call["args"][1] in flag_loads


@pytest.mark.parametrize(
    "body",
    ["flag = 1", "flag: int = 1", "flag += 1", "flag, other = (1, 2)"],
)
@pytest.mark.parametrize(
    "flow",
    [
        "if input():\n{body}\n",
        "while input():\n{body}\n    break\n",
        "for item in input():\n{body}\n",
        "try:\n{body}\nexcept Exception:\n    pass\n",
    ],
)
def test_module_binding_has_one_publication_per_assignment(
    body: str, flow: str
) -> None:
    ir = compile_to_tir("flag = 0\n" + flow.format(body=indent(body, "    ")))
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    # One initial assignment and one body assignment: control-flow promotion
    # must not re-publish the initial value, nor may storage repeat the body write.
    assert len(_module_attr_accesses(ops, "module_set_attr", "flag")) == 2


@pytest.mark.parametrize(
    "body", ["flag = 1", "flag: int = 1", "flag += 1", "flag, other = (1, 2)"]
)
def test_function_global_binding_has_one_publication_per_assignment(body: str) -> None:
    ir = compile_to_tir("def f():\n    global flag\n" + indent(body, "    ") + "\n")
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "__main____f")
    active_globals = {
        op["out"]
        for op in ops
        if op.get("kind") == "call" and op.get("s_value") == "molt_globals_builtin"
    }
    names = {
        op["out"]
        for op in ops
        if op.get("kind") == "const_str" and op.get("s_value") == "flag"
    }
    stores = [
        op
        for op in ops
        if op.get("kind") == "dict_set"
        and isinstance(op.get("args"), list)
        and len(op["args"]) == 3
        and op["args"][1] in names
    ]
    assert len(stores) == 1
    assert stores[0]["args"][0] in active_globals
    assert not _module_attr_accesses(ops, "module_set_attr", "flag")


@pytest.mark.parametrize(
    "definition",
    [
        "def flag():\n    return 1",
        "def flag():\n    yield 1",
        "async def flag():\n    return 1",
        "async def flag():\n    yield 1",
        "class flag:\n    pass",
    ],
)
def test_control_flow_definition_has_one_namespace_publication(definition: str) -> None:
    source = "try:\n" + indent(definition, "    ") + "\nexcept Exception:\n    pass\n"
    ir = compile_to_tir(source)
    ops = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    assert len(_module_attr_accesses(ops, "module_set_attr", "flag")) == 1


@pytest.mark.parametrize("deferred", [False, True])
def test_module_control_flow_promotion_flushes_only_unpublished_values(
    deferred: bool,
) -> None:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(""))
    generator.defer_module_attrs = deferred
    flag = MoltValue(generator.next_var(), type_hint="int")
    generator.emit(MoltOp(kind="CONST", args=[7], result=flag))
    generator._store_local_value("flag", flag, publish_module=True)
    generator.globals["flag"] = flag
    generator._prepare_mutable_control_flow_bindings({"flag"})
    generator._prepare_mutable_control_flow_bindings({"flag"})
    generator._flush_deferred_module_attrs()
    ops = next(
        fn["ops"]
        for fn in generator.to_json()["functions"]
        if fn["name"] == "molt_main"
    )
    assert len(_module_attr_accesses(ops, "module_set_attr", "flag")) == 1
    assert "flag" not in generator.deferred_module_attrs
    assert "flag" not in generator.locals
    assert generator._module_attr_type_hints["flag"] == "int"


@pytest.mark.parametrize(
    "expression",
    [
        "print()",
        "print(1)",
        "print(1, 2)",
        "print(1, end='')",
        "print(*(1, 2))",
        "print(**{})",
    ],
)
def test_print_value_uses_shared_builtin_call_authority(expression: str) -> None:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(f"result = [{expression}]\n"))
    ops = generator.funcs_map["molt_main"]["ops"]
    calls = [op for op in ops if op.kind == "CALL_BIND"]
    assert len(calls) == 1
    assert calls[0].result.type_hint == "None"
    assert not {"PRINT", "PRINT_NEWLINE", "STRING_JOIN"}.intersection(
        op.kind for op in ops
    )
    assert any(op.kind == "LIST_NEW" and calls[0].result in op.args for op in ops)


def test_sync_try_except_uses_explicit_exit_when_body_enters_with() -> None:
    source = (
        "def f(p):\n"
        "    try:\n"
        "        with open(p) as fp:\n"
        "            raise ValueError(1)\n"
        "    except ValueError:\n"
        "        return 1\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "context_exit" for op in func_ops)
    assert not {"context_depth", "context_unwind", "context_unwind_to"}.intersection(
        op.get("kind") for op in func_ops
    )


def test_break_inside_try_except_keeps_enclosing_with_until_loop_end() -> None:
    source = (
        "class C:\n"
        "    def __enter__(self):\n"
        "        return self\n"
        "    def __exit__(self, exc_type, exc, tb):\n"
        "        return False\n"
        "\n"
        "def f():\n"
        "    with C():\n"
        "        for item in [1]:\n"
        "            try:\n"
        "                raise ValueError(item)\n"
        "            except ValueError:\n"
        "                break\n"
        "    return 1\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    loop_end = next(i for i, op in enumerate(func_ops) if op.get("kind") == "loop_end")
    first_exit = next(
        i for i, op in enumerate(func_ops) if op.get("kind") == "context_exit"
    )
    assert loop_end < first_exit
    assert not {"context_depth", "context_unwind", "context_unwind_to"}.intersection(
        op.get("kind") for op in func_ops
    )


def test_return_inside_try_nested_in_with_uses_explicit_scope_exit() -> None:
    source = (
        "class C:\n"
        "    def __enter__(self):\n"
        "        return self\n"
        "    def __exit__(self, exc_type, exc, tb):\n"
        "        return False\n"
        "\n"
        "def f(flag):\n"
        "    with C():\n"
        "        try:\n"
        "            if flag:\n"
        "                return 1\n"
        "        except ValueError:\n"
        "            return 2\n"
        "    return 3\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "context_exit" for op in func_ops)
    assert not {"context_depth", "context_unwind", "context_unwind_to"}.intersection(
        op.get("kind") for op in func_ops
    )


def test_sync_try_except_splits_clean_and_pending_cleanup_lanes() -> None:
    source = (
        "def f(i):\n"
        "    total = 0\n"
        "    try:\n"
        "        if i:\n"
        "            raise ValueError(i)\n"
        "        total += i\n"
        "    except ValueError as e:\n"
        "        total += int(str(e))\n"
        "    return total\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    pop_indices = [
        idx for idx, op in enumerate(func_ops) if op.get("kind") == "exception_pop"
    ]

    assert len(pop_indices) >= 2
    assert any(func_ops[idx + 1].get("kind") == "jump" for idx in pop_indices)
    assert any(
        func_ops[idx + 1].get("kind") == "check_exception" for idx in pop_indices
    )


def test_module_exception_binding_cleanup_uses_safe_delete_primitive() -> None:
    source = (
        "try:\n"
        "    raise ValueError('x')\n"
        "except ValueError as exc:\n"
        "    print(exc)\n"
        "print('done')\n"
    )
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    kinds = [op.get("kind") for op in main_ops]

    assert "module_del_global_if_present" in kinds
    assert "exception_kind" not in kinds
    assert "exception_set_last" not in kinds


def test_zip_lowering_uses_call_bind() -> None:
    source = "print(list(zip([1, 2], [3, 4])))\n"
    assert _first_builtin_call_kind(source, "molt_zip_builtin") == "call_bind"


def test_map_lowering_uses_call_bind() -> None:
    source = "print(list(map(lambda x: x + 1, [1, 2, 3])))\n"
    assert _first_builtin_call_kind(source, "molt_map_builtin") == "call_bind"


def test_local_require_builtin_intrinsic_wrapper_lowers_known_intrinsic() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "_NS = globals()\n"
        "def _require_builtin_intrinsic(name: str) -> object:\n"
        "    return _require_intrinsic(name, _NS)\n"
        "_HOOK = _require_builtin_intrinsic('molt_asyncgen_hooks_get')\n"
    )
    assert _has_builtin_func(source, "molt_asyncgen_hooks_get")


def test_local_warnings_intrinsic_wrapper_lowers_known_intrinsic() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "def _warnings_intrinsic(name: str) -> object:\n"
        "    return _require_intrinsic(name)\n"
        "_HOOK = _warnings_intrinsic('molt_getargv')\n"
    )
    assert _has_builtin_func(source, "molt_getargv")


def test_local_callable_guard_intrinsic_wrapper_lowers_known_intrinsic() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "def _require_callable_intrinsic(name: str):\n"
        "    value = _require_intrinsic(name)\n"
        "    if not callable(value):\n"
        "        raise RuntimeError(f'{name} intrinsic unavailable')\n"
        "    return value\n"
        "_HOOK = _require_callable_intrinsic('molt_gc_collect')\n"
    )
    assert _has_builtin_func(source, "molt_gc_collect")


@pytest.mark.parametrize("local", [False, True])
def test_local_inner_import_intrinsic_wrapper_lowers_known_intrinsic(
    local: bool,
) -> None:
    source = (
        "def _require_intrinsic(name: str, namespace: dict[str, object] | None = None):\n"
        "    from _intrinsics import require_intrinsic as _require\n"
        "    return _require(name, namespace)\n"
        "_HOOK = _require_intrinsic('molt_importlib_module_spec_is_package')\n"
    )
    if local:
        source = "def probe():\n" + indent(source, "    ")
    ir = compile_to_tir(source)
    ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == ("__main____probe" if local else "molt_main")
    )
    assert (
        any(
            op.get("kind") == "builtin_func"
            and op.get("s_value") == "molt_importlib_module_spec_is_package"
            for op in ops
        )
        is local
    )
    if not local:
        # Eager annotation callbacks can populate the definition's module
        # target; releasing that old binding can replace the new wrapper.
        targets = _module_attr_accesses(ops, "module_get_global", "_require_intrinsic")
        call = _positional_call(ops, targets, 1)
        assert any(
            op.get("kind") == "const_str"
            and op.get("s_value") == "molt_importlib_module_spec_is_package"
            and op.get("out") == call["args"][1]
            for op in ops
        )


def test_intrinsic_require_lowers_to_public_runtime_symbol() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "_HOOK = _require_intrinsic('molt_async_sleep')\n"
    )
    assert not _has_runtime_intrinsic_lookup_call(source, "molt_async_sleep")
    assert _has_builtin_func(source, "molt_async_sleep")
    assert _has_builtin_func(source, "molt_require_intrinsic_runtime")


@pytest.mark.parametrize("name", ["globals", "locals", "vars", "__import__"])
@pytest.mark.parametrize("chunked", [False, True])
def test_invalidated_builtin_acquisition_keeps_mutable_lookup_name(
    name: str, chunked: bool
) -> None:
    gen = SimpleTIRGenerator(
        module_name="builtin_lookup_probe",
        module_chunking=chunked,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse(f"import unknown_module\nvalue = {name}\n"))
    ir = gen.to_json()
    lookups = []
    for function in ir["functions"]:
        ops = function["ops"]
        constants = {
            op["out"]: op["s_value"] for op in ops if op["kind"] == "const_str"
        }
        lookups.extend(
            op
            for op in ops
            if op["kind"] == "module_get_global"
            and constants.get(op["args"][1]) == name
        )
    assert lookups
    assert all("runtime_symbol" not in op for op in lookups), (
        "possible fallback is not exact callable provenance"
    )


@pytest.mark.parametrize(
    "runtime_name",
    [
        "molt_type_of_borrowed",
        "molt_dict_getitem_borrowed",
        "molt_list_getitem_borrowed",
        "molt_tuple_getitem_borrowed",
    ],
)
def test_raw_borrowed_intrinsic_cannot_be_published_as_callable(
    runtime_name: str,
) -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        f"_HOOK = _require_intrinsic({runtime_name!r})\n"
    )
    with pytest.raises(CompatibilityError, match="raw non-callable ABI"):
        compile_to_tir(source)


def test_chunked_stdlib_intrinsics_import_binding_survives_reset() -> None:
    source_path = (
        Path(__file__).resolve().parents[1]
        / "src"
        / "molt"
        / "stdlib"
        / "json"
        / "__init__.py"
    )
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "_HOOK = _require_intrinsic('molt_json_parse_scalar_obj')\n"
    )
    gen = SimpleTIRGenerator(
        module_name="json",
        source_path=str(source_path),
        entry_module="json",
        module_chunking=True,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse(source))
    assert gen.global_imported_names["_require_intrinsic"] == "_intrinsics"
    op_groups = [func["ops"] for func in gen.funcs_map.values()]
    assert not _ops_have_runtime_intrinsic_lookup_call(
        op_groups, "molt_json_parse_scalar_obj"
    )
    assert any(
        _op_field(op, "kind") == "builtin_func"
        and _op_field(op, "s_value") == "molt_json_parse_scalar_obj"
        for ops in op_groups
        for op in ops
    )


def test_chunked_stdlib_intrinsics_value_binding_uses_runtime_require_intrinsic() -> (
    None
):
    source_path = (
        Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib" / "sys.py"
    )
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "def _safe(name, _ri=_require_intrinsic):\n"
        "    return _ri\n"
    )
    gen = SimpleTIRGenerator(
        module_name="sys",
        source_path=str(source_path),
        entry_module="sys",
        module_chunking=True,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse(source))
    assert gen.global_imported_names["_require_intrinsic"] == "_intrinsics"
    assert any(
        op.kind == "BUILTIN_FUNC" and op.args[0] == "molt_require_intrinsic_runtime"
        for func in gen.funcs_map.values()
        for op in func["ops"]
    )


def test_module_chunking_starts_new_chunk_before_large_top_level_statement() -> None:
    source = (
        "seed = 0\n"
        "limit = 1\n"
        "if seed:\n"
        + "".join(f"    limit = limit + {i}\n" for i in range(60))
        + "else:\n"
        "    limit = limit + 1\n"
    )
    gen = SimpleTIRGenerator(
        module_name="chunk_probe",
        module_chunking=True,
        module_chunk_max_ops=120,
    )
    gen.visit(ast.parse(source))

    assert len(gen.module_chunk_symbols) >= 2
    first_chunk = gen.funcs_map[gen.module_chunk_symbols[0]]["ops"]
    second_chunk = gen.funcs_map[gen.module_chunk_symbols[1]]["ops"]

    assert len(first_chunk) < 120
    assert [3] not in [op.args for op in first_chunk if op.kind == "LINE"]
    assert [3] in [op.args for op in second_chunk if op.kind == "LINE"]


def test_module_chunking_starts_new_chunk_before_large_multiline_assignment() -> None:
    entries = "".join(f"    {i}: {i},\n" for i in range(80))
    source = f"seed = 0\nlimit = 1\ntable = {{\n{entries}}}\n"
    gen = SimpleTIRGenerator(
        module_name="chunk_probe_assign",
        module_chunking=True,
        module_chunk_max_ops=120,
    )
    gen.visit(ast.parse(source))

    assert len(gen.module_chunk_symbols) >= 2
    first_chunk = gen.funcs_map[gen.module_chunk_symbols[0]]["ops"]
    second_chunk = gen.funcs_map[gen.module_chunk_symbols[1]]["ops"]

    assert len(first_chunk) < 120
    assert [3] not in [op.args for op in first_chunk if op.kind == "LINE"]
    assert [3] in [op.args for op in second_chunk if op.kind == "LINE"]


def test_module_chunk_failure_cleanup_is_not_success_fallthrough() -> None:
    gen = SimpleTIRGenerator(
        module_name="chunk_cleanup",
        entry_module="chunk_cleanup",
        module_chunking=True,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse("first = 1\nsecond = 2\n"))
    ir = gen.to_json()
    checked_names = {"molt_main", *gen.module_chunk_symbols}

    for func in ir["functions"]:
        if func["name"] not in checked_names:
            continue
        ops = func["ops"]
        cache_del_index = next(
            idx for idx, op in enumerate(ops) if op.get("kind") == "module_cache_del"
        )
        ret_void_index = next(
            idx for idx, op in enumerate(ops) if op.get("kind") == "ret_void"
        )
        assert ret_void_index < cache_del_index, func["name"]
        assert all(
            op.get("kind") != "exception_last_pending"
            for op in ops[ret_void_index + 1 : cache_del_index]
        ), func["name"]


def test_function_metadata_uses_runtime_helper_instead_of_attr_storm() -> None:
    source = (
        "class Box:\n"
        "    def first(self, x, y=1):\n"
        "        return x + y\n"
        "    def second(self):\n"
        "        return 2\n"
    )
    gen = SimpleTIRGenerator(
        module_name="meta_probe",
        module_chunking=True,
        module_chunk_max_ops=400,
    )
    gen.visit(ast.parse(source))

    ops = [op for func in gen.funcs_map.values() for op in func["ops"]]
    assert any(
        op.kind == "CALL"
        and op.args
        and op.args[0] == "molt_function_init_metadata_packed"
        for op in ops
    )
    assert not any(
        op.kind == "BUILTIN_FUNC"
        and op.args == ["molt_function_init_metadata_packed", 4]
        for op in ops
    )
    metadata_attrs = {
        "__name__",
        "__qualname__",
        "__module__",
        "__molt_arg_names__",
        "__molt_posonly__",
        "__molt_kwonly_names__",
        "__molt_vararg__",
        "__molt_varkw__",
        "__defaults__",
        "__kwdefaults__",
        "__doc__",
        "__code__",
        "__molt_bind_kind__",
        "__molt_is_generator__",
        "__molt_is_coroutine__",
        "__molt_is_async_generator__",
        "__molt_closure_size__",
    }
    assert not any(
        op.kind == "SETATTR_GENERIC_OBJ"
        and len(op.args) >= 2
        and isinstance(op.args[1], str)
        and op.args[1] in metadata_attrs
        for op in ops
    )


@pytest.mark.parametrize(
    ("definition", "expected_kind"),
    [
        ("def probe(self=None):\n    return 1\n", 0),
        ("def probe(self=None):\n    yield 1\n", 1),
        ("async def probe(self=None):\n    return 1\n", 2),
        ("async def probe(self=None):\n    yield 1\n", 3),
    ],
)
@pytest.mark.parametrize("scope", ["module", "nested", "method"])
def test_function_metadata_publishes_typed_execution_kind(
    definition: str, expected_kind: int, scope: str
) -> None:
    source = definition
    if scope == "nested":
        source = "def outer(captured):\n" + indent(
            definition.replace("return 1", "return captured").replace(
                "yield 1", "yield captured"
            ),
            "    ",
        )
    elif scope == "method":
        source = "class Box:\n" + indent(definition, "    ")
    gen = SimpleTIRGenerator(module_name="metadata_kind")
    gen.visit(ast.parse(source))
    observed = []
    definitions = []
    forbidden_markers = {
        "__molt_is_generator__",
        "__molt_is_coroutine__",
        "__molt_is_async_generator__",
        "__molt_closure_size__",
    }
    for func in gen.funcs_map.values():
        producers = {}
        for op in func["ops"]:
            if (
                op.kind == "CALL"
                and op.args
                and op.args[0] == "molt_function_init_metadata_packed"
            ):
                # The callable ABI stays four arguments; only the one packed
                # schema changes, with no legacy compatibility branch.
                assert len(op.args) == 5
                packed = producers[op.args[2].name]
                assert packed.kind == "TUPLE_NEW"
                assert len(packed.args) == 14
                qualname = producers[packed.args[1].name].args[0]
                kind = producers[packed.args[11].name]
                assert producers[packed.args[12].name].kind == "TUPLE_NEW"
                assert producers[packed.args[13].name].kind == "TUPLE_NEW"
                assert kind.kind == "CONST"
                if qualname == "probe" or qualname.endswith(".probe"):
                    observed.append(kind.args[0])
                    function_def = producers[op.args[1].name]
                    assert function_def.kind in {"FUNC_NEW", "FUNC_NEW_CLOSURE"}
                    (code_slot,) = (
                        candidate
                        for candidate in func["ops"]
                        if candidate.kind == "CODE_SLOT_SET"
                        and candidate.args[0] is op.args[3]
                    )
                    assert (
                        code_slot.metadata["code_id"]
                        == gen.func_code_ids[function_def.args[0]]
                    )
                    definitions.append(function_def)
            assert not (
                op.kind == "SETATTR_GENERIC_OBJ"
                and len(op.args) >= 2
                and isinstance(op.args[1], str)
                and op.args[1] in forbidden_markers
            )
            if op.result is not None:
                producers[op.result.name] = op
    assert observed == [expected_kind]
    assert len(definitions) == 1
    definition_op = definitions[0]
    body = gen.funcs_map[definition_op.args[0]]
    frame_plan = body.get("stateful_frame_plan")
    assert (frame_plan is not None) == (expected_kind != 0)
    if frame_plan is not None:
        assert frame_plan.poll_symbol == definition_op.args[0]
        assert (
            frame_plan.callable_task_metadata(0)["task_kind"]
            == {
                1: "generator",
                2: "coroutine",
                3: "async_generator",
            }[expected_kind]
        )
    serialized = [
        op
        for func in gen.to_json()["functions"]
        for op in func["ops"]
        if op.get("kind") in {"func_new", "func_new_closure"}
        and op.get("s_value") == definition_op.args[0]
    ]
    assert len(serialized) == 1
    entry = serialized[0]
    if expected_kind == 0:
        assert "task_kind" not in entry
        assert "task_closure_size" not in entry
    else:
        assert (
            entry["task_kind"]
            == {
                1: "generator",
                2: "coroutine",
                3: "async_generator",
            }[expected_kind]
        )
        assert isinstance(entry["task_closure_size"], int)
        assert entry["task_closure_size"] >= 0
        assert entry["task_closure_size"] % 8 == 0
        assert entry["task_closure_size"] == definition_op.metadata["task_closure_size"]


def test_generator_lambda_definition_publishes_final_task_layout() -> None:
    gen = SimpleTIRGenerator(module_name="metadata_lambda")
    gen.visit(ast.parse("def outer(captured):\n    return lambda: (yield captured)\n"))
    ir = gen.to_json()
    definitions = [
        op
        for func in ir["functions"]
        for op in func["ops"]
        if op.get("kind") == "func_new_closure" and op.get("task_kind") == "generator"
    ]
    assert len(definitions) == 1
    definition = definitions[0]
    assert gen.funcs_map[definition["s_value"]]["stateful_frame_plan"].has_closure
    poll = next(
        func for func in ir["functions"] if func["name"] == definition["s_value"]
    )
    offsets = [
        op["value"]
        for op in poll["ops"]
        if op.get("kind") in {"closure_store", "closure_load"}
        and isinstance(op.get("value"), int)
    ]
    assert offsets
    assert definition["task_closure_size"] >= max(offsets) + 8


def test_stateful_scope_and_alias_hints_use_frame_plan_not_symbol_spelling() -> None:
    from molt.frontend._types import GEN_CONTROL_SIZE
    from molt.frontend.sema.funcmeta import FunctionKind, stateful_function_frame_plan

    gen = SimpleTIRGenerator(module_name="opaque_scope")
    gen.start_function("ordinary_poll", needs_return_slot=True)
    assert not gen.is_async()
    assert gen.return_slot is None
    for kind in (
        FunctionKind.GENERATOR,
        FunctionKind.ASYNC,
        FunctionKind.ASYNC_GENERATOR,
    ):
        target = f"opaque_body_{kind.value}"
        plan = stateful_function_frame_plan(
            kind=kind,
            poll_symbol=target,
            param_count=0,
            has_closure=False,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        gen.start_function(
            target,
            params=["self"],
            compiler_params={"self"},
            stateful_frame_plan=plan,
            needs_return_slot=True,
        )
        assert gen.is_async()
        assert gen.return_slot is not None
        assert gen.funcs_map[target]["stateful_frame_plan"] is plan
        hint = plan.function_type_hint(64)
        gen.locals["source"] = MoltValue("source", type_hint=hint)
        alias = MoltValue("alias", type_hint="Any")
        gen._propagate_func_type_hint(alias, ast.Name(id="source", ctx=ast.Load()))
        assert alias.type_hint == hint
        gen.current_func_name = "ordinary_poll"
        assert not gen.is_async()
        gen.current_func_name = target
        assert gen.is_async()


@pytest.mark.parametrize(
    "source",
    [
        "values = (value for value in (1, 2))\n",
        "def outer(captured):\n    return (captured + value for value in (1, 2))\n",
        "async def outer(values):\n    return (value async for value in values)\n",
    ],
)
def test_generator_expression_uses_shared_callable_and_code_authority(
    source: str,
) -> None:
    gen = SimpleTIRGenerator(module_name="genexpr_scope")
    gen.visit(ast.parse(source))
    plans = [
        func["stateful_frame_plan"]
        for func in gen.funcs_map.values()
        if "stateful_frame_plan" in func
    ]
    assert plans
    ops = [op for func in gen.funcs_map.values() for op in func["ops"]]
    assert not any(
        op.kind in {"ALLOC_TASK", "ASYNCGEN_NEW", "FN_PTR_CODE_SET"} for op in ops
    )
    ir = gen.to_json()
    for plan in plans:
        assert plan.param_count == 1
        (definition,) = (
            op
            for op in ops
            if op.kind in {"FUNC_NEW", "FUNC_NEW_CLOSURE"}
            and op.args[0] == plan.poll_symbol
        )
        assert (
            definition.metadata["task_kind"]
            == plan.callable_task_metadata(0)["task_kind"]
        )
        code_id = gen.func_code_ids[plan.poll_symbol]
        assert any(
            op.kind == "CODE_SLOT_SET" and op.metadata["code_id"] == code_id
            for op in ops
        )
        body = next(
            func for func in ir["functions"] if func["name"] == plan.poll_symbol
        )
        assert body["ops"][0] == {"kind": "trace_enter_slot", "value": code_id}


@pytest.mark.parametrize(
    "metadata",
    [
        {"task_kind": "generator"},
        {"task_closure_size": 16},
        {"task_kind": "unknown", "task_closure_size": 16},
        {"task_kind": "generator", "task_closure_size": True},
        {"task_kind": "generator", "task_closure_size": -1},
    ],
)
def test_function_task_definition_requires_complete_typed_layout(
    metadata: dict,
) -> None:
    gen = SimpleTIRGenerator()
    definition = MoltOp(
        kind="FUNC_NEW",
        args=["probe", 0],
        result=MoltValue("function"),
        metadata=metadata,
    )
    with pytest.raises(ValueError, match="function task definition"):
        gen.map_ops_to_json([definition], function_name="probe")


def test_frontend_intrinsic_function_objects_carry_manifest_defaults() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "length_hint = _require_intrinsic('molt_operator_length_hint')\n"
    )
    gen = SimpleTIRGenerator(module_name="intrinsic_defaults_probe")
    gen.visit(ast.parse(source))

    ops = [op for func in gen.funcs_map.values() for op in func["ops"]]
    const_str_by_var = {
        op.result.name: op.args[0]
        for op in ops
        if op.kind == "CONST_STR" and isinstance(op.args[0], str)
    }
    builtin_index = next(
        idx
        for idx, op in enumerate(ops)
        if op.kind == "BUILTIN_FUNC"
        and len(op.args) == 3
        and op.args[:2] == ["molt_operator_length_hint", 2]
        and isinstance(op.args[2], MoltValue)
        and const_str_by_var.get(op.args[2].name) == "molt_operator_length_hint"
        and op.metadata == {"builtin_name": "molt_operator_length_hint"}
    )
    func_var = ops[builtin_index].result
    tuple_vars = {
        op.result.name
        for op in ops[builtin_index + 1 :]
        if op.kind == "TUPLE_NEW"
        and len(op.args) == 1
        and isinstance(op.args[0], MoltValue)
    }

    assert any(
        op.kind == "SETATTR_GENERIC_OBJ"
        and op.args[0] == func_var
        and op.args[1] == "__defaults__"
        and isinstance(op.args[2], MoltValue)
        and op.args[2].name in tuple_vars
        for op in ops[builtin_index + 1 :]
    )


def test_python_builtin_func_serializes_metadata_name_operand() -> None:
    gen = SimpleTIRGenerator(module_name="open_builtin_metadata_probe")
    # Test explicit wrapper publication, not a Python reference: source-level
    # open must capture the actual namespace binding before its arguments.
    gen._emit_builtin_function("open")
    gen._emit_function_exception_handler()
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
    )
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    open_ops = [
        op
        for op in main_ops
        if op.get("kind") == "builtin_func" and op.get("s_value") == "molt_open_builtin"
    ]
    assert open_ops
    open_op = open_ops[0]
    assert open_op.get("builtin_name") == "open"
    assert len(open_op.get("args") or []) == 1
    assert const_str[open_op["args"][0]] == "open"


def test_non_phi_or_with_call_avoids_list_cell_result_plumbing() -> None:
    source = (
        "def left():\n"
        "    return None\n"
        "def f():\n"
        "    values = left() or (1, 2, 3)\n"
        "    return values\n"
    )
    gen = SimpleTIRGenerator(module_name="partner_boolop", enable_phi=False)
    gen.visit(ast.parse(source))
    func_ops = gen.funcs_map["partner_boolop__f"]["ops"]
    kinds = [op.kind for op in func_ops]
    assert "LIST_NEW" not in kinds
    assert "STORE_INDEX" not in kinds
    assert "INDEX" not in kinds


def test_non_phi_and_with_call_avoids_list_cell_result_plumbing() -> None:
    source = (
        "def left():\n"
        "    return 1\n"
        "def right():\n"
        "    return 2\n"
        "def f():\n"
        "    values = left() and right()\n"
        "    return values\n"
    )
    gen = SimpleTIRGenerator(module_name="partner_boolop", enable_phi=False)
    gen.visit(ast.parse(source))
    func_ops = gen.funcs_map["partner_boolop__f"]["ops"]
    kinds = [op.kind for op in func_ops]
    assert "LIST_NEW" not in kinds
    assert "STORE_INDEX" not in kinds
    assert "INDEX" not in kinds


def test_try_wrapped_return_avoids_list_return_slot_in_sync_function() -> None:
    source = (
        "from _intrinsics import require_intrinsic as r\n"
        "def safe(name, default=None, _ri=r):\n"
        "    try:\n"
        "        fn = _ri(name)\n"
        "        if callable(fn):\n"
        "            return fn\n"
        "    except (RuntimeError, TypeError):\n"
        "        pass\n"
        "    if default is not None:\n"
        "        return default\n"
        "    return lambda: None\n"
    )
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____safe"
    )
    kinds = [op["kind"] for op in func_ops]
    assert "list_new" not in kinds
    assert "store_index" not in kinds
    assert "index" not in kinds


def test_nested_listcomp_function_does_not_capture_comprehension_target() -> None:
    source = (
        "def outer():\n"
        "    data = ('a', 'b')\n"
        "    def inner(kw):\n"
        "        return [kw.get(name) for name in data]\n"
    )
    ir = compile_to_tir(source)
    outer_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____outer"
    )
    data_literal_var = next(
        op["out"]
        for op in outer_ops
        if op.get("kind") == "tuple_new" and len(op.get("args", [])) == 2
    )
    data_cell_var = next(
        op["args"][0]
        for op in outer_ops
        if op.get("kind") == "call"
        and op.get("s_value") == "molt_cell_set"
        and op.get("args", [None, None])[1] == data_literal_var
    )
    assert any(
        op.get("kind") == "call"
        and op.get("s_value") == "molt_cell_new"
        and op.get("out") == data_cell_var
        for op in outer_ops
    )
    for idx, op in enumerate(outer_ops):
        if op["kind"] != "func_new_closure" or op.get("s_value") != "__main____inner":
            continue
        closure_tuple_var = op["args"][0]
        tuple_op = next(
            candidate
            for candidate in outer_ops[:idx]
            if candidate.get("kind") == "tuple_new"
            and candidate.get("out") == closure_tuple_var
        )
        assert tuple_op["args"] == [data_cell_var]
        break
    else:
        raise AssertionError("missing inner func_new_closure")


@pytest.mark.parametrize(
    ("parameters", "returned", "cellvars", "freevars"),
    [
        (
            "parameter",
            "parameter, local",
            ("parameter", "local"),
            ("local", "parameter"),
        ),
        (
            "z, a, /, *args, k, **kwargs",
            "z, a, args, k, kwargs, local",
            ("z", "a", "k", "args", "kwargs", "local"),
            ("a", "args", "k", "kwargs", "local", "z"),
        ),
    ],
)
def test_closure_tuple_and_code_metadata_share_one_name_order(
    parameters: str,
    returned: str,
    cellvars: tuple[str, ...],
    freevars: tuple[str, ...],
) -> None:
    ir = compile_to_tir(
        f"def outer({parameters}):\n"
        "    local = 1\n"
        "    def inner():\n"
        f"        return {returned}\n"
        "    return inner\n"
    )
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    outer_definition = next(
        op
        for op in main_ops
        if op.get("kind") == "func_new" and op.get("s_value", "").endswith("outer")
    )
    outer_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == outer_definition["s_value"]
    )
    inner_definition = next(
        op
        for op in outer_ops
        if op.get("kind") == "func_new_closure"
        and op.get("s_value", "").endswith("inner")
    )

    def metadata_names(ops, definition, index: int) -> tuple[str, ...]:
        producers = {op["out"]: op for op in ops if "out" in op}
        metadata = next(
            op
            for op in ops
            if op.get("kind") == "call"
            and op.get("s_value") == "molt_function_init_metadata_packed"
            and op["args"][0] == definition["out"]
        )
        packed = producers[metadata["args"][1]]
        assert len(packed["args"]) == 14
        names = producers[packed["args"][index]]
        return tuple(producers[value]["s_value"] for value in names["args"])

    assert metadata_names(main_ops, outer_definition, 12) == ()
    assert metadata_names(main_ops, outer_definition, 13) == cellvars
    assert metadata_names(outer_ops, inner_definition, 12) == freevars
    assert metadata_names(outer_ops, inner_definition, 13) == ()

    producers = {op["out"]: op for op in outer_ops if "out" in op}
    closure = producers[inner_definition["args"][0]]
    assert closure["kind"] == "tuple_new"
    assert len(closure["args"]) == len(freevars)
    assert all(
        producers[value].get("kind") == "call"
        and producers[value].get("s_value") == "molt_cell_new"
        for value in closure["args"]
    )


@pytest.mark.parametrize("local", [False, True])
def test_imported_class_ctor_avoids_cross_module_name_collision(local: bool) -> None:
    # Model the collision lane explicitly: compiler metadata says "Path" points
    # at zipfile._path.Path.__init__, while source imports Path from pathlib.
    known_classes = {
        "Path": {
            "fields": {},
            "size": 24,
            "dynamic": False,
            "static": True,
            "methods": {
                "__init__": {
                    "func": MoltValue(
                        "Path___init__", type_hint="Func:zipfile__path__Path___init__"
                    ),
                    "attr": MoltValue("__init__", type_hint="str"),
                    "descriptor": "function",
                    "return_hint": None,
                    "param_count": 2,
                    "defaults": [],
                    "posonly_count": 0,
                    "kwonly_count": 0,
                    "has_vararg": False,
                    "has_varkw": False,
                    "has_closure": False,
                    "property_field": None,
                    "property_update": None,
                }
            },
            "mro": ["Path", "object"],
        }
    }
    gen = SimpleTIRGenerator(known_classes=known_classes)
    source = "from pathlib import Path\nPath('x')\n"
    if local:
        source = "def probe():\n" + indent(source, "    ")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    main_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == ("__main____probe" if local else "molt_main")
    )
    const_str: dict[str, str] = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str" and isinstance(op.get("out"), str)
    }
    imported_path_values = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_import_from"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "Path"
    }
    path_class_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_global"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "Path"
    }
    assert imported_path_values, "expected from-import to materialize pathlib.Path"
    # Module publication may release a callback-populated old Path and reload
    # its replacement; a private fast local retains the imported value.
    path_call_targets = (
        _local_import_reads(main_ops, "Path", imported_path_values)
        if local
        else path_class_vars
    )
    assert path_call_targets, "expected pathlib.Path imported value in lowered main ops"
    supplied = (
        _bound_positional_args(main_ops, path_call_targets)
        if local
        else _positional_call(main_ops, path_call_targets, 1)["args"][1:]
    )
    assert len(supplied) == 1
    assert const_str[supplied[0]] == "x"
    assert all(
        op.get("s_value") != "zipfile__path__Path___init__"
        for op in main_ops
        if op.get("kind") == "call"
    ), "main lowering should not hardwire zipfile._path.Path.__init__ for pathlib.Path"


def test_imported_exception_class_ctor_uses_imported_class_value() -> None:
    known_classes = {
        "AxisError": {
            "fields": {},
            "size": 0,
            "dynamic": False,
            "static": True,
            "methods": {},
            "mro": ["AxisError", "ValueError", "Exception", "BaseException", "object"],
            "module": "numpy.exceptions",
            "exception_subclass": True,
        }
    }
    gen = SimpleTIRGenerator(
        module_name="scipy._lib._util",
        known_modules={"numpy.exceptions", "scipy._lib._util"},
        known_classes=known_classes,
        direct_call_modules={"scipy._lib._util"},
        native_support_function_roots={"normalize_axis_index"},
    )
    gen.visit(
        ast.parse(
            "from numpy.exceptions import AxisError\n\n"
            "def normalize_axis_index(axis, ndim):\n"
            "    raise AxisError('bad')\n"
        )
    )
    ir = gen.to_json()
    func_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == "scipy__lib__util__normalize_axis_index"
    )

    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "AxisError"), 1
    )
    argument = next(op for op in func_ops if op.get("out") == call["args"][1])
    assert argument["kind"] == "const_str" and argument["s_value"] == "bad"
    assert any(
        op.get("kind") == "raise" and op.get("args") == [call["out"]] for op in func_ops
    )
    assert not any(op["kind"].startswith("exception_new") for op in func_ops)


def test_imported_uppercase_constructor_uses_live_binding_outside_module_scope() -> (
    None
):
    gen = SimpleTIRGenerator(
        module_name="scipy._lib._util",
        known_modules={"scipy._lib._util"},
        direct_call_modules={"scipy._lib._util"},
        native_support_function_roots={"normalize_axis_index"},
    )
    gen.visit(
        ast.parse(
            "from numpy.exceptions import AxisError\n\n"
            "def normalize_axis_index(axis, ndim):\n"
            "    raise AxisError('bad')\n"
        )
    )
    ir = gen.to_json()
    func_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == "scipy__lib__util__normalize_axis_index"
    )

    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "AxisError"), 1
    )
    argument = next(op for op in func_ops if op.get("out") == call["args"][1])
    assert argument["kind"] == "const_str" and argument["s_value"] == "bad"
    assert any(
        op.get("kind") == "raise" and op.get("args") == [call["out"]] for op in func_ops
    )
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "AxisError")
        for op in func_ops
    )


def test_imported_known_vararg_function_call_uses_live_published_value() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"typing"},
        stdlib_allowlist={"typing"},
        known_func_defaults={
            "typing": {
                "TypeVar": {
                    "params": 1,
                    "defaults": [],
                    "kwonly": 0,
                    "has_vararg": True,
                }
            }
        },
    )
    gen.visit(ast.parse("from typing import TypeVar\nT = TypeVar('T')\n"))
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
    )
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str" and isinstance(op.get("out"), str)
    }
    imported_typevar_values = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_import_from"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "TypeVar"
    }
    assert imported_typevar_values, "expected from-import to materialize TypeVar"
    publications = [
        op
        for op in main_ops
        if op.get("kind") == "module_set_attr"
        and op["args"][2] in imported_typevar_values
        and const_str.get(op["args"][1]) == "TypeVar"
    ]
    assert len(publications) == 1, "the imported binding must publish exactly once"
    module, key, _ = publications[0]["args"]
    imported_typevar_loads = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_global" and op["args"] == [module, key]
    }
    assert imported_typevar_loads, (
        "calls must observe the live published module binding"
    )
    supplied = _bound_positional_args(main_ops, imported_typevar_loads)
    assert len(supplied) == 1
    assert const_str[supplied[0]] == "T"


@pytest.mark.parametrize("version", [(3, 12), (3, 13), (3, 14)])
def test_locals_calls_share_live_frame_custody_for_every_target(
    version: tuple[int, int],
) -> None:
    gen = SimpleTIRGenerator(target_python=version)
    gen.visit(
        ast.parse("def f():\n    a = locals()\n    b = locals()\n    return a is b\n")
    )
    ops = next(
        function["ops"]
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    # Snapshot policy belongs to the invoked builtin's runtime target, not a
    # duplicated frontend dictionary allocation path. The name can be rebound.
    lookups = _module_attr_accesses(ops, "module_get_global", "locals")
    assert len(lookups) == 2
    for target in lookups:
        _positional_call(ops, {target}, 0)
    assert sum(op["kind"] == "dict_new" for op in ops) == 1


@pytest.mark.parametrize(
    "source",
    [
        "def f(mapping, tail):\n    return dict(**mapping, value=tail())\n",
        "def f(mapping, tail):\n    result = {}\n    result.update(**mapping, value=tail())\n    return result\n",
        "def f(mapping, tail):\n    class Result(**mapping, value=tail()):\n        pass\n    return Result\n",
    ],
)
def test_keyword_merge_consumers_share_argument_assembly(source: str) -> None:
    gen = SimpleTIRGenerator()
    gen.visit(ast.parse(source))
    ops = [op for function in gen.funcs_map.values() for op in function["ops"]]
    assert any(op.kind == "CALLARGS_EXPAND_KWSTAR" for op in ops)
    assert all(op.kind != "DICT_UPDATE_KWSTAR" for op in ops)


def _counter_known_classes() -> dict[str, dict[str, object]]:
    return {
        "Counter": {
            "methods": {},
            "fields": {"_handle": 0},
            "mro": ["Counter", "object"],
            "static": True,
            "size": 8,
        }
    }


def test_imported_counter_list_constructor_uses_global_binding_path() -> None:
    gen = SimpleTIRGenerator(
        known_classes=_counter_known_classes(),
        known_modules={"collections"},
        stdlib_allowlist={"collections"},
    )
    gen.visit(
        ast.parse(
            "from collections import Counter\n"
            'words = "a b a".split()\n'
            "c = Counter(words)\n"
        )
    )
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
    )

    counter_global_values = _module_attr_accesses(
        main_ops, "module_get_global", "Counter"
    )
    call = _positional_call(main_ops, counter_global_values, 1)
    assert call["args"][1] in _module_attr_accesses(
        main_ops, "module_get_global", "words"
    )
    assert all(
        not (
            op.get("kind") == "builtin_func"
            and op.get("s_value") == "molt_counter_from_iterable"
        )
        for op in main_ops
    )
    assert all(op.get("kind") != "object_new_bound" for op in main_ops)


def test_local_module_counter_list_constructor_uses_intrinsic_handle_path() -> None:
    gen = SimpleTIRGenerator(
        known_classes=_counter_known_classes(),
        known_modules={"collections"},
        stdlib_allowlist={"collections"},
    )
    gen.visit(
        ast.parse(
            "def probe():\n"
            "    import collections\n"
            '    words = ["a", "b", "a"]\n'
            "    c = collections.Counter(words)\n"
        )
    )
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "__main____probe"
    )

    assert any(
        op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_counter_from_iterable"
        for op in main_ops
    )
    assert any(op.get("kind") == "object_new_bound" for op in main_ops)
    assert any(
        op.get("kind") == "set_attr_generic_obj" and op.get("s_value") == "_handle"
        for op in main_ops
    )
    assert all(op.get("kind") != "call_bind" for op in main_ops)


def test_stdlib_direct_call_requires_lowered_target_module() -> None:
    gen = SimpleTIRGenerator(
        module_name="collections",
        known_modules={"collections"},
        stdlib_allowlist={"collections", "copy"},
        known_func_defaults={
            "copy": {
                "copy": {
                    "params": 1,
                    "defaults": [],
                    "kwonly": 0,
                    "has_vararg": False,
                }
            }
        },
    )
    gen.visit(
        ast.parse(
            "def userdict_copy(self):\n"
            "    import copy as _copy\n"
            "    return _copy.copy(self)\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "collections__userdict_copy"
    )

    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "copy__copy"
        for op in func_ops
    )
    assert any(op.get("kind") == "call_bind" for op in func_ops)
    assert "copy" in _importlib_transaction_targets(func_ops)


def test_stdlib_lowered_target_guards_the_actual_imported_callable() -> None:
    gen = SimpleTIRGenerator(
        module_name="collections",
        known_modules={"collections", "copy"},
        stdlib_allowlist={"collections", "copy"},
        direct_call_modules={"copy"},
        known_func_defaults={
            "copy": {
                "copy": {
                    "params": 1,
                    "defaults": [],
                    "kwonly": 0,
                    "has_vararg": False,
                }
            }
        },
    )
    gen.visit(
        ast.parse(
            "def userdict_copy(self):\n"
            "    import copy as _copy\n"
            "    return _copy.copy(self)\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "collections__userdict_copy"
    )

    (guarded,) = (
        op
        for op in func_ops
        if op.get("kind") == "call_guarded" and op.get("s_value") == "copy__copy"
    )
    assert len(guarded["args"]) == 2
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "copy__copy"
        for op in func_ops
    )
    assert all(op.get("kind") not in {"call_bind", "callargs_new"} for op in func_ops)


def _assert_stateful_callable_call(ops: list[dict], supplied_args: int) -> dict:
    calls = [op for op in ops if op.get("kind") == "call_func"]
    assert len(calls) == 1, ops
    call = calls[0]
    assert len(call.get("args") or []) == supplied_args + 1, call
    assert any(op.get("out") == call["args"][0] for op in ops), ops
    assert not any(
        op.get("kind") in {"alloc_task", "asyncgen_new", "function_closure_bits"}
        for op in ops
    ), ops
    return call


def test_imported_plain_generator_calls_actual_function() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"tinygrad.engine.realize"},
        direct_call_modules={"tinygrad.engine.realize"},
        known_func_defaults={
            "tinygrad.engine.realize": {
                "unwrap_multi": {
                    "params": 1,
                    "defaults": [],
                    "posonly": 0,
                    "kwonly": 0,
                    "kind": "gen",
                    "has_decorators": False,
                }
            }
        },
    )
    gen.visit(
        ast.parse(
            "from tinygrad.engine.realize import unwrap_multi\n"
            "def run(items):\n"
            "    return unwrap_multi(items)\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"].endswith("__run")
    )

    _assert_stateful_callable_call(func_ops, supplied_args=1)
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "tinygrad_engine_realize__unwrap_multi"
        )
        for op in func_ops
    ), func_ops


def test_imported_decorated_generator_uses_runtime_binding() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"tinygrad.helpers"},
        known_func_defaults={
            "tinygrad.helpers": {
                "cpu_profile": {
                    "params": 1,
                    "defaults": [],
                    "posonly": 0,
                    "kwonly": 0,
                    "kind": "gen",
                    "has_decorators": True,
                }
            }
        },
    )
    gen.visit(
        ast.parse(
            "from tinygrad.helpers import cpu_profile\n"
            "def run(label):\n"
            "    return cpu_profile(label)\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"].endswith("__run")
    )

    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "cpu_profile"), 1
    )
    assert call["args"][1] in _local_reads(func_ops, "label")
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "tinygrad_helpers__cpu_profile"
        )
        for op in func_ops
    ), func_ops
    assert all(
        not (
            op.get("kind") == "alloc_task"
            and op.get("s_value") == "tinygrad_helpers__cpu_profile_poll"
        )
        for op in func_ops
    ), func_ops


def test_from_import_generator_keeps_defaults_on_actual_callable() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
        direct_call_modules={"helpers"},
        stdlib_allowlist={"helpers"},
        known_func_defaults={
            "helpers": {
                "cpu_profile": {
                    "params": 3,
                    "defaults": [
                        {"const": True, "value": "TINY"},
                        {"const": True, "value": True},
                    ],
                    "kwonly": 0,
                    "has_vararg": False,
                }
            }
        },
        known_func_kinds={"helpers": {"cpu_profile": "gen"}},
    )
    gen.visit(
        ast.parse(
            "from helpers import cpu_profile\ndef run():\n    return cpu_profile('x')\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )

    _assert_stateful_callable_call(func_ops, supplied_args=1)
    assert not any(op.get("s_value") == "TINY" for op in func_ops)
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )
    assert all(
        not (
            op.get("kind") == "call_bind"
            and op.get("s_value") == "helpers__cpu_profile"
        )
        for op in func_ops
    )


def test_aliased_import_generator_kind_without_defaults_never_direct_calls_base() -> (
    None
):
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
        direct_call_modules={"helpers"},
        stdlib_allowlist={"helpers"},
        known_func_kinds={"helpers": {"cpu_profile": "gen"}},
    )
    gen.visit(
        ast.parse(
            "from helpers import cpu_profile as prof\n"
            "def run():\n"
            "    return prof('x', True)\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )
    _assert_stateful_callable_call(func_ops, supplied_args=2)
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )


def test_assigned_alias_of_imported_generator_owns_live_defaults() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
        direct_call_modules={"helpers"},
        stdlib_allowlist={"helpers"},
        known_func_defaults={
            "helpers": {
                "cpu_profile": {
                    "params": 2,
                    "defaults": [{"const": True, "value": "TINY"}],
                    "kwonly": 0,
                    "has_vararg": False,
                }
            }
        },
        known_func_kinds={"helpers": {"cpu_profile": "gen"}},
    )
    gen.visit(
        ast.parse(
            "from helpers import cpu_profile\n"
            "profile = cpu_profile\n"
            "def run():\n"
            "    return profile('x')\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )
    _assert_stateful_callable_call(func_ops, supplied_args=1)
    assert not any(op.get("s_value") == "TINY" for op in func_ops)
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )


def test_aliased_import_async_generator_calls_actual_function() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
        direct_call_modules={"helpers"},
        stdlib_allowlist={"helpers"},
        known_func_defaults={
            "helpers": {
                "events": {
                    "params": 2,
                    "defaults": [{"const": True, "value": 5}],
                    "kwonly": 0,
                    "has_vararg": False,
                }
            }
        },
        known_func_kinds={"helpers": {"events": "asyncgen"}},
    )
    gen.visit(
        ast.parse(
            "from helpers import events as stream\n"
            "def run():\n"
            "    return stream('cpu')\n"
        )
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )
    _assert_stateful_callable_call(func_ops, supplied_args=1)
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__events"
        for op in func_ops
    )
    assert not any(
        op.get("kind") == "call_bind" and op.get("s_value") == "helpers__events"
        for op in func_ops
    )


def test_imported_module_attr_generator_uses_same_callable_path() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
        direct_call_modules={"helpers"},
        stdlib_allowlist={"helpers"},
        known_func_defaults={
            "helpers": {
                "cpu_profile": {
                    "params": 2,
                    "defaults": [{"const": True, "value": "TINY"}],
                    "kwonly": 0,
                    "has_vararg": False,
                }
            }
        },
        known_func_kinds={"helpers": {"cpu_profile": "gen"}},
    )
    gen.visit(
        ast.parse("import helpers as h\ndef run():\n    return h.cpu_profile('x')\n")
    )
    func_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )
    _assert_stateful_callable_call(func_ops, supplied_args=1)
    assert not any(op.get("s_value") == "TINY" for op in func_ops)
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )


@pytest.mark.parametrize(
    "definition",
    [
        "def produce(value='original'):\n    yield value\n",
        "async def produce(value='original'):\n    return value\n",
        "async def produce(value='original'):\n    yield value\n",
    ],
)
@pytest.mark.parametrize("nested", [False, True])
def test_named_stateful_calls_leave_defaults_and_layout_on_callable(
    definition: str, nested: bool
) -> None:
    if nested:
        source = (
            "def run():\n"
            "    captured = 'closure'\n"
            + indent(definition.replace("value\n", "(value, captured)\n"), "    ")
            + "    return produce()\n"
        )
    else:
        source = definition + "def run():\n    return produce()\n"
    gen = SimpleTIRGenerator(module_name="main")
    gen.visit(ast.parse(source))
    ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )
    _assert_stateful_callable_call(ops, supplied_args=0)


@pytest.mark.parametrize("arguments", ["value='changed'", "*values", "**values"])
def test_stateful_dynamic_arguments_use_actual_callable_binder(arguments: str) -> None:
    gen = SimpleTIRGenerator(module_name="main")
    gen.visit(
        ast.parse(
            "def produce(value='original'):\n    yield value\n"
            f"def run(values):\n    return produce({arguments})\n"
        )
    )
    ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "main__run"
    )
    calls = [op for op in ops if op.get("kind") == "call_indirect"]
    assert len(calls) == 1, ops
    assert calls[0]["args"][0] in _module_attr_accesses(
        ops, "module_get_global", "produce"
    )
    builder = calls[0]["args"][1]
    assert any(op["kind"] == "callargs_new" and op.get("out") == builder for op in ops)
    push_kind = {
        "value='changed'": "callargs_push_kw",
        "*values": "callargs_expand_star",
        "**values": "callargs_expand_kwstar",
    }[arguments]
    assert any(op["kind"] == push_kind and op["args"][0] == builder for op in ops)
    assert not any(op.get("kind") == "alloc_task" for op in ops), ops


@pytest.mark.parametrize("caller_prefix", ["def", "async def"])
def test_deferred_python_function_calls_preserve_actual_callable_context(
    caller_prefix: str,
) -> None:
    gen = SimpleTIRGenerator(module_name="main")
    gen.visit(
        ast.parse(
            "def helper(value):\n    return value\n"
            f"{caller_prefix} invoke(unused, value):\n    return helper(value)\n"
        )
    )
    ops = [
        op
        for func in gen.to_json()["functions"]
        if func["name"] in {"main__invoke", "main__invoke_poll"}
        for op in func["ops"]
    ]
    call = _positional_call(
        ops, _module_attr_accesses(ops, "module_get_global", "helper"), 1
    )
    if caller_prefix == "async def":
        plan = gen.funcs_map["main__invoke_poll"]["stateful_frame_plan"]
        assert plan.param_count == 2
        producers = {op["out"]: op for op in ops if "out" in op}
        argument = producers[call["args"][1]]
        assert argument["kind"] == "closure_load"
        assert argument["args"] == ["self"]
        # The second Python argument is transported through the second task
        # parameter slot, not a poll-function local or the first argument.
        assert argument["value"] == plan.async_locals_base + 8
    else:
        assert call["args"][1] in _local_reads(ops, "value")
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "main__helper" for op in ops
    ), ops


def test_counter_string_constructor_keeps_general_constructor_path() -> None:
    gen = SimpleTIRGenerator(
        known_classes=_counter_known_classes(),
        known_modules={"collections"},
        stdlib_allowlist={"collections"},
    )
    gen.visit(ast.parse('from collections import Counter\nc = Counter("aba")\n'))
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
    )

    assert all(
        not (
            op.get("kind") == "builtin_func"
            and op.get("s_value") == "molt_counter_from_iterable"
        )
        for op in main_ops
    )
    assert all(op.get("kind") != "object_new_bound" for op in main_ops)


@pytest.mark.parametrize("local", [False, True])
def test_collections_namedtuple_kwonly_defaults_use_live_binding(local: bool) -> None:
    gen = SimpleTIRGenerator(
        known_modules={"collections"},
        stdlib_allowlist={"collections"},
        known_func_defaults={
            "collections": {
                "namedtuple": {
                    "params": 5,
                    "defaults": [
                        {
                            "const": True,
                            "value": False,
                            "kwonly": True,
                            "name": "rename",
                        },
                        {
                            "const": True,
                            "value": None,
                            "kwonly": True,
                            "name": "defaults",
                        },
                        {
                            "const": True,
                            "value": None,
                            "kwonly": True,
                            "name": "module",
                        },
                    ],
                    "posonly": 0,
                    "kwonly": 3,
                    "kind": "sync",
                    "has_decorators": False,
                }
            }
        },
    )
    source = "from collections import namedtuple\nT = namedtuple('T', ['x'])\n"
    if local:
        source = "def probe():\n" + indent(source, "    ")
    gen.visit(ast.parse(source))
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == ("__main____probe" if local else "molt_main")
    )
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str" and isinstance(op.get("out"), str)
    }
    imported_namedtuple_values = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_import_from"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "namedtuple"
    }

    assert imported_namedtuple_values, "expected from-import to materialize namedtuple"
    call_targets = (
        _local_import_reads(main_ops, "namedtuple", imported_namedtuple_values)
        if local
        else set(_module_attr_accesses(main_ops, "module_get_global", "namedtuple"))
    )
    supplied = (
        _bound_positional_args(main_ops, call_targets)
        if local
        else _positional_call(main_ops, call_targets, 2)["args"][1:]
    )
    assert len(supplied) == 2
    assert const_str[supplied[0]] == "T"
    fields = next(op for op in main_ops if op.get("out") == supplied[1])
    assert fields["kind"] == "list_new"
    assert [const_str[value] for value in fields["args"]] == ["x"]
    assert all(
        not (
            op.get("kind") == "call_bind"
            and len(op.get("args") or []) == 2
            and op["args"][0] == "namedtuple"
        )
        for op in main_ops
    ), main_ops
    assert all(
        not (
            op.get("kind") == "call_func"
            and isinstance(op.get("args"), list)
            and op["args"]
            and op["args"][0] == "namedtuple"
        )
        for op in main_ops
    ), main_ops


def test_minmax_direct_abi_path_does_not_attach_python_call_metadata() -> None:
    gen = SimpleTIRGenerator()
    gen.visit(ast.parse("a = max(1, 2)\n"))
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
    )

    max_call = next(
        op
        for op in main_ops
        if op.get("kind") == "call" and op.get("s_value") == "molt_max_builtin"
    )
    assert not any(
        op.get("kind") == "builtin_func" and op.get("s_value") == "molt_max_builtin"
        for op in main_ops
    ), main_ops
    metadata_targets = {
        op["args"][0]
        for op in main_ops
        if op.get("kind") == "call"
        and op.get("s_value") == "molt_function_init_metadata_packed"
        and isinstance(op.get("args"), list)
        and op["args"]
    }
    assert max_call["out"] not in metadata_targets


@pytest.mark.parametrize("expression", ['c["a"]', "len(c)"])
@pytest.mark.parametrize("local", [False, True])
def test_counter_operation_respects_live_callable_binding(
    expression: str,
    local: bool,
) -> None:
    gen = SimpleTIRGenerator(
        known_classes=_counter_known_classes(),
        known_modules={"collections"},
        stdlib_allowlist={"collections"},
    )
    source = (
        "import collections\n"
        'words = ["a", "b", "a"]\n'
        "c = collections.Counter(words)\n"
        f"result = {expression}\n"
    )
    if local:
        source = "def probe():\n" + indent(source, "    ")
    gen.visit(ast.parse(source))
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == ("__main____probe" if local else "molt_main")
    )

    if expression == 'c["a"]' and local:
        assert any(
            op.get("kind") == "builtin_func"
            and op.get("s_value") == "molt_counter_getitem"
            for op in main_ops
        )
        assert all(op.get("kind") != "index" for op in main_ops)
    elif expression == 'c["a"]':
        live_receiver = _module_attr_accesses(main_ops, "module_get_global", "c")
        assert live_receiver
        assert any(
            op.get("kind") == "index" and op["args"][0] in live_receiver
            for op in main_ops
        )
        assert all(
            op.get("kind") != "builtin_func"
            or op.get("s_value") != "molt_counter_getitem"
            for op in main_ops
        )
    else:
        # The constructor can execute Python and rebind module-global len.
        # The instance's known layout does not prove the next callable's identity.
        live_len = _module_attr_accesses(main_ops, "module_get_global", "len")
        call = _positional_call(main_ops, live_len, 1)
        receiver = (
            _local_reads(main_ops, "c")
            if local
            else _module_attr_accesses(main_ops, "module_get_global", "c")
        )
        assert call["args"][1] in receiver
        assert not any(
            op.get("kind") == "builtin_func" and op.get("s_value") == "molt_counter_len"
            for op in main_ops
        )


def test_dotted_import_alias_uses_runtime_module_import_when_parent_allowlisted() -> (
    None
):
    gen = SimpleTIRGenerator(known_modules={"__main__"}, stdlib_allowlist={"os"})
    gen.visit(ast.parse("import os.path\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    targets = _importlib_transaction_targets(main_ops)
    assert "os.path" in targets


def test_deferred_user_class_ctor_does_not_inherit_lexical_field_layout() -> None:
    ir = compile_to_tir(
        "class Point:\n"
        "    x: int\n"
        "    y: int\n"
        "    def __init__(self, x: int = 0, y: int = 0) -> None:\n"
        "        self.x = x\n"
        "        self.y = y\n"
        "\n"
        "def make(i: int) -> int:\n"
        "    p = Point(0, 0)\n"
        "    p.x = i\n"
        "    p.y = i + 1\n"
        "    return i\n"
    )
    make_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____make"
    )

    call = _positional_call(
        make_ops, _module_attr_accesses(make_ops, "module_get_global", "Point"), 2
    )
    producers = {op["out"]: op for op in make_ops if "out" in op}
    assert all(
        producers[arg]["kind"] == "const" and producers[arg]["value"] == 0
        for arg in call["args"][1:]
    )
    assert any(
        op.get("kind") == "store_var"
        and op.get("var") == "p"
        and op.get("args") == [call["out"]]
        for op in make_ops
    )
    stores = {
        op["s_value"]: op for op in make_ops if op.get("kind") == "set_attr_generic_obj"
    }
    assert set(stores) == {"x", "y"}
    assert all(op["args"][0] in _local_reads(make_ops, "p") for op in stores.values())
    assert stores["x"]["args"][1] in _local_reads(make_ops, "i")
    addition = producers[stores["y"]["args"][1]]
    assert addition["kind"] == "add"
    assert addition["args"][0] in _local_reads(make_ops, "i")
    assert producers[addition["args"][1]]["value"] == 1
    assert all(
        op.get("kind")
        not in {"object_new_bound", "store", "store_init", "guarded_field_init"}
        for op in make_ops
    )
    assert all(op.get("kind") != "call_bind" for op in make_ops)
    assert all(op.get("kind") != "callargs_new" for op in make_ops)


def test_dishonest_return_annotation_does_not_authorize_field_layout() -> None:
    ir = compile_to_tir(
        "class Expected:\n"
        "    guarded: int\n"
        "\n"
        "def dishonest() -> Expected:\n"
        "    return 3\n"
        "\n"
        "def exercise():\n"
        "    value = dishonest()\n"
        "    before = value.guarded\n"
        "    value.guarded = 4\n"
        "    return before\n",
        type_hint_policy="check",
    )
    exercise_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____exercise"
    )

    assert any(op.get("kind") == "get_attr_generic_obj" for op in exercise_ops)
    assert any(op.get("kind") == "set_attr_generic_obj" for op in exercise_ops)
    assert not any(
        op.get("kind") in {"load", "store", "dataclass_get", "dataclass_set"}
        for op in exercise_ops
    )


def test_deferred_user_class_ctor_does_not_inherit_lexical_finalizer_fact() -> None:
    ir = compile_to_tir(
        "class Item:\n"
        "    def __del__(self):\n"
        "        pass\n"
        "\n"
        "def make():\n"
        "    return Item()\n"
    )
    make_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____make"
    )
    call = _positional_call(
        make_ops, _module_attr_accesses(make_ops, "module_get_global", "Item"), 0
    )
    assert call.get("type_hint") in {None, "Any"}
    assert call.get("defines_del") is not True
    assert any(
        op.get("kind") == "ret" and op.get("args") == [call["out"]] for op in make_ops
    )
    assert all(op.get("kind") != "object_new_bound" for op in make_ops)


def test_delete_function_local_releases_previous_binding_after_missing_store() -> None:
    ir = compile_to_tir(
        "class Item:\n"
        "    def __del__(self):\n"
        "        pass\n"
        "\n"
        "def run():\n"
        "    item = Item()\n"
        "    del item\n"
        "    return 0\n"
    )
    ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____run"
    )

    missing_defs = {
        op["out"]
        for op in ops
        if op.get("kind") == "missing" and isinstance(op.get("out"), str)
    }
    load_idx, old_var = next(
        (idx, op["out"])
        for idx, op in reversed(list(enumerate(ops)))
        if op.get("kind") == "load_var"
        and op.get("var") == "item"
        and isinstance(op.get("out"), str)
    )
    delete_idx, delete_args = next(
        (idx, op.get("args") or [])
        for idx, op in enumerate(ops[load_idx + 1 :], start=load_idx + 1)
        if op.get("kind") == "delete_var"
        and op.get("var") == "item"
        and len(op.get("args") or []) == 2
        and (op.get("args") or [None])[0] in missing_defs
    )

    assert load_idx < delete_idx
    assert delete_args[1] == old_var
    assert all(
        op.get("kind") != "store_var" or op.get("var") != "item"
        for op in ops[load_idx + 1 : delete_idx + 1]
    )


def test_delete_nonlocal_cell_releases_previous_binding_after_missing_store() -> None:
    ir = compile_to_tir(
        "def outer():\n"
        "    item = object()\n"
        "    def inner():\n"
        "        nonlocal item\n"
        "        del item\n"
        "    inner()\n"
    )
    ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____inner"
    )

    missing_defs = {
        op["out"]
        for op in ops
        if op.get("kind") == "missing" and isinstance(op.get("out"), str)
    }
    store_idx = next(
        idx
        for idx, op in enumerate(ops)
        if op.get("kind") == "call"
        and op.get("s_value") == "molt_cell_set"
        and len(op.get("args") or []) == 2
        and op["args"][1] in missing_defs
    )
    cell_var = ops[store_idx]["args"][0]
    producers = {op["out"]: op for op in ops if "out" in op}

    def captured_cell(value: str) -> tuple[str, int]:
        access = producers[value]
        assert access["kind"] == "index"
        index = producers[access["args"][1]]
        assert index["kind"] == "const"
        return access["args"][0], index["value"]

    cell_identity = captured_cell(cell_var)
    assert cell_identity == ("__molt_closure__", 0)
    old_def_idx = next(
        idx
        for idx, op in enumerate(ops[:store_idx])
        if op.get("kind") == "call"
        and op.get("s_value") == "molt_cell_get"
        and captured_cell(op["args"][0]) == cell_identity
    )

    # The shared retained-cell setter owns publish-before-release. The compiler
    # reads the old value only for lexical empty-cell validation, not a second
    # independently owned sequence slot or a hand-emitted release protocol.
    assert old_def_idx < store_idx
    old_value = ops[old_def_idx]["out"]
    assert any(
        op.get("kind") == "dec_ref" and op.get("args") == [old_value]
        for op in ops[store_idx + 1 :]
    )
    assert not any(
        op.get("kind") == "dec_ref" and op.get("args") == [old_value]
        for op in ops[old_def_idx + 1 : store_idx]
    )
    assert not any(
        op.get("kind") == "store_index" and op.get("args", [None])[0] == cell_var
        for op in ops
    )


def test_unstable_globals_user_class_ctor_calls_live_class_value() -> None:
    ir = compile_to_tir(
        "globals()\nclass A:\n    pass\n\ndef make():\n    return A()\n"
    )
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____make"
    )
    const_str = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str" and isinstance(op.get("out"), str)
    }
    class_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_global"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "A"
    }
    assert class_vars, "expected local class lookup in lowered module chunk"
    call = _positional_call(main_ops, class_vars, 0)
    assert any(
        op.get("kind") == "ret" and op.get("args") == [call["out"]] for op in main_ops
    )
    assert all(op.get("kind") not in {"alloc_class"} for op in main_ops), (
        "globals-escaped class constructor should not lower via synthetic object allocation"
    )
    assert all(op.get("kind") != "object_new_bound" for op in main_ops)


def test_function_param_types_cover_kwonly_and_varkw_slots() -> None:
    ir = compile_to_tir(
        "def f(x: int, *, strict=None, parse=None, **kw):\n    return x\n"
    )
    fn = next(func for func in ir["functions"] if func["name"] == "__main____f")
    assert fn["params"] == ["x", "strict", "parse", "kw"]
    assert fn["param_types"] == ["i64", "i64", "i64", "i64"]


def test_known_module_import_uses_runtime_import_boundary() -> None:
    gen = SimpleTIRGenerator(known_modules={"sys"})
    gen.visit(ast.parse("import sys\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert "sys" in _importlib_transaction_targets(main_ops)
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "molt_init_sys")
        for op in main_ops
    )


def test_target_sys_platform_prunes_unreachable_darwin_guarded_module_code() -> None:
    source = (
        "import sys\nif sys.platform == 'darwin':\n    polyval([1], [2])\nanswer = 1\n"
    )
    gen = SimpleTIRGenerator(
        module_name="numpy",
        known_modules={"sys"},
        target_sys_platform="wasm",
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    assert all(op.get("s_value") != "polyval" for op in main_ops)


def test_target_sys_platform_keeps_reachable_matching_platform_module_code() -> None:
    source = (
        "import sys\nif sys.platform == 'darwin':\n    polyval([1], [2])\nanswer = 1\n"
    )
    gen = SimpleTIRGenerator(
        module_name="numpy",
        known_modules={"sys"},
        target_sys_platform="darwin",
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    assert any(op.get("s_value") == "polyval" for op in main_ops)


@pytest.mark.parametrize(
    "name,target,before,between,delete,elided",
    [
        ("_helper", "wasm", "", "", True, True),
        ("_helper", "wasm", "", "", False, False),
        ("helper", "wasm", "", "", False, False),
        ("_helper", "darwin", "", "", True, False),
        ("_helper", "wasm", "", "snapshot = globals()\n", True, False),
        ("_helper", "wasm", "view = locals()\n", "saved = view.copy()\n", True, False),
        ("_helper", "wasm", "view = globals().keys()\n", "", True, False),
        ("_helper", "wasm", "", "observer()\n", True, False),
        ("_helper", "wasm", "import foreign\n", "", True, False),
    ],
)
def test_module_helper_pruning_requires_unobserved_deleted_lifetime(
    name: str,
    target: str,
    before: str,
    between: str,
    delete: bool,
    elided: bool,
) -> None:
    # Isolate reachability from third-party imports that can rewrite sys.platform.
    # Private names remain public module attributes regardless of dead local uses.
    source = (
        "import sys\n"
        + before
        + f"def {name}():\n    return 1\n"
        + between
        + f"if sys.platform == 'darwin':\n    {name}()\n"
        + (f"del {name}\n" if delete else "")
    )
    gen = SimpleTIRGenerator(
        module_name="sample",
        known_modules={"sys", "foreign"},
        stdlib_allowlist={"sys", "foreign"},
        target_sys_platform=target,
    )
    gen.visit(ast.parse(source))
    names = {function["name"] for function in gen.to_json()["functions"]}
    assert (f"sample__{name}" not in names) == elided


def test_deleted_definition_does_not_elide_later_rebinding_with_same_name() -> None:
    gen = SimpleTIRGenerator(module_name="redefinition")
    source = "def helper():\n    return 1\ndel helper\ndef helper():\n    return 2\n"
    gen.visit(ast.parse(source))
    assert any("helper" in function["name"] for function in gen.to_json()["functions"])
    assert "helper" not in gen._module_elidable_deleted_functions(ast.parse(source))


def test_source_import_statements_use_import_transaction_details() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"json", "json.tool", "pkg", "pkg.child"},
        stdlib_allowlist={"json", "json.tool"},
    )
    gen.visit(ast.parse("import json\nimport json.tool as jt\nfrom pkg import child\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    details = _import_transaction_details(main_ops)
    transaction_arities = {
        op.get("value")
        for op in main_ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_transaction"
    }
    assert transaction_arities == {
        wasm_runtime_callable_arity("molt_importlib_import_transaction")
    }
    assert ("json", (), 0) in details
    assert ("json.tool", (), 0) in details
    assert ("pkg", ("child",), 0) in details
    assert "json.tool" not in _importlib_import_module_targets(main_ops)
    assert "json" not in _module_import_targets(main_ops)
    assert "json.tool" not in _module_import_targets(main_ops)
    assert "pkg" not in _module_import_targets(main_ops)
    assert any(op.get("kind") == "module_import_from" for op in main_ops)


def test_bootstrap_source_imports_keep_internal_module_import_boundary() -> None:
    for module_name in (
        "builtins",
        "_molt_importer",
        "importlib",
        "importlib._bootstrap",
    ):
        gen = SimpleTIRGenerator(
            module_name=module_name,
            known_modules={"json"},
            stdlib_allowlist={"json"},
        )
        gen.visit(ast.parse("import json\n"))
        ir = gen.to_json()
        main_ops = next(
            func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
        )

        assert "json" in _module_import_targets(main_ops)
        assert "json" not in _importlib_transaction_targets(main_ops)


def test_known_child_from_import_uses_transaction_owned_fromlist() -> None:
    gen = SimpleTIRGenerator(known_modules={"pkg", "pkg.child"})
    gen.visit(ast.parse("from pkg import child\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    assert ("pkg", ("child",), 0) in _import_transaction_details(main_ops)
    assert "pkg.child" not in _importlib_transaction_targets(main_ops)
    assert all(
        op.get("s_value") != "molt_module_prepare_from_import_child" for op in main_ops
    )
    assert any(op.get("kind") == "module_import_from" for op in main_ops)


def test_importlib_import_module_literal_lowers_to_import_module_leaf() -> None:
    main_ops = _importlib_literal_main_ops(
        "import importlib\nmod = importlib.import_module('json')\n"
    )
    assert "json" in _importlib_import_module_targets(main_ops)
    assert "json" not in _importlib_transaction_targets(main_ops)
    assert "json" not in _module_import_targets(main_ops)
    assert "import_module" not in _module_get_attr_names(main_ops)
    assert not _has_static_call(main_ops, "importlib__import_module")


def test_importlib_import_module_literal_alias_lowers_to_import_module_leaf() -> None:
    main_ops = _importlib_literal_main_ops(
        "import importlib as loader\nmod = loader.import_module('json')\n"
    )
    assert "json" in _importlib_import_module_targets(main_ops)
    assert "json" not in _importlib_transaction_targets(main_ops)
    assert "json" not in _module_import_targets(main_ops)
    assert "import_module" not in _module_get_attr_names(main_ops)
    assert not _has_static_call(main_ops, "importlib__import_module")


def test_importlib_import_module_literal_from_import_lowers_to_import_module_leaf() -> (
    None
):
    main_ops = _importlib_literal_main_ops(
        "from importlib import import_module\nmod = import_module('json')\n"
    )
    assert "json" in _importlib_import_module_targets(main_ops)
    assert "json" not in _importlib_transaction_targets(main_ops)
    assert "json" not in _module_import_targets(main_ops)
    assert not _has_static_call(main_ops, "importlib__import_module")


def test_importlib_import_module_literal_in_function_calls_live_attribute() -> None:
    func_ops = _importlib_literal_function_ops(
        "import importlib\ndef f():\n    return importlib.import_module('json')\n",
        "__main____f",
    )
    modules = _module_attr_accesses(func_ops, "module_get_global", "importlib")
    targets = {
        op["out"]
        for op in func_ops
        if op.get("kind") == "get_attr_generic_obj"
        and op.get("s_value") == "import_module"
        and op["args"][0] in modules
    }
    call = _positional_call(func_ops, targets, 1)
    argument = next(op for op in func_ops if op.get("out") == call["args"][1])
    assert argument["kind"] == "const_str" and argument["s_value"] == "json"
    assert "json" not in _importlib_import_module_targets(func_ops)
    assert "json" not in _importlib_transaction_targets(func_ops)
    assert "json" not in _module_import_targets(func_ops)
    assert "import_module" not in _module_get_attr_names(func_ops)
    assert not _has_static_call(func_ops, "importlib__import_module")


def test_importlib_import_module_literal_respects_local_shadowing() -> None:
    func_ops = _importlib_literal_function_ops(
        "import importlib\n"
        "def f(importlib):\n"
        "    return importlib.import_module('json')\n",
        "__main____f",
    )
    assert "json" not in _module_import_targets(func_ops)
    assert "json" not in _importlib_transaction_targets(func_ops)
    assert "json" not in _importlib_import_module_targets(func_ops)


def test_importlib_import_module_literal_respects_module_attr_rebinding() -> None:
    main_ops = _importlib_literal_main_ops(
        "import importlib\n"
        "def fake(name):\n"
        "    return 'fake:' + name\n"
        "importlib.import_module = fake\n"
        "mod = importlib.import_module('json')\n"
    )
    assert "json" not in _module_import_targets(main_ops)
    assert "json" not in _importlib_transaction_targets(main_ops)
    assert "json" not in _importlib_import_module_targets(main_ops)
    assert not _has_static_call(main_ops, "importlib__import_module")


def test_importlib_import_module_literal_respects_aliased_module_attr_rebinding() -> (
    None
):
    main_ops = _importlib_literal_main_ops(
        "import importlib as loader\n"
        "def fake(name):\n"
        "    return 'fake:' + name\n"
        "loader.import_module = fake\n"
        "mod = loader.import_module('json')\n"
    )
    assert "json" not in _module_import_targets(main_ops)
    assert "json" not in _importlib_transaction_targets(main_ops)
    assert "json" not in _importlib_import_module_targets(main_ops)
    assert not _has_static_call(main_ops, "importlib__import_module")


def test_importlib_import_module_literal_unresolved_name_uses_importlib_runtime() -> (
    None
):
    gen = SimpleTIRGenerator(
        known_modules={"importlib"},
        stdlib_allowlist={"importlib"},
    )
    gen.visit(
        ast.parse(
            "import importlib\n"
            "mod = importlib.import_module('molt_missing_importlib_literal_target')\n"
        )
    )
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert "molt_missing_importlib_literal_target" not in _module_import_targets(
        main_ops
    )
    assert (
        "molt_missing_importlib_literal_target"
        not in _importlib_transaction_targets(main_ops)
    )
    assert (
        "molt_missing_importlib_literal_target"
        not in _importlib_import_module_targets(main_ops)
    )
    assert _has_static_call(main_ops, "importlib__import_module")


def test_importlib_import_module_dynamic_calls_live_attribute() -> None:
    func_ops = _importlib_literal_function_ops(
        "import importlib\ndef f(name):\n    return importlib.import_module(name)\n",
        "__main____f",
    )

    assert not _has_static_call(func_ops, "importlib__import_module")
    assert "json" not in _importlib_import_module_targets(func_ops)
    modules = _module_attr_accesses(func_ops, "module_get_global", "importlib")
    targets = {
        op["out"]
        for op in func_ops
        if op.get("kind") == "get_attr_generic_obj"
        and op.get("s_value") == "import_module"
        and op["args"][0] in modules
    }
    call = _positional_call(func_ops, targets, 1)
    assert call["args"][1] in _local_reads(func_ops, "name")


def _admitted_sum_reduction(
    expression: str, *parameters: str
) -> tuple[SimpleTIRGenerator, MoltValue]:
    """Exercise reducer lowering only after exact builtin-call admission."""
    gen = SimpleTIRGenerator()
    gen.start_function(
        "admitted_sum", params=list(parameters), param_types=["Any"] * len(parameters)
    )
    for parameter in parameters:
        gen.locals[parameter] = MoltValue(parameter, type_hint="Any")
    node = ast.parse(expression, mode="eval").body
    assert isinstance(node, ast.Call)
    return gen, gen._emit_sum_call("sum", node, needs_bind=False)


def test_admitted_sum_generator_expr_lowers_as_full_consumption_reducer() -> None:
    gen, result = _admitted_sum_reduction("sum(v for v in data if v % 2 == 0)", "data")
    ops = gen.current_ops

    assert result.type_hint == "Any"
    assert any(op.kind == "LOOP_START" for op in ops)
    assert any(op.kind == "ADD" for op in ops)
    assert not any(
        op.kind in {"ALLOC_TASK", "FUNC_NEW", "BUILTIN_FUNC", "CALL_FUNC", "CALL_BIND"}
        or (op.metadata or {}).get("task_kind") == "generator"
        for op in ops
    )


def test_admitted_sum_listcomp_lowers_as_full_consumption_reducer() -> None:
    gen, _ = _admitted_sum_reduction("sum([v * 2 for v in data if v > 3])", "data")
    ops = gen.current_ops

    assert any(op.kind == "LOOP_START" for op in ops)
    assert any(op.kind == "MUL" for op in ops)
    assert any(op.kind == "ADD" for op in ops)
    assert not any(
        op.kind
        in {
            "ALLOC_TASK",
            "FUNC_NEW",
            "LIST_NEW",
            "BUILTIN_FUNC",
            "CALL_FUNC",
            "CALL_BIND",
        }
        or (op.metadata or {}).get("task_kind") == "generator"
        for op in ops
    )


def test_admitted_sum_generator_expr_tuple_target_lowers_inline() -> None:
    gen, _ = _admitted_sum_reduction("sum(a * b for a, b in pairs if a > 2)", "pairs")
    ops = gen.current_ops

    assert any(op.kind == "UNPACK_SEQUENCE" for op in ops)
    assert any(op.kind == "MUL" for op in ops)
    assert any(op.kind == "ADD" for op in ops)
    assert not any(
        op.kind in {"ALLOC_TASK", "FUNC_NEW", "BUILTIN_FUNC", "CALL_FUNC", "CALL_BIND"}
        or (op.metadata or {}).get("task_kind") == "generator"
        for op in ops
    )


def test_sum_generator_expr_with_start_calls_live_binding_with_generator_frame() -> (
    None
):
    gen = SimpleTIRGenerator()
    gen.visit(
        ast.parse("def f(data, start):\n    return sum((v for v in data), start)\n")
    )
    ir = gen.to_json()
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    (definition,) = (op for op in func_ops if op.get("kind") == "func_new")
    assert definition["task_kind"] == "generator"
    poll = next(
        func for func in ir["functions"] if func["name"] == definition["s_value"]
    )
    frame = gen.funcs_map[poll["name"]]["stateful_frame_plan"]
    assert frame.poll_symbol == poll["name"]
    assert not frame.has_closure
    frame_size = definition["task_closure_size"]
    assert isinstance(frame_size, int) and frame_size > 0 and frame_size % 8 == 0
    offsets = [
        op["value"]
        for op in poll["ops"]
        if op.get("kind") in {"closure_store", "closure_load"}
        and isinstance(op.get("value"), int)
    ]
    assert offsets and frame_size >= max(offsets) + 8
    producers = {op["out"]: op for op in func_ops if "out" in op}
    (metadata,) = (
        op
        for op in func_ops
        if op.get("kind") == "call"
        and op.get("s_value") == "molt_function_init_metadata_packed"
        and op["args"][0] == definition["out"]
    )
    packed = producers[metadata["args"][1]]
    assert packed["kind"] == "tuple_new" and len(packed["args"]) == 14
    assert producers[packed["args"][11]]["value"] == 1  # Immutable generator kind.
    (code_slot,) = (
        op
        for op in func_ops
        if op.get("kind") == "code_slot_set" and op["args"][0] == metadata["args"][2]
    )
    assert code_slot["value"] == gen.func_code_ids[poll["name"]]
    generator_call = _positional_call(func_ops, {definition["out"]}, 1)
    iterator = producers[generator_call["args"][1]]
    assert iterator["kind"] == "iter"
    assert iterator["args"][0] in _local_reads(func_ops, "data")
    sum_targets = _module_attr_accesses(func_ops, "module_get_global", "sum")
    sum_call = _positional_call(func_ops, sum_targets, 2)
    assert sum_call["args"][1] == generator_call["out"]
    assert sum_call["args"][2] in _local_reads(func_ops, "start")
    assert func_ops.index(generator_call) < func_ops.index(sum_call)
    assert not any(
        op.get("kind") in {"alloc_task", "loop_start", "add"}
        or op["kind"].startswith("callargs_")
        for op in func_ops
    )


def test_admitted_sum_generator_expr_target_shadow_does_not_leak() -> None:
    gen = SimpleTIRGenerator()
    gen.start_function("admitted_sum_shadow", params=["data"], param_types=["Any"])
    gen.locals["data"] = MoltValue("data", type_hint="Any")
    outer = MoltValue("outer_v", type_hint="int")
    gen.locals["v"] = outer
    node = ast.parse("sum(v for v in data)", mode="eval").body
    assert isinstance(node, ast.Call)

    gen._emit_sum_call("sum", node, needs_bind=False)

    assert gen.locals["v"] is outer
    assert not any(
        op.kind in {"ALLOC_TASK", "FUNC_NEW", "BUILTIN_FUNC", "CALL_FUNC", "CALL_BIND"}
        or (op.metadata or {}).get("task_kind") == "generator"
        for op in gen.current_ops
    )


@pytest.mark.parametrize("name", ["any", "all"])
def test_admitted_any_all_generator_expr_use_scalar_result_slots(name: str) -> None:
    # Exercise the reducer after callable admission, not by assuming that an
    # unbound source spelling in a deferred function proves builtin identity.
    gen = SimpleTIRGenerator()
    gen.start_function("admitted_reduction", params=["data"], param_types=["Any"])
    gen.locals["data"] = MoltValue("data", type_hint="Any")
    node = ast.parse(f"{name}(v for v in data)", mode="eval").body
    assert isinstance(node, ast.Call)
    result = gen._emit_any_all_call(name, node, needs_bind=False)
    ops = gen.current_ops
    stores = [
        op
        for op in ops
        if op.kind == "STORE_VAR"
        and op.metadata.get("var", "").startswith(f"__molt_{name}_result_")
    ]
    assert len(stores) == 2
    slot = stores[0].metadata["var"]
    assert stores[1].metadata["var"] == slot
    producers = {op.result.name: op for op in ops}
    initial = producers[stores[0].args[0].name]
    terminal = producers[stores[1].args[0].name]
    assert initial.kind == terminal.kind == "CONST_BOOL"
    assert initial.args == [name == "all"]
    assert terminal.args == [name == "any"]
    (load,) = (
        op for op in ops if op.kind == "LOAD_VAR" and op.metadata.get("var") == slot
    )
    assert load.result is result
    assert result.type_hint == "bool"
    assert sum(op.kind == "LOOP_START" for op in ops) == 1
    assert sum(op.kind == "LOOP_END" for op in ops) == 1
    assert sum(op.kind == "LOOP_BREAK" for op in ops) == 1
    assert (
        ops.index(stores[1])
        < next(i for i, op in enumerate(ops) if op.kind == "LOOP_BREAK")
        < ops.index(load)
    )
    assert not any(
        op.kind in {"ALLOC_TASK", "LIST_NEW", "BUILTIN_FUNC"}
        or (op.metadata or {}).get("task_kind") == "generator"
        for op in ops
    )


@pytest.mark.parametrize("name", ["sum", "any", "all"])
@pytest.mark.parametrize("binding", ["deferred_global", "parameter", "rebound_global"])
def test_reducer_generator_expr_preserves_unproven_callable_and_generator(
    name: str, binding: str
) -> None:
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )

    parameters = f"data, {name}" if binding == "parameter" else "data"
    source = f"def f({parameters}):\n    return {name}(v for v in data)\n"
    if binding == "rebound_global":
        source += f"{name} = replacement\n"
    node = next(
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.Call)
    )
    fact = analyze_python_source_bindings(source).call_fact(node)
    assert fact is not None
    assert fact.exact_builtin_name() is None
    assert not fact.callee_elision_safe

    ir = compile_to_tir(source)
    function = next(func for func in ir["functions"] if func["name"] == "__main____f")
    ops = function["ops"]
    targets = (
        _local_reads(ops, name)
        if binding == "parameter"
        else _module_attr_accesses(ops, "module_get_global", name)
    )
    call = _positional_call(ops, targets, 1, parameters=function["params"])
    (definition,) = (op for op in ops if op.get("task_kind") == "generator")
    assert definition["kind"] == "func_new"
    assert any(func["name"] == definition["s_value"] for func in ir["functions"])
    generator_call = _positional_call(ops, {definition["out"]}, 1)
    producers = {op["out"]: op for op in ops if "out" in op}
    iterator = producers[generator_call["args"][1]]
    assert iterator["kind"] == "iter"
    assert iterator["args"][0] in _local_reads(ops, "data")
    assert call["args"][1] == generator_call["out"]
    assert ops.index(iterator) < ops.index(generator_call) < ops.index(call)
    if binding != "parameter":
        # Capture the callable before eager outer-iterator acquisition, which
        # can invoke Python and replace the module's reducer binding.
        assert ops.index(producers[call["args"][0]]) < ops.index(iterator)
    result_prefix = "__molt_sum_acc_" if name == "sum" else f"__molt_{name}_result_"
    assert not any(
        op.get("kind") in {"alloc_task", "loop_start"}
        or op.get("var", "").startswith(result_prefix)
        or (
            op.get("kind") == "builtin_func"
            and op.get("s_value") == f"molt_{name}_builtin"
        )
        for op in ops
    )


def test_globals_pop_specializes_only_with_live_builtin_identity() -> None:
    ir = compile_to_tir(
        "globals().pop('_require_intrinsic', None)\n"
        "def f():\n"
        "    return globals().pop('_require_intrinsic', None)\n"
    )

    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert any(op.get("kind") == "dict_pop" for op in main_ops)
    assert all(op.get("kind") != "call_indirect" for op in main_ops)
    deferred_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    # The module may replace globals before the deferred function runs.
    live_globals = _module_attr_accesses(deferred_ops, "module_get_global", "globals")
    globals_call = _positional_call(deferred_ops, live_globals, 0)
    (pop_attr,) = (
        op
        for op in deferred_ops
        if op.get("kind") == "get_attr_generic_obj"
        and op.get("s_value") == "pop"
        and op["args"] == [globals_call["out"]]
    )
    pop_call = _positional_call(deferred_ops, {pop_attr["out"]}, 2)
    supplied = pop_call["args"][1:]
    producers = {op["out"]: op for op in deferred_ops if "out" in op}
    key = producers[supplied[0]]
    assert key["kind"] == "const_str" and key["s_value"] == "_require_intrinsic"
    assert producers[supplied[1]]["kind"] == "const_none"
    assert not any(op["kind"].startswith("callargs_") for op in deferred_ops)
    assert not any(op.get("kind") == "dict_pop" for op in deferred_ops)


@pytest.mark.parametrize("name", ["dict", "globals", "locals", "vars"])
def test_shadowed_dictionary_constructor_does_not_bypass_method_dispatch(
    name: str,
) -> None:
    gen = SimpleTIRGenerator()
    gen.visit(
        ast.parse(
            f"def f({name}):\n    value = {name}()\n    return value.pop('key', None)\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"] == "__main____f"
    )
    ops = function["ops"]
    assert not any(op.get("kind") == "dict_pop" for op in ops)
    constructor = _positional_call(
        ops,
        _local_reads(ops, name),
        0,
        parameters=function["params"],
    )
    assert any(
        op.get("kind") == "store_var"
        and op.get("var") == "value"
        and op["args"] == [constructor["out"]]
        for op in ops
    )
    (attribute,) = (
        op
        for op in ops
        if op.get("kind") == "get_attr_generic_obj" and op.get("s_value") == "pop"
    )
    assert attribute["args"][0] in _local_reads(ops, "value")
    call = _positional_call(ops, {attribute["out"]}, 2)
    producers = {op["out"]: op for op in ops if "out" in op}
    key = producers[call["args"][1]]
    assert key["kind"] == "const_str" and key["s_value"] == "key"
    assert producers[call["args"][2]]["kind"] == "const_none"
    assert not any(op["kind"].startswith("callargs_") for op in ops)


def test_dict_comprehension_result_methods_use_exact_dict_ops() -> None:
    ir = compile_to_tir(
        "def f():\n"
        "    data = {i: i for i in (0, 1, 2, 3, 4)}\n"
        "    values = data.values()\n"
        "    inverted = {v: k for k, v in data.items()}\n"
        "    return values, inverted\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "dict_values" for op in func_ops)
    assert any(op.get("kind") == "dict_items" for op in func_ops)
    assert not any(
        op.get("kind") == "get_attr_generic_obj"
        and op.get("s_value") in {"values", "items"}
        for op in func_ops
    )
    assert all(op.get("kind") != "call_indirect" for op in func_ops)


def test_internal_module_function_import_calls_live_binding() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"molt.gpu.tensor"},
        direct_call_modules={"molt.gpu.tensor"},
    )
    gen.visit(
        ast.parse(
            "from molt.gpu.tensor import tensor_linear\n"
            "def f(x, w):\n"
            "    return tensor_linear(x, w)\n"
        )
    )
    ir = gen.to_json()
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    call = _positional_call(
        func_ops,
        _module_attr_accesses(func_ops, "module_get_global", "tensor_linear"),
        2,
    )
    assert call["args"][1] in _local_reads(func_ops, "x")
    assert call["args"][2] in _local_reads(func_ops, "w")
    assert not any(
        op.get("kind") in {"call", "call_guarded"}
        and op.get("s_value") == "molt_gpu_tensor__tensor_linear"
        for op in func_ops
    ), func_ops
    assert all(
        op.get("kind") not in {"call_bind", "callargs_new"} for op in func_ops
    ), func_ops


def test_internal_module_imported_class_ctor_calls_live_binding() -> None:
    ir = compile_to_tir(
        "from molt.gpu.tensor import Tensor\ndef f(x):\n    return Tensor(x)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "Tensor"), 1
    )
    assert call["args"][1] in _local_reads(func_ops, "x")
    assert all(
        not (
            op.get("kind") == "call" and op.get("s_value") == "molt_gpu_tensor__Tensor"
        )
        for op in func_ops
    ), func_ops


def test_internal_module_vararg_function_import_preserves_supplied_tuple() -> None:
    ir = compile_to_tir(
        "from molt.gpu.tensor import zeros\ndef f():\n    return zeros((2, 3))\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "zeros"), 1
    )
    producers = {op["out"]: op for op in func_ops if "out" in op}
    shape = producers[call["args"][1]]
    assert shape["kind"] == "tuple_new"
    assert [producers[arg]["value"] for arg in shape["args"]] == [2, 3]
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "molt_gpu_tensor__zeros")
        for op in func_ops
    ), func_ops


@pytest.fixture(scope="module")
def tensor_module_ops() -> dict[str, list[dict[str, object]]]:
    path = Path("src/molt/gpu/tensor.py")
    # Lower once, using real module identity so relative imports have custody.
    gen = SimpleTIRGenerator(module_name="molt.gpu.tensor", source_path=str(path))
    gen.visit(ast.parse(path.read_text(encoding="utf-8")))
    return {
        function["name"]: function["ops"] for function in gen.to_json()["functions"]
    }


def test_tensor_linear_uses_internal_fast_tensor_wrap_helper(tensor_module_ops) -> None:
    func_ops = tensor_module_ops["molt_gpu_tensor__tensor_linear"]
    helpers = _module_attr_accesses(func_ops, "module_get_global", "_tensor_from_parts")
    call = _positional_call(func_ops, helpers, 6)
    bits, dtype, size, fmt, shape, tensor_dtype = call["args"][1:]
    assert bits in _local_reads(func_ops, "out_bits")
    assert dtype in _local_reads(func_ops, "result_dtype")
    assert fmt in _local_reads(func_ops, "result_format")
    assert shape in _local_reads(func_ops, "out_shape")
    assert tensor_dtype in _local_reads(func_ops, "result_dtype")
    product = next(op for op in func_ops if op.get("out") == size)
    assert product["kind"] == "mul"
    assert product["args"][0] in _local_reads(func_ops, "outer")
    assert product["args"][1] in _local_reads(func_ops, "out_features")


@pytest.mark.parametrize(
    "function, helper",
    [
        ("tensor_reshape_view", "_tensor_from_buffer"),
        ("tensor_data_list", "_buffer_to_list"),
    ],
)
def test_tensor_view_helpers_use_internal_fast_wrap_helpers(
    tensor_module_ops, function: str, helper: str
) -> None:
    ops = tensor_module_ops[f"molt_gpu_tensor__{function}"]
    helpers = _module_attr_accesses(ops, "module_get_global", helper)
    reshape = function == "tensor_reshape_view"
    call = _positional_call(ops, helpers, 3 if reshape else 2)
    producers = {op["out"]: op for op in ops if "out" in op}
    attrs = [(call["args"][1], "_buf")]
    if reshape:
        assert call["args"][2] in _local_reads(ops, "shape")
        attrs.append((call["args"][3], "_dtype"))
    else:
        attrs.append((call["args"][2], "size"))
    for value, attr in attrs:
        access = producers[value]
        assert access["kind"] == "get_attr_generic_obj"
        assert access["s_value"] == attr
        assert access["args"][0] in _local_reads(ops, "x")


def test_internal_module_intrinsic_alias_import_calls_live_binding() -> None:
    gen = SimpleTIRGenerator(
        source_path="src/molt/stdlib/abc.py",
        module_name="abc",
        stdlib_allowlist={"abc", "_abc"},
        known_modules={"abc", "_abc"},
        known_func_defaults={"abc": {}, "_abc": {}},
    )
    gen.visit(
        ast.parse("from _abc import _abc_init\ndef f(x):\n    return _abc_init(x)\n")
    )
    ir = gen.to_json()
    func_ops = next(func["ops"] for func in ir["functions"] if func["name"] == "abc__f")
    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "_abc_init"), 1
    )
    assert call["args"][1] in _local_reads(func_ops, "x")
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "_abc___abc_init")
        for op in func_ops
    ), func_ops


def test_tensor_linear_family_helpers_inline_result_format_selection(
    tensor_module_ops,
) -> None:
    for func_name in (
        "molt_gpu_tensor__tensor_linear",
        "molt_gpu_tensor__tensor_linear_split_last_dim",
        "molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved",
    ):
        func_ops = tensor_module_ops[func_name]
        assert not _module_attr_accesses(
            func_ops, "module_get_global", "_preferred_float_format"
        )
        assert all(
            not (
                op.get("kind") == "call"
                and op.get("s_value") == "molt_gpu_tensor___preferred_float_format"
            )
            for op in func_ops
        ), (func_name, func_ops)


def _assert_optional_loader_is_python_call(
    ir: dict[str, object], intrinsic_name: str
) -> None:
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")
    producers = {op["out"]: op for op in ops if "out" in op}
    (publication,) = (
        op
        for op in ops
        if op.get("kind") == "module_set_attr"
        and producers[op["args"][1]].get("s_value") == "_load_optional_intrinsic"
    )
    created = producers[publication["args"][2]]
    assert created["kind"] == "func_new"
    assert created["s_value"] in {func["name"] for func in ir["functions"]}
    targets = _module_attr_accesses(
        ops, "module_get_global", "_load_optional_intrinsic"
    )
    # During module initialization the published, freshly created callable is
    # also an authoritative operand; a redundant global reload is not required.
    targets.append(created["out"])
    (call,) = (
        op
        for op in ops
        if op.get("kind") in {"call_func", "call_guarded"} and op["args"][0] in targets
    )
    assert len(call["args"]) == 2
    assert ops.index(created) < ops.index(publication) < ops.index(call)
    if call["kind"] == "call_guarded":
        assert call["args"][0] == created["out"]
        assert call["s_value"] == created["s_value"]
    callee_index = ops.index(producers[call["args"][0]])
    assert not any(
        op["kind"].startswith("callargs_") for op in ops[callee_index : ops.index(call)]
    )
    argument = producers[call["args"][1]]
    assert argument["kind"] == "const_str" and argument["s_value"] == intrinsic_name
    assert any(
        op.get("kind") == "module_set_attr"
        and producers[op["args"][1]].get("s_value") == "_MOLT_GPU"
        and op["args"][2] == call["out"]
        for op in ops
    )
    assert not any(
        op.get("kind") == "builtin_func" and op.get("s_value") == intrinsic_name
        for op in ops
    )


@pytest.mark.parametrize("annotation", ["", ": object"])
def test_fake_optional_intrinsic_loader_preserves_python_calls(annotation: str) -> None:
    ir = compile_to_tir(
        "def _load_optional_intrinsic(name):\n"
        "    return None\n"
        f"_MOLT_GPU{annotation} = _load_optional_intrinsic('molt_gpu_linear_contiguous')\n"
        "def f(a, b, c, d, e, f0, g, h):\n"
        "    if _MOLT_GPU is not None:\n"
        "        return _MOLT_GPU(a, b, c, d, e, f0, g, h)\n"
        "    return None\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    _assert_optional_loader_is_python_call(ir, "molt_gpu_linear_contiguous")
    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "_MOLT_GPU"), 8
    )
    for argument, name in zip(
        call["args"][1:], ("a", "b", "c", "d", "e", "f0", "g", "h")
    ):
        assert argument in _local_reads(func_ops, name)
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "molt_gpu_linear_contiguous"
        for op in func_ops
    ), func_ops


@pytest.mark.parametrize("annotation", ["", ": object"])
def test_fake_optional_intrinsic_loader_does_not_reserve_python_symbol(
    annotation: str,
) -> None:
    source = (
        "def _load_optional_intrinsic(name):\n"
        "    return None\n"
        f"_MOLT_GPU{annotation} = _load_optional_intrinsic("
        "'molt_gpu_tensor__tensor_linear_split_last_dim')\n"
        "def tensor_linear_split_last_dim(a, b, c):\n"
        "    if _MOLT_GPU is not None:\n"
        "        return _MOLT_GPU(a, b, c)\n"
        "    return None\n"
    )
    tree = ast.parse(source, filename="molt/gpu/tensor.py")
    gen = SimpleTIRGenerator(
        source_path="molt/gpu/tensor.py",
        module_name="molt.gpu.tensor",
        entry_module="molt.gpu.tensor",
    )
    gen.visit(tree)
    ir = gen.to_json()

    function_names = [func["name"] for func in ir["functions"]]
    assert "molt_gpu_tensor__tensor_linear_split_last_dim" in function_names
    assert not any(
        name.startswith("molt_gpu_tensor__tensor_linear_split_last_dim_")
        for name in function_names
    ), function_names

    func_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == "molt_gpu_tensor__tensor_linear_split_last_dim"
    )
    _assert_optional_loader_is_python_call(
        ir, "molt_gpu_tensor__tensor_linear_split_last_dim"
    )
    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "_MOLT_GPU"), 3
    )
    for argument, name in zip(call["args"][1:], ("a", "b", "c")):
        assert argument in _local_reads(func_ops, name)
    assert not any(
        op.get("kind") == "call"
        and op.get("s_value") == "molt_gpu_tensor__tensor_linear_split_last_dim"
        for op in func_ops
    ), func_ops


def test_getattr_without_default_preserves_live_binding_and_two_arguments() -> None:
    ir = compile_to_tir("def f(obj):\n    return getattr(obj, 'missing_attr')\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    call = _positional_call(
        func_ops, _module_attr_accesses(func_ops, "module_get_global", "getattr"), 2
    )
    assert call["args"][1] in _local_reads(func_ops, "obj")
    argument = next(op for op in func_ops if op.get("out") == call["args"][2])
    assert argument["kind"] == "const_str" and argument["s_value"] == "missing_attr"
    assert not any(
        op.get("kind") in {"get_attr_name", "get_attr_name_default"} for op in func_ops
    ), func_ops
