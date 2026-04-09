from __future__ import annotations

import ast
from pathlib import Path

from molt.frontend import MoltValue, SimpleTIRGenerator, compile_to_tir


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


def test_chunked_stdlib_intrinsics_import_binding_survives_reset() -> None:
    source_path = (
        Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib" / "json" / "__init__.py"
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
    assert any(
        op.kind == "BUILTIN_FUNC" and op.args[0] == "molt_json_parse_scalar_obj"
        for func in gen.funcs_map.values()
        for op in func["ops"]
    )


def test_chunked_stdlib_intrinsics_value_binding_uses_runtime_require_intrinsic() -> None:
    source_path = Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib" / "sys.py"
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
    source = (
        "seed = 0\n"
        "limit = 1\n"
        "table = {\n"
        f"{entries}"
        "}\n"
    )
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
        op.get("kind") not in {"alloc_class", "alloc_class_static", "alloc_class_trusted"}
        for op in main_ops
    ), "local class constructor should not lower via synthetic object allocation"


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
