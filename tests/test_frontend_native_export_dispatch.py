from __future__ import annotations

import ast

import pytest

from molt.frontend import SimpleTIRGenerator
from molt.frontend.sema import FunctionKind


IMPORTS = [
    ("from nativepkg.ops import {name} as operation", "operation"),
    ("from nativepkg import ops", "ops.{name}"),
    ("import nativepkg.ops as ops", "ops.{name}"),
    ("import nativepkg.ops", "nativepkg.ops.{name}"),
    ("import nativepkg.ops as ops\nalias = ops", "alias.{name}"),
]


def assert_bound(source: str, name: str = "run") -> None:
    generator = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ops"},
        direct_call_modules={"__main__"},
        stdlib_allowlist=set(),
        native_python_exports={f"nativepkg.ops.{name}"},
    )
    generator.visit(ast.parse(source))
    operations = [
        op for function in generator.to_json()["functions"] for op in function["ops"]
    ]
    assert not any(op["kind"] == "invoke_ffi" for op in operations)
    assert not any("nativepkg_ops__" in str(op) for op in operations)
    call_line = len(source.splitlines())
    assert any(
        op["kind"] in {"call_bind", "call_func", "call_indirect", "call_guarded"}
        and op.get("source_line") == call_line
        for op in operations
    ), "the source call must dispatch an actual callable, not only import helpers"


@pytest.mark.parametrize("name", ["run", "split"])
@pytest.mark.parametrize("import_source,callee", IMPORTS)
@pytest.mark.parametrize("local", [False, True])
def test_visibility_only_export_uses_actual_callable(
    name: str, import_source: str, callee: str, local: bool
) -> None:
    source = import_source.format(name=name) + "\n" + callee.format(name=name) + "(1)\n"
    if local:
        source = "def invoke():\n" + "".join(
            "    " + line + "\n" for line in source.splitlines()
        )
    assert_bound(source, name)


@pytest.mark.parametrize(
    "body",
    [
        "alias = run\nsecond = alias\nsecond(1)",
        "def invoke(value):\n    return value(1)\ninvoke(run)",
        "box = {'run': run}\nbox['run'](1)",
        "box = {run: replacement, 'safe': replacement}\nbox['safe'](1)",
        "def invoke(value=run):\n    return value(1)\ninvoke()",
        "list(map(run, [1]))",
        "def outer():\n    captured = run\n    def inner():\n        return captured(1)\n    return inner\nouter()()",
        "@run\ndef decorated():\n    return 1\ndecorated()",
        "import nativepkg.ops as ops\nops.run = replacement\nrun(1)",
        "import nativepkg.ops as ops\ndef patch():\n    ops.run = replacement\npatch()\nops.run(1)",
        "import nativepkg.ops as ops\nsaved = ops.run\ndef restore():\n    ops.run = saved\nops.run = replacement\nrestore()\nops.run(1)",
        "import nativepkg.ops as ops\nowner = ops if condition else other\nowner.run = replacement\nops.run(1)",
        "import nativepkg.ops as ops\nif condition:\n    ops.run = replacement\nops.run(1)",
        "def invoke(run):\n    return run(1)\ninvoke(replacement)",
        "def run(value):\n    return value\nrun(1)",
    ],
)
def test_callable_value_family_preserves_dynamic_dispatch(body: str) -> None:
    assert_bound("from nativepkg.ops import run\n" + body + "\n")


@pytest.mark.parametrize(
    "override",
    [
        {"abi": "unknown.machine_abi"},
        {"binding": "direct_symbol"},
        {"binding": "direct_symbol", "symbol": ""},
        {"binding": "direct_symbol", "symbol": "bad symbol"},
        {"binding": "module_attr", "abi": "molt.forward_f32_v1"},
        {"abi": "molt.pyinit_module_v1"},
        {"binding": "direct_symbol", "symbol": "native_run", "arity": 2},
    ],
)
def test_explicit_machine_abi_metadata_remains_fail_closed(
    override: dict[str, object],
) -> None:
    spec = {
        "module": "nativepkg.ops",
        "name": "run",
        "binding": "module_attr",
        "abi": "molt.object_callargs_v1",
        **override,
    }
    with pytest.raises(ValueError, match="native callable export.*invalid"):
        SimpleTIRGenerator(native_callable_exports={"nativepkg.ops.run": spec})


@pytest.mark.parametrize(
    "definition",
    [
        "@run\nasync def decorated():\n    return 1\ndecorated()",
        "@run\ndef decorated():\n    yield 1\ndecorated()",
        "@run\nasync def decorated():\n    yield 1\ndecorated()",
        "@run\nclass Decorated:\n    pass\nDecorated()",
        "class Owner:\n    @run\n    def method(self):\n        return 1\nOwner().method()",
    ],
)
def test_implicit_decorator_call_family_uses_object_dispatch(definition: str) -> None:
    assert_bound("from nativepkg.ops import run\n" + definition + "\n")


@pytest.mark.parametrize("metadata_only", [False, True])
@pytest.mark.parametrize("fallback", ["error", "bridge"])
@pytest.mark.parametrize(
    "call", ["ops.run(1)", "ops.run(value=1)", "ops.run(*arguments, **keywords)"]
)
def test_shared_native_dispatch_is_total_before_allowlist_and_bridge(
    metadata_only: bool, fallback: str, call: str
) -> None:
    exports = (
        {
            "nativepkg.ops.run": {
                "module": "nativepkg.ops",
                "name": "run",
                "binding": "module_attr",
                "abi": "molt.object_callargs_v1",
            }
        }
        if metadata_only
        else {}
    )
    generator = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ops"},
        native_python_exports=set() if metadata_only else {"nativepkg.ops.run"},
        native_callable_exports=exports,
        fallback_policy=fallback,
    )
    generator.visit(ast.parse("import nativepkg.ops as ops\n"))
    node = ast.parse(call).body[0].value
    result = generator._try_emit_imported_module_direct_or_task_call(
        "nativepkg.ops",
        "run",
        node,
        needs_bind=bool(node.keywords),
    )
    assert result is not None, (
        "native object dispatch must consume the call before later policy fallthrough"
    )
    operations = [
        op for function in generator.to_json()["functions"] for op in function["ops"]
    ]
    assert not any(op["kind"] == "invoke_ffi" for op in operations)
    actual = [op for op in operations if op.get("out") == result.name]
    assert len(actual) == 1
    if node.keywords or any(isinstance(arg, ast.Starred) for arg in node.args):
        assert actual[0]["kind"] == "call_indirect"
        assert len(actual[0]["args"]) == 2
        callargs_name = actual[0]["args"][1]
        assert any(
            op["kind"] == "callargs_new" and op.get("out") == callargs_name
            for op in operations
        )
    else:
        assert actual[0]["kind"] == "call_func"

    callee_name = actual[0]["args"][0]
    producers = [op for op in operations if op.get("out") == callee_name]
    assert len(producers) == 1
    assert producers[0]["kind"] not in {"func_ref", "const_str"}, (
        "native dispatch must retain the published object rather than fabricate a symbol"
    )


@pytest.mark.parametrize("kind", [kind.value for kind in FunctionKind])
@pytest.mark.parametrize("native", ["visibility", "metadata", "unlinked_source"])
@pytest.mark.parametrize("local", [False, True])
@pytest.mark.parametrize(
    "body",
    [
        "from nativepkg.ops import run\nalias = run\nsecond = alias\nsecond(1)",
        "import nativepkg.ops as ops\nops.run = replacement\nfrom nativepkg.ops import run\nrun(1)",
        "import nativepkg.ops as ops\nops.run: object = replacement\nfrom nativepkg.ops import run\nrun(1)",
        "import nativepkg.ops as ops\nops.run += replacement\nfrom nativepkg.ops import run\nrun(1)",
    ],
)
def test_hybrid_or_unlinked_import_never_mints_native_function_hint(
    kind: str, native: str, body: str, local: bool
) -> None:
    exports = (
        {
            "nativepkg.ops.run": {
                "module": "nativepkg.ops",
                "name": "run",
                "binding": "module_attr",
                "abi": "molt.object_callargs_v1",
            }
        }
        if native == "metadata"
        else {}
    )
    generator = SimpleTIRGenerator(
        entry_module="__main__",
        known_modules={"nativepkg", "nativepkg.ops"},
        direct_call_modules={"__main__"}
        if native == "unlinked_source"
        else {"__main__", "nativepkg.ops"},
        known_func_kinds={"nativepkg.ops": {"run": kind}},
        native_python_exports={"nativepkg.ops.run"}
        if native == "visibility"
        else set(),
        native_callable_exports=exports,
    )
    assert generator._known_module_function_type_hint("nativepkg.ops", "run") is None
    assert generator._known_function_symbol_target("nativepkg_ops__run") is None
    source = body + "\n"
    if local:
        source = "def invoke():\n" + "".join(
            "    " + line + "\n" for line in body.splitlines()
        )
    generator.visit(ast.parse(source))
    operations = [
        op for function in generator.to_json()["functions"] for op in function["ops"]
    ]
    assert "nativepkg_ops__run" not in repr(generator.to_json())
    assert not any(op["kind"] == "invoke_ffi" for op in operations)
    assert any(
        op["kind"] in {"call_func", "call_bind", "call_indirect"}
        and op.get("source_line") == len(source.splitlines())
        for op in operations
    )


@pytest.mark.parametrize("kind", [kind.value for kind in FunctionKind])
def test_linked_python_source_retains_function_hint_authority(kind: str) -> None:
    generator = SimpleTIRGenerator(
        known_modules={"sourcepkg"},
        direct_call_modules={"sourcepkg"},
        known_func_kinds={"sourcepkg": {"run": kind}},
    )
    assert generator._known_module_function_type_hint("sourcepkg", "run") is not None
    assert generator._known_function_symbol_target("sourcepkg__run") == (
        "sourcepkg",
        "run",
    )


@pytest.mark.parametrize("metadata_only", [False, True])
@pytest.mark.parametrize("local", [False, True])
@pytest.mark.parametrize(
    "call",
    [
        "OpsError(effect())",
        "OpsError(value=effect())",
        "OpsError(*arguments, **keywords)",
    ],
)
def test_published_native_class_collision_uses_object_before_argument_effects(
    metadata_only: bool, local: bool, call: str
) -> None:
    exports = (
        {
            "nativepkg.ops.OpsError": {
                "module": "nativepkg.ops",
                "name": "OpsError",
                "binding": "module_attr",
                "abi": "molt.object_callargs_v1",
            }
        }
        if metadata_only
        else {}
    )
    generator = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ops"},
        direct_call_modules={"__main__", "nativepkg.ops"},
        known_classes={
            "OpsError": {"module": "nativepkg.ops", "exception_subclass": True}
        },
        native_python_exports=set() if metadata_only else {"nativepkg.ops.OpsError"},
        native_callable_exports=exports,
    )
    source = "from nativepkg.ops import OpsError\n" + call + "\n"
    if local:
        source = "def invoke():\n" + "".join(
            "    " + line + "\n" for line in source.splitlines()
        )
    generator.visit(ast.parse(source))
    operations = [
        op for function in generator.to_json()["functions"] for op in function["ops"]
    ]
    assert not any(
        op["kind"]
        in {"exception_new_from_class", "object_new", "object_new_bound", "invoke_ffi"}
        for op in operations
    )
    calls = [
        op
        for op in operations
        if op["kind"] in {"call_func", "call_indirect", "call_bind"}
        and op.get("source_line") == len(source.splitlines())
    ]
    actual = calls[-1]
    callee = actual["args"][0]
    producer = next(
        index for index, op in enumerate(operations) if op.get("out") == callee
    )
    assert operations[producer]["kind"] not in {"func_ref", "const_str"}
    assert not any("nativepkg_ops__OpsError" in str(op) for op in operations)
    if "effect()" in call:
        effect_call = calls[0]
        assert operations.index(effect_call) > producer, (
            "capture actual callable before evaluating argument callbacks"
        )


@pytest.mark.parametrize("name", ["run", "sleep"])
@pytest.mark.parametrize("native", [False, True])
def test_asyncio_intrinsic_hint_requires_link_and_native_authority(
    name: str, native: bool
) -> None:
    generator = SimpleTIRGenerator(
        known_modules={"asyncio"},
        direct_call_modules={"__main__", "asyncio"} if native else {"__main__"},
        native_python_exports={f"asyncio.{name}"} if native else set(),
    )
    generator.visit(
        ast.parse(f"from asyncio import {name} as operation\nalias = operation\n")
    )
    assert f"asyncio__{name}" not in repr(generator.to_json())
    assert "operation" not in generator._module_attr_type_hints


@pytest.mark.parametrize("name", ["run", "sleep"])
@pytest.mark.parametrize("kind", [FunctionKind.SYNC, FunctionKind.ASYNC])
def test_linked_asyncio_intrinsic_hint_preserves_source_authority(
    name: str, kind: FunctionKind
) -> None:
    generator = SimpleTIRGenerator(
        known_modules={"asyncio"},
        direct_call_modules={"__main__", "asyncio"},
        known_func_kinds={"asyncio": {name: kind.value}},
    )
    generator.visit(ast.parse(f"from asyncio import {name} as operation\n"))
    hint = generator._module_attr_type_hints["operation"]
    if kind is FunctionKind.SYNC:
        assert hint == f"Func:asyncio__{name}"
    else:
        assert hint.startswith(f"AsyncFunc:asyncio__{name}_poll:")


def _assert_callback_and_callee_once(operations: list[dict], source: str) -> None:
    def callback_argument(node: ast.Call) -> ast.Call | None:
        return next(
            (
                argument
                for argument in [
                    *node.args,
                    *(keyword.value for keyword in node.keywords),
                ]
                if isinstance(argument, ast.Call)
                and isinstance(argument.func, ast.Name)
                and argument.func.id == "effect"
            ),
            None,
        )

    outer = next(
        node
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.Call) and callback_argument(node) is not None
    )
    callback = callback_argument(outer)
    assert callback is not None
    call_kinds = {"call_func", "call_bind", "call_indirect", "call_guarded"}

    def emitted_calls(node: ast.Call) -> list[int]:
        return [
            i
            for i, op in enumerate(operations)
            if op["kind"] in call_kinds
            and op.get("col_offset") == node.col_offset
            and op.get("end_col_offset") == node.end_col_offset
        ]

    callback_calls = emitted_calls(callback)
    outer_calls = emitted_calls(outer)
    assert len(callback_calls) == len(outer_calls) == 1, (
        "each source call executes once; import initialization is a different operation"
    )
    call = operations[outer_calls[0]]
    callee = next(
        i for i, op in enumerate(operations) if op.get("out") == call["args"][0]
    )
    assert operations[callee]["kind"] not in {"func_ref", "const_str"}
    assert callee < callback_calls[0] < outer_calls[0], (
        "descriptor/callee captured before effectful arguments"
    )


@pytest.mark.parametrize("method", ["join", "startswith", "endswith"])
@pytest.mark.parametrize("import_source,callee", IMPORTS)
@pytest.mark.parametrize("metadata_only", [False, True])
def test_native_method_spelling_evaluates_callee_and_argument_once(
    method: str, import_source: str, callee: str, metadata_only: bool
) -> None:
    qualified = f"nativepkg.ops.{method}"
    generator = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ops"},
        native_python_exports=set() if metadata_only else {qualified},
        native_callable_exports={
            qualified: {
                "module": "nativepkg.ops",
                "name": method,
                "binding": "module_attr",
                "abi": "molt.object_callargs_v1",
            }
        }
        if metadata_only
        else {},
    )
    source = (
        import_source.format(name=method)
        + "\n"
        + callee.format(name=method)
        + "(effect())\n"
    )
    generator.visit(ast.parse(source))
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == "molt_main"
    )
    _assert_callback_and_callee_once(operations, source)


@pytest.mark.parametrize("method", ["join", "startswith", "endswith"])
@pytest.mark.parametrize("factory", [False, True])
def test_untyped_receiver_is_retained_and_argument_evaluated_once(
    method: str, factory: bool
) -> None:
    receiver = "factory()" if factory else "receiver"
    generator = SimpleTIRGenerator()
    generator.visit(
        ast.parse(
            f"def invoke(receiver, factory, effect):\n    return {receiver}.{method}(effect())\n"
        )
    )
    operations = next(
        f["ops"]
        for f in generator.to_json()["functions"]
        if f["name"] == "__main____invoke"
    )
    calls = [
        i
        for i, op in enumerate(operations)
        if op["kind"] in {"call_func", "call_bind", "call_indirect", "call_guarded"}
    ]
    assert len(calls) == 2 + int(factory)
    loads = [
        i
        for i, op in enumerate(operations)
        if op["kind"] == "get_attr_generic_obj" and op.get("s_value") == method
    ]
    assert len(loads) == 1
    assert loads[0] < calls[-2] < calls[-1]
    if factory:
        assert calls[0] < loads[0]


@pytest.mark.parametrize("method", ["startswith", "endswith"])
@pytest.mark.parametrize("arguments", ["", "1, 2, 3, 4"])
def test_published_callable_does_not_inherit_string_method_arity(
    method: str, arguments: str
) -> None:
    generator = SimpleTIRGenerator(
        known_modules={"nativepkg", "nativepkg.ops"},
        native_python_exports={f"nativepkg.ops.{method}"},
    )
    generator.visit(
        ast.parse(f"import nativepkg.ops as ops\nops.{method}({arguments})\n")
    )
    assert any(
        op["kind"] in {"call_func", "call_bind", "call_indirect"}
        for f in generator.to_json()["functions"]
        for op in f["ops"]
    )


@pytest.mark.parametrize(
    "module,name", [("collections", "Counter"), ("dataclasses", "field")]
)
def test_native_publication_precedes_imported_intrinsic_specialization(
    module: str, name: str
) -> None:
    generator = SimpleTIRGenerator(
        known_modules={module},
        stdlib_allowlist={module},
        native_python_exports={f"{module}.{name}"},
    )
    generator.visit(
        ast.parse(f"import {module} as provider\nprovider.{name}(effect())\n")
    )
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == "molt_main"
    )
    _assert_callback_and_callee_once(
        operations, f"import {module} as provider\nprovider.{name}(effect())\n"
    )


@pytest.mark.parametrize(
    "receiver,method,primitive",
    [
        ("'text'", "join", "string_join"),
        ("'text'", "startswith", "string_startswith"),
        ("'text'", "endswith", "string_endswith"),
        ("b'text'", "startswith", "bytes_startswith"),
        ("b'text'", "endswith", "bytes_endswith"),
        ("bytearray(b'text')", "startswith", "bytearray_startswith"),
        ("bytearray(b'text')", "endswith", "bytearray_endswith"),
    ],
)
def test_exact_receiver_retains_specialized_method_with_one_callback(
    receiver: str, method: str, primitive: str
) -> None:
    generator = SimpleTIRGenerator()
    # A later function's global bytearray name is rebindable. Module source
    # order proves this constructor; immutable literals are exact in either.
    source = f"{receiver}.{method}(effect())\n"
    function_name = "molt_main"
    if not receiver.startswith("bytearray"):
        source = "def invoke(effect):\n    return " + source
        function_name = "__main____invoke"
    generator.visit(ast.parse(source))
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == function_name
    )
    callbacks = [
        i
        for i, op in enumerate(operations)
        if op["kind"] in {"call_func", "call_bind", "call_indirect", "call_guarded"}
    ]
    specialized = [i for i, op in enumerate(operations) if op["kind"] == primitive]
    assert len(callbacks) == len(specialized) == 1
    assert callbacks[0] < specialized[0]


@pytest.mark.parametrize("format_string", ["{", "}"])
@pytest.mark.parametrize("keyword", [False, True])
@pytest.mark.parametrize("local", [False, True])
def test_malformed_literal_format_evaluates_arguments_before_error(
    format_string: str, keyword: bool, local: bool
) -> None:
    argument = "named=effect()" if keyword else "effect()"
    source = f"{format_string!r}.format({argument})\n"
    function_name = "molt_main"
    if local:
        source = "def invoke(effect):\n    return " + source
        function_name = "__main____invoke"
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(source))
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == function_name
    )
    assert not any(op["kind"] == "raise" for op in operations), (
        "frontend parsing cannot decide field/error precedence"
    )
    _assert_callback_and_callee_once(operations, source)


@pytest.mark.parametrize("method", ["strip", "lstrip", "rstrip"])
@pytest.mark.parametrize("receiver", ["b'text'", "bytearray(b'text')"])
def test_unspecialized_bytes_strip_family_captures_callee_before_one_argument(
    method: str, receiver: str
) -> None:
    source = f"{receiver}.{method}(effect())\n"
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(source))
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == "molt_main"
    )
    _assert_callback_and_callee_once(operations, source)


@pytest.mark.parametrize(
    "expression",
    [
        "'text'.startswith('t', start=effect())",
        "'text'.endswith('t', end=effect())",
        "'text'.join(effect(), extra=effect())",
        "'text'.lower(unexpected=effect())",
        "'text'.strip(chars=effect())",
        "b'text'.count(b't', start=effect())",
        "'text'.startswith(*effect())",
        "'text'.startswith('t', **effect())",
    ],
)
def test_receiver_fast_paths_never_drop_keyword_or_expansion_effects(
    expression: str,
) -> None:
    source = "def invoke(effect):\n    return " + expression + "\n"
    tree = ast.parse(source)
    callbacks = [
        n
        for n in ast.walk(tree)
        if isinstance(n, ast.Call)
        and isinstance(n.func, ast.Name)
        and n.func.id == "effect"
    ]
    generator = SimpleTIRGenerator()
    generator.visit(tree)
    operations = next(
        f["ops"]
        for f in generator.to_json()["functions"]
        if f["name"] == "__main____invoke"
    )
    calls = [
        i
        for i, op in enumerate(operations)
        if op["kind"] in {"call_func", "call_bind", "call_indirect", "call_guarded"}
    ]
    # CALL_FUNCTION_EX also calls the builtin tuple materializer. Count the
    # actual source callback by its AST span, not legitimate protocol calls.
    source_callbacks = []
    for callback in callbacks:
        matches = [
            i
            for i in calls
            if operations[i].get("col_offset") == callback.col_offset
            and operations[i].get("end_col_offset") == callback.end_col_offset
        ]
        assert len(matches) == 1
        source_callbacks.extend(matches)
    outer = calls[-1]
    assert operations[outer]["kind"] in {"call_bind", "call_indirect"}
    callee = next(
        i
        for i, op in enumerate(operations)
        if op.get("out") == operations[outer]["args"][0]
    )
    assert callee < min(source_callbacks) <= max(source_callbacks) < outer


@pytest.mark.parametrize("minor", [12, 13, 14])
@pytest.mark.parametrize(
    "receiver,old,new",
    [
        ("'text'", "'t'", "'T'"),
        ("b'text'", "b't'", "b'T'"),
        ("bytearray(b'text')", "b't'", "b'T'"),
    ],
)
def test_replace_keyword_specialization_is_minor_and_receiver_owned(
    minor: int, receiver: str, old: str, new: str
) -> None:
    generator = SimpleTIRGenerator(target_python=(3, minor))
    source = f"{receiver}.replace({old}, {new}, count=effect())\n"
    generator.visit(ast.parse(source))
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == "molt_main"
    )
    specialized = any(op["kind"] == "string_replace" for op in operations)
    assert specialized == (receiver == "'text'" and minor >= 13)
    assert any(
        op["kind"] == "call_func"
        and op.get("col_offset")
        == ast.parse(source).body[0].value.keywords[0].value.col_offset
        for op in operations
    )
    if not specialized:
        assert any(op["kind"] in {"call_bind", "call_indirect"} for op in operations)


@pytest.mark.parametrize(
    "format_string", ["{}{", "{!r}{", "{!s}{", "{0.attr}{", "{0[key]}{"]
)
@pytest.mark.parametrize("local", [False, True])
def test_field_before_bad_format_brace_is_owned_by_actual_formatter(
    format_string: str, local: bool
) -> None:
    source = f"{format_string!r}.format(effect())\n"
    function_name = "molt_main"
    if local:
        source = "def invoke(effect):\n    return " + source
        function_name = "__main____invoke"
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(source))
    operations = next(
        f["ops"] for f in generator.to_json()["functions"] if f["name"] == function_name
    )
    assert not any(op["kind"] == "raise" for op in operations)
    _assert_callback_and_callee_once(operations, source)


@pytest.mark.parametrize(
    "mutation",
    [
        "ba.append(*[1])",
        "ba.clear(*[])",
        "ba.extend(*[b'x'])",
        "ba.insert(*[0, 1])",
        "ba.pop(*[])",
        "ba.remove(*[0])",
        "ba.resize(*[5])",
        "ba.extend(iterable=effect())",
    ],
)
def test_dynamic_bytearray_mutation_retires_length_fact(mutation: str) -> None:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse("ba = bytearray(4)\n"))
    assert generator.bytearray_len_hints.get("ba") == 4
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse("ba = bytearray(4)\n" + mutation + "\n"))
    assert "ba" not in generator.bytearray_len_hints
