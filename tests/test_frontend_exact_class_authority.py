from __future__ import annotations

import ast
from pathlib import Path

import pytest

from molt.frontend import (
    ClassId,
    DeclaredClass,
    MoltOp,
    MoltValue,
    SimpleTIRGenerator,
    compile_to_tir,
)
from molt.frontend.lowering.serialization_context import SerializationContext

POINT = ClassId("exact_class_authority", "Point")
RECORD = ClassId("exact_class_authority", "Record")
ELSEWHERE_POINT = ClassId("elsewhere", "Point")


@pytest.mark.parametrize("kind", ["GETATTR", "GUARDED_GETATTR", "GETATTR_GENERIC_PTR"])
def test_attribute_serialization_is_independent_of_global_cache_allocation(
    kind: str,
) -> None:
    owner = MoltValue("owner")
    args = {
        "GETATTR": [owner, "x", POINT],
        "GUARDED_GETATTR": [owner, MoltValue("version"), "x", POINT],
        "GETATTR_GENERIC_PTR": [owner, "x"],
    }[kind]
    generators = [SimpleTIRGenerator(), SimpleTIRGenerator()]
    for generator in generators + generators[::-1]:
        context = SerializationContext([], set(), None)
        assert generator._serialize_object_attr_op(
            MoltOp(kind, args, MoltValue("result")), context
        )
        assert context.json_ops == [
            {
                "kind": "get_attr_generic_ptr",
                "args": ["owner"],
                "s_value": "x",
                "out": "result",
            }
        ]


def test_guarded_field_serialization_uses_only_class_layout_authority() -> None:
    # The op carries the ClassId bound at emission; serialization reads that
    # class's offsets, and the wire names the class statement the guard trusts
    # (HF-220), never a class operand.
    generator = SimpleTIRGenerator()
    generator.classes[POINT] = {"binding_name": "Point", "fields": {"x": 8}}
    context = SerializationContext([], set(), None)
    assert generator._serialize_object_attr_op(
        MoltOp(
            "GUARDED_GETATTR",
            [MoltValue("owner"), MoltValue("version"), "x", POINT],
            MoltValue("result"),
            metadata={"class_definition": POINT.definition_symbol},
        ),
        context,
    )
    assert context.json_ops == [
        {
            "kind": "guarded_field_get",
            "args": ["owner", "version"],
            "s_value": "x",
            "value": 8,
            "class_definition": "class:exact_class_authority:Point",
            "out": "result",
        }
    ]


def _ops(source: str, *, function: str | None = None) -> list[MoltOp]:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.visit(ast.parse(source))
    if function is not None:
        return generator.funcs_map[function]["ops"]
    return [
        op
        for function in generator.funcs_map.values()
        for op in function.get("ops", [])
    ]


def _attribute_ops(ops: list[MoltOp], attr: str) -> list[MoltOp]:
    result = []
    for op in ops:
        position = 2 if op.kind in {"GUARDED_GETATTR", "GUARDED_SETATTR"} else 1
        if len(op.args) > position and op.args[position] == attr:
            result.append(op)
    return result


def test_deferred_constructor_call_does_not_publish_lexical_class_as_exact() -> None:
    ops = _ops(
        "class Point:\n"
        "    x: int\n"
        "\n"
        "def mutate():\n"
        "    point = Point()\n"
        "    point.x = 1\n"
    )

    stores = _attribute_ops(ops, "x")
    assert any(
        op.kind in {"SETATTR_GENERIC_OBJ", "SETATTR_GENERIC_PTR"} for op in stores
    )
    assert all(op.kind != "SETATTR" for op in stores)
    assert all(
        op.result.exact_class is None
        for op in ops
        if op.kind in {"CALL_BIND", "CALL_FUNC", "CALL_GUARDED"}
    )


@pytest.mark.parametrize(
    "source",
    [
        (
            "class Point:\n"
            "    x: int\n"
            "    def __new__(cls):\n"
            "        return object()\n"
            "point = Point()\n"
            "point.x = 1\n"
        ),
        (
            "class Meta(type):\n"
            "    def __call__(cls):\n"
            "        return object()\n"
            "class Point(metaclass=Meta):\n"
            "    x: int\n"
            "point = Point()\n"
            "point.x = 1\n"
        ),
        (
            "def replace(cls):\n"
            "    return object\n"
            "@replace\n"
            "class Point:\n"
            "    x: int\n"
            "point = Point()\n"
            "point.x = 1\n"
        ),
        ("class Point:\n    x: int\nAlias = Point\npoint = Alias()\npoint.x = 1\n"),
        (
            "class Point:\n"
            "    x: int\n"
            "def dishonest() -> Point:\n"
            "    return object()\n"
            "point = dishonest()\n"
            "point.x = 1\n"
        ),
        (
            "class Base:\n"
            "    x: int\n"
            "class Point(Base):\n"
            "    def __new__(cls):\n"
            "        return Base()\n"
            "point = Point()\n"
            "point.x = 1\n"
        ),
        (
            "class Other:\n"
            "    x: int\n"
            "class Point:\n"
            "    x: int\n"
            "    def __init__(self):\n"
            "        self.__class__ = Other\n"
            "point = Point()\n"
            "point.x = 1\n"
        ),
        (
            "class Other:\n"
            "    x: int\n"
            "def replace_class(value):\n"
            "    value.__class__ = Other\n"
            "class Point:\n"
            "    x: int\n"
            "    def __init__(self):\n"
            "        replace_class(self)\n"
            "point = Point()\n"
            "point.x = 1\n"
        ),
    ],
    ids=[
        "custom-new",
        "custom-metaclass",
        "decorated-replacement",
        "class-alias",
        "dishonest-annotation",
        "subclass-custom-new",
        "init-class-reassignment",
        "init-callback-class-reassignment",
    ],
)
def test_runtime_selected_constructor_shapes_never_authorize_fixed_layout(
    source: str,
) -> None:
    ops = _ops(source)
    stores = _attribute_ops(ops, "x")
    assert any(
        op.kind in {"SETATTR_GENERIC_OBJ", "SETATTR_GENERIC_PTR"} for op in stores
    )
    assert all(
        op.result.exact_class is None
        for op in ops
        if op.kind in {"CALL_BIND", "CALL_FUNC", "CALL_GUARDED"}
    )


def test_exact_binding_alias_and_lowered_value_drive_direct_field_consumers() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    first_field_op = len(generator.current_ops)
    generator.classes[POINT] = {
        "binding_name": "Point",
        "fields": {"x": 24},
        "layout_version": 7,
        "static": True,
    }
    allocated = generator._stamp_exact_class(
        MoltValue("allocated_point", type_hint=DeclaredClass(POINT)), POINT
    )
    generator._update_exact_local("point", None, allocated)
    alias = generator._stamp_exact_class(
        MoltValue("alias_value", type_hint=DeclaredClass(POINT)), POINT
    )
    generator._update_exact_local("alias", ast.Name(id="point", ctx=ast.Load()), alias)
    generator._emit_guarded_getattr(alias, "x", POINT, obj_name="alias")
    exact_alias = generator._stamp_exact_class(
        MoltValue("exact_alias", type_hint=DeclaredClass(POINT)), POINT
    )
    generator._emit_guarded_setattr(
        exact_alias,
        "x",
        MoltValue("one", type_hint="int"),
        POINT,
    )

    assert [op.kind for op in generator.current_ops[first_field_op:]] == [
        "GETATTR",
        "CHECK_EXCEPTION",
        "SETATTR",
        "CHECK_EXCEPTION",
    ]


def test_rebinding_an_exact_alias_clears_layout_authority() -> None:
    ops = _ops(
        "class Point:\n"
        "    x: int\n"
        "def replacement():\n"
        "    return object()\n"
        "point = Point()\n"
        "alias = point\n"
        "alias = replacement()\n"
        "alias.x = 1\n"
    )

    stores = _attribute_ops(ops, "x")
    assert any(
        op.kind in {"SETATTR_GENERIC_OBJ", "SETATTR_GENERIC_PTR"} for op in stores
    )
    assert all(op.kind != "SETATTR" for op in stores)


def test_arbitrary_callback_expires_reachable_exact_instance_fact() -> None:
    ops = _ops(
        "class Point:\n"
        "    x: int\n"
        "def mutate(value):\n"
        "    value.__class__ = object\n"
        "point = Point()\n"
        "mutate(point)\n"
        "point.x = 1\n"
    )

    stores = _attribute_ops(ops, "x")
    assert stores[-1].kind in {
        "SETATTR_GENERIC_OBJ",
        "SETATTR_GENERIC_PTR",
        "GUARDED_SETATTR",
    }


@pytest.mark.parametrize(
    "flow",
    [
        "if flag:\n    point = Point()\n",
        "if flag:\n    point = Point()\nelse:\n    point = replacement()\n",
        "while flag:\n    point = Point()\n    break\n",
        "for unused in values:\n    point = Point()\n",
        "try:\n    point = Point()\n    replacement()\nexcept Exception:\n    pass\n",
        "match flag:\n    case True:\n        point = Point()\n    case _:\n        pass\n",
    ],
    ids=["if-only", "if-else", "while", "for", "try", "match"],
)
def test_non_taken_paths_cannot_publish_exact_class_after_join(flow: str) -> None:
    ops = _ops(
        "class Point:\n    x: int\n"
        "def replacement():\n    return object()\n"
        "flag = replacement()\nvalues = replacement()\npoint = replacement()\n"
        + flow
        + "point.x = 13\n"
    )
    stores = _attribute_ops(ops, "x")
    assert stores
    assert stores[-1].kind in {
        "SETATTR_GENERIC_OBJ",
        "SETATTR_GENERIC_PTR",
        "GUARDED_SETATTR",
    }


def test_method_self_is_not_an_exact_receiver_identity() -> None:
    ops = _ops("class Point:\n    x: int\n    def mutate(self):\n        self.x = 19\n")
    stores = _attribute_ops(ops, "x")
    assert stores
    assert all(op.kind != "SETATTR" for op in stores)


def test_lowered_exact_result_is_the_binding_publication_authority() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    lowered = generator._stamp_exact_class(
        MoltValue("lowered_record", type_hint=DeclaredClass(RECORD)), RECORD
    )
    generator._update_exact_local(
        "record",
        ast.Call(func=ast.Name(id="factory", ctx=ast.Load()), args=[], keywords=[]),
        lowered,
    )

    assert generator.exact_locals["record"].class_id == RECORD
    assert generator._exact_class_for_value(lowered) == RECORD
    loaded_record = generator._stamp_exact_class(
        MoltValue("loaded_record", type_hint=DeclaredClass(RECORD)), RECORD
    )
    assert generator._exact_class_for_value(loaded_record, "record") == RECORD


def test_expired_named_and_temporary_facts_cannot_resurrect() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    lowered = generator._stamp_exact_class(
        MoltValue("lowered_point", type_hint=DeclaredClass(POINT)), POINT
    )
    generator._update_exact_local("point", None, lowered)

    generator._expire_exact_class_facts()

    assert generator._exact_class_for_value(lowered) is None
    assert generator._exact_class_for_value(lowered, "point") is None


def test_old_receiver_cannot_borrow_same_name_republished_fact() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    old_receiver = generator._stamp_exact_class(
        MoltValue("old_point", type_hint=DeclaredClass(POINT)), POINT
    )
    generator._update_exact_local("point", None, old_receiver)

    generator._expire_exact_class_facts()
    fresh_receiver = generator._stamp_exact_class(
        MoltValue("fresh_point", type_hint=DeclaredClass(POINT)), POINT
    )
    generator._update_exact_local("point", None, fresh_receiver)

    assert generator._exact_class_for_value(fresh_receiver, "point") == POINT
    assert generator._exact_class_for_value(old_receiver, "point") is None


def test_comprehension_scope_does_not_restore_fact_across_callback_epoch() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator._publish_exact_local("point", POINT)
    restore = generator._mask_exact_binding_projection({"point"})

    generator._expire_exact_class_facts()
    restore()

    assert generator._exact_class_for_name("point") is None


def test_hoisted_layout_assumption_expires_with_its_exact_fact() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator._publish_exact_local("point", POINT)
    generator.loop_guard_assumptions.append(
        {"point": (POINT, True, generator.exact_class_token)}
    )
    assert generator._loop_guard_assumption("point", POINT) is True

    generator._expire_exact_class_facts()

    assert generator._loop_guard_assumption("point", POINT) is None


def test_loop_callback_cannot_reuse_pre_callback_access_on_next_iteration() -> None:
    ops = _ops(
        "from dataclasses import dataclass\n"
        "@dataclass\n"
        "class Other:\n"
        "    x: int\n"
        "@dataclass\n"
        "class Point:\n"
        "    x: int\n"
        "def mutate(value):\n"
        "    value.__class__ = Other\n"
        "def read():\n"
        "    point = Point(0)\n"
        "    index = 0\n"
        "    while index < 2:\n"
        "        before = point.x\n"
        "        mutate(point)\n"
        "        index += 1\n"
        "    return before\n"
    )

    assert all(op.kind != "DATACLASS_GET" for op in ops)
    loads = _attribute_ops(ops, "x")
    assert loads
    assert all(op.kind != "GETATTR" for op in loads)
    assert any(
        op.kind
        in {
            "GUARDED_GETATTR",
            "GETATTR_GENERIC_OBJ",
            "GETATTR_GENERIC_PTR",
        }
        for op in loads
    )


def test_matching_branch_facts_publish_one_live_join_authority() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator._publish_exact_local("point", POINT)
    left = generator._snapshot_live_exact_bindings()
    left_token = generator.exact_class_token
    generator._advance_exact_class_token()
    generator._publish_exact_local("point", POINT)
    right = generator._snapshot_live_exact_bindings()
    right_token = generator.exact_class_token

    generator.exact_locals = generator._join_exact_binding_states(
        left, left_token, right, right_token
    )

    assert generator._exact_class_for_name("point") == POINT


def test_proven_exact_dataclass_value_authorizes_dataclass_field_access() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.classes[RECORD] = {
        "binding_name": "Record",
        "dataclass": True,
        "fields": {"value": 0},
        "field_hints": {"value": "int"},
    }
    record = generator._stamp_exact_class(
        MoltValue("record", type_hint=DeclaredClass(RECORD)), RECORD
    )
    node = ast.Attribute(
        value=ast.Name(id="record", ctx=ast.Load()),
        attr="value",
        ctx=ast.Load(),
    )

    generator._emit_attribute_load(node, record, None, RECORD)

    assert any(op.kind == "DATACLASS_GET" for op in generator.current_ops)


def test_dataclass_type_hint_does_not_authorize_direct_field_storage() -> None:
    ops = _ops(
        "from dataclasses import dataclass\n"
        "@dataclass\n"
        "class Record:\n"
        "    value: int\n"
        "def touch(record: Record):\n"
        "    before = record.value\n"
        "    record.value = 2\n"
        "    setattr(record, 'value', 3)\n"
        "    return getattr(record, 'value')\n"
    )

    assert all(op.kind not in {"DATACLASS_GET", "DATACLASS_SET"} for op in ops)
    assert any(op.kind in {"GETATTR_GENERIC_OBJ", "GETATTR_NAME_DEFAULT"} for op in ops)
    assert any(op.kind == "SETATTR_GENERIC_OBJ" for op in ops)


@pytest.mark.parametrize(
    ("default", "default_kind"),
    [("fallback()", "CALL_GUARDED"), ("1 // 0", "FLOORDIV")],
    ids=["callback", "raising-primitive"],
)
def test_getattr_default_dataflow_preserves_evaluation_order(
    default: str, default_kind: str
) -> None:
    ops = _ops(
        "class Point:\n"
        "    x: int\n"
        "def fallback():\n"
        "    return 3\n"
        "def read(point: Point):\n"
        f"    return getattr(point, 'x', {default})\n",
        function="exact_class_authority__read",
    )
    strings = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    callee = next(
        op
        for op in ops
        if op.kind == "MODULE_GET_GLOBAL" and strings.get(op.args[1].name) == "getattr"
    )
    access = next(
        op for op in ops if op.kind == "CALL_FUNC" and op.args[0] is callee.result
    )
    assert len(access.args) == 4
    assert strings[access.args[2].name] == "x"
    default_op = next(op for op in ops if op.result is access.args[3])
    assert default_op.kind == default_kind
    if default_kind == "CALL_GUARDED":
        # The guard checks the live global read, not a cached callable.
        fallback = next(op for op in ops if op.result is default_op.args[0])
        assert fallback.kind == "MODULE_GET_GLOBAL"
        assert strings[fallback.args[1].name] == "fallback"
        assert default_op.metadata["target"] == "exact_class_authority__fallback"
    assert ops.index(callee) < ops.index(default_op) < ops.index(access)


@pytest.mark.parametrize(
    "declaration",
    [
        "class Native(list):\n    value: int\n",
        "class Base(list):\n    value: int\nclass Native(Base):\n    extra: int\n",
        "class Native(list):\n    __slots__ = ('value',)\n",
    ],
)
def test_native_base_layout_never_publishes_inferred_prefix_offsets(
    declaration: str,
) -> None:
    generator = SimpleTIRGenerator(module_name="native_base_layout")
    generator.visit(ast.parse(declaration))
    assert generator.classes[ClassId("native_base_layout", "Native")]["dynamic"]
    ops = [
        op
        for function in generator.funcs_map.values()
        for op in function.get("ops", [])
    ]
    assert not any(
        op.kind == "CONST_STR" and op.args == ["__molt_field_offsets__"] for op in ops
    )


def _emit_guard(
    generator: SimpleTIRGenerator, operation: str, class_id: ClassId
) -> list[MoltOp]:
    first_op = len(generator.current_ops)
    owner = MoltValue("owner", type_hint=DeclaredClass(class_id))
    if operation == "read":
        generator._emit_guarded_getattr(owner, "x", class_id)
    elif operation == "write":
        generator._emit_guarded_setattr(owner, "x", MoltValue("value"), class_id)
    else:
        generator._emit_layout_guard(owner, class_id)
    return generator.current_ops[first_op:]


def _assert_guard_names_statement(ops: list[MoltOp], class_id: ClassId) -> None:
    """One guard names ``class_id``'s statement and reads no class binding."""
    guards = [
        op
        for op in ops
        if op.kind in {"GUARDED_GETATTR", "GUARDED_SETATTR", "GUARD_LAYOUT"}
    ]
    assert len(guards) == 1
    assert (guards[0].metadata or {}) == {
        "class_definition": class_id.definition_symbol
    }
    assert not any(
        op.kind
        in {
            "DICT_GET",
            "LOAD_VAR",
            "CLOSURE_LOAD",
            "MODULE_GET_GLOBAL",
            "MODULE_IMPORT",
            "MODULE_CACHE_GET",
            "RAISE",
        }
        for op in ops
    )


@pytest.mark.parametrize("operation", ["read", "write", "loop"])
def test_synthesized_layout_guards_name_their_class_statement(operation: str) -> None:
    # HF-220: a guard outside the defining scope trusts the class statement,
    # not the module binding, so rebinding the name cannot redirect it.
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.current_func_name = "retained_method"
    generator.classes[POINT] = {
        "binding_name": "Point",
        "static": True,
        "fields": {"x": 8},
    }
    _assert_guard_names_statement(_emit_guard(generator, operation, POINT), POINT)


def test_optional_layout_identity_does_not_change_explicit_source_name() -> None:
    ops = _ops(
        "class Point:\n    x: int\n    def read(self):\n        return self.x, Point\n"
    )
    strings = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    assert any(
        op.kind == "MODULE_GET_GLOBAL" and strings.get(op.args[1].name) == "Point"
        for op in ops
    )
    # The guard names the statement; only the explicit source read loads it.
    assert not any(op.kind == "DICT_GET" for op in ops)
    assert any(
        (op.metadata or {}).get("class_definition") == POINT.definition_symbol
        for op in ops
        if op.kind == "GUARDED_GETATTR"
    )


@pytest.mark.parametrize("operation", ["read", "write", "loop"])
def test_external_layout_identity_does_not_import_or_read_module(
    operation: str,
) -> None:
    # An imported class's guard names the exporting statement directly: no
    # import, module read or binding is needed to construct it.
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.current_func_name = "retained_method"
    generator.classes[ELSEWHERE_POINT] = {
        "binding_name": "Point",
        "static": True,
        "fields": {"x": 8},
    }
    ops = _emit_guard(generator, operation, ELSEWHERE_POINT)
    _assert_guard_names_statement(ops, ELSEWHERE_POINT)
    assert ELSEWHERE_POINT.definition_symbol == "class:elsewhere:Point"


def test_retained_layout_differential_capsule_lowers_optional_guards() -> None:
    source = (
        Path(__file__).parent / "differential/basic/attribute_lookup_transactions.py"
    )
    ir = compile_to_tir(source.read_text(encoding="utf-8"))
    for method, guard in (
        ("read", "guarded_field_get"),
        ("write", "guarded_field_set"),
        ("loop", "guarded_field_get"),
    ):
        function = next(
            function
            for function in ir["functions"]
            if function["name"].endswith(f"__RetainedLayoutOwner_{method}")
        )
        guards = [op for op in function["ops"] if op["kind"] == guard]
        assert guards
        assert {op["class_definition"] for op in guards} == {
            "class:__main__:RetainedLayoutOwner"
        }
        assert not any(op["kind"] == "dict_get" for op in function["ops"])


@pytest.mark.parametrize("local", [False, True], ids=["global-alias", "local-alias"])
def test_external_layout_identity_ignores_imported_aliases(local: bool) -> None:
    # Aliases of an imported class are bindings; the guard names the class
    # statement and reads none of them.
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.current_func_name = "retained_method"
    generator.funcs_map["retained_method"] = generator.funcs_map["molt_main"].copy()
    generator.classes[ELSEWHERE_POINT] = {
        "binding_name": "Point",
        "static": True,
        "fields": {"x": 8},
    }
    generator.global_imported_names["Point"] = "unrelated"
    if local:
        generator.imported_names["Alias"] = "elsewhere"
        generator.imported_attr_names["Alias"] = "Point"
        generator.local_imported_names.add("Alias")
        generator.locals["Alias"] = MoltValue("imported_alias", type_hint="type")
    else:
        generator.global_imported_names["Alias"] = "elsewhere"
        generator.global_imported_attr_names["Alias"] = "Point"
    ops = _emit_guard(generator, "read", ELSEWHERE_POINT)
    _assert_guard_names_statement(ops, ELSEWHERE_POINT)


@pytest.mark.parametrize("operation", ["read", "write", "loop"])
@pytest.mark.parametrize("storage", ["local", "cell", "global"])
def test_local_class_guard_reads_no_class_storage(operation: str, storage: str) -> None:
    # A function-local class statement's guard names the statement wherever
    # its binding lives; the binding may be rebound or unbound.
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.current_func_name = "retained_method"
    generator.funcs_map["retained_method"] = generator.funcs_map["molt_main"].copy()
    generator.classes[POINT] = {
        "binding_name": "Point",
        "static": False,
        "fields": {"x": 8},
    }
    generator.unbound_check_names.add("Point")
    if storage == "global":
        generator.global_decls.add("Point")
    elif storage == "cell":
        generator.boxed_locals["Point"] = MoltValue("point_cell", type_hint="cell")
    else:
        generator.locals["Point"] = MoltValue("point_binding", type_hint="type")
        generator.scope_assigned.add("Point")
    ops = _emit_guard(generator, operation, POINT)
    _assert_guard_names_statement(ops, POINT)
    assert not any(
        arg == MoltValue("point_cell", type_hint="cell")
        for op in ops
        for arg in op.args
    )


@pytest.mark.parametrize(
    "returned",
    [
        "None",
        "True",
        "7",
        "1.5",
        "'sentinel'",
        "{'marker': 23}",
        "type('Existing', (), {'marker': 'original'})",
    ],
)
@pytest.mark.parametrize("metaclass_is_type", [False, True])
def test_metaclass_return_does_not_authorize_layout_mutation(
    returned: str, metaclass_is_type: bool
) -> None:
    if metaclass_is_type:
        factory = (
            "class Meta(type):\n"
            "    def __new__(cls, name, bases, namespace):\n"
            "        events.append(namespace['marker'])\n"
            "        return result\n"
        )
    else:
        factory = (
            "def Meta(name, bases, namespace):\n"
            "    events.append(namespace['marker'])\n"
            "    return result\n"
        )
    source = (
        f"result = {returned}\nevents = []\n"
        + factory
        + "class Subject(metaclass=Meta):\n"
        "    marker = 42\n"
        "    def method(self):\n"
        "        return (__class__, self.marker)\n"
    )
    reference: dict = {}
    try:
        exec(compile(source, "metaclass_return.py", "exec"), reference)
    except RuntimeError as exc:
        # A returned type still has CPython's original-cell obligation; an
        # arbitrary non-type return deliberately bypasses that check.
        assert isinstance(reference["result"], type)
        assert str(exc).startswith("__class__ not set defining 'Subject'")
        assert "Subject" not in reference
    else:
        assert reference["Subject"] is reference["result"]
    assert reference["events"] == [42]
    if isinstance(reference["result"], type):
        assert reference["result"].marker == "original"

    # The native constructor owns storage. A class statement cannot infer
    # ownership from the type of an arbitrary metaclass return value.
    ops = _ops(source)
    assert all(
        op.kind not in {"CLASS_MERGE_LAYOUT", "CLASS_SET_LAYOUT_VERSION"} for op in ops
    )
    assert all(
        op.kind not in {"GUARDED_GETATTR", "GETATTR"}
        for op in _attribute_ops(ops, "marker")
    )
