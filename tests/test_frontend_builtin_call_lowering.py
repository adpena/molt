from __future__ import annotations

import ast
from pathlib import Path

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator, compile_to_tir


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


def test_builtins_import_alias_float_lowers_as_builtin_constructor() -> None:
    ir = compile_to_tir(
        "from builtins import float as _float\n"
        "def f(value):\n"
        "    return _float(value)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    assert any(op.get("kind") == "float_from_obj" for op in func_ops), func_ops


def test_builtin_exception_constructor_uses_canonical_tagged_lane() -> None:
    ir = compile_to_tir("def f(i):\n    return ValueError(i)\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    builtin_ops = [
        op for op in func_ops if op.get("kind") == "exception_new_builtin_one"
    ]

    assert builtin_ops
    assert builtin_ops[0]["s_value"] == "ValueError"
    assert builtin_ops[0]["value"] == 5
    assert not any(op.get("kind") == "tuple_new" for op in func_ops)
    assert not any(op.get("kind") == "exception_new" for op in func_ops)


def test_empty_builtin_exception_constructor_skips_args_tuple() -> None:
    ir = compile_to_tir("def f():\n    return ValueError()\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    builtin_ops = [
        op for op in func_ops if op.get("kind") == "exception_new_builtin_empty"
    ]

    assert builtin_ops
    assert builtin_ops[0]["s_value"] == "ValueError"
    assert builtin_ops[0]["value"] == 5
    assert not any(op.get("kind") == "tuple_new" for op in func_ops)


def test_multi_arg_builtin_exception_keeps_tuple_constructor() -> None:
    ir = compile_to_tir("def f(i):\n    return ValueError(i, i + 1)\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    builtin_ops = [op for op in func_ops if op.get("kind") == "exception_new_builtin"]

    assert builtin_ops
    assert builtin_ops[0]["s_value"] == "ValueError"
    assert builtin_ops[0]["value"] == 5
    assert any(op.get("kind") == "tuple_new" for op in func_ops)


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
    assert next_control == {"kind": "jump", "value": handler_label}

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
    flag_loads = _module_attr_accesses(main_ops, "module_get_attr", "flag")
    print_args = [
        args[0]
        for op in main_ops
        if op.get("kind") == "print"
        and isinstance(args := op.get("args"), list)
        and args
    ]
    assert any(load in print_args for load in flag_loads)


def test_sync_try_except_keeps_context_unwind_when_body_enters_with() -> None:
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

    assert any(op.get("kind") == "context_depth" for op in func_ops)
    assert any(op.get("kind") == "context_unwind_to" for op in func_ops)


def test_break_inside_try_except_nested_in_with_does_not_unwind_unmarked_try() -> None:
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

    assert any(op.get("kind") == "context_depth" for op in func_ops)
    assert not any(op.get("kind") == "context_unwind_to" for op in func_ops)


def test_return_inside_try_nested_in_with_unwinds_only_marked_scopes() -> None:
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

    assert any(op.get("kind") == "context_depth" for op in func_ops)
    assert any(op.get("kind") == "context_unwind_to" for op in func_ops)


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


def test_local_inner_import_intrinsic_wrapper_lowers_known_intrinsic() -> None:
    source = (
        "def _require_intrinsic(name: str, namespace: dict[str, object] | None = None):\n"
        "    from _intrinsics import require_intrinsic as _require\n"
        "    return _require(name, namespace)\n"
        "_HOOK = _require_intrinsic('molt_importlib_module_spec_is_package')\n"
    )
    assert _has_builtin_func(source, "molt_importlib_module_spec_is_package")


def test_intrinsic_require_lowers_to_public_runtime_symbol() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "_HOOK = _require_intrinsic('molt_async_sleep')\n"
    )
    assert not _has_runtime_intrinsic_lookup_call(source, "molt_async_sleep")
    assert _has_builtin_func(source, "molt_async_sleep")
    assert _has_builtin_func(source, "molt_require_intrinsic_runtime")


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
    }
    assert not any(
        op.kind == "SETATTR_GENERIC_OBJ"
        and len(op.args) >= 2
        and isinstance(op.args[1], str)
        and op.args[1] in metadata_attrs
        for op in ops
    )


def test_frontend_intrinsic_function_objects_carry_manifest_defaults() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "length_hint = _require_intrinsic('molt_operator_length_hint')\n"
    )
    gen = SimpleTIRGenerator(module_name="intrinsic_defaults_probe")
    gen.visit(ast.parse(source))

    ops = [op for func in gen.funcs_map.values() for op in func["ops"]]
    builtin_index = next(
        idx
        for idx, op in enumerate(ops)
        if op.kind == "BUILTIN_FUNC" and op.args == ["molt_operator_length_hint", 2]
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
        if op.get("kind") == "store_index"
        and len(op.get("args", [])) == 3
        and op["args"][2] == data_literal_var
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


def test_imported_class_ctor_avoids_cross_module_name_collision() -> None:
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
    gen.visit(ast.parse("from pathlib import Path\nPath('x')\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    const_str: dict[str, str] = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str" and isinstance(op.get("out"), str)
    }
    pathlib_module_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_cache_get"
        and len(op.get("args") or []) == 1
        and const_str.get(op["args"][0]) == "pathlib"
    }
    path_class_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_attr"
        and len(op.get("args") or []) == 2
        and op["args"][0] in pathlib_module_vars
        and const_str.get(op["args"][1]) == "Path"
    }
    assert path_class_vars, "expected pathlib.Path lookup in lowered main ops"
    assert any(
        op.get("kind") == "call_bind"
        and len(op.get("args") or []) >= 1
        and op["args"][0] in path_class_vars
        for op in main_ops
    ), "expected imported pathlib.Path constructor to lower via call_bind"
    assert all(
        op.get("s_value") != "zipfile__path__Path___init__"
        for op in main_ops
        if op.get("kind") == "call"
    ), "main lowering should not hardwire zipfile._path.Path.__init__ for pathlib.Path"


def test_imported_known_vararg_function_call_bind_uses_imported_value() -> None:
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
    assert any(
        op.get("kind") == "call_bind"
        and len(op.get("args") or []) == 2
        and op["args"][0] in imported_typevar_values
        for op in main_ops
    )
    assert all(
        not (
            op.get("kind") == "call_bind"
            and len(op.get("args") or []) == 2
            and op["args"][0] == "TypeVar"
        )
        for op in main_ops
    )


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


def test_imported_counter_list_constructor_uses_intrinsic_handle_path() -> None:
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


def test_module_counter_list_constructor_uses_intrinsic_handle_path() -> None:
    gen = SimpleTIRGenerator(
        known_classes=_counter_known_classes(),
        known_modules={"collections"},
        stdlib_allowlist={"collections"},
    )
    gen.visit(
        ast.parse(
            "import collections\n"
            'words = "a b a".split()\n'
            "c = collections.Counter(words)\n"
        )
    )
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
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


def test_stdlib_direct_call_uses_symbol_when_target_module_is_lowered() -> None:
    gen = SimpleTIRGenerator(
        module_name="collections",
        known_modules={"collections", "copy"},
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

    assert any(
        op.get("kind") == "call" and op.get("s_value") == "copy__copy"
        for op in func_ops
    )
    assert all(op.get("kind") != "call_bind" for op in func_ops)


def test_imported_plain_generator_uses_poll_task_symbol() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"tinygrad.engine.realize"},
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

    assert any(
        op.get("kind") == "alloc_task"
        and op.get("s_value") == "tinygrad_engine_realize__unwrap_multi_poll"
        and op.get("task_kind") == "generator"
        for op in func_ops
    ), func_ops
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

    assert any(op.get("kind") == "call_bind" for op in func_ops), func_ops
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


def test_from_import_generator_kind_lowers_to_poll_task_with_defaults() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
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

    assert any(
        op.get("kind") == "alloc_task"
        and op.get("s_value") == "helpers__cpu_profile_poll"
        and op.get("task_kind") == "generator"
        and len(op.get("args") or []) == 3
        for op in func_ops
    )
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
    alloc_tasks = [
        op
        for op in func_ops
        if op.get("kind") == "alloc_task"
        and op.get("s_value") == "helpers__cpu_profile_poll"
    ]

    assert alloc_tasks
    assert all(
        isinstance(op.get("value"), int) and op["value"] >= 16 for op in alloc_tasks
    )
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )


def test_assigned_alias_of_imported_generator_preserves_poll_task_defaults() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
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
    alloc_task = next(
        op
        for op in func_ops
        if op.get("kind") == "alloc_task"
        and op.get("s_value") == "helpers__cpu_profile_poll"
    )

    assert alloc_task.get("task_kind") == "generator"
    assert len(alloc_task.get("args") or []) == 2
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )


def test_aliased_import_async_generator_lowers_to_generator_task_then_asyncgen() -> (
    None
):
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
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
    alloc_task = next(
        op
        for op in func_ops
        if op.get("kind") == "alloc_task"
        and op.get("s_value") == "helpers__events_poll"
    )
    asyncgen_new = [op for op in func_ops if op.get("kind") == "asyncgen_new"]

    assert alloc_task.get("task_kind") == "generator"
    assert len(alloc_task.get("args") or []) == 2
    assert asyncgen_new
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__events"
        for op in func_ops
    )
    assert not any(
        op.get("kind") == "call_bind" and op.get("s_value") == "helpers__events"
        for op in func_ops
    )


def test_imported_module_attr_generator_uses_same_poll_task_path() -> None:
    gen = SimpleTIRGenerator(
        module_name="main",
        known_modules={"helpers", "main"},
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
    alloc_task = next(
        op
        for op in func_ops
        if op.get("kind") == "alloc_task"
        and op.get("s_value") == "helpers__cpu_profile_poll"
    )

    assert alloc_task.get("task_kind") == "generator"
    assert len(alloc_task.get("args") or []) == 2
    assert not any(
        op.get("kind") == "call" and op.get("s_value") == "helpers__cpu_profile"
        for op in func_ops
    )


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


def test_collections_namedtuple_kwonly_defaults_use_call_bind() -> None:
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
    gen.visit(
        ast.parse("from collections import namedtuple\nT = namedtuple('T', ['x'])\n")
    )
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
    imported_namedtuple_values = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_import_from"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "namedtuple"
    }

    assert imported_namedtuple_values, "expected from-import to materialize namedtuple"
    assert any(
        op.get("kind") == "call_bind"
        and len(op.get("args") or []) == 2
        and op["args"][0] in imported_namedtuple_values
        for op in main_ops
    ), main_ops
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

    max_var = next(
        op["out"]
        for op in main_ops
        if op.get("kind") == "builtin_func" and op.get("s_value") == "molt_max_builtin"
    )
    assert any(
        op.get("kind") == "call_func"
        and isinstance(op.get("args"), list)
        and op["args"]
        and op["args"][0] == max_var
        for op in main_ops
    ), main_ops
    builtin_names = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "builtin_func"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    metadata_targets = {
        op["args"][1]
        for op in main_ops
        if op.get("kind") == "call_func"
        and isinstance(op.get("args"), list)
        and len(op["args"]) >= 2
        and builtin_names.get(op["args"][0]) == "molt_function_init_metadata_packed"
    }
    assert max_var not in metadata_targets


def test_counter_index_and_len_use_intrinsic_handle_path() -> None:
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
            'x = c["a"]\n'
            "n = len(c)\n"
        )
    )
    main_ops = next(
        func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] == "molt_main"
    )

    assert any(
        op.get("kind") == "builtin_func" and op.get("s_value") == "molt_counter_getitem"
        for op in main_ops
    )
    assert any(
        op.get("kind") == "builtin_func" and op.get("s_value") == "molt_counter_len"
        for op in main_ops
    )
    assert all(op.get("kind") != "index" for op in main_ops)
    assert all(op.get("kind") != "len" for op in main_ops)


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


def test_stable_user_class_ctor_lowers_to_structural_allocation() -> None:
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

    assert any(op.get("kind") == "object_new_bound" for op in make_ops)
    assert any(op.get("kind") == "store_init" for op in make_ops)
    assert any(op.get("kind") == "store" for op in make_ops)
    assert all(op.get("kind") != "call_bind" for op in make_ops)
    assert all(op.get("kind") != "callargs_new" for op in make_ops)


def test_finalizer_user_class_ctor_call_bind_carries_finalizer_fact() -> None:
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
    call_binds = [op for op in make_ops if op.get("kind") == "call_bind"]

    assert len(call_binds) == 1
    assert call_binds[0].get("type_hint") == "Item"
    assert call_binds[0].get("defines_del") is True
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
        for idx, op in enumerate(ops)
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
        if op.get("kind") == "store_index"
        and len(op.get("args") or []) == 3
        and op["args"][2] in missing_defs
    )
    dec_idx, old_var = next(
        (idx, op["args"][0])
        for idx, op in enumerate(ops[store_idx + 1 :], start=store_idx + 1)
        if op.get("kind") == "dec_ref"
        and len(op.get("args") or []) == 1
        and isinstance(op["args"][0], str)
    )
    old_def_idx = next(
        idx
        for idx, op in enumerate(ops[:store_idx])
        if op.get("kind") == "index" and op.get("out") == old_var
    )

    assert old_def_idx < store_idx < dec_idx


def test_unstable_globals_user_class_ctor_lowers_via_call_bind() -> None:
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
        if op.get("kind") == "module_get_attr"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "A"
    }
    assert class_vars, "expected local class lookup in lowered module chunk"
    assert any(
        op.get("kind") == "call_bind"
        and len(op.get("args") or []) >= 1
        and op["args"][0] in class_vars
        for op in main_ops
    ), (
        "expected globals-escaped class constructor to lower via call_bind on the class object"
    )
    assert all(
        op.get("kind")
        not in {"alloc_class", "alloc_class_static", "alloc_class_trusted"}
        for op in main_ops
    ), (
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


def test_importlib_import_module_literal_in_function_lowers_to_import_module_leaf() -> (
    None
):
    func_ops = _importlib_literal_function_ops(
        "import importlib\ndef f():\n    return importlib.import_module('json')\n",
        "__main____f",
    )
    assert "json" in _importlib_import_module_targets(func_ops)
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


def test_importlib_import_module_dynamic_requires_default_metadata_for_static_call() -> (
    None
):
    func_ops = _importlib_literal_function_ops(
        "import importlib\ndef f(name):\n    return importlib.import_module(name)\n",
        "__main____f",
    )

    assert not _has_static_call(func_ops, "importlib__import_module")
    assert "json" not in _importlib_import_module_targets(func_ops)
    assert any(op.get("kind") == "call_bind" for op in func_ops)


def test_sum_generator_expr_lowers_without_generator_task_or_builtin_call() -> None:
    ir = compile_to_tir("def f(data):\n    return sum(v for v in data if v % 2 == 0)\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "loop_start" for op in func_ops)
    assert any(op.get("kind") == "add" for op in func_ops)
    assert all(op.get("kind") != "alloc_task" for op in func_ops)
    assert not any(
        op.get("kind") == "builtin_func" and op.get("s_value") == "molt_sum_builtin"
        for op in func_ops
    )
    assert not any(
        op.get("kind") == "call_func"
        and any(
            isinstance(arg, str) and arg.startswith("v") for arg in op.get("args") or []
        )
        for op in func_ops
    )


def test_sum_listcomp_lowers_as_full_consumption_reducer() -> None:
    ir = compile_to_tir("def f(data):\n    return sum([v * 2 for v in data if v > 3])\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "loop_start" for op in func_ops)
    assert any(op.get("kind") == "mul" for op in func_ops)
    assert any(op.get("kind") == "add" for op in func_ops)
    assert all(op.get("kind") != "alloc_task" for op in func_ops)
    assert not any(
        op.get("kind") == "builtin_func" and op.get("s_value") == "molt_sum_builtin"
        for op in func_ops
    )


def test_sum_generator_expr_tuple_target_lowers_inline() -> None:
    ir = compile_to_tir(
        "def f(pairs):\n    return sum(a * b for a, b in pairs if a > 2)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "unpack_sequence" for op in func_ops)
    assert any(op.get("kind") == "mul" for op in func_ops)
    assert any(op.get("kind") == "add" for op in func_ops)
    assert all(op.get("kind") != "alloc_task" for op in func_ops)


def test_sum_generator_expr_with_start_stays_on_builtin_path() -> None:
    ir = compile_to_tir(
        "def f(data, start):\n    return sum((v for v in data), start)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert any(op.get("kind") == "alloc_task" for op in func_ops)
    assert any(
        op.get("kind") == "builtin_func" and op.get("s_value") == "molt_sum_builtin"
        for op in func_ops
    )


def test_sum_generator_expr_target_shadow_does_not_leak() -> None:
    ir = compile_to_tir(
        "def f(data):\n"
        "    v = 10\n"
        "    total = sum(v for v in data)\n"
        "    return v + total\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )

    assert all(op.get("kind") != "alloc_task" for op in func_ops)
    store_vars = [
        op.get("var")
        for op in func_ops
        if op.get("kind") == "store_var" and isinstance(op.get("var"), str)
    ]
    assert "v" in store_vars


def test_any_all_generator_expr_use_scalar_result_slots() -> None:
    ir = compile_to_tir(
        "def f(data):\n"
        "    return any(v for v in data)\n"
        "def g(data):\n"
        "    return all(v for v in data)\n"
    )

    for func_name, slot_prefix in [
        ("__main____f", "__molt_any_result_"),
        ("__main____g", "__molt_all_result_"),
    ]:
        func_ops = next(
            func["ops"] for func in ir["functions"] if func["name"] == func_name
        )
        slots = {
            op.get("var")
            for op in func_ops
            if op.get("kind") in {"store_var", "load_var"}
            and isinstance(op.get("var"), str)
            and op.get("var", "").startswith(slot_prefix)
        }

        assert slots
        assert all(op.get("kind") != "list_new" for op in func_ops)
        assert any(op.get("kind") == "loop_break" for op in func_ops)
        assert any(
            op.get("kind") == "load_var" and op.get("var") in slots for op in func_ops
        )
        assert not any(
            op.get("kind") == "builtin_func"
            and op.get("s_value") in {"molt_any_builtin", "molt_all_builtin"}
            for op in func_ops
        )


def test_globals_pop_lowers_to_exact_dict_pop() -> None:
    ir = compile_to_tir(
        "globals().pop('_require_intrinsic', None)\n"
        "def f():\n"
        "    return globals().pop('_require_intrinsic', None)\n"
    )

    for func_name in ["molt_main", "__main____f"]:
        func_ops = next(
            func["ops"] for func in ir["functions"] if func["name"] == func_name
        )

        assert any(op.get("kind") == "dict_pop" for op in func_ops)
        assert not any(
            op.get("kind") == "get_attr_generic_obj" and op.get("s_value") == "pop"
            for op in func_ops
        )
        assert all(op.get("kind") != "callargs_new" for op in func_ops)
        assert all(op.get("kind") != "call_indirect" for op in func_ops)


def test_dict_comprehension_result_methods_use_exact_dict_ops() -> None:
    ir = compile_to_tir(
        "def f():\n"
        "    data = {str(i): i for i in range(5)}\n"
        "    total = sum(v for v in data.values())\n"
        "    inverted = {v: k for k, v in data.items()}\n"
        "    return total + len(inverted)\n"
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


def test_internal_module_function_import_lowers_via_direct_call() -> None:
    ir = compile_to_tir(
        "from molt.gpu.tensor import tensor_linear\n"
        "def f(x, w):\n"
        "    return tensor_linear(x, w)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    assert any(
        op.get("kind") == "call"
        and op.get("s_value") == "molt_gpu_tensor__tensor_linear"
        for op in func_ops
    ), func_ops
    assert all(op.get("kind") != "call_bind" for op in func_ops), func_ops


def test_internal_module_imported_class_ctor_stays_on_call_bind() -> None:
    ir = compile_to_tir(
        "from molt.gpu.tensor import Tensor\ndef f(x):\n    return Tensor(x)\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    assert any(op.get("kind") == "call_bind" for op in func_ops), func_ops
    assert all(
        not (
            op.get("kind") == "call" and op.get("s_value") == "molt_gpu_tensor__Tensor"
        )
        for op in func_ops
    ), func_ops


def test_internal_module_vararg_function_import_stays_on_call_bind() -> None:
    ir = compile_to_tir(
        "from molt.gpu.tensor import zeros\ndef f():\n    return zeros((2, 3))\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    assert any(op.get("kind") == "call_bind" for op in func_ops), func_ops
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "molt_gpu_tensor__zeros")
        for op in func_ops
    ), func_ops


def test_tensor_linear_uses_internal_fast_tensor_wrap_helper() -> None:
    source = Path("src/molt/gpu/tensor.py").read_text(encoding="utf-8")
    ir = compile_to_tir(source)
    func_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == "__main____tensor_linear"
    )
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "__main_____tensor_from_parts"
        for op in func_ops
    ), func_ops


def test_tensor_view_helpers_use_internal_fast_wrap_helpers() -> None:
    source = Path("src/molt/gpu/tensor.py").read_text(encoding="utf-8")
    ir = compile_to_tir(source)
    reshape_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == "__main____tensor_reshape_view"
    )
    data_list_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"] == "__main____tensor_data_list"
    )
    assert any(
        op.get("kind") == "call"
        and op.get("s_value") == "__main_____tensor_from_buffer"
        for op in reshape_ops
    ), reshape_ops
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "__main_____buffer_to_list"
        for op in data_list_ops
    ), data_list_ops


def test_internal_module_intrinsic_alias_import_stays_on_call_bind() -> None:
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
    assert any(op.get("kind") == "call_bind" for op in func_ops), func_ops
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "_abc___abc_init")
        for op in func_ops
    ), func_ops


def test_tensor_linear_family_helpers_inline_result_format_selection() -> None:
    source = Path("src/molt/gpu/tensor.py").read_text(encoding="utf-8")
    ir = compile_to_tir(source)
    for func_name in (
        "__main____tensor_linear",
        "__main____tensor_linear_split_last_dim",
        "__main____tensor_linear_squared_relu_gate_interleaved",
    ):
        func_ops = next(
            func["ops"] for func in ir["functions"] if func["name"] == func_name
        )
        assert all(
            not (
                op.get("kind") == "call"
                and op.get("s_value") == "__main_____preferred_float_format"
            )
            for op in func_ops
        ), (func_name, func_ops)


def test_module_optional_intrinsic_global_call_lowers_directly() -> None:
    ir = compile_to_tir(
        "def _load_optional_intrinsic(name):\n"
        "    return None\n"
        "_MOLT_GPU = _load_optional_intrinsic('molt_gpu_linear_contiguous')\n"
        "def f(a, b, c, d, e, f0, g, h):\n"
        "    if _MOLT_GPU is not None:\n"
        "        return _MOLT_GPU(a, b, c, d, e, f0, g, h)\n"
        "    return None\n"
    )
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "molt_gpu_linear_contiguous"
        for op in func_ops
    ), func_ops


def test_module_optional_intrinsic_call_avoids_python_symbol_collision() -> None:
    import ast

    source = (
        "def _load_optional_intrinsic(name):\n"
        "    return None\n"
        "_MOLT_GPU = _load_optional_intrinsic('molt_gpu_tensor__tensor_linear_split_last_dim')\n"
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
    assert "molt_gpu_tensor__tensor_linear_split_last_dim" not in function_names
    assert any(
        name.startswith("molt_gpu_tensor__tensor_linear_split_last_dim_")
        for name in function_names
    ), function_names

    func_ops = next(
        func["ops"]
        for func in ir["functions"]
        if func["name"].startswith("molt_gpu_tensor__tensor_linear_split_last_dim_")
    )
    assert any(
        op.get("kind") == "call"
        and op.get("s_value") == "molt_gpu_tensor__tensor_linear_split_last_dim"
        for op in func_ops
    ), func_ops


def test_getattr_without_default_lowers_via_missing_default_path() -> None:
    ir = compile_to_tir("def f(obj):\n    return getattr(obj, 'missing_attr')\n")
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "__main____f"
    )
    assert any(op.get("kind") == "missing" for op in func_ops), func_ops
    assert any(op.get("kind") == "get_attr_name_default" for op in func_ops), func_ops
    assert not any(op.get("kind") == "get_attr_name" for op in func_ops), func_ops
