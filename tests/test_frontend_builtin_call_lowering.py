from __future__ import annotations

import ast

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


def test_zip_lowering_uses_call_bind() -> None:
    source = "print(list(zip([1, 2], [3, 4])))\n"
    assert _first_builtin_call_kind(source, "molt_zip_builtin") == "call_bind"


def test_map_lowering_uses_call_bind() -> None:
    source = "print(list(map(lambda x: x + 1, [1, 2, 3])))\n"
    assert _first_builtin_call_kind(source, "molt_map_builtin") == "call_bind"


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


def test_allowlisted_module_attr_call_falls_back_when_symbol_unavailable() -> None:
    gen = SimpleTIRGenerator(known_modules={"__main__"}, stdlib_allowlist={"copy"})
    gen.visit(ast.parse("import copy\ncopy.copy([1])\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    const_str: dict[str, str] = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    copy_module_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_cache_get"
        and len(op.get("args") or []) == 1
        and const_str.get(op["args"][0]) == "copy"
    }
    copy_attr_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_attr"
        and len(op.get("args") or []) == 2
        and op["args"][0] in copy_module_vars
        and const_str.get(op["args"][1]) == "copy"
    }

    assert copy_attr_vars, "expected module_get_attr for copy.copy"
    assert all(
        not (op.get("kind") == "call" and op.get("s_value") == "copy__copy")
        for op in main_ops
    )
    assert any(
        op.get("kind") == "call_bind"
        and len(op.get("args") or []) >= 1
        and op["args"][0] in copy_attr_vars
        for op in main_ops
    ), "expected fallback call_bind when direct copy__copy symbol is unavailable"


def test_allowlisted_module_attr_call_uses_direct_symbol_when_known() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"__main__", "copy"},
        known_func_defaults={
            "copy": {"copy": {"params": 1, "kwonly": 0, "defaults": []}}
        },
        stdlib_allowlist={"copy"},
    )
    gen.visit(ast.parse("import copy\ncopy.copy([1])\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "copy__copy"
        for op in main_ops
    )


def test_moltlib_concurrency_attr_call_uses_direct_symbol_when_known() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"__main__", "moltlib.concurrency"},
        known_func_defaults={
            "moltlib.concurrency": {
                "channel": {"params": 1, "kwonly": 0, "defaults": []}
            }
        },
    )
    gen.visit(ast.parse("import moltlib.concurrency as mc\nmc.channel(1)\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "moltlib_concurrency__channel"
        for op in main_ops
    )


def test_known_project_module_attr_call_uses_direct_symbol_when_known() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"__main__", "ui_rendering"},
        known_func_defaults={
            "ui_rendering": {
                "render_cards": {"params": 1, "kwonly": 0, "defaults": []}
            }
        },
    )
    gen.visit(ast.parse("import ui_rendering\nui_rendering.render_cards(1)\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    assert any(
        op.get("kind") == "call" and op.get("s_value") == "ui_rendering__render_cards"
        for op in main_ops
    )


def test_known_project_from_import_call_uses_direct_symbol_when_known() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"__main__", "ui_rendering"},
        known_func_defaults={
            "ui_rendering": {
                "render_cards": {"params": 1, "kwonly": 0, "defaults": []}
            }
        },
    )
    gen.visit(ast.parse("from ui_rendering import render_cards\nrender_cards(1)\n"))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    const_str: dict[str, str] = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    imported_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_attr"
        and len(op.get("args") or []) == 2
        and const_str.get(op["args"][1]) == "render_cards"
    }
    assert imported_vars
    assert any(
        op.get("kind") == "call_func"
        and len(op.get("args") or []) >= 1
        and op["args"][0] in imported_vars
        for op in main_ops
    )


def test_module_chunking_resets_module_cache_temporaries_per_chunk() -> None:
    source = """
import math
x0 = math.sqrt
x1 = math.sin
x2 = math.cos
x3 = math.tan
x4 = math.atan
x5 = math.asin
x6 = math.acos
"""
    gen = SimpleTIRGenerator(
        known_modules={"__main__"},
        module_chunking=True,
        module_chunk_max_ops=40,
        stdlib_allowlist={"math"},
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    chunk_funcs = [
        func for func in ir["functions"] if "molt_module_chunk_" in func["name"]
    ]
    assert len(chunk_funcs) >= 2
    for func in chunk_funcs:
        defined: set[str] = set(func["params"])
        for op in func["ops"]:
            if op.get("kind") in {"module_get_attr", "module_get_global"}:
                args = op.get("args") or []
                assert args
                module_var = args[0]
                assert module_var in defined, (
                    f"{func['name']} uses undefined module var {module_var}"
                )
            out = op.get("out")
            if isinstance(out, str) and out != "none":
                defined.add(out)


def test_module_chunking_preserves_import_alias_bindings_across_chunks() -> None:
    source = """
import abc as _abc
pad0 = 0
pad1 = 1
pad2 = 2
pad3 = 3
pad4 = 4
pad5 = 5
pad6 = 6
pad7 = 7
pad8 = 8
pad9 = 9
pad10 = 10
pad11 = 11
pad12 = 12
pad13 = 13
pad14 = 14
pad15 = 15
pad16 = 16
pad17 = 17
pad18 = 18
pad19 = 19


class Demo(_abc.ABC):
    @_abc.abstractmethod
    def f(self):
        return None
"""
    gen = SimpleTIRGenerator(
        known_modules={"__main__"},
        module_chunking=True,
        module_chunk_max_ops=40,
        stdlib_allowlist={"abc", "_abc"},
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    chunk_funcs = [
        func for func in ir["functions"] if "molt_module_chunk_" in func["name"]
    ]
    assert len(chunk_funcs) >= 2

    later_chunks = chunk_funcs[1:]
    saw_alias_global_get = False
    for func in later_chunks:
        const_str = {
            op["out"]: op["s_value"]
            for op in func["ops"]
            if op.get("kind") == "const_str"
            and isinstance(op.get("out"), str)
            and isinstance(op.get("s_value"), str)
        }
        for op in func["ops"]:
            if op.get("kind") == "module_get_global":
                args = op.get("args") or []
                if len(args) == 2 and const_str.get(args[1]) == "_abc":
                    saw_alias_global_get = True
            if op.get("kind") == "module_cache_get":
                args = op.get("args") or []
                if len(args) == 1 and const_str.get(args[0]) == "_abc":
                    raise AssertionError(
                        f"{func['name']} reloaded _abc module instead of using alias"
                    )

    assert saw_alias_global_get, "expected chunked alias lookup via module_get_global"


def test_module_cache_values_do_not_leak_across_conditional_paths() -> None:
    source = """
kind = 0
if kind == 1 and "KqueueSelector" in globals():
    selected = "kqueue"
elif kind == 2 and "EpollSelector" in globals():
    selected = "epoll"
else:
    selected = "select"
"""
    gen = SimpleTIRGenerator(known_modules={"__main__"}, stdlib_allowlist={"builtins"})
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    defined: set[str] = set()
    for op in main_ops:
        if op.get("kind") == "module_get_attr":
            args = op.get("args") or []
            assert args
            module_var = args[0]
            assert module_var in defined, f"module_get_attr uses undefined {module_var}"
        out = op.get("out")
        if isinstance(out, str) and out != "none":
            defined.add(out)


def test_imported_namedtuple_call_does_not_append_kwonly_defaults() -> None:
    gen = SimpleTIRGenerator(
        known_modules={"__main__"},
        stdlib_allowlist={"collections"},
    )
    gen.visit(
        ast.parse(
            "from collections import namedtuple\n"
            "Point = namedtuple('Point', ['x', 'y'])\n"
        )
    )
    ir = gen.to_json()
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    const_str: dict[str, str] = {
        op["out"]: op["s_value"]
        for op in main_ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    collections_module_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_cache_get"
        and len(op.get("args") or []) == 1
        and const_str.get(op["args"][0]) == "collections"
    }
    namedtuple_vars = {
        op["out"]
        for op in main_ops
        if op.get("kind") == "module_get_attr"
        and len(op.get("args") or []) == 2
        and op["args"][0] in collections_module_vars
        and const_str.get(op["args"][1]) == "namedtuple"
    }
    assert namedtuple_vars
    namedtuple_calls = [
        op
        for op in main_ops
        if op.get("kind") == "call_func"
        and len(op.get("args") or []) >= 1
        and op["args"][0] in namedtuple_vars
    ]
    assert namedtuple_calls
    assert all(len(op.get("args") or []) == 3 for op in namedtuple_calls), (
        "namedtuple should receive only explicit positional args; "
        "kw-only defaults must not be appended positionally"
    )


def test_inline_prune_preserves_guarded_call_targets() -> None:
    source = (
        "_Any = object\n"
        "def _cast(_tp, value):\n"
        "    return value\n"
        "cast = _cast\n"
        "x = cast(int, 1)\n"
    )
    gen = SimpleTIRGenerator(
        module_name="_collections_abc",
        known_modules={"__main__", "_collections_abc"},
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    function_names = {func["name"] for func in ir["functions"]}
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    guarded_targets = [
        op.get("s_value")
        for op in main_ops
        if op.get("kind") == "call_guarded" and isinstance(op.get("s_value"), str)
    ]
    assert "_collections_abc___cast" in guarded_targets
    assert "_collections_abc___cast" in function_names


def test_recursive_closure_call_uses_call_func_not_guarded_target() -> None:
    source = """
def outer():
    def render_level(st, buf, level):
        if st:
            render_level(st - 1, buf, level + 1)
    render_level(1, [], 0)
"""
    gen = SimpleTIRGenerator(
        module_name="asyncio", known_modules={"__main__", "asyncio"}
    )
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    outer_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "asyncio__outer"
    )
    assert not any(
        op.get("kind") == "call_guarded"
        and op.get("s_value") == "asyncio__render_level"
        for op in outer_ops
    )
    assert any(op.get("kind") == "call_func" for op in outer_ops)
