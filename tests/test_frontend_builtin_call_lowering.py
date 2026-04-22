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


def test_intrinsic_alias_lowers_to_canonical_runtime_symbol() -> None:
    source = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "_HOOK = _require_intrinsic('molt_async_sleep')\n"
    )
    assert not _has_runtime_intrinsic_lookup_call(source, "molt_async_sleep")
    assert _has_builtin_func(source, "molt_async_sleep_new")
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


def test_dotted_import_alias_uses_runtime_module_import_when_parent_allowlisted() -> (
    None
):
    gen = SimpleTIRGenerator(known_modules={"__main__"}, stdlib_allowlist={"os"})
    gen.visit(ast.parse("import os.path\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    targets = _module_import_targets(main_ops)
    assert "os.path" in targets


def test_local_user_class_ctor_lowers_via_call_bind() -> None:
    ir = compile_to_tir("class A:\n    pass\n\nA()\n")
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
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
    ), "expected local class constructor to lower via call_bind on the class object"
    assert all(
        op.get("kind")
        not in {"alloc_class", "alloc_class_static", "alloc_class_trusted"}
        for op in main_ops
    ), "local class constructor should not lower via synthetic object allocation"


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
    assert "sys" in _module_import_targets(main_ops)
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "molt_init_sys")
        for op in main_ops
    )


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
