from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import ExceptionEdgeKind


import ast


import random


import types

from collections.abc import Callable, Sequence
from typing import Literal


from pathlib import Path

import pytest

from molt.frontend import ClassId, DeclaredClass, MoltOp, MoltValue, SimpleTIRGenerator

from molt.frontend._types import (
    _MOLT_CLOSURE_PARAM,
    CodeSlotDeclaration,
)

from tests.process_guard_common import install_module_view

from molt.frontend.cfg_analysis import (
    CFGEdgeKind,
    CFGGraph,
    DominatorTree,
    build_cfg,
)

from molt.frontend.lowering.op_kinds_generated import (
    FRONTEND_FIRST_TRAILING_RESULT_ARG,
    SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION,
    SIMPLEIR_RUNTIME_REQUIREMENT_IMPORT_PROTOCOL,
    SIMPLEIR_RUNTIME_SYMBOL_REQUIREMENTS,
    WireOpKind,
)


def test_same_module_static_class_call_reuses_current_class_binding() -> None:
    source = """
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

total = 0
for i in range(3):
    point = Point(i, i)
    total += i
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    assert _module_attr_reads_named(ops, "Point") == []


def test_module_control_flow_name_reads_use_load_global_semantics() -> None:
    source = """
try:
    __NUMPY_SETUP__
except NameError:
    __NUMPY_SETUP__ = False

if __NUMPY_SETUP__:
    state = "setup"
else:
    state = "runtime"
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    assert _module_global_reads_named(ops, "__NUMPY_SETUP__")
    assert _module_attr_reads_named(ops, "__NUMPY_SETUP__") == []


def test_imported_module_control_flow_name_reads_use_load_global_semantics() -> None:
    # The ``try: __NUMPY_SETUP__ / except NameError`` guard runs inside an
    # IMPORTED package initializer (module_name != entry_module) when a user
    # program does ``import numpy``, and large package inits additionally lower
    # with module chunking.  Bare-name reads of a not-yet-bound module global
    # must use MODULE_GET_GLOBAL (NameError on a miss) in those configurations
    # too — MODULE_GET_ATTR would leak AttributeError into ``except NameError``.
    source = """
try:
    __NUMPY_SETUP__
except NameError:
    __NUMPY_SETUP__ = False

if __NUMPY_SETUP__:
    state = "setup"
else:
    state = "runtime"
"""
    generators = {
        "imported": SimpleTIRGenerator(
            module_name="numpy",
            known_modules={"main", "numpy"},
        ),
        "imported-chunked": SimpleTIRGenerator(
            module_name="numpy",
            known_modules={"main", "numpy"},
            module_chunking=True,
            module_chunk_max_ops=8,
        ),
    }
    for config_name, gen in generators.items():
        gen.visit(ast.parse(source))
        ir = gen.to_json()

        global_read_funcs = [
            func["name"]
            for func in ir["functions"]
            if _module_global_reads_named(func["ops"], "__NUMPY_SETUP__")
        ]
        attr_read_funcs = [
            func["name"]
            for func in ir["functions"]
            if _module_attr_reads_named(func["ops"], "__NUMPY_SETUP__")
        ]
        assert attr_read_funcs == [], (config_name, attr_read_funcs)
        assert global_read_funcs, config_name


@pytest.mark.parametrize("condition", ["False", "unknown_condition"])
def test_module_control_flow_named_calls_capture_live_callable_before_arguments(
    condition: str,
) -> None:
    source = f"""
flag = {condition}
if flag:
    len = 7

value = len(globals())
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    (callee,) = _module_global_reads_named(ops, "len")
    assert _module_attr_reads_named(ops, "len") == []
    # Even when the local assignment is unreachable, Python evaluates the
    # live callable before arguments. Spelling/flow facts cannot replace that
    # captured object with an unconditional LEN operation.
    invocation = next(
        op
        for op in ops
        if op["kind"] in {"call_func", "call_bind", "call_indirect", "call_guarded"}
        and op.get("args", [None])[0] == callee["out"]
    )
    arguments = {op["out"]: op for op in ops if "out" in op}
    assert len(invocation["args"]) == 2
    assert ops.index(callee) < ops.index(arguments[invocation["args"][1]])
    assert ops.index(arguments[invocation["args"][1]]) < ops.index(invocation)


def test_module_chunks_share_the_enclosing_python_execution_frame() -> None:
    gen = SimpleTIRGenerator(
        module_name="hello",
        module_execution_kind="module",
        module_chunking=True,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse("print(42)\n"))
    ir = gen.to_json()

    chunks = [
        func for func in ir["functions"] if "__molt_module_chunk_" in func["name"]
    ]
    assert chunks
    for chunk in chunks:
        kinds = [op["kind"] for op in chunk["ops"]]
        assert "line" in kinds
        assert "trace_enter_slot" not in kinds
        assert "trace_exit" not in kinds
        assert chunk["execution_context"] == "inherited"
    chunk_calls = [
        op
        for func in ir["functions"]
        for op in func["ops"]
        if op.get("s_value") in {chunk["name"] for chunk in chunks}
    ]
    assert chunk_calls
    assert all(op.get("passes_execution_context") is True for op in chunk_calls)


@pytest.mark.parametrize("chunked", [False, True])
@pytest.mark.parametrize("with_machinery", [False, True])
@pytest.mark.parametrize(
    "module_name",
    [
        "sys",
        "builtins",
        "importlib",
        "importlib.machinery",
        "app",
        "__main__",
        "package",
    ],
)
def test_module_metadata_follows_builtin_capture_and_owns_attempt_cleanup(
    module_name: str, chunked: bool, with_machinery: bool
) -> None:
    gen = SimpleTIRGenerator(
        module_name=module_name,
        source_path=(
            "/modules/package/__init__.py"
            if module_name == "package"
            else f"/modules/{module_name}.py"
        ),
        module_execution_kind="script" if module_name == "__main__" else "imported",
        known_modules=(
            {"sys", "builtins", "importlib", "importlib.machinery", "app"}
            if with_machinery
            else {module_name}
        ),
        module_chunking=chunked,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse("value = 1\n"))
    # Inspect authored language-lowering obligations at the frontend boundary.
    ops = gen.funcs_map["molt_main"]["ops"]
    kinds = [op.kind for op in ops]
    strings = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    enter = kinds.index("TRACE_ENTER_SLOT")
    locals_set = kinds.index("FRAME_LOCALS_SET")
    assert kinds.index("MODULE_CACHE_SET") < kinds.index("CODE_SLOT_SET") < enter
    assert not any(op.kind == "MODULE_IMPORT" for op in ops[:enter])
    plain_spec_stores = [
        index
        for index, op in enumerate(ops)
        if op.kind == "MODULE_SET_ATTR" and strings.get(op.args[1].name) == "__spec__"
    ]
    spec_admissions = [
        index
        for index, op in enumerate(ops)
        if op.kind == "CALL" and op.args[0] == "molt_module_initialization_spec"
    ]
    if module_name == "__main__":
        assert not spec_admissions
        spec_stores = plain_spec_stores
    else:
        assert not plain_spec_stores
        spec_stores = spec_admissions
    assert len(spec_stores) == 1
    assert enter < spec_stores[0] < locals_set
    assert (
        ops[spec_stores[0]].args[1 if module_name != "__main__" else 0]
        == gen.module_obj
    )
    assert ops[locals_set].metadata == {"source_module_publication_boundary": True}
    # Metadata must exist before any facade executes its source body. A public
    # ModuleSpec lookup can recurse into a partially initialized machinery.
    assert not any(
        op.kind == "MODULE_GET_ATTR" and strings.get(op.args[1].name) == "ModuleSpec"
        for op in ops
    )
    assert not any(op.kind == "MODULE_IMPORT" for op in ops[enter:locals_set])
    spec_value = ops[spec_stores[0]].args[2]
    producer = next(op for op in ops if op.result == spec_value)
    if module_name == "__main__":
        assert producer.kind == "CONST_NONE"
    else:
        assert producer.kind == "CALL_FUNC"
        cls_value = producer.args[0]
        cls_call = next(op for op in ops if op.result == cls_value)
        assert cls_call.kind == "CALL"
        assert cls_call.args == ["molt_importlib_module_spec_type"]
        loader_call = next(op for op in ops if op.result == producer.args[2])
        assert loader_call.kind == "CALL"
        assert loader_call.args == ["molt_importlib_compiled_loader"]
        location_stores = [
            op
            for op in ops
            if op.kind == "SETATTR_GENERIC_OBJ"
            and op.args[:2] == [spec_value, "has_location"]
        ]
        if module_name == "namespace":
            assert not location_stores
        else:
            assert len(location_stores) == 1
            location = next(op for op in ops if op.result == location_stores[0].args[2])
            assert location.kind == "CONST_BOOL"
            assert location.args == [True]

    pre_frame_label = gen.module_pre_frame_exception_label
    attempt_label = gen.function_exception_label
    assert pre_frame_label is not None and attempt_label != pre_frame_label
    assert ops[enter + 1].kind == "CHECK_EXCEPTION"
    assert ops[enter + 1].args == [attempt_label]
    assert all(
        op.args == [pre_frame_label]
        for op in ops[:enter]
        if op.kind == "CHECK_EXCEPTION"
    )
    assert all(
        op.args == [attempt_label]
        for op in ops[enter + 1 : locals_set + 1]
        if op.kind == "CHECK_EXCEPTION"
    )
    for label, expected_exits in [(pre_frame_label, 0), (attempt_label, 1)]:
        start = next(
            index
            for index, op in enumerate(ops)
            if op.kind == "LABEL" and op.args == [label]
        )
        end = next(
            index
            for index in range(start + 1, len(ops))
            if ops[index].kind in {"ret", "ret_void"}
        )
        handler = kinds[start:end]
        # CPython keeps a failed script's __main__ namespace published; only
        # failed imports retract their partially initialized module.
        assert handler.count("MODULE_CACHE_DEL") == (
            0 if module_name == "__main__" else 1
        )
        assert handler.count("TRACE_EXIT") == expected_exits


def test_importlib_machinery_owns_a_local_frame_and_threads_it_to_chunks() -> None:
    gen = SimpleTIRGenerator(
        module_name="importlib.machinery",
        module_chunking=True,
        module_chunk_max_ops=1,
    )
    gen.visit(ast.parse("first = 1\nsecond = 2\n"))
    ir = gen.to_json()

    module_init = next(func for func in ir["functions"] if func["name"] == "molt_main")
    init_kinds = [op["kind"] for op in module_init["ops"]]
    assert module_init["execution_context"] == "local"
    assert init_kinds.count("trace_enter_slot") == 1
    enter_index = init_kinds.index("trace_enter_slot")
    assert all(
        kind not in {"line", "frame_locals_set", "trace_exit"}
        for kind in init_kinds[:enter_index]
    )

    chunks = [
        func for func in ir["functions"] if "__molt_module_chunk_" in func["name"]
    ]
    assert chunks
    chunk_names = {chunk["name"] for chunk in chunks}
    assert all(chunk["execution_context"] == "inherited" for chunk in chunks)
    assert all(
        op["kind"] not in {"trace_enter_slot", "trace_exit"}
        for chunk in chunks
        for op in chunk["ops"]
    )
    threaded_calls = [
        op for op in module_init["ops"] if op.get("s_value") in chunk_names
    ]
    assert {op["s_value"] for op in threaded_calls} == chunk_names
    assert all(op.get("passes_execution_context") is True for op in threaded_calls)


def test_local_frame_entry_is_typed_checked_and_serialization_is_deterministic() -> (
    None
):
    gen = SimpleTIRGenerator(module_name="frame_entry", target_python=(3, 14))
    gen.visit(
        ast.parse(
            "value: int = 1\n"
            "def leaf():\n    return 1\n"
            "def guarded():\n"
            "    try:\n        return leaf()\n"
            "    except ValueError:\n        return 2\n"
            "def managed():\n    with manager():\n        return 3\n"
            "callback = lambda: 4\n"
            "class Sample:\n    def method(self):\n        return 5\n"
            "def generator():\n    yield 6\n"
            "async def coroutine():\n    return 7\n"
            "async def async_generator():\n    yield 8\n"
            "comprehension = (item for item in (1, 2))\n"
        )
    )
    local_functions = {
        name: data
        for name, data in gen.funcs_map.items()
        if "frame_entry_failure_label" in data
    }
    assert len(local_functions) >= 10
    assert any("__annotate__" in name for name in local_functions)
    assert any("__lambda" in name for name in local_functions)
    for name, data in local_functions.items():
        assert data["return_abi"] == "value", name
        ops = data["ops"]
        failure_label = data["frame_entry_failure_label"]
        assert ops[0].kind == "TRACE_ENTER_SLOT", name
        assert ops[0].args == [name]
        assert ops[1].kind == "CHECK_EXCEPTION", name
        assert ops[1].args == [failure_label]
        cleanup = next(
            index
            for index, op in enumerate(ops)
            if op.kind == "LABEL" and op.args == [failure_label]
        )
        assert [op.kind for op in ops[cleanup:]] == [
            "LABEL",
            "TRACE_EXIT",
            "ret_void",
        ], name

    first = gen.to_json()
    second = gen.to_json()
    assert first["functions"] == second["functions"]
    for function in first["functions"]:
        assert function["return_abi"] == gen.funcs_map[function["name"]]["return_abi"]
        if function["name"] not in local_functions:
            continue
        ops = function["ops"]
        assert ops[0] == {"kind": "trace_enter_slot", "s_value": function["name"]}
        assert ops[1]["kind"] == "check_exception"
        entry_cleanup = next(
            index
            for index, op in enumerate(ops)
            if op["kind"] == "label" and op["value"] == ops[1]["value"]
        )
        assert [op["kind"] for op in ops[entry_cleanup:]] == [
            "label",
            "trace_exit",
            "ret_void",
        ]


def test_return_abi_survives_removal_of_all_value_returns() -> None:
    gen = SimpleTIRGenerator(module_name="return_abi")
    gen.visit(ast.parse("def leaf():\n    return 1\n"))
    name = next(name for name in gen.funcs_map if name.endswith("leaf"))
    function = gen.funcs_map[name]
    assert function["return_abi"] == "value"
    function["ops"][:] = [
        MoltOp(kind="ret_void", args=[], result=MoltValue("none"))
        if op.kind == "ret"
        else op
        for op in function["ops"]
    ]
    wire = next(item for item in gen.to_json()["functions"] if item["name"] == name)
    assert wire["return_abi"] == "value"
    assert not any(op["kind"] == "ret" for op in wire["ops"])


def test_function_lifecycle_is_the_only_trace_exit_authority() -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            """
def explicit(flag):
    if flag:
        return 1
    return 2

def fallthrough(flag):
    if flag:
        return 3

def exceptional():
    raise ValueError("boom")
"""
        )
    )
    ir = gen.to_json()

    functions = {
        func["name"].rsplit("__", 1)[-1]: func["ops"]
        for func in ir["functions"]
        if func["name"].rsplit("__", 1)[-1]
        in {"explicit", "fallthrough", "exceptional"}
    }
    assert set(functions) == {"explicit", "fallthrough", "exceptional"}

    for ops in functions.values():
        kinds = [op["kind"] for op in ops]
        assert all(
            pair != ("trace_exit", "trace_exit")
            for pair in zip(kinds, kinds[1:], strict=False)
        )
        for index, kind in enumerate(kinds):
            if kind in {"ret", "ret_void"}:
                assert index > 0
                assert kinds[index - 1] == "trace_exit"
            if kind == "trace_exit":
                assert index + 1 < len(kinds)
                assert kinds[index + 1] in {"ret", "ret_void"}
        # Each exit immediately terminates, and each return has exactly one
        # exit. Unreachable authored returns remain until Rust DCE; counting
        # their static exit sites cannot establish a double exit at runtime.


@pytest.mark.parametrize(
    ("source", "symbol"),
    [
        ("import sys as system\nget_frame = system._getframe\n", "molt_getframe"),
        (
            "import inspect as inspection\ncurrent_frame = inspection.currentframe\n",
            "molt_inspect_currentframe",
        ),
        ("from sys import getprofile as imported_getprofile\n", "molt_sys_getprofile"),
        ("from sys import gettrace as imported_gettrace\n", "molt_sys_gettrace"),
        (
            "import sys as system\nset_profile = system.setprofile\n",
            "molt_sys_setprofile",
        ),
        ("import sys as system\nset_trace = system.settrace\n", "molt_sys_settrace"),
    ],
)
def test_frame_introspection_imports_emit_canonical_runtime_callable_provenance(
    source: str, symbol: str
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    symbols = {
        op["runtime_symbol"]
        for func in ir["functions"]
        for op in func["ops"]
        if "runtime_symbol" in op
    }
    assert symbols == {symbol}


@pytest.mark.parametrize(
    ("transport", "source", "function_suffix", "symbol"),
    [
        (
            "direct-in-function",
            "import sys as system\ndef f():\n    return system._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "function-local-module-alias",
            "def f():\n    import sys as s\n    return s._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "ordinary-module-alias",
            "import sys\ndef f():\n    s = sys\n    return s._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "nested-ordinary-module-alias",
            "import sys\ndef f():\n    s = sys\n    t = s\n    return t._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "global-ordinary-module-alias",
            "import sys\ns = sys\ndef f():\n    return s._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "nonlocal-ordinary-module-alias",
            "import sys\ndef outer():\n    s = None\n    def inner():\n        nonlocal s\n        s = sys\n        return s._getframe\n    return inner\n",
            "__inner",
            "molt_getframe",
        ),
        (
            "chained-ordinary-module-alias",
            "import sys\ndef f():\n    left = right = sys\n    return left._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "nested-function-in-loop-does-not-corrupt-parent-flow",
            "import sys\ndef f(items):\n    s = sys\n    for item in items:\n        def inner():\n            return item\n    alias = s\n    return alias._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "nested-lambda-in-try-does-not-corrupt-parent-flow",
            "import sys\ndef f():\n    s = sys\n    try:\n        callback = lambda: s\n    except Exception:\n        callback = lambda: None\n    alias = s\n    return alias._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "function-local-from-alias",
            "def f():\n    from sys import _getframe as gf\n    return gf\n",
            "__f",
            "molt_getframe",
        ),
        (
            "function-local-inspect-from",
            "def f():\n    from inspect import currentframe\n    return currentframe\n",
            "__f",
            "molt_inspect_currentframe",
        ),
        (
            "stdlib-dotted-module-alias",
            "def f():\n    import molt.stdlib.inspect as inspection\n    return inspection.currentframe\n",
            "__f",
            "molt_inspect_currentframe",
        ),
        (
            "stdlib-dotted-from-alias",
            "def f():\n    from molt.stdlib.sys import _getframe as gf\n    return gf\n",
            "__f",
            "molt_getframe",
        ),
        (
            "imported-global-in-function",
            "from sys import gettrace as global_trace\ndef f():\n    return global_trace\n",
            "__f",
            "molt_sys_gettrace",
        ),
        (
            "parameter",
            "import sys as system\ndef sink(value):\n    return value\ndef f():\n    return sink(system.gettrace)\n",
            "__f",
            "molt_sys_gettrace",
        ),
        (
            "return",
            "import inspect as inspection\ndef f():\n    return inspection.currentframe\n",
            "__f",
            "molt_inspect_currentframe",
        ),
        (
            "closure",
            "import sys as system\ndef f():\n    captured = system.setprofile\n    def inner():\n        return captured\n    return inner\n",
            "__f",
            "molt_sys_setprofile",
        ),
        (
            "list",
            "import sys as system\ndef f():\n    return [system.getprofile]\n",
            "__f",
            "molt_sys_getprofile",
        ),
        (
            "dict",
            "import sys as system\ndef f():\n    return {'hook': system.settrace}\n",
            "__f",
            "molt_sys_settrace",
        ),
        (
            "object",
            "import sys as system\nclass Box:\n    pass\ndef f(box):\n    box.hook = system._getframe\n    return box\n",
            "__f",
            "molt_getframe",
        ),
        (
            "dynamic-call-moot-at-producer",
            "import sys as system\ndef f():\n    target = system._getframe\n    return target()\n",
            "__f",
            "molt_getframe",
        ),
    ],
)
def test_runtime_callable_capability_is_preserved_at_every_frontend_acquisition(
    transport: str,
    source: str,
    function_suffix: str,
    symbol: str,
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    function_ops = [
        op
        for function in ir["functions"]
        if function["name"].endswith(function_suffix)
        for op in function["ops"]
    ]
    required = SIMPLEIR_RUNTIME_SYMBOL_REQUIREMENTS[symbol]
    acquisitions = [
        op
        for op in function_ops
        if (
            op.get("runtime_requirement_bits", 0)
            | SIMPLEIR_RUNTIME_SYMBOL_REQUIREMENTS.get(op.get("runtime_symbol"), 0)
        )
        & required
        == required
    ]
    assert acquisitions, transport
    assert all(
        op["kind"]
        in {
            "module_get_attr",
            "module_get_global",
            "module_import_from",
            "get_attr_generic_obj",
        }
        for op in acquisitions
    ), (transport, acquisitions)
    # Invalidated global/nonlocal bindings intentionally lose exact symbol
    # identity, but protected acquisition must keep its generated capability.
    # Exact import-symbol provenance has its own test above.
    if transport == "dynamic-call-moot-at-producer":
        # Prologue/frame calls can precede acquisition. Follow the acquired
        # callable through actual SSA copies/local slots to its invocation.
        acquired: set[str] = set()
        slots: set[str] = set()
        home_slots: set[int] = set()
        target_calls: list[int] = []
        for index, op in enumerate(function_ops):
            args = op.get("args", [])
            if op in acquisitions:
                acquired.add(op["out"])
            elif op["kind"] == "frame_home_store":
                if args and args[0] in acquired:
                    home_slots.add(op["value"])
                    acquired.add(op["out"])
                else:
                    home_slots.discard(op["value"])
            elif op["kind"] == "frame_home_load" and op["value"] in home_slots:
                acquired.add(op["out"])
            elif op["kind"] == "store_var":
                if args and args[0] in acquired:
                    slots.add(op["var"])
                else:
                    slots.discard(op["var"])
            elif op["kind"] == "load_var" and op.get("var") in slots:
                acquired.add(op["out"])
            elif (
                op["kind"] in {"copy", "binding_alias"} and args and args[0] in acquired
            ):
                acquired.add(op["out"])
            elif op["kind"] in {"call_bind", "call_indirect", "call_func"}:
                if args and args[0] in acquired:
                    target_calls.append(index)
        assert target_calls, function_ops
        assert function_ops.index(acquisitions[0]) < min(target_calls)


@pytest.mark.parametrize(
    "signature",
    ["system", "system, /", "*, system", "*system", "**system"],
)
def test_runtime_callable_unknown_parameter_receiver_is_stamped_fail_closed(
    signature: str,
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            f"import sys as system\ndef f({signature}):\n    return system._getframe\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    assert any(
        op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
        for op in function["ops"]
    )


@pytest.mark.parametrize(
    ("case", "source", "function_suffix", "expected_symbol"),
    [
        (
            "if-may-module",
            "import sys\ndef f(flag, other):\n    if flag:\n        s = sys\n    else:\n        s = other\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "conditional-expression-may-module",
            "import sys\ndef f(flag, other):\n    s = sys if flag else other\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "boolean-expression-may-module",
            "import sys\ndef f(flag, other):\n    s = flag and sys or other\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "branch-local-import-with-implicit-nonmodule-path",
            "def f(flag):\n    if flag:\n        import sys as s\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "loop-carried-may-module",
            "import sys\ndef f(items, other):\n    s = other\n    for item in items:\n        s = sys\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "async-loop-zero-iteration-may-module",
            "import sys\nasync def f(items):\n    s = sys\n    async for s in items:\n        pass\n    return s._getframe\n",
            "__f_poll",
            None,
        ),
        (
            "tuple-module-alias",
            "import sys\ndef f():\n    s, value = sys, 1\n    return s._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "walrus-module-alias",
            "import sys\ndef f():\n    return (s := sys)._getframe\n",
            "__f",
            "molt_getframe",
        ),
        (
            "global-may-module",
            "import sys\ns = None\ndef install():\n    global s\n    s = sys\ndef use():\n    return s._getframe\n",
            "__use",
            None,
        ),
        (
            "try-except-finally-may-module",
            "import sys\ndef f(flag, other):\n    s = other\n    try:\n        if flag:\n            s = sys\n    except Exception:\n        s = other\n    finally:\n        flag = False\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "try-exception-prefix-may-module",
            "import sys\ndef f(other, risky):\n    s = other\n    try:\n        risky()\n        s = sys\n    except RuntimeError:\n        pass\n    return s._getframe\n",
            "__f",
            None,
        ),
        (
            "guarded-match-with-no-match-path",
            "import sys\ndef f(value, guard, other):\n    s = other\n    match value:\n        case 1 if guard:\n            s = sys\n    return s._getframe\n",
            "__f",
            None,
        ),
    ],
)
def test_runtime_callable_module_alias_provenance_joins_conservatively(
    case: str, source: str, function_suffix: str, expected_symbol: str | None
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith(function_suffix)
    )
    if expected_symbol is None:
        assert any(
            op.get("runtime_requirement_bits")
            == SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
            for op in function["ops"]
        ), case
    else:
        assert any(
            op.get("runtime_symbol") == expected_symbol for op in function["ops"]
        ), case


def test_runtime_callable_unknown_rebind_loses_exact_symbol_but_keeps_capability() -> (
    None
):
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            "import sys\n"
            "def f(flag, left, right):\n"
            "    s = sys\n"
            "    if flag:\n"
            "        s = left\n"
            "    else:\n"
            "        s = right\n"
            "    return s._getframe\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    assert not any(
        op.get("runtime_symbol") == "molt_getframe" for op in function["ops"]
    )
    assert any(
        op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
        for op in function["ops"]
    )


@pytest.mark.parametrize(
    ("case", "expression"),
    [
        ("direct-conditional", "(sys if flag else other)"),
        ("direct-boolean", "(flag and sys or other)"),
        ("tuple-index", "(sys,)[0]"),
        ("list-index", "[sys][0]"),
        ("dict-index", "{'module': sys}['module']"),
        ("identity-call", "identity(sys)"),
    ],
)
def test_runtime_callable_protected_acquisition_survives_value_transport(
    case: str, expression: str
) -> None:
    source = (
        "import sys\n"
        "def identity(value):\n"
        "    return value\n"
        "def f(flag, other):\n"
        f"    return {expression}._getframe\n"
    )
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    protected = [
        op
        for op in function["ops"]
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    ]
    assert len(protected) == 1, (case, protected)
    assert protected[0]["kind"] == "get_attr_generic_obj"


@pytest.mark.parametrize(
    "source",
    [
        "def f():\n    return __import__\n",
        "import builtins\ndef f():\n    return builtins.__import__\n",
        "from importlib import import_module as load\ndef f():\n    return load\n",
        "import importlib\ndef f():\n    return importlib.import_module\n",
        "import importlib\ndef f():\n    return (importlib,)[0].reload\n",
        "import importlib\ndef f(other, flag):\n    module = importlib if flag else other\n    return module.import_module\n",
        'import importlib\ndef f():\n    return getattr(importlib, "reload")\n',
        "import importlib\ndef f(name):\n    return getattr(importlib, name)\n",
        'import importlib\ndef f():\n    return importlib.__dict__["reload"]\n',
        'import importlib\ndef f():\n    return vars(importlib)["reload"]\n',
        "import runpy\ndef f():\n    return runpy.run_module\n",
    ],
)
def test_runtime_callable_import_requirements_survive_acquisition(source: str) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    requirements = [
        op.get("runtime_requirement_bits", 0)
        | SIMPLEIR_RUNTIME_SYMBOL_REQUIREMENTS.get(op.get("runtime_symbol"), 0)
        for op in function["ops"]
    ]
    assert any(
        bits & SIMPLEIR_RUNTIME_REQUIREMENT_IMPORT_PROTOCOL for bits in requirements
    ), function["ops"]


def test_live_global_requirement_unions_imported_binding_with_builtin_fallback() -> (
    None
):
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            "from sys import gettrace as __import__\ndef f():\n    return __import__\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    (producer,) = _module_global_reads_named(function["ops"], "__import__")
    assert producer.get("runtime_requirement_bits") == (
        SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
        | SIMPLEIR_RUNTIME_REQUIREMENT_IMPORT_PROTOCOL
    )
    assert "runtime_symbol" not in producer


@pytest.mark.parametrize("name", ["__import__", "getattr", "vars"])
def test_runtime_protected_builtin_parameter_shadows_are_not_stamped(
    name: str,
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(f"def f({name}):\n    return {name}\n"))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    assert not _module_global_reads_named(function["ops"], name)
    assert all(
        not op.get("runtime_requirement_bits") and "runtime_symbol" not in op
        for op in function["ops"]
    )


@pytest.mark.parametrize(
    "attribute", ["_getframe", "__dict__", "__getattribute__", None]
)
def test_producer_owned_exact_class_excludes_protected_attribute_requirement(
    attribute: str | None,
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    safe = ClassId("__main__", "Safe")
    exact = gen._stamp_exact_class(
        MoltValue("safe", type_hint=DeclaredClass(safe)), safe
    )
    assert (
        gen._runtime_protected_attribute_requirement_bits(
            exact,
            attribute,
            exact_class=gen._exact_class_for_value(exact),
        )
        == 0
    )


@pytest.mark.parametrize(
    ("case", "setup", "expression"),
    [
        ("getattr-literal", "", 'getattr(sys, "_getframe")'),
        ("getattr-transport", "", 'getattr(identity(sys), "_getframe")'),
        (
            "getattr-dynamic-name",
            '    name = "_getframe"\n',
            "getattr(sys, name)",
        ),
        ("module-dict", "", 'sys.__dict__["_getframe"]'),
        ("vars-module", "", 'vars(sys)["_getframe"]'),
        (
            "object-getattribute",
            "",
            'object.__getattribute__(sys, "_getframe")',
        ),
    ],
)
def test_runtime_callable_protected_acquisition_covers_reflection_gateways(
    case: str, setup: str, expression: str
) -> None:
    source = (
        "import sys\n"
        "def identity(value):\n"
        "    return value\n"
        "def f():\n"
        f"{setup}"
        f"    return {expression}\n"
    )
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    protected = [
        op
        for op in function["ops"]
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    ]
    assert protected, (case, function["ops"])


@pytest.mark.parametrize(
    ("transport", "source", "expected_kind"),
    [
        (
            "builtin-alias-call",
            'import sys\ndef f():\n    gateway = getattr\n    return gateway(sys, "_getframe")\n',
            "module_get_global",
        ),
        (
            "builtin-list-store",
            "def f():\n    return [vars]\n",
            "module_get_global",
        ),
        (
            "builtin-argument-pass",
            "def sink(value):\n    return value\ndef f():\n    return sink(getattr)\n",
            "module_get_global",
        ),
        (
            "builtins-qualified",
            "import builtins\ndef f():\n    return builtins.getattr\n",
            "get_attr_generic_obj",
        ),
        (
            "operator-alias",
            "import operator as op\ndef f():\n    return op.attrgetter\n",
            "get_attr_generic_obj",
        ),
        (
            "operator-methodcaller",
            "import operator\ndef f():\n    return operator.methodcaller\n",
            "get_attr_generic_obj",
        ),
        (
            "inspect-getattr-static",
            "import inspect\ndef f():\n    return inspect.getattr_static\n",
            "get_attr_generic_obj",
        ),
        (
            "from-import-builtins",
            "from builtins import getattr as gateway\ndef f():\n    return gateway\n",
            "module_get_global",
        ),
        (
            "from-import-operator",
            "from operator import attrgetter as gateway\ndef f():\n    return gateway\n",
            "module_get_global",
        ),
        (
            "recursive-builtin-getattr",
            'import builtins\ndef f():\n    return getattr(builtins, "getattr")\n',
            "module_get_global",
        ),
    ],
)
def test_runtime_protected_gateway_callable_identity_is_stamped_at_acquisition(
    transport: str, source: str, expected_kind: WireOpKind
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    protected = [
        op
        for op in function["ops"]
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    ]
    assert len(protected) == 1, (transport, protected)
    assert protected[0]["kind"] == expected_kind, (transport, protected)
    if expected_kind == "module_get_global":
        assert "runtime_symbol" not in protected[0], (transport, protected)


def test_foreign_import_keeps_recursive_gateway_dispatch_dynamic() -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            "import operator as op\n"
            "def f():\n"
            "    direct = op.attrgetter\n"
            "    return direct, getattr(op, 'attrgetter')\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    ops = function["ops"]
    # Import hooks may replace getattr. Its spelling must not bypass dynamic
    # call dispatch; independently acquired operator gateways remain protected.
    (callee,) = _module_global_reads_named(ops, "getattr")
    assert callee.get("runtime_requirement_bits", 0) & (
        SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    )
    assert "runtime_symbol" not in callee
    assert any(
        op["kind"] == "call_func" and op.get("args") and op["args"][0] == callee["out"]
        for op in ops
    )
    assert not any(op["kind"] == "get_attr_name_default" for op in ops)
    protected = [
        op
        for op in ops
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    ]
    assert {op["kind"] for op in protected} == {
        "module_get_global",
        "get_attr_generic_obj",
    }
    assert callee in protected


@pytest.mark.parametrize(
    ("module_name", "attr_name"),
    [
        ("builtins", "getattr"),
        ("builtins", "vars"),
        ("inspect", "getattr_static"),
        ("operator", "attrgetter"),
        ("operator", "methodcaller"),
    ],
)
def test_runtime_protected_gateway_from_import_stamps_import_and_global_load(
    module_name: str, attr_name: str
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            f"from {module_name} import {attr_name} as gateway\n"
            "def f():\n"
            "    return gateway\n"
        )
    )
    protected_kinds = {
        op["kind"]
        for function in gen.to_json()["functions"]
        for op in function["ops"]
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    }
    assert {"module_import_from", "module_get_global"} <= protected_kinds


def test_runtime_gateway_deferred_global_class_receiver_is_fail_closed() -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            "class Safe:\n"
            "    attrgetter = 1\n"
            "def f():\n"
            "    safe = Safe()\n"
            "    return safe.attrgetter\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    assert not any(op.get("kind") == "object_new_bound" for op in function["ops"])
    protected = [
        op
        for op in function["ops"]
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    ]
    assert protected
    assert any(op["kind"] == "get_attr_generic_obj" for op in protected)
    assert all("runtime_symbol" not in op for op in protected)


def test_runtime_callable_reflection_gateways_fail_closed_for_deferred_class() -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            "class Safe:\n"
            "    def __init__(self):\n"
            "        self._getframe = 1\n"
            "def f(name):\n"
            "    safe = Safe()\n"
            "    return (\n"
            "        getattr(safe, name),\n"
            "        vars(safe),\n"
            "        safe.__dict__['_getframe'],\n"
            "        safe.__getattribute__('_getframe'),\n"
            "    )\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    protected = [
        op
        for op in function["ops"]
        if op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    ]
    assert protected
    assert all("runtime_symbol" not in op for op in protected)


def test_live_getattr_acquisition_retains_requirements_for_safe_literal() -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse('def f(value):\n    return getattr(value, "safe_name")\n'))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    (callee,) = _module_global_reads_named(function["ops"], "getattr")
    assert (
        callee.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
    )
    assert "runtime_symbol" not in callee
    assert any(
        op["kind"] == "call_func" and op.get("args", [None])[0] == callee["out"]
        for op in function["ops"]
    )


@pytest.mark.parametrize(
    ("scope", "source", "function_suffix", "expected_exact_reads"),
    [
        (
            "local-shadow-after-import",
            "def f(value):\n"
            "    import sys as s\n"
            "    before = s._getframe\n"
            "    s = value\n"
            "    after = s._getframe\n"
            "    return before, after\n",
            "__f",
            1,
        ),
        (
            "global-rebind",
            "import sys as s\n"
            "def f(value):\n"
            "    global s\n"
            "    before = s._getframe\n"
            "    s = value\n"
            "    after = s._getframe\n"
            "    return before, after\n",
            "__f",
            0,
        ),
        (
            "nonlocal-rebind",
            "def outer():\n"
            "    import sys as s\n"
            "    def inner(value):\n"
            "        nonlocal s\n"
            "        before = s._getframe\n"
            "        s = value\n"
            "        after = s._getframe\n"
            "        return before, after\n"
            "    return inner\n",
            "__inner",
            0,
        ),
    ],
)
def test_runtime_callable_provenance_stops_exactly_at_lexical_rebinding(
    scope: str, source: str, function_suffix: str, expected_exact_reads: int
) -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith(function_suffix)
    )
    frame_reads = [
        op
        for op in function["ops"]
        if op.get("kind") == "module_get_attr"
        and op.get("runtime_symbol") == "molt_getframe"
    ]
    assert len(frame_reads) == expected_exact_reads, (scope, frame_reads)
    if expected_exact_reads == 0:
        assert not any(
            op.get("runtime_symbol") == "molt_getframe" for op in function["ops"]
        ), scope
    assert any(
        op.get("runtime_requirement_bits", 0)
        & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
        for op in function["ops"]
    ), scope


def test_branch_local_import_stamps_the_acquisition_without_downstream_taint() -> None:
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(
        ast.parse(
            "def f(flag):\n"
            "    if flag:\n"
            "        from sys import _getframe as gf\n"
            "    return gf\n"
        )
    )
    function = next(
        function
        for function in gen.to_json()["functions"]
        if function["name"].endswith("__f")
    )
    producers = [
        op for op in function["ops"] if op.get("runtime_symbol") == "molt_getframe"
    ]
    assert len(producers) == 1
    assert producers[0]["kind"] == "module_import_from"


@pytest.mark.parametrize(
    "definition, suffix",
    [
        ("def inner():\n    return EXPR", "__inner"),
        ("def inner():\n    yield EXPR", "__inner_poll"),
        ("async def inner():\n    return EXPR", "__inner_poll"),
        ("async def inner():\n    yield EXPR", "__inner_poll"),
        ("inner = lambda: EXPR", "lambda_1"),
        ("inner = lambda: (yield EXPR)", "lambda_1_poll"),
    ],
)
@pytest.mark.parametrize("from_import", [False, True])
def test_captured_imports_are_mutable_activation_inputs(
    definition: str, suffix: str, from_import: bool
) -> None:
    acquisition = "from sys import _getframe as s" if from_import else "import sys as s"
    expression = "s()" if from_import else "s._getframe"
    source = (
        f"def outer():\n    {acquisition}\n"
        + "\n".join(
            "    " + line
            for line in definition.replace("EXPR", expression).splitlines()
        )
        + "\n    return inner\n"
    )
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ops = next(
        function["ops"]
        for function in gen.to_json()["functions"]
        if function["name"].endswith(suffix)
    )
    assert not any(op.get("runtime_symbol") == "molt_getframe" for op in ops)
    assert any(op.get("s_value") == "molt_cell_get" for op in ops)
    if not from_import:
        assert any(
            op.get("runtime_requirement_bits", 0)
            & SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION
            for op in ops
        )


def test_module_control_flow_class_calls_use_load_global_semantics() -> None:
    source = """
flag = False
if flag:
    class C:
        pass

try:
    C()
except NameError:
    marker = 1
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    assert _module_global_reads_named(ops, "C")
    assert _module_attr_reads_named(ops, "C") == []


def test_deferred_global_class_call_does_not_publish_exact_result_type() -> None:
    source = """
class Point:
    def __init__(self, x):
        self.x = x

def make():
    return Point(3)
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(
        func["ops"] for func in ir["functions"] if func["name"].endswith("__make")
    )

    assert not any(op.get("kind") == "object_new_bound" for op in ops)
    (callee,) = _module_global_reads_named(ops, "Point")
    assert any(
        op["kind"] == "call_func"
        and op.get("args")
        and op["args"][0] == callee["out"]
        and op.get("type_hint") != "Point"
        for op in ops
    )


def test_custom_new_classes_do_not_use_static_constructor_allocation() -> None:
    source = """
class Base:
    def __new__(cls, value):
        obj = super().__new__(cls)
        obj.value = value
        return obj

class Inherited(Base):
    pass

class Overridden(Base):
    def __new__(cls, value):
        return Base.__new__(cls, value)

inherited = Inherited(3)
overridden = Overridden(4)
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    assert all(op.get("kind") != "object_new_bound" for op in ops)
    for name in ("Inherited", "Overridden"):
        (callee,) = _module_global_reads_named(ops, name)
        assert any(
            op["kind"] == "call_func" and op.get("args", [None])[0] == callee["out"]
            for op in ops
        ), name


def test_guarded_setattr_uses_the_sole_guarded_store_spelling() -> None:
    gen = SimpleTIRGenerator()
    point = ClassId("__main__", "Point")
    gen.classes[point] = {
        "binding_name": "Point",
        "fields": {"x": 24},
        "layout_version": 7,
        "dynamic": False,
        "definition_exposed": False,
    }

    lowered = gen.map_ops_to_json(
        [
            MoltOp(
                kind="GUARDED_SETATTR",
                args=[MoltValue("obj"), MoltValue("ver"), "x", MoltValue("val"), point],
                result=MoltValue("out"),
                metadata={"class_definition": point.definition_symbol},
            )
        ]
    )

    kinds = [op.get("kind") for op in lowered]
    assert "guarded_field_init" not in kinds
    guarded_set = next(op for op in lowered if op.get("kind") == "guarded_field_set")
    assert guarded_set == {
        "kind": "guarded_field_set",
        "args": ["obj", "ver", "val"],
        "s_value": "x",
        "value": 24,
        "class_definition": "class:__main__:Point",
        "out": "out",
    }


def test_return_unwind_deactivates_popped_try_handler_label() -> None:
    source = """
def f(xs):
    try:
        return next(xs)
    except StopIteration:
        return 0
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"].endswith("f"))

    try_label = next(op["value"] for op in ops if op.get("kind") == "try_start")
    first_pop_idx = next(
        idx
        for idx, op in enumerate(ops)
        if idx > 0 and op.get("kind") == "exception_pop"
    )
    first_ret_idx = next(
        idx
        for idx, op in enumerate(ops[first_pop_idx + 1 :], first_pop_idx + 1)
        if op.get("kind") == "ret"
    )

    cleanup_checks = [
        op
        for op in ops[first_pop_idx + 1 : first_ret_idx]
        if op.get("kind") == "check_exception"
    ]
    assert cleanup_checks
    assert all(op.get("value") != try_label for op in cleanup_checks)


def _cpython_code(source: str, name: str) -> types.CodeType:
    pending = [compile(source, "<frame-homes>", "exec")]
    while pending:
        code = pending.pop()
        if code.co_name == name:
            return code
        pending.extend(
            const for const in code.co_consts if isinstance(const, types.CodeType)
        )
    raise AssertionError(f"no code object named {name!r}")


def _cpython_code_slots(source: str, name: str) -> tuple[str, ...]:
    """CPython's localsplus layout: locals, cells that are not locals, frees."""
    code = _cpython_code(source, name)
    return (
        code.co_varnames
        + tuple(cell for cell in code.co_cellvars if cell not in code.co_varnames)
        + code.co_freevars
    )


def _home_slots(ops: list[dict], kind: WireOpKind) -> set[int]:
    return {op["value"] for op in ops if op.get("kind") == kind}


def _lowered_function(source: str, suffix: str, **options: object) -> dict:
    gen = SimpleTIRGenerator(module_name="__main__", **options)
    gen.visit(ast.parse(source))
    return next(
        func for func in gen.to_json()["functions"] if func["name"].endswith(suffix)
    )


def test_sync_frame_homes_follow_the_cpython_code_slot_layout() -> None:
    source = """
def outer(a, b, *args, c, **kw):
    x = a
    def inner():
        return x + b
    y = [z for z in args]
    del y
    return inner
"""
    slot = _cpython_code_slots(source, "outer").index
    ops = _lowered_function(source, "__outer")["ops"]
    # Plain parameters and locals bind in their homes, a captured variable's
    # home holds its cell, and PEP 709 moves the comprehension's slot aside.
    assert {slot(name) for name in ("a", "args", "c", "kw", "y", "z")} <= (
        _home_slots(ops, "frame_home_store")
    )
    assert _home_slots(ops, "frame_home_cell") == {slot("x"), slot("b")}
    assert _home_slots(ops, "frame_home_take") == {slot("z")}
    assert _home_slots(ops, "frame_home_clear") == {slot("y")}
    inner_slot = _cpython_code_slots(source, "inner").index
    inner_ops = _lowered_function(source, "__inner")["ops"]
    assert _home_slots(inner_ops, "frame_home_cell") == {
        inner_slot("x"),
        inner_slot("b"),
    }


def test_sync_frame_bindings_are_released_only_by_their_homes() -> None:
    source = """
def f(value, seq):
    other = object()
    for item in seq:
        other = item
    del other
    return value
"""
    ops = _lowered_function(source, "__f")["ops"]
    kinds = [op.get("kind") for op in ops]
    assert "del_boundary" not in kinds
    assert "delete_var" not in kinds
    assert not any(op.get("bound_local") for op in ops)
    ret_index = kinds.index("ret")
    assert kinds[ret_index - 1] == "trace_exit"
    producers = {op["out"]: op for op in ops if isinstance(op.get("out"), str)}
    # The frame's exit releases its homes, so the returned binding is first
    # captured as an owned value.
    assert producers[ops[ret_index]["args"][0]].get("kind") == "binding_alias"


@pytest.mark.parametrize("enable_phi", [False, True])
@pytest.mark.parametrize(
    "expression", ["left if flag else right", "left and right", "left or right"]
)
def test_branch_returns_capture_the_selected_frame_binding(
    enable_phi: bool, expression: str
) -> None:
    ops = _lowered_function(
        f"def choose(flag, left, right):\n    return {expression}\n",
        "__choose",
        enable_phi=enable_phi,
    )["ops"]
    producers = {op["out"]: op for op in ops if "out" in op}
    ret_index = next(i for i, op in enumerate(ops) if op["kind"] == "ret")
    assert ops[ret_index - 1]["kind"] == "trace_exit"
    result = producers[ops[ret_index]["args"][0]]
    if result["kind"] == "binding_alias":
        captures = [result]
    else:
        assert result["kind"] == "load_var"
        stores = [
            op for op in ops if op["kind"] == "store_var" and op["var"] == result["var"]
        ]
        assert stores
        captures = [producers[op["args"][0]] for op in stores]
    # A merge may capture on each incoming edge or after its PHI. Every
    # selected binding needs an independent owner before the frame releases it.
    assert all(op["kind"] == "binding_alias" for op in captures)
    assert all(ops.index(op) < ret_index - 1 for op in captures)


def test_sync_frame_entries_adopt_their_python_arguments() -> None:
    source = """
def f(a, b):
    def g(c):
        return a + c
    return g

def make(x):
    yield x
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    functions = gen.to_json()["functions"]
    f = next(func for func in functions if func["name"].endswith("__f"))
    g = next(func for func in functions if func["name"].endswith("__g"))
    assert f["parameter_custody"] == ["transferred", "transferred"]
    # The function object lends the closure transport parameter.
    assert g["params"][0] == _MOLT_CLOSURE_PARAM
    assert g["parameter_custody"] == ["borrowed", "transferred"]
    # A stateful poll entry and module code borrow.
    assert all(
        "parameter_custody" not in func
        for func in functions
        if func["name"].endswith("_poll") or func["name"] == "molt_main"
    )


def test_frame_entry_custody_omits_entries_that_transfer_no_arguments() -> None:
    source = """
def empty():
    local = 1
    return local

def outer(value):
    def closed():
        return value
    def variadic(*args, **kwargs):
        return value, args, kwargs
    return closed, variadic

def suspended():
    yield 1
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    functions = gen.to_json()["functions"]

    def by_suffix(suffix: str) -> dict:
        return next(func for func in functions if func["name"].endswith(suffix))

    empty = by_suffix("__empty")
    closed = by_suffix("__closed")
    assert empty["params"] == []
    assert closed["params"] == [_MOLT_CLOSURE_PARAM]
    assert "parameter_custody" not in empty
    assert "parameter_custody" not in closed
    # Omitting entry custody must not remove ownership of the frame's locals.
    assert any(op["kind"] == "frame_home_store" for op in empty["ops"])
    assert any(op["kind"] == "frame_home_cell" for op in closed["ops"])
    assert by_suffix("__outer")["parameter_custody"] == ["transferred"]
    assert by_suffix("__variadic")["parameter_custody"] == [
        "borrowed",
        "transferred",
        "transferred",
    ]
    for func in functions:
        if "parameter_custody" in func:
            assert len(func["parameter_custody"]) == len(func["params"])
            assert "transferred" in func["parameter_custody"]
        if func["name"].endswith("_poll") or func["name"] == "molt_main":
            assert "parameter_custody" not in func


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
def test_local_read_after_a_callback_comes_from_its_home_from_313(
    target_python: tuple[int, int],
) -> None:
    source = """
def f(callback):
    value = object()
    callback()
    return value
"""
    slots = _cpython_code_slots(source, "f")
    ops = _lowered_function(source, "__f", target_python=target_python)["ops"]
    loads = _home_slots(ops, "frame_home_load")
    if target_python >= (3, 13):
        # The global object callable may rebind callback through a PEP 667 frame
        # proxy; that callback may then rebind value. Each later read needs its home.
        assert loads == {slots.index("callback"), slots.index("value")}
        object_call, callback_call = [
            i for i, op in enumerate(ops) if op["kind"] == "call_func"
        ]
        callback_load, value_load = [
            (i, op["value"])
            for i, op in enumerate(ops)
            if op["kind"] == "frame_home_load"
        ]
        assert object_call < callback_load[0] < callback_call < value_load[0]
        assert callback_load[1] == slots.index("callback")
        assert value_load[1] == slots.index("value")
    else:
        assert loads == set()


def test_comprehension_puts_the_enclosing_binding_back_on_both_exits() -> None:
    source = """
def f(items):
    item = "outer"
    names = [str(item) for item in items]
    return item, names
"""
    slot = _cpython_code_slots(source, "f").index("item")
    ops = _lowered_function(source, "__f")["ops"]
    (take,) = [op for op in ops if op.get("kind") == "frame_home_take"]
    assert take["value"] == slot
    restores = [
        index
        for index, op in enumerate(ops)
        if op.get("kind") == "frame_home_store"
        and op.get("args") == [take["out"]]
        and op["value"] == slot
    ]
    assert len(restores) == 2
    # One restore is the normal exit. The other runs at the cleanup label that
    # the comprehension body's exception checks target.
    check_targets = {
        op.get("value") for op in ops if op.get("kind") == "check_exception"
    }
    assert (
        sum(
            ops[index - 1].get("kind") == "label"
            and ops[index - 1].get("value") in check_targets
            for index in restores
        )
        == 1
    )


def test_home_backed_locals_carry_no_missing_transport() -> None:
    source = """
def f(flag, n):
    if flag:
        seen = n
    while n > 0:
        n = n - 1
    return seen
"""
    slot = _cpython_code_slots(source, "f").index
    ops = _lowered_function(source, "__f", target_python=(3, 12))["ops"]
    missing = {op["out"] for op in ops if op.get("kind") == "missing"}
    # No missing value seeds a home-backed local's SSA transport: its home is
    # the canonical unbound state.
    assert not any(
        op.get("kind") == "store_var" and set(op.get("args") or []) & missing
        for op in ops
    )
    # Only the read that may precede its store loads the home. A parameter is
    # bound from entry, so the loop reads its transport, not the home.
    assert _home_slots(ops, "frame_home_load") == {slot("seen")}


@pytest.mark.parametrize(
    ("target_python", "shared"), [((3, 12), True), ((3, 13), False), ((3, 14), False)]
)
def test_function_locals_dict_is_read_from_the_runtime_frame(
    target_python: tuple[int, int], shared: bool
) -> None:
    gen = SimpleTIRGenerator(target_python=target_python)
    gen.start_function(
        "f",
        params=["value"],
        code_slots=CodeSlotDeclaration(("value",), ("value",), (), ()),
    )
    gen.locals["value"] = MoltValue("value", type_hint="Any")
    gen.parameter_bindings = {"value": "value"}
    result = gen._emit_locals_dict()
    (frame_locals,) = [op for op in gen.current_ops if op.kind == "FRAME_LOCALS"]
    assert frame_locals.result is result
    # Before PEP 667 an activation's locals() is one refreshed dict.
    assert frame_locals.metadata == {"shared": shared}
    assert not any(
        op.kind in {"DICT_NEW", "DICT_SET", "DICT_UPDATE_MISSING", "FRAME_LOCALS_SET"}
        for op in gen.current_ops
    )


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("platform", ["linux", "darwin", "win32"])
def test_builtin_exception_names_follow_target_version_and_platform(
    target_python: tuple[int, int], platform: str
) -> None:
    source = """
def f():
    return PythonFinalizationError, WindowsError
"""

    gen = SimpleTIRGenerator(
        module_name="__main__",
        target_python=target_python,
        target_sys_platform=platform,
    )
    assert gen._builtin_exception_is_available("PythonFinalizationError") == (
        target_python >= (3, 13)
    )
    assert gen._builtin_exception_is_available("WindowsError") == (platform == "win32")
    gen.visit(ast.parse(source))
    ops = next(
        func["ops"] for func in gen.to_json()["functions"] if func["name"].endswith("f")
    )
    # Availability gates the builtin namespace, not live LOAD_GLOBAL resolution:
    # globals can shadow an available builtin or supply an unavailable name.
    assert not any(op["kind"] == "exception_class" for op in ops)
    for name in ("PythonFinalizationError", "WindowsError"):
        assert len(_module_global_reads_named(ops, name)) == 1


@pytest.mark.parametrize("host_platform", ["linux", "darwin", "win32"])
def test_unknown_target_platform_does_not_inherit_host_exception_namespace(
    monkeypatch: pytest.MonkeyPatch, host_platform: Literal["linux", "darwin", "win32"]
) -> None:
    import sys

    # Every loaded Molt module that reads `sys` sees the faked host; pytest and
    # the rest of the process keep the real platform.
    readers = sorted(
        (
            module
            for name, module in tuple(sys.modules.items())
            if name.split(".", 1)[0] == "molt" and getattr(module, "sys", None) is sys
        ),
        key=lambda module: module.__name__,
    )
    assert readers, "the host-platform fake must reach the compiler"
    install_module_view(monkeypatch, "sys", sys, *readers, platform=host_platform)
    gen = SimpleTIRGenerator(target_sys_platform=None)
    assert not gen._builtin_exception_is_available("WindowsError")


def test_with_context_exit_checks_after_exception_frame_release() -> None:
    source = """
def f(cm):
    with cm:
        x = 1
    return x
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"].endswith("f"))

    context_exit_indices = [
        idx for idx, op in enumerate(ops) if op.get("kind") == "context_exit"
    ]
    assert context_exit_indices
    for idx in context_exit_indices:
        assert ops[idx + 1]["kind"] == "check_exception"
        body_end = max(i for i in range(idx) if ops[i]["kind"] == "try_end")
        assert any(op["kind"] == "exception_pop" for op in ops[body_end + 1 : idx])


def test_rebound_same_module_class_call_keeps_runtime_lookup() -> None:
    source = """
class Point:
    def __init__(self, x):
        self.x = x

Point = int
value = Point(3)
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    assert all(op.get("kind") != "object_new_bound" for op in ops)


def test_globals_escape_keeps_static_class_runtime_lookup() -> None:
    source = """
class Point:
    def __init__(self, x):
        self.x = x

ns = globals()
value = Point(3)
"""
    gen = SimpleTIRGenerator(module_name="__main__")
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    ops = next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")

    # Once globals() escapes, the class read must stay a runtime lookup with
    # LOAD_GLOBAL semantics (MODULE_GET_GLOBAL): ns mutation must be observed
    # and a deleted binding must raise NameError, not AttributeError.
    reads = _module_global_reads_named(ops, "Point")
    assert reads
    assert _module_attr_reads_named(ops, "Point") == []
    assert all(op.get("effect_proof") is None for op in reads)


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("loop", ["while i < 3:", "for i in range(3):"])
def test_function_loop_class_lookup_stays_on_executed_path(
    loop: str, target_python: tuple[int, int]
) -> None:
    source = """
class Point:
    def __init__(self, x):
        self.x = x

def main():
    i = 0
    LOOP
        point = Point(i)
        i += 1
"""
    gen = SimpleTIRGenerator(module_name="__main__", target_python=target_python)
    gen.visit(ast.parse(source.replace("LOOP", loop)))
    ir = gen.to_json()
    func_ops = next(
        func["ops"] for func in ir["functions"] if func["name"].endswith("main")
    )

    cache_vars = [
        op.get("var")
        for op in func_ops
        if op.get("kind") == "store_var"
        and str(op.get("var", "")).startswith("__molt_static_class_")
    ]
    assert not cache_vars
    assert not _module_attr_reads_named(func_ops, "Point")
    callees = _module_global_reads_named(func_ops, "Point")
    assert callees
    # Loop specialization may clone the body. Every lookup and its invocation
    # must remain inside its own loop, after the condition's exit check.
    loop_exits: list[bool] = []
    checked_callees: set[str] = set()
    called_callees: set[str] = set()
    for op in func_ops:
        kind = op["kind"]
        if kind == "loop_start":
            loop_exits.append(False)
        elif kind == "loop_end":
            loop_exits.pop()
        elif kind in {"loop_break_if_false", "loop_break_if_true"}:
            loop_exits[-1] = True
        elif op in callees:
            assert loop_exits and loop_exits[-1]
            checked_callees.add(op["out"])
        elif kind == "call_func" and op.get("args", [None])[0] in checked_callees:
            assert loop_exits and loop_exits[-1]
            called_callees.add(op["args"][0])
    assert called_callees == {op["out"] for op in callees}


def test_frontend_defers_loop_motion_to_canonical_tir() -> None:
    assert not hasattr(SimpleTIRGenerator(), "_hoist_loop_invariant_pure_ops")


def test_serialization_keeps_dynamic_arithmetic_inside_source_loop() -> None:
    ops = [
        MoltOp("MISSING", [], MoltValue("obj")),
        MoltOp("CONST", [1], MoltValue("one")),
        MoltOp("LOOP_START", [], MoltValue("none")),
        MoltOp("MISSING", [], MoltValue("cond")),
        MoltOp("LOOP_BREAK_IF_TRUE", [MoltValue("cond")], MoltValue("none")),
        MoltOp("ADD", [MoltValue("obj"), MoltValue("one")], MoltValue("maybe_raises")),
        MoltOp("FLOORDIV", [MoltValue("obj"), MoltValue("one")], MoltValue("division")),
        MoltOp("LOOP_END", [], MoltValue("none")),
    ]
    lowered = SimpleTIRGenerator().map_ops_to_json(ops)
    positions = {
        kind: next(i for i, op in enumerate(lowered) if op["kind"] == kind)
        for kind in ("loop_start", "add", "floordiv", "loop_end")
    }
    assert (
        positions["loop_start"]
        < positions["add"]
        < positions["floordiv"]
        < positions["loop_end"]
    )


_FRAME_NAMES = frozenset({"self", "none", ""})


def _stateful_polls(source: str) -> dict[str, list[MoltOp]]:
    gen = SimpleTIRGenerator()
    gen.visit(ast.parse(source))
    return {
        name: function["ops"]
        for name, function in gen.funcs_map.items()
        if any(op.kind == "STATE_SWITCH" for op in function["ops"])
    }


def _op_reads_and_writes(op: MoltOp) -> tuple[list[str], list[str]]:
    first_output = FRONTEND_FIRST_TRAILING_RESULT_ARG.get(op.kind, len(op.args))
    reads = [
        arg.name
        for arg in op.args[:first_output]
        if isinstance(arg, MoltValue) and arg.name not in _FRAME_NAMES
    ]
    writes = [
        value.name
        for value in (op.result, *op.args[first_output:])
        if isinstance(value, MoltValue) and value.name not in _FRAME_NAMES
    ]
    if op.kind == "LOAD_VAR":
        reads.append("home:" + op.metadata["var"])
    elif op.kind == "STORE_VAR":
        writes.append("home:" + op.metadata["var"])
    return reads, writes


def _forward_must(
    cfg: CFGGraph,
    gen_sets: list[set],
    entry_blocks: dict[int, frozenset],
    edges: Callable[[int, int], bool],
) -> dict[int, frozenset]:
    """Intersect each block's predecessors' out sets until nothing changes."""
    state: dict[int, frozenset] = dict(entry_blocks)
    changed = True
    while changed:
        changed = False
        for block in range(len(cfg.blocks)):
            if block in entry_blocks:
                continue
            incoming = [
                state[pred] | gen_sets[pred]
                for pred in cfg.predecessors.get(block, ())
                if pred in state and edges(pred, block)
            ]
            if not incoming:
                continue
            merged = frozenset.intersection(*map(frozenset, incoming))
            if state.get(block) != merged:
                state[block] = merged
                changed = True
    return state


def _reads_without_definition(ops: list[MoltOp]) -> list[tuple[int, str]]:
    cfg = build_cfg(ops)
    defined = [
        {
            name
            for idx in range(block.start, block.end)
            for name in _op_reads_and_writes(ops[idx])[1]
        }
        for block in cfg.blocks
    ]
    state = _forward_must(cfg, defined, {0: frozenset()}, lambda _p, _b: True)
    missing = []
    for block_id, block in enumerate(cfg.blocks):
        if block_id not in state:
            continue
        current = set(state[block_id])
        for idx in range(block.start, block.end):
            reads, writes = _op_reads_and_writes(ops[idx])
            missing.extend((idx, name) for name in reads if name not in current)
            current.update(writes)
    return missing


def _spill_reloads(ops: list[MoltOp]) -> list[int]:
    """Frame reloads transport into a home with entry/resume definitions."""
    stores = {}
    for op in ops:
        if op.kind == "STORE_VAR":
            home = op.metadata["var"]
            stores[home] = stores.get(home, 0) + 1
    return [
        idx
        for idx, op in enumerate(ops[:-1])
        if op.kind == "LOAD_CLOSURE"
        and ops[idx + 1].kind == "STORE_VAR"
        and ops[idx + 1].args == [op.result]
        and stores[ops[idx + 1].metadata["var"]] > 1
    ]


def _reloads_before_their_store(ops: list[MoltOp]) -> list[int]:
    """Reloads that the first activation reaches before storing their slot."""
    cfg = build_cfg(ops)
    switch_block = next(
        cfg.op_blocks[idx] for idx, op in enumerate(ops) if op.kind == "STATE_SWITCH"
    )
    label_blocks = {
        cfg.op_blocks[idx] for idx, op in enumerate(ops) if op.kind == "STATE_LABEL"
    }

    def first_activation_edge(pred: int, block: int) -> bool:
        return not (pred == switch_block and block in label_blocks)

    stored = [
        {
            ops[idx].args[1]
            for idx in range(block.start, block.end)
            if ops[idx].kind == "STORE_CLOSURE"
        }
        for block in cfg.blocks
    ]
    state = _forward_must(cfg, stored, {0: frozenset()}, first_activation_edge)
    early = []
    reloads = set(_spill_reloads(ops))
    for block_id, block in enumerate(cfg.blocks):
        if block_id not in state:
            continue
        current = set(state[block_id])
        for idx in range(block.start, block.end):
            op = ops[idx]
            if idx in reloads and op.args[1] not in current:
                early.append(idx)
            if op.kind == "STORE_CLOSURE":
                current.add(op.args[1])
    return early


_RESUME_IN_LOOP_SOURCES = {
    "yield_from": "def f(iterable):\n    return (yield from iterable)\n",
    # HF-104: an outer synchronous comprehension around an awaiting one.
    "nested_comprehension": (
        "async def f(agen, inc):\n"
        "    return [[await inc(y) async for y in agen()] for _ in range(2)]\n"
    ),
    "awaiting_element": (
        "async def f(inc):\n    return [await inc(i) for i in range(4)]\n"
    ),
    "awaiting_set_and_dict": (
        "async def f(inc):\n"
        "    return {await inc(i) for i in range(3)}, {i: await inc(i) for i in range(3)}\n"
    ),
    "awaiting_innermost_clause": (
        "async def f(inc):\n"
        "    return [(i, await inc(j)) for i in range(2) for j in range(2)]\n"
    ),
    "async_generator_comprehension": (
        "async def f(inc):\n"
        "    for base in range(2):\n"
        "        yield [await inc(base + i) for i in range(3)]\n"
    ),
}


def test_yield_from_send_and_throw_results_have_distinct_ssa_definitions() -> None:
    polls = _stateful_polls("def f(iterable):\n    return (yield from iterable)\n")
    assert polls
    for name, ops in polls.items():
        definitions = [op.result.name for op in ops if op.result.name != "none"]
        assert len(definitions) == len(set(definitions))
        gen = SimpleTIRGenerator()
        gen.map_ops_to_json(ops, function_name=name)


@pytest.mark.parametrize(
    "source", _RESUME_IN_LOOP_SOURCES.values(), ids=_RESUME_IN_LOOP_SOURCES.keys()
)
def test_values_read_after_a_resume_inside_a_loop_are_defined(source: str) -> None:
    polls = _stateful_polls(source)
    assert polls
    for name, ops in polls.items():
        assert _reads_without_definition(ops) == [], name
        assert _reloads_before_their_store(ops) == [], name


def test_spilled_results_are_published_only_after_the_exception_check() -> None:
    gen = SimpleTIRGenerator()
    value = MoltValue("checked_result")
    gen.current_ops[:] = [
        MoltOp("STATE_SWITCH", [], MoltValue("none")),
        MoltOp("CALL", ["effect"], value),
        MoltOp(
            "CHECK_EXCEPTION",
            [900],
            MoltValue("none"),
            exception_edge=ExceptionEdgeKind.ORIGIN,
        ),
        MoltOp("STATE_YIELD", [], MoltValue("none")),
        MoltOp("STATE_LABEL", [901], MoltValue("none")),
        MoltOp("ret", [value], MoltValue("none")),
        MoltOp("LABEL", [900], MoltValue("none")),
        MoltOp("ret_void", [], MoltValue("none")),
    ]
    gen._spill_async_temporaries()
    polls = {"checked_result": gen.current_ops}
    checked = 0
    for ops in polls.values():
        producers = {op.result.name: index for index, op in enumerate(ops)}
        for index, op in enumerate(ops):
            if op.kind != "STORE_VAR":
                continue
            producer = producers[op.args[0].name]
            # CHECK_EXCEPTION is the success boundary of a raising producer.
            # Publishing beforehand would expose a failed operation's value
            # to a handler rather than preserving the home's previous value.
            boundary = next(
                (
                    i
                    for i in range(producer + 1, index + 2)
                    if i < len(ops) and ops[i].kind == "CHECK_EXCEPTION"
                ),
                None,
            )
            if boundary is not None:
                assert producer < boundary < index
                checked += 1
    assert checked, "fixture must spill a checked producer across an await"


def test_prologue_values_keep_the_first_activation_value() -> None:
    # The handler makes the poll save its exception-stack baselines in the
    # prologue, and the return restores them after the await's resume.
    polls = _stateful_polls(
        "async def f(g):\n"
        "    try:\n"
        "        await g()\n"
        "    finally:\n"
        "        pass\n"
        "    return 1\n"
    )
    (ops,) = polls.values()
    switch = next(idx for idx, op in enumerate(ops) if op.kind == "STATE_SWITCH")
    prologue = {name for op in ops[:switch] for name in _op_reads_and_writes(op)[1]}
    baseline_homes = {
        op.metadata["var"]
        for op in ops[:switch]
        if op.kind == "STORE_VAR" and op.args[0].name in prologue
    }
    baseline_slots = {
        ops[idx].args[1]
        for idx in _spill_reloads(ops)
        if ops[idx + 1].metadata["var"] in baseline_homes
    }
    assert baseline_slots, "resume must reload the first activation's baselines"
    definitions = [op.result.name for op in ops if op.result.name != "none"]
    assert len(definitions) == len(set(definitions))
    assert _reads_without_definition(ops) == []
    assert _reloads_before_their_store(ops) == []
    # No resume may overwrite the frame's first-activation baseline.
    cfg = build_cfg(ops)
    from_resume: set[int] = set()
    pending = [
        cfg.op_blocks[idx] for idx, op in enumerate(ops) if op.kind == "STATE_LABEL"
    ]
    while pending:
        block = pending.pop()
        if block not in from_resume:
            from_resume.add(block)
            pending.extend(cfg.successors.get(block, ()))
    for idx, op in enumerate(ops):
        if op.kind == "STORE_CLOSURE" and op.args[1] in baseline_slots:
            assert idx > switch, "a prologue store would save every activation"
            assert cfg.op_blocks[idx] not in from_resume, idx


def test_dictionary_display_uses_strict_mapping_primitive() -> None:
    function = _lowered_function(
        "def f(mapping):\n    return {1: 2, **mapping, 3: 4, **mapping}\n", "f"
    )
    operations = function["ops"]
    assert sum(op["kind"] == "dict_unpack" for op in operations) == 2
    assert not any(op["kind"] == "dict_update" for op in operations)
    for op in operations:
        if op["kind"] == "dict_unpack":
            assert len(op["args"]) == 2


def test_general_dictionary_update_keeps_pair_iterable_primitive() -> None:
    function = _lowered_function(
        "def f(pairs):\n    value = {}\n    value.update(pairs)\n    return value\n",
        "f",
    )
    assert any(op["kind"] == "dict_update" for op in function["ops"])
    assert not any(op["kind"] == "dict_unpack" for op in function["ops"])
    outputs = [
        op["out"] for op in function["ops"] if "out" in op and op["out"] != "none"
    ]
    assert len(outputs) == len(set(outputs)), "one producer for every SSA result"


def _reachable_without_node(
    successors: Sequence[Sequence[int]], entry: int, excluded: int | None
) -> set[int]:
    """Independent dominance oracle: remove a node and search remaining paths."""
    reached: set[int] = set()
    pending = [entry]
    while pending:
        node = pending.pop()
        if node == excluded or node in reached:
            continue
        reached.add(node)
        pending.extend(successors[node])
    return reached


def _assert_dominance_matches_path_removal(
    successors: Sequence[Sequence[int]], entry: int, dominance: DominatorTree
) -> None:
    reached = _reachable_without_node(successors, entry, None)
    without = [
        _reachable_without_node(successors, entry, node)
        for node in range(len(successors))
    ]
    for usage in range(len(successors)):
        assert dominance.is_reachable(usage) == (usage in reached)
        proper_dominators = []
        for definition in range(len(successors)):
            expected = usage in reached and usage not in without[definition]
            assert dominance.dominates(definition, usage) == expected, (
                successors,
                entry,
                definition,
                usage,
            )
            if expected and definition != usage:
                proper_dominators.append(definition)
        # The closest proper dominator is itself dominated by every other one.
        closest = [
            node
            for node in proper_dominators
            if all(node not in without[other] for other in proper_dominators)
        ]
        assert len(closest) <= 1
        assert dominance.immediate_dominator(usage) == (closest[0] if closest else None)


def test_cfg_dominance_matches_exhaustive_path_removal() -> None:
    # All directed four-node graphs without self edges: diamonds, backedges,
    # irreducible loops, crossed DFS siblings and unreachable predecessors.
    edges = [
        (source, dest) for source in range(4) for dest in range(4) if source != dest
    ]
    for mask in range(1 << len(edges)):
        successors: list[list[int]] = [[] for _ in range(4)]
        for bit, (source, dest) in enumerate(edges):
            if mask & (1 << bit):
                successors[source].append(dest)
        _assert_dominance_matches_path_removal(
            successors, 0, DominatorTree.compute(successors)
        )


def test_cfg_dominance_nonzero_entry_self_edges_and_absent_nodes() -> None:
    successors = [[0, 1, 1], [1], [0, 3], [1, 3], [2]]
    dominance = DominatorTree.compute(successors, entry=2)
    _assert_dominance_matches_path_removal(successors, 2, dominance)
    for node in (-1, 4, 5):
        assert not dominance.is_reachable(node)
        assert not dominance.dominates(node, node)
        assert not dominance.dominates(2, node)
        assert not dominance.dominates(node, 2)
        assert dominance.immediate_dominator(node) is None
    empty = build_cfg([])
    assert empty.reachable == set()
    assert not empty.dominance.dominates(0, 0)
    assert empty.dominance.immediate_dominator(0) is None
    for entry in (-1, len(successors)):
        absent = DominatorTree.compute(successors, entry=entry)
        for node in range(len(successors)):
            assert not absent.is_reachable(node)
            assert not absent.dominates(node, node)
            assert absent.immediate_dominator(node) is None


def test_cfg_dominance_handles_deep_chain_without_recursive_traversal() -> None:
    # Exercise the real CFG producer with depth well above Python's call stack.
    ops = [MoltOp("LABEL", [index], MoltValue("none")) for index in range(8192)]
    ops.append(MoltOp("ret_void", [], MoltValue("none")))
    cfg = build_cfg(ops)
    last = cfg.op_blocks[len(ops) - 1]
    assert cfg.reachable == set(range(len(cfg.blocks)))
    assert cfg.dominance.immediate_dominator(0) is None
    for block in range(1, len(cfg.blocks)):
        assert cfg.dominance.immediate_dominator(block) == block - 1
        assert cfg.dominance.dominates(0, block)
        assert cfg.dominance.dominates(block, last)
        assert not cfg.dominance.dominates(block, block - 1)


def test_cfg_dominance_respects_resume_bypass() -> None:
    ops = [
        MoltOp("STATE_SWITCH", [], MoltValue("none")),
        MoltOp("CONST", [1], MoltValue("before_yield")),
        MoltOp("STATE_YIELD", [], MoltValue("none")),
        MoltOp("ret", [MoltValue("before_yield")], MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    switch = cfg.op_blocks[0]
    definition = cfg.op_blocks[1]
    yielding = cfg.op_blocks[2]
    resume = cfg.op_blocks[3]
    assert cfg.edge_kinds[switch, resume] & CFGEdgeKind.RESUME
    assert cfg.successors[yielding] == ()
    assert cfg.dominance.dominates(switch, resume)
    assert not cfg.dominance.dominates(definition, resume)
    _assert_dominance_matches_path_removal(
        [cfg.successors[block.id] for block in cfg.blocks], 0, cfg.dominance
    )


def test_cfg_models_check_exception_target_edge() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(
            kind="CHECK_EXCEPTION",
            exception_edge=ExceptionEdgeKind.ORIGIN,
            args=[7],
            result=MoltValue("none"),
        ),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[7], result=MoltValue("none")),
        MoltOp(kind="ret", args=[MoltValue("x")], result=MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    check_block = cfg.op_blocks[1]
    label_block = cfg.label_to_block["7"]
    assert label_block in cfg.successors.get(check_block, [])
    assert cfg.edge_kinds[check_block, label_block] & CFGEdgeKind.EXCEPTION
    assert not cfg.dominance.dominates(cfg.op_blocks[2], label_block)
    _assert_dominance_matches_path_removal(
        [cfg.successors[block.id] for block in cfg.blocks], 0, cfg.dominance
    )


def test_cfg_tracks_path_local_try_closes_without_changing_if_structure() -> None:
    ops = [
        MoltOp("TRY_START", [11], MoltValue("none")),
        MoltOp("IF", [MoltValue("cond")], MoltValue("none")),
        MoltOp("TRY_END", [11], MoltValue("none")),
        MoltOp("ret_void", [], MoltValue("none")),
        MoltOp("END_IF", [], MoltValue("none")),
        MoltOp("TRY_END", [11], MoltValue("none")),
        MoltOp("ret_void", [], MoltValue("none")),
        MoltOp("LABEL", [11], MoltValue("none")),
        MoltOp("ret_void", [], MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    assert cfg.control.if_to_end == {1: 4}
    handler = cfg.label_to_block["11"]
    assert cfg.block_entry_label[handler] == "11"
    assert handler in cfg.successors[cfg.op_blocks[0]]


def _jump_label_ops(target: int, label: int, constant: int) -> list[MoltOp]:
    # Blocks: 0 CONST, 1 JUMP, 2 CONST, 3 LABEL, 4 ret_void.
    return [
        MoltOp(kind="CONST", args=[constant], result=MoltValue("v0")),
        MoltOp(kind="JUMP", args=[target], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[constant + 1], result=MoltValue("v1")),
        MoltOp(kind="LABEL", args=[label], result=MoltValue("none")),
        MoltOp(kind="ret_void", args=[], result=MoltValue("none")),
    ]


def test_cfg_is_shared_by_op_lists_with_equal_control() -> None:
    # Operands the CFG never reads may differ; the graph is the same object.
    first = build_cfg(_jump_label_ops(1, 1, constant=10))
    second = build_cfg(_jump_label_ops(1, 1, constant=20))

    assert second is first
    assert first.successors[1] == (3,)


def test_cfg_differs_when_a_label_operand_differs() -> None:
    to_label = build_cfg(_jump_label_ops(1, 1, constant=10))
    to_nowhere = build_cfg(_jump_label_ops(2, 1, constant=10))

    assert to_nowhere is not to_label
    assert to_label.successors[1] == (3,)
    assert to_nowhere.successors[1] == ()


@pytest.mark.parametrize(
    ("kinds", "expected"),
    [
        ((), ()),
        (("CONST", "CONST", "ADD", "ret_void"), (0, 0, 0, 1)),
        (
            ("IF", "CONST", "CONST", "ELSE", "CONST", "END_IF", "ret_void"),
            (0, 1, 1, 2, 3, 4, 5),
        ),
    ],
)
def test_cfg_dense_operation_blocks_keep_absent_indices_absent(
    kinds: tuple[str, ...], expected: tuple[int, ...]
) -> None:
    # The expected partition is written independently of the CFG block builder.
    ops = [MoltOp(kind, [], MoltValue("none")) for kind in kinds]
    cfg = build_cfg(ops)

    assert cfg.op_blocks == expected
    assert tuple(cfg.block_for_op(index) for index in range(len(ops))) == expected
    for index in (-len(ops) - 1, -1, len(ops), len(ops) + 1, 10**100):
        assert cfg.block_for_op(index) is None


def test_resume_liveness_rejects_indices_outside_the_operation_stream() -> None:
    from molt.frontend.value_liveness import ValueLiveness

    live = ValueLiveness(
        [MoltOp("ADD", [MoltValue("left"), MoltValue("right")], MoltValue("result"))]
    )
    assert live.live_into(0) == ["left", "right"]
    for index in (-1, 1, 10**100):
        with pytest.raises(KeyError) as failure:
            live.live_into(index)
        assert failure.value.args == (index,)


def test_shared_cfg_cannot_be_mutated() -> None:
    cfg = build_cfg(_jump_label_ops(1, 1, constant=10))

    with pytest.raises(TypeError):
        cfg.successors[0] = (1,)  # type: ignore[index]
    with pytest.raises(AttributeError):
        cfg.successors[0].append(1)  # type: ignore[attr-defined]
    with pytest.raises(TypeError):
        cfg.label_to_block["1"] = 0  # type: ignore[index]
    with pytest.raises(AttributeError):
        cfg.reachable.add(3)  # type: ignore[attr-defined]
    with pytest.raises(TypeError):
        cfg.op_blocks[0] = 2  # type: ignore[index]
    assert not hasattr(cfg.blocks[0], "__dict__")


@pytest.mark.parametrize("seed", range(300))
def test_cfg_dominance_matches_the_path_definition(seed: int) -> None:
    # Extend the exhaustive four-node oracle to larger, seeded cyclic graphs.
    # Unreachable nodes cannot establish a value-availability proof.
    rng = random.Random(seed)
    count = rng.randint(1, 24)
    successors = [
        sorted(rng.sample(range(count), rng.randint(0, min(3, count))))
        for _ in range(count)
    ]
    _assert_dominance_matches_path_removal(
        successors, 0, DominatorTree.compute(successors)
    )


def test_shared_cfg_computes_dominance_only_on_demand(tmp_path: Path) -> None:
    label = str(tmp_path / "lazy_dominance")

    def ops(constant: int) -> list[MoltOp]:
        return [
            MoltOp("CONST", [constant], MoltValue("value")),
            MoltOp("JUMP", [label], MoltValue("none")),
            MoltOp("LABEL", [label], MoltValue("none")),
            MoltOp("ret_void", [], MoltValue("none")),
        ]

    first = build_cfg(ops(1))
    assert "dominance" not in first.__dict__
    dominance = first.dominance
    second = build_cfg(ops(2))
    assert second is first
    assert second.dominance is dominance
    assert dominance.dominates(0, first.op_blocks[3])


def test_shared_cfg_keeps_exception_handler_identity() -> None:
    def ops(handler: str) -> list[MoltOp]:
        return [
            MoltOp("TRY_START", [handler], MoltValue("none")),
            MoltOp("ret_void", [], MoltValue("none")),
            MoltOp("LABEL", ["first"], MoltValue("none")),
            MoltOp("ret_void", [], MoltValue("none")),
            MoltOp("LABEL", ["second"], MoltValue("none")),
            MoltOp("ret_void", [], MoltValue("none")),
        ]

    first, second = build_cfg(ops("first")), build_cfg(ops("second"))
    assert first is not second
    start = first.op_blocks[0]
    first_handler, second_handler = first.op_blocks[2], first.op_blocks[4]
    assert first.edge_kinds[start, first_handler] == CFGEdgeKind.EXCEPTION
    assert (start, second_handler) not in first.edge_kinds
    assert second.edge_kinds[start, second_handler] == CFGEdgeKind.EXCEPTION
    assert (start, first_handler) not in second.edge_kinds


def test_shared_cfg_keeps_resume_edge_kind() -> None:
    def ops(terminator: str) -> list[MoltOp]:
        return [
            MoltOp("STATE_SWITCH", [], MoltValue("none")),
            MoltOp(terminator, [], MoltValue("none")),
            MoltOp("CONST", [1], MoltValue("resumed")),
            MoltOp("ret_void", [], MoltValue("none")),
        ]

    suspending, returning = build_cfg(ops("STATE_YIELD")), build_cfg(ops("ret_void"))
    assert suspending is not returning
    target = suspending.op_blocks[2]
    assert suspending.edge_kinds[0, target] == CFGEdgeKind.RESUME
    assert target in suspending.reachable
    assert target not in returning.reachable
    assert (0, returning.op_blocks[2]) not in returning.edge_kinds


def _module_attr_reads_named(ops: list[dict], name: str) -> list[dict]:
    const_strings = {
        op["out"]: op["s_value"]
        for op in ops
        if op.get("kind") == "const_str" and "out" in op
    }
    return [
        op
        for op in ops
        if op.get("kind") == "module_get_attr"
        and len(op.get("args") or []) >= 2
        and const_strings.get(op["args"][1]) == name
    ]


def _module_global_reads_named(ops: list[dict], name: str) -> list[dict]:
    const_strings = {
        op["out"]: op["s_value"]
        for op in ops
        if op.get("kind") == "const_str" and "out" in op
    }
    return [
        op
        for op in ops
        if op.get("kind") == "module_get_global"
        and len(op.get("args") or []) >= 2
        and const_strings.get(op["args"][1]) == name
    ]
