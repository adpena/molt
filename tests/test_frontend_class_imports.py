from __future__ import annotations

import ast

import pytest

from molt.compat import CompatibilityError
from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator


IMPORT_BINDINGS = (
    "import os as bound",
    "from os import getenv as bound",
    "from nativepkg import child as bound",
    "from _intrinsics import require_intrinsic as bound",
)


def _generate(source: str) -> tuple[SimpleTIRGenerator, list[MoltOp]]:
    gen = SimpleTIRGenerator(known_modules={"nativepkg", "nativepkg.child"})
    gen.visit(ast.parse(source))
    return gen, [op for fn in gen.funcs_map.values() for op in fn["ops"]]


def _named_stores(ops: list[MoltOp], kind: str, name: str) -> list[MoltOp]:
    keys = {
        op.result.name for op in ops if op.kind == "CONST_STR" and op.args == [name]
    }
    return [
        op
        for op in ops
        if op.kind == kind
        and len(op.args) == 3
        and isinstance(op.args[1], MoltValue)
        and op.args[1].name in keys
    ]


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
@pytest.mark.parametrize("enclosing_global", [False, True])
def test_class_import_publishes_to_class_mapping_not_enclosing_module(
    statement: str, enclosing_global: bool
) -> None:
    body = f"class C(metaclass=meta):\n    {statement}\n"
    if enclosing_global:
        body = "def outer():\n    global bound\n" + "".join(
            f"    {line}\n" for line in body.splitlines()
        )
    gen, ops = _generate(body)
    stores = _named_stores(ops, "STORE_INDEX", "bound")
    assert len(stores) == 1
    assert isinstance(stores[0].args[0], MoltValue)
    assert stores[0].args[0] != gen.module_obj
    assert not _named_stores(ops, "MODULE_SET_ATTR", "bound")
    assert "bound" not in gen.global_imported_modules
    assert "bound" not in gen.global_imported_names
    assert "bound" not in gen.imported_modules
    assert "bound" not in gen.imported_names


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
def test_explicit_class_global_import_publishes_once_to_module(statement: str) -> None:
    gen, ops = _generate(
        "class C(metaclass=meta):\n    global bound\n" + f"    {statement}\n"
    )
    assert len(_named_stores(ops, "MODULE_SET_ATTR", "bound")) == 1
    assert not _named_stores(ops, "STORE_INDEX", "bound")
    assert (
        "bound" in gen.global_imported_modules or "bound" in gen.global_imported_names
    )


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
def test_function_local_import_has_no_module_publication(statement: str) -> None:
    gen, ops = _generate(f"def f():\n    {statement}\n    return bound\n")
    assert not _named_stores(ops, "MODULE_SET_ATTR", "bound")
    assert "bound" not in gen.global_imported_modules
    assert "bound" not in gen.global_imported_names


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
def test_function_global_import_publishes_once_to_active_globals(
    statement: str,
) -> None:
    _, ops = _generate(f"def f():\n    global bound\n    {statement}\n")
    active_globals = {
        op.result.name
        for op in ops
        if op.kind == "CALL" and op.args == ["molt_globals_builtin"]
    }
    stores = _named_stores(ops, "DICT_SET", "bound")
    assert len(stores) == 1
    assert stores[0].args[0].name in active_globals
    assert not _named_stores(ops, "MODULE_SET_ATTR", "bound")


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
def test_module_loop_import_has_one_publication_per_dynamic_iteration(
    statement: str,
) -> None:
    # A replaced imported binding can own a finalizer. The loop's static body
    # must contain one publication, not separate mutation and loop setters.
    _, ops = _generate(f"for item in items:\n    {statement}\n")
    assert len(_named_stores(ops, "MODULE_SET_ATTR", "bound")) == 1


def test_live_module_mutation_retires_deferred_publication_without_replay() -> None:
    gen = SimpleTIRGenerator()
    gen.module_obj = MoltValue("module", type_hint="module")
    gen.defer_module_attrs = True
    old, new = MoltValue("old"), MoltValue("new")
    gen.locals["bound"] = old
    gen._emit_module_attr_set("bound", old)
    assert gen.deferred_module_attrs == {"bound"}
    assert not _named_stores(gen.current_ops, "MODULE_SET_ATTR", "bound")
    gen.module_global_mutations.add("bound")
    gen._emit_module_attr_set("bound", new)
    assert not gen.deferred_module_attrs
    gen._flush_deferred_module_attrs()
    stores = _named_stores(gen.current_ops, "MODULE_SET_ATTR", "bound")
    assert len(stores) == 1
    assert stores[0].args[2] == new


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
def test_boxed_module_loop_import_has_one_module_store_and_one_cell_store(
    statement: str,
) -> None:
    gen = SimpleTIRGenerator(known_modules={"nativepkg", "nativepkg.child"})
    gen.module_obj = MoltValue("module", type_hint="module")
    # An arbitrary old object may run release callbacks. Keep its boxed owner
    # explicit while checking that overlapping loop/mutation requirements do
    # not publish the new imported object twice.
    gen.locals["bound"] = MoltValue("displaced", type_hint="Any")
    gen._box_local("bound")
    cell = gen.boxed_locals["bound"]
    gen.scope_assigned.add("bound")
    gen.control_flow_depth = 1
    start = len(gen.current_ops)
    gen.visit(ast.parse(statement).body[0])
    ops = gen.current_ops[start:]
    module_stores = _named_stores(ops, "MODULE_SET_ATTR", "bound")
    assert len(module_stores) == 1
    cell_stores = [
        op for op in ops if op.kind == "CALL" and op.args[:2] == ["molt_cell_set", cell]
    ]
    assert len(cell_stores) == 1
    assert module_stores[0].args[2] == cell_stores[0].args[2]


def test_class_global_provenance_does_not_replace_enclosing_function_local() -> None:
    gen = SimpleTIRGenerator()
    gen.current_func_name = "outer"
    gen.imported_modules["bound"] = "os"
    gen.imported_module_provenance["bound"] = frozenset({"os"})
    gen.local_imported_modules.add("bound")
    original_modules = gen.imported_modules
    saved = gen._capture_class_import_state()
    gen.imported_modules["bound"] = "sys"
    gen.global_imported_modules["bound"] = "sys"
    gen.global_imported_module_provenance["bound"] = frozenset({"sys"})
    gen._restore_class_import_state(saved, frozenset({"bound"}))
    assert gen.imported_modules is original_modules
    assert gen.imported_modules["bound"] == "os"
    assert gen.global_imported_modules["bound"] == "sys"
    assert "bound" in gen.local_imported_modules


@pytest.mark.parametrize("initial_attribute", [None, "getenv"])
@pytest.mark.parametrize("body_import_taken", [False, True])
def test_class_nonlocal_import_does_not_restore_stale_enclosing_origin(
    initial_attribute: str | None, body_import_taken: bool
) -> None:
    gen = SimpleTIRGenerator()
    gen.current_func_name = "outer"
    gen.locals["bound"] = MoltValue("original")
    gen._record_import_binding_origin("bound", "os", attr_name=initial_attribute)
    saved = gen._capture_class_import_state()
    if body_import_taken:
        gen._record_import_binding_origin("bound", "sys")
    gen._restore_class_import_state(saved, frozenset(), frozenset({"bound"}))
    assert "bound" not in gen.imported_modules
    assert "bound" not in gen.imported_names
    assert "bound" not in gen.imported_attr_names
    assert "bound" not in gen.local_imported_modules
    assert "bound" not in gen.local_imported_names
    assert gen._imported_module_binding_target("bound") is None
    assert "bound" not in gen.global_imported_modules
    assert "bound" not in gen.global_imported_names


@pytest.mark.parametrize("statement", IMPORT_BINDINGS)
def test_class_nonlocal_import_publishes_to_enclosing_cell_not_class_or_module(
    statement: str,
) -> None:
    gen, ops = _generate(
        "def outer():\n    bound = None\n"
        "    class C(metaclass=meta):\n        nonlocal bound\n"
        f"        {statement}\n    return bound\n"
    )
    assert not _named_stores(ops, "MODULE_SET_ATTR", "bound")
    assert not _named_stores(ops, "STORE_INDEX", "bound")
    outer = next(
        fn for symbol, fn in gen.funcs_map.items() if symbol.endswith("__outer")
    )
    # Raw ops carry LINE markers, not a source_line on each store/load. The
    # serializer is the canonical source-location projection consumed downstream.
    # Inspect that actual consumer and keep its exact cell/return operand chain.
    lowered = gen.map_ops_to_json(outer["ops"], run_midend=False)
    cell_stores = [
        op
        for op in lowered
        if op["kind"] == "call"
        and op.get("s_value") == "molt_cell_set"
        and op.get("source_line") == 5
    ]
    assert len(cell_stores) == 1, "the import must replace the enclosing boxed binding"
    store = cell_stores[0]
    loads = [
        op
        for op in lowered[lowered.index(store) + 1 :]
        if op["kind"] == "call"
        and op.get("s_value") == "molt_cell_get"
        and op.get("source_line") == 6
        and op["args"] == [store["args"][0]]
    ]
    assert len(loads) == 1
    assert any(
        op["kind"] == "ret" and op["args"] == [loads[0]["out"]] for op in lowered
    ), "the enclosing return must consume the updated cell"


def test_class_import_shadow_does_not_replace_outer_alias_provenance() -> None:
    gen, _ = _generate(
        "import os as bound\nclass C(metaclass=meta):\n    import sys as bound\n"
    )
    assert gen.imported_modules["bound"] == "os"
    assert gen.global_imported_modules["bound"] == "os"


def test_class_global_import_replaces_outer_alias_provenance() -> None:
    gen, _ = _generate(
        "import os as bound\n"
        "class C(metaclass=meta):\n    global bound\n    import sys as bound\n"
    )
    assert gen.imported_modules["bound"] == "sys"
    assert gen.global_imported_modules["bound"] == "sys"


@pytest.mark.parametrize("module", ["os", "_intrinsics"])
def test_class_star_import_is_rejected_before_synthetic_or_normal_import(
    module: str,
) -> None:
    with pytest.raises(
        CompatibilityError, match="import \\* only allowed at module level"
    ):
        _generate(f"class C(metaclass=meta):\n    from {module} import *\n")


# Function-level imports (HF-557). CPython executes an import statement in a
# function body only when the body runs, and resolves a relative one against
# the function's package anchor. Each body below binds `helper` or `compute`
# by import and calls it.
ABSENT_MODULE_CALLERS = {
    "function": "def f():\n    from absent_mod import helper\n    return helper(1)\n",
    "generator": "def f():\n    from absent_mod import helper\n    yield helper(1)\n",
    "coroutine": (
        "async def f():\n    from absent_mod import helper\n    return helper(1)\n"
    ),
    "guarded-global": (
        "try:\n    from absent_mod import helper\nexcept ImportError:\n"
        "    helper = None\ndef f():\n    return helper(1)\n"
    ),
}


def _function_ir(
    source: str, *, module_name: str, source_path: str
) -> dict[str, list[dict]]:
    gen = SimpleTIRGenerator(
        module_name=module_name,
        source_path=source_path,
        module_execution_kind="imported",
        known_modules={module_name, "helper", "pkg.helper"},
        direct_call_modules={"helper", "pkg.helper"},
        known_func_kinds={
            "helper": {"compute": "sync"},
            "pkg.helper": {"compute": "sync"},
        },
    )
    gen.visit(ast.parse(source))
    return {
        func["name"]: func["ops"]
        for func in gen.to_json()["functions"]
        if func["name"] != "molt_main"
    }


@pytest.mark.parametrize("source", ABSENT_MODULE_CALLERS.values(), ids=list)
def test_call_through_import_from_unadmitted_module_is_an_ordinary_call(
    source: str,
) -> None:
    """The import statement raises ImportError if it runs; the call never refuses."""
    functions = _function_ir(source, module_name="top", source_path="/x/top.py")
    ops = [op for body in functions.values() for op in body]
    calls = [
        op
        for op in ops
        if op["kind"] in {"call_func", "call_bind", "call_indirect", "call_guarded"}
    ]
    assert calls, "the call must lower to a runtime call on the bound value"
    assert not any(
        op["kind"] == "call_guarded" and "absent_mod" in str(op.get("s_value"))
        for op in calls
    )


def test_function_relative_import_binds_the_package_child_not_a_top_level_module() -> (
    None
):
    """`.helper` in package `pkg` is `pkg.helper`, never a top-level `helper`."""
    functions = _function_ir(
        "def f():\n    from .helper import compute\n    yield compute(1)\n",
        module_name="pkg",
        source_path="/x/pkg/__init__.py",
    )
    targets = {
        op.get("s_value")
        for body in functions.values()
        for op in body
        if op["kind"] == "call_guarded"
    }
    assert targets == {"pkg_helper__compute"}
    transactions = [
        op
        for body in functions.values()
        for op in body
        if op["kind"] == "call" and op["s_value"] == "molt_importlib_import_transaction"
    ]
    assert len(transactions) == 1, "the import keeps its runtime package anchor"


def test_function_relative_import_without_a_package_anchor_has_no_origin() -> None:
    """With no parent package the import raises when it runs; `helper` is not it."""
    functions = _function_ir(
        "def f():\n    from .helper import compute\n    return compute(1)\n",
        module_name="top",
        source_path="/x/top.py",
    )
    ops = [op for body in functions.values() for op in body]
    assert not any(op["kind"] == "call_guarded" for op in ops)
    assert any(op["kind"] == "call_func" for op in ops)
