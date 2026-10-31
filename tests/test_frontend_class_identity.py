"""Typed class identity in the frontend (HF-203).

Class facts are keyed by ``ClassId`` (module, qualname, occurrence), never by a
class's spelling. These tests pin the identity rules; the differential tests in
``tests/differential/basic/class_*`` pin the runtime behavior against CPython.
"""

from __future__ import annotations

import ast

import pytest

from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_source_keys import python_node_source_key
from molt.compiler_analysis.python_identity_catalog_generated import ALL_INVALID_MEMBERS

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


@pytest.mark.parametrize("method", ["__init__", "value"])
def test_method_code_mutation_rejects_class_body_folding(method):
    source = (
        "class C:\n"
        "    def __init__(self): self.x = 1\n"
        "    def value(self): return self.x\n"
        "def replacement(self):\n"
        "    self.x = 2\n"
        f"    return {'None' if method == '__init__' else 'self.x'}\n"
        f"C.{method}.__code__ = replacement.__code__\n"
    )
    namespace = {}
    exec(compile(source, "<method-code-mutation>", "exec"), namespace)
    assert namespace["C"]().value() == 2
    tree = ast.parse(source)
    changed = next(item for item in tree.body[0].body if item.name == method)
    premise = PythonNamespacePremise((), 0, 0, (), (python_node_source_key(changed),))
    generator = SimpleTIRGenerator(module_name="mod", namespace_premise=premise)
    generator.visit(tree)
    info = generator.classes[ClassId("mod", "C")]
    assert info["dynamic"]
    assert not info["constructor_binding_proven"]


def test_non_none_initializer_requires_runtime_constructor_validation():
    source = "class C:\n    def __init__(self): return 9\n"
    namespace = {}
    exec(compile(source, "<initializer-return>", "exec"), namespace)
    with pytest.raises(TypeError, match="__init__.*return None"):
        namespace["C"]()
    generator = SimpleTIRGenerator(
        module_name="mod", namespace_premise=PythonNamespacePremise((), 0, 0, (), ())
    )
    generator.visit(ast.parse(source + "C()\n"))
    assert not generator.classes[ClassId("mod", "C")].get("constructor_fold_safe")
    assert not any(op.kind == "OBJECT_NEW_BOUND" for op in _ops(generator))


def test_unknown_foreign_namespace_cannot_certify_constructor_identity():
    source = "class C:\n    def __init__(self, value): self.value = value\nC(3)\n"
    generator = SimpleTIRGenerator(
        module_name="mod",
        namespace_premise=PythonNamespacePremise(
            None, ALL_INVALID_MEMBERS, ALL_INVALID_MEMBERS, (), ()
        ),
    )
    generator.visit(ast.parse(source))
    assert not generator.classes[ClassId("mod", "C")]["constructor_binding_proven"]
    assert not any(op.kind == "OBJECT_NEW_BOUND" for op in _ops(generator))


@pytest.mark.parametrize(
    "prefix,decorator",
    [
        ("def dataclass(cls): return cls\n", "dataclass"),
        (
            "from dataclasses import dataclass\ndef dataclass(cls): return cls\n",
            "dataclass",
        ),
        (
            "class dataclasses:\n    @staticmethod\n    def dataclass(cls): return cls\n",
            "dataclasses.dataclass",
        ),
        (
            "def dataclass(**options):\n    return lambda cls: cls\n",
            "dataclass(custom='value')",
        ),
    ],
)
def test_user_decorator_spelling_never_supplies_dataclass_identity(prefix, decorator):
    source = prefix + f"@{decorator}\nclass C:\n    x: int\n"
    namespace = {}
    exec(compile(source, "<decorator-binding>", "exec"), namespace)
    assert not hasattr(namespace["C"], "__dataclass_fields__")
    generator = _lower(source)
    assert not generator.classes[ClassId("mod", "C")].get("dataclass", False)


@pytest.mark.parametrize(
    "prefix,decorator",
    [
        ("from dataclasses import dataclass as record\n", "record"),
        ("import dataclasses as records\n", "records.dataclass"),
        ("from dataclasses import dataclass\nrecord = dataclass\n", "record"),
    ],
)
def test_dataclass_identity_survives_import_and_value_aliases(prefix, decorator):
    source = prefix + f"@{decorator}\nclass C:\n    x: int\n"
    namespace = {}
    exec(compile(source, "<decorator-binding>", "exec"), namespace)
    assert namespace["C"](7).x == 7
    generator = _lower(source)
    assert generator.classes[ClassId("mod", "C")]["dataclass"]


@pytest.mark.parametrize("name", ["overload", "kernel"])
def test_user_function_decorator_keeps_its_runtime_function(name):
    source = f"def {name}(fn): return fn\n@{name}\ndef f(): return 7\n"
    namespace = {}
    exec(compile(source, "<decorator-binding>", "exec"), namespace)
    assert namespace["f"]() == 7
    generator = _lower(source)
    assert "mod__f" in generator.funcs_map
    assert all(
        op.kind != "CALL" or op.args[0] != "molt_gpu_kernel_descriptor_set"
        for op in _ops(generator)
    )


def test_source_callable_guard_is_reserved_before_its_definition_body():
    generator = _lower(
        "def caller():\n    return target(3)\n"
        "def target(value):\n    return value + 1\n"
        "print(caller())\n"
    )
    assert any(
        op.kind == "CALL_GUARDED" and op.metadata["target"] == "mod__target"
        for op in generator.funcs_map["mod__caller"]["ops"]
    )


def test_static_attributes_follow_cpython_syntax_not_descriptor_identity():
    # CPython 3.13.17's class metadata is syntactic: decorated/nested bodies
    # count literal self stores; a differently named receiver and annotations
    # do not. This differs from the physical instance-layout proof.
    source = """\
def passthrough(fn): return fn
class C:
    annotation: int
    @classmethod
    def class_method(self): self.class_field = 1
    @staticmethod
    def static_method(self): self.static_field = 1
    @passthrough
    def decorated(self): self.decorated_field = 1
    def other(first): first.not_literal_self = 1
    def nested(self):
        def inner(): self.inner_field = 1
    def unpack(self): self.z, self.a = 1, 2
    class Inner:
        def inner(self): self.belongs_to_inner = 1
"""
    generator = SimpleTIRGenerator(target_python=(3, 13))
    assert generator._collect_static_attributes(ast.parse(source).body[1]) == (
        "a",
        "class_field",
        "decorated_field",
        "inner_field",
        "static_field",
        "z",
    )


@pytest.mark.parametrize("minor", [13, 14])
@pytest.mark.parametrize(
    "source,expected13,expected14",
    [
        (
            "class C:\n def f(self):\n  self.only: int\n  self.valued: int = 2\n",
            ("valued",),
            ("valued",),
        ),
        (
            "class C:\n f = lambda: [0 for self.lambda_field in []]\n",
            ("lambda_field",),
            ("lambda_field",),
        ),
        (
            "class C:\n def f(x=lambda: [0 for self.default_field in []]): pass\n",
            ("default_field",),
            ("default_field",),
        ),
        (
            "class C:\n xs = [0 for self.class_comp in []]\n",
            (),
            (),
        ),
        (
            "class C:\n xs = (0 for self.generator_field in [])\n",
            ("generator_field",),
            ("generator_field",),
        ),
        (
            "class C:\n x: [0 for self.class_ann in []]\n def f(x: [0 for self.method_ann in []]): pass\n",
            (),
            ("class_ann", "method_ann"),
        ),
        (
            "class C:\n x: (lambda: [0 for self.ann_lambda in []])\n",
            ("ann_lambda",),
            ("ann_lambda",),
        ),
        (
            "from __future__ import annotations\nclass C:\n x: (lambda: [0 for self.ann_lambda in []])\n",
            (),
            (),
        ),
        (
            "class C:\n type Alias = [0 for self.alias_field in []]\n",
            ("alias_field",),
            ("alias_field",),
        ),
    ],
)
def test_static_attributes_match_pinned_cpython_code_scopes(
    minor, source, expected13, expected14
):
    # Independent CPython 3.13.17/3.14.9 compiler oracles, including PEP 649's
    # annotation code units. Physical instance layout is a separate contract.
    tree = ast.parse(source)
    generator = SimpleTIRGenerator(target_python=(3, minor))
    generator.future_annotations = generator._module_has_future_annotations(tree)
    node = next(item for item in tree.body if isinstance(item, ast.ClassDef))
    assert generator._collect_static_attributes(node) == (
        expected13 if minor == 13 else expected14
    )


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
