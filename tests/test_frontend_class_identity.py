"""Typed class identity in the frontend (HF-203).

Class facts are keyed by ``ClassId`` (module, qualname, occurrence), never by a
class's spelling. These tests pin the identity rules; the differential tests in
``tests/differential/basic/class_*`` pin the runtime behavior against CPython.
"""

from __future__ import annotations

import ast

import pytest

from molt.frontend import (
    BoundMethodHint,
    BuiltinClass,
    ClassId,
    DeclaredClass,
    MoltOp,
    ReceiverKind,
    SimpleTIRGenerator,
)

_REGISTERED_CLASSES = """\
REGISTRY = []
class A:
    class B:
        pass
    REGISTRY.append(B)
REGISTRY.append(A)
def outer():
    class Local:
        pass
    REGISTRY.append(Local)
    def inner():
        global Promoted
        class Promoted:
            pass
        REGISTRY.append(Promoted)
    inner()
outer()
class P:
    pass
REGISTRY.append(P)
class P:
    class Inner:
        pass
    REGISTRY.append(Inner)
REGISTRY.append(P)
"""


def _lower(source: str, module_name: str = "mod") -> SimpleTIRGenerator:
    generator = SimpleTIRGenerator(module_name=module_name)
    generator.visit(ast.parse(source))
    return generator


def _ops(generator: SimpleTIRGenerator) -> list[MoltOp]:
    return [op for function in generator.funcs_map.values() for op in function["ops"]]


def test_class_ids_carry_cpython_qualnames_and_separate_repeats() -> None:
    namespace: dict[str, object] = {}
    exec(compile(_REGISTERED_CLASSES, "<hf203>", "exec"), namespace)
    registry = namespace["REGISTRY"]
    assert isinstance(registry, list)
    expected = sorted(cls.__qualname__ for cls in registry)

    generator = _lower(_REGISTERED_CLASSES)
    class_ids = generator.local_class_ids
    assert sorted(class_id.qualname for class_id in class_ids) == expected
    assert all(class_id.module == "mod" for class_id in class_ids)
    repeated = [class_id for class_id in class_ids if class_id.qualname == "P"]
    assert len(repeated) == 2 and repeated[0] != repeated[1]
    assert all(class_id.occurrence is not None for class_id in repeated)
    unique = [class_id for class_id in class_ids if class_id.qualname != "P"]
    assert all(class_id.occurrence is None for class_id in unique)
    assert len(generator.classes) == len(class_ids)


def test_builtin_classes_never_equal_same_named_user_classes() -> None:
    assert BuiltinClass("Exception") != ClassId("mod", "Exception")
    assert BuiltinClass("list") != ClassId("mod", "list")
    with pytest.raises(ValueError):
        BuiltinClass("Point")


def test_each_field_op_carries_the_class_statement_that_emitted_it() -> None:
    generator = _lower(
        "class P:\n"
        "    def __init__(self):\n"
        "        self.a = 1\n"
        "        self.b = 2\n"
        "class P:\n"
        "    def __init__(self):\n"
        "        self.z = 0\n"
        "        self.a = 2\n"
    )
    first, second = generator.local_class_ids
    assert generator.classes[first]["fields"] != generator.classes[second]["fields"]
    for class_id, symbol_suffix in ((first, 0), (second, 1)):
        init_symbols = sorted(
            name for name in generator.funcs_map if name.startswith("mod__P___init__")
        )
        ops = generator.funcs_map[init_symbols[symbol_suffix]]["ops"]
        field_ops = [
            op
            for op in ops
            if op.kind in {"SETATTR", "GUARDED_SETATTR", "GETATTR", "GUARDED_GETATTR"}
        ]
        assert field_ops
        assert all(op.args[-1] == class_id for op in field_ops)


def test_layout_guards_name_their_own_class_statement() -> None:
    # HF-220: a layout guard trusts the class statement whose layout facts it
    # carries, never a namespace binding. Two statements that bind ``P`` stamp
    # distinct identities, each method's guards name its own statement, and no
    # guard reads the class through the module namespace.
    redefined = _lower(
        "class P:\n"
        "    def get(self):\n"
        "        return self.a\n"
        "    def __init__(self):\n"
        "        self.a = 1\n"
        "class P:\n"
        "    def get(self):\n"
        "        return self.a\n"
        "    def __init__(self):\n"
        "        self.a = 2\n"
    )
    unique = _lower(
        "class P:\n"
        "    def get(self):\n"
        "        return self.a\n"
        "    def __init__(self):\n"
        "        self.a = 1\n"
    )

    def stamps(generator: SimpleTIRGenerator) -> list[str]:
        return sorted(
            (op.metadata or {})["class_definition"]
            for op in _ops(generator)
            if op.kind == "CLASS_SET_DEFINITION"
        )

    def guards(generator: SimpleTIRGenerator) -> dict[str, set[str]]:
        named: dict[str, set[str]] = {}
        for symbol, function in generator.funcs_map.items():
            for op in function["ops"]:
                if op.kind in {"GUARDED_GETATTR", "GUARDED_SETATTR", "GUARD_LAYOUT"}:
                    named.setdefault(symbol, set()).add(
                        (op.metadata or {})["class_definition"]
                    )
        return named

    first, second = redefined.local_class_ids
    assert first.definition_symbol != second.definition_symbol
    assert stamps(redefined) == sorted(
        [first.definition_symbol, second.definition_symbol]
    )
    for class_id, suffix in ((first, 0), (second, 1)):
        for method in ("get", "__init__"):
            symbol = sorted(
                name
                for name in redefined.funcs_map
                if name.startswith(f"mod__P_{method}")
            )[suffix]
            assert guards(redefined)[symbol] == {class_id.definition_symbol}

    (only,) = unique.local_class_ids
    assert only.definition_symbol == "class:mod:P"
    assert stamps(unique) == ["class:mod:P"]
    assert set().union(*guards(unique).values()) == {"class:mod:P"}
    for generator in (redefined, unique):
        assert not any(op.kind == "DICT_GET" for op in _ops(generator))


def test_dynamic_classes_stamp_no_identity_and_take_the_generic_path() -> None:
    # A class with several bases builds without published offsets: its
    # statement stamps nothing, so no guard can trust it.
    generator = _lower(
        "class A:\n"
        "    pass\n"
        "class B:\n"
        "    pass\n"
        "class C(A, B):\n"
        "    def __init__(self):\n"
        "        self.x = 1\n"
        "    def get(self):\n"
        "        return self.x\n"
    )
    class_c = next(cid for cid in generator.local_class_ids if cid.name == "C")
    assert generator.classes[class_c]["dynamic"]
    stamped = {
        (op.metadata or {})["class_definition"]
        for op in _ops(generator)
        if op.kind == "CLASS_SET_DEFINITION"
    }
    assert class_c.definition_symbol not in stamped
    assert not any(
        op.kind in {"GUARDED_GETATTR", "GUARDED_SETATTR", "GUARD_LAYOUT"}
        for op in _ops(generator)
    )


def test_instance_attribute_store_does_not_mutate_its_class() -> None:
    instance_store = _lower(
        "class C:\n    def __init__(self):\n        self.x = 1\nc = C()\nc.y = 7\n"
    )
    class_store = _lower(
        "class C:\n    def __init__(self):\n        self.x = 1\nC.y = 7\n"
    )
    # A mutated class object builds dynamically; an instance store leaves the
    # class's static layout intact.
    class_id = ClassId("mod", "C")
    assert not instance_store.classes[class_id]["dynamic"]
    assert class_store.classes[class_id]["dynamic"]


@pytest.mark.parametrize(
    "rebuilt_base",
    [
        "def deco(cls):\n    return cls\n@deco\nclass Base:\n    x: int\n",
        "from dataclasses import dataclass\n"
        "@dataclass(slots=True)\nclass Base:\n    x: int\n",
    ],
    ids=["decorated", "slots-dataclass"],
)
def test_rebound_class_statement_is_not_a_static_base(rebuilt_base: str) -> None:
    # The binding holds the decorator's result or the rebuilt class, so the
    # subclass cannot inherit the statement's static layout.
    generator = _lower(rebuilt_base + "class Child(Base):\n    y: int\n")
    child = ClassId("mod", "Child")
    assert generator.classes[child]["dynamic"]


def test_alias_reads_do_not_inherit_class_binding_custody() -> None:
    generator = _lower(
        "class Point:\n"
        "    def __init__(self):\n"
        "        self.x = 1\n"
        "Alias = Point\n"
        "class Child(Alias):\n"
        "    pass\n"
    )
    assert generator.classes[ClassId("mod", "Child")]["dynamic"]


def test_method_receivers_are_declared_by_kind() -> None:
    generator = _lower(
        "class Point:\n"
        "    def method(self):\n"
        "        return self\n"
        "    @classmethod\n"
        "    def build(cls):\n"
        "        return cls\n"
    )
    point = ClassId("mod", "Point")
    assert generator._class_method_receiver_hint(point, "function", 0) == DeclaredClass(
        point, ReceiverKind.INSTANCE
    )
    assert generator._class_method_receiver_hint(
        point, "classmethod", 0
    ) == DeclaredClass(point, ReceiverKind.CLASS_OBJECT)


def test_wire_hints_never_spell_a_user_class_as_a_builtin() -> None:
    project = SimpleTIRGenerator._wire_type_hint
    bound = SimpleTIRGenerator._bound_method_wire
    assert project(DeclaredClass(ClassId("mod", "Point"))) == "Point"
    # The backend reads these spellings as builtins; a user class with that
    # name travels as no hint at all.
    assert project(DeclaredClass(ClassId("mod", "int"))) is None
    assert project(DeclaredClass(ClassId("mod", "Any"))) is None
    assert bound(BoundMethodHint(BuiltinClass("list"), "append")) == (
        "BoundMethod:list:append"
    )
    assert bound(BoundMethodHint(ClassId("mod", "list"), "append")) is None
    assert bound(BoundMethodHint(ClassId("mod", "Point"), "move")) == (
        "BoundMethod:Point:move"
    )


def test_user_class_named_like_a_hint_spelling_is_not_matched_by_spelling() -> None:
    # ``typing`` defines ``class Any``; an untyped value's "Any" hint must not
    # look up that class's layout.
    generator = _lower(
        "class Any:\n"
        "    def __init__(self):\n"
        "        self.keys = 1\n"
        "def read(value):\n"
        "    return value.keys\n"
    )
    read_ops = generator.funcs_map["mod__read"]["ops"]
    assert [op.kind for op in read_ops if op.kind.startswith("GETATTR")] == [
        "GETATTR_GENERIC_OBJ"
    ]
