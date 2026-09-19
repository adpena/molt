from __future__ import annotations

import ast

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend.lowering.serialization_context import SerializationContext


@pytest.mark.parametrize("kind", ["GETATTR", "GUARDED_GETATTR", "GETATTR_GENERIC_PTR"])
def test_attribute_serialization_is_independent_of_global_cache_allocation(
    kind: str,
) -> None:
    owner = MoltValue("owner")
    args = {
        "GETATTR": [owner, "x", "Point"],
        "GUARDED_GETATTR": [
            owner,
            MoltValue("class"),
            MoltValue("version"),
            "x",
            "Point",
        ],
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
    generator = SimpleTIRGenerator()
    generator.classes["Point"] = {"fields": {"x": 8}}
    context = SerializationContext([], set(), None)
    assert generator._serialize_object_attr_op(
        MoltOp(
            "GUARDED_GETATTR",
            [
                MoltValue("owner"),
                MoltValue("class"),
                MoltValue("version"),
                "x",
                "Point",
            ],
            MoltValue("result"),
        ),
        context,
    )
    assert context.json_ops == [
        {
            "kind": "guarded_field_get",
            "args": ["owner", "class", "version"],
            "s_value": "x",
            "value": 8,
            "out": "result",
            "class": "Point",
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
        position = 3 if op.kind in {"GUARDED_GETATTR", "GUARDED_SETATTR"} else 1
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
    generator.classes["Point"] = {
        "fields": {"x": 24},
        "layout_version": 7,
        "static": True,
    }
    allocated = generator._stamp_exact_class(
        MoltValue("allocated_point", type_hint="Point"), "Point"
    )
    generator._update_exact_local("point", None, allocated)
    alias = generator._stamp_exact_class(
        MoltValue("alias_value", type_hint="Point"), "Point"
    )
    generator._update_exact_local("alias", ast.Name(id="point", ctx=ast.Load()), alias)
    generator._emit_guarded_getattr(alias, "x", "Point", obj_name="alias")
    exact_alias = generator._stamp_exact_class(
        MoltValue("exact_alias", type_hint="Point"), "Point"
    )
    generator._emit_guarded_setattr(
        exact_alias,
        "x",
        MoltValue("one", type_hint="int"),
        "Point",
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
        MoltValue("lowered_record", type_hint="Record"), "Record"
    )
    generator._update_exact_local(
        "record",
        ast.Call(func=ast.Name(id="factory", ctx=ast.Load()), args=[], keywords=[]),
        lowered,
    )

    assert generator.exact_locals["record"].class_id == "Record"
    assert generator._exact_class_for_value(lowered) == "Record"
    loaded_record = generator._stamp_exact_class(
        MoltValue("loaded_record", type_hint="Record"), "Record"
    )
    assert generator._exact_class_for_value(loaded_record, "record") == "Record"


def test_expired_named_and_temporary_facts_cannot_resurrect() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    lowered = generator._stamp_exact_class(
        MoltValue("lowered_point", type_hint="Point"), "Point"
    )
    generator._update_exact_local("point", None, lowered)

    generator._expire_exact_class_facts()

    assert generator._exact_class_for_value(lowered) is None
    assert generator._exact_class_for_value(lowered, "point") is None


def test_old_receiver_cannot_borrow_same_name_republished_fact() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    old_receiver = generator._stamp_exact_class(
        MoltValue("old_point", type_hint="Point"), "Point"
    )
    generator._update_exact_local("point", None, old_receiver)

    generator._expire_exact_class_facts()
    fresh_receiver = generator._stamp_exact_class(
        MoltValue("fresh_point", type_hint="Point"), "Point"
    )
    generator._update_exact_local("point", None, fresh_receiver)

    assert generator._exact_class_for_value(fresh_receiver, "point") == "Point"
    assert generator._exact_class_for_value(old_receiver, "point") is None


def test_comprehension_scope_does_not_restore_fact_across_callback_epoch() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator._publish_exact_local("point", "Point")
    restore = generator._mask_exact_binding_projection({"point"})

    generator._expire_exact_class_facts()
    restore()

    assert generator._exact_class_for_name("point") is None


def test_hoisted_layout_assumption_expires_with_its_exact_fact() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator._publish_exact_local("point", "Point")
    generator.loop_guard_assumptions.append(
        {"point": ("Point", True, generator.exact_class_token)}
    )
    assert generator._loop_guard_assumption("point", "Point") is True

    generator._expire_exact_class_facts()

    assert generator._loop_guard_assumption("point", "Point") is None


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
    generator._publish_exact_local("point", "Point")
    left = generator._snapshot_live_exact_bindings()
    left_token = generator.exact_class_token
    generator._advance_exact_class_token()
    generator._publish_exact_local("point", "Point")
    right = generator._snapshot_live_exact_bindings()
    right_token = generator.exact_class_token

    generator.exact_locals = generator._join_exact_binding_states(
        left, left_token, right, right_token
    )

    assert generator._exact_class_for_name("point") == "Point"


def test_proven_exact_dataclass_value_authorizes_dataclass_field_access() -> None:
    generator = SimpleTIRGenerator(module_name="exact_class_authority")
    generator.classes["Record"] = {
        "dataclass": True,
        "fields": {"value": 0},
        "field_hints": {"value": "int"},
    }
    record = generator._stamp_exact_class(
        MoltValue("record", type_hint="Record"), "Record"
    )
    node = ast.Attribute(
        value=ast.Name(id="record", ctx=ast.Load()),
        attr="value",
        ctx=ast.Load(),
    )

    generator._emit_attribute_load(node, record, None, "Record")

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
    [("fallback()", "CALL_FUNC"), ("1 // 0", "FLOORDIV")],
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
    if default_kind == "CALL_FUNC":
        fallback = next(op for op in ops if op.result is default_op.args[0])
        assert fallback.kind == "MODULE_GET_GLOBAL"
        assert strings[fallback.args[1].name] == "fallback"
    assert ops.index(callee) < ops.index(default_op) < ops.index(access)
