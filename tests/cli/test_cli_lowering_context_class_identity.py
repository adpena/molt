"""Class tables in the lowering context are keyed by typed class identity.

Two contracts keep the per-module lowering cache correct and stable (HF-203):

* **Identity, not spelling.** ``known_classes`` maps a ``ClassId`` (module,
  qualname, occurrence) to the facts of one class statement. Two modules that
  both define ``P``, or one module that defines ``P`` twice, keep separate
  entries, and the context digest sees every one of them.
* **Frozen facts.** ``visit_ClassDef`` builds a ``ClassInfo`` and nothing writes
  it afterwards; lowering-time derived facts (such as exception-subclass status)
  live in generator side tables. A dependent module therefore cannot change a
  shared ``ClassInfo``, so a fresh-lowered and a cache-hydrated snapshot hash
  identically without any digest-only projection.

The digest must also be deterministic over ``ClassId`` keys (written in
``ClassId.sort_key`` order) and never silently ``None`` for a program with
classes, which would switch the frontend cache off.
"""

from __future__ import annotations

import ast
import copy
import dataclasses
import importlib
import json

import pytest

from molt.frontend import BuiltinClass, ClassId, MoltValue, SimpleTIRGenerator
from molt.frontend.dataclass_init import DataclassInitField, project_initializer_fields

CK = importlib.import_module("molt.cli.cache_keys")
MC = importlib.import_module("molt.cli.module_cache")

_BASE_SOURCE = (
    "class Error(ValueError):\n"
    "    pass\n"
    "class Point:\n"
    "    def __init__(self, x: int, peer: 'Point') -> None:\n"
    "        self.x = x\n"
    "        self.peer = peer\n"
    "class Point3(Point):\n"
    "    def __init__(self, x, peer, z):\n"
    "        super().__init__(x, peer)\n"
    "        self.z = z\n"
)


def _digest(known_classes: dict) -> str | None:
    return MC._module_lowering_context_digest(
        {"version": 3, "module_name": "pkg.consumer", "known_classes": known_classes}
    )


def _round_trip(value: object) -> object:
    encoded = json.dumps(CK._cache_json_value(value), default=CK._json_ir_default)
    return MC._decode_cached_json_value(json.loads(encoded))


def _snapshot(value: object) -> object:
    """A structural copy: AST nodes by dump, values by every field."""
    if isinstance(value, ast.AST):
        return ("ast", ast.dump(value))
    if isinstance(value, MoltValue):
        return ("value", dataclasses.astuple(value))
    if isinstance(value, dict):
        return {key: _snapshot(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return type(value)(_snapshot(item) for item in value)
    if isinstance(value, (set, frozenset)):
        return frozenset(_snapshot(item) for item in value)
    return value


def _lowered_classes(module_name: str, source: str, known: dict | None = None):
    generator = SimpleTIRGenerator(
        module_name=module_name, known_classes=known, module_source_role="user"
    )
    generator.visit(ast.parse(source))
    return generator, {
        class_id: generator.classes[class_id] for class_id in generator.local_class_ids
    }


def test_lowered_class_table_has_a_present_round_trip_stable_digest() -> None:
    _generator, classes = _lowered_classes("pkg.base", _BASE_SOURCE)
    assert set(classes) == {
        ClassId("pkg.base", "Error"),
        ClassId("pkg.base", "Point"),
        ClassId("pkg.base", "Point3"),
    }
    fresh = _digest(classes)
    assert fresh is not None, "an unencodable ClassId would switch the cache off"
    hydrated = _round_trip(classes)
    assert isinstance(hydrated, dict) and set(hydrated) == set(classes)
    assert _digest(hydrated) == fresh


def test_digest_is_independent_of_class_table_insertion_order() -> None:
    _generator, classes = _lowered_classes("pkg.base", _BASE_SOURCE)
    reordered = dict(reversed(list(classes.items())))
    assert list(reordered) != list(classes)
    assert _digest(reordered) == _digest(classes)


def test_same_named_classes_keep_separate_entries_and_digests() -> None:
    _gen_a, module_a = _lowered_classes("pkg.a", "class P:\n    pass\n")
    _gen_b, module_b = _lowered_classes(
        "pkg.b", "class P:\n    def __init__(self):\n        self.q = 1\n"
    )
    merged = {**module_a, **module_b}
    assert set(merged) == {ClassId("pkg.a", "P"), ClassId("pkg.b", "P")}
    assert _digest(merged) not in {_digest(module_a), _digest(module_b)}

    _gen, redefined = _lowered_classes(
        "pkg.c", "class P:\n    pass\nclass P:\n    x = 1\n"
    )
    assert set(redefined) == {
        ClassId("pkg.c", "P", (1, 0)),
        ClassId("pkg.c", "P", (3, 0)),
    }


def test_real_class_change_still_changes_digest() -> None:
    _generator, classes = _lowered_classes("pkg.base", _BASE_SOURCE)
    point = ClassId("pkg.base", "Point")
    changed = copy.deepcopy(classes)
    changed[point]["mro"] = (point, BuiltinClass("dict"))
    assert _digest(changed) != _digest(classes)


def test_lowering_a_dependent_module_leaves_known_class_info_frozen() -> None:
    _generator, known = _lowered_classes("pkg.base", _BASE_SOURCE)
    before = _snapshot(known)
    digest_before = _digest(known)
    consumer, _local = _lowered_classes(
        "pkg.consumer",
        "from pkg.base import Error, Point\n"
        "def use(p: Point) -> int:\n"
        "    if p.x < 0:\n"
        "        raise Error('negative')\n"
        "    return p.peer.x\n"
        "class Local(Error):\n"
        "    pass\n",
        known=known,
    )
    # The consumer derived exception-subclass facts in its own side table.
    assert consumer._exception_subclass_memo
    assert _snapshot(known) == before, "a dependent module mutated a shared ClassInfo"
    assert _digest(known) == digest_before


@pytest.mark.parametrize(
    "last_field",
    [
        "factory: list = field(init=False, kw_only=True, default_factory=list)",
        "transient: InitVar[int] = 4",
    ],
)
def test_dataclass_projection_survives_cache_and_inherited_projection(
    last_field,
) -> None:
    source = (
        """from dataclasses import dataclass, field, InitVar
@dataclass
class Base:
    required: int
    ordinary: int = 3
"""
        + f"    {last_field}\n"
    )
    namespace = {"__name__": __name__}
    exec(compile(source, "dataclass-oracle", "exec", dont_inherit=True), namespace)

    def expected(cls):
        return tuple(
            (
                field.name,
                field.init,
                field.kw_only,
                isinstance(field.type, dataclasses.InitVar),
                2
                if field.default_factory is not dataclasses.MISSING
                else 1
                if field.default is not dataclasses.MISSING
                else 0,
            )
            for field in cls.__dataclass_fields__.values()
        )

    # Keep the only helper call last: a callback can expire binding facts
    # for subsequent fields, whose projection is deliberately conservative.
    _generator, classes = _lowered_classes("pkg.base", source)
    projection = classes[ClassId("pkg.base", "Base")]["dataclass_init_projection"]
    assert projection is not None
    assert tuple(field.schema() for field in projection) == expected(namespace["Base"])
    hydrated = _round_trip(classes)
    assert _digest(hydrated) == _digest(classes)
    child_source = """from dataclasses import dataclass
@dataclass
class Child(Base):
    extra: int = 9
"""
    exec(compile(child_source, "child-oracle", "exec", dont_inherit=True), namespace)
    child_definition = ast.parse(child_source).body[-1]
    assert isinstance(child_definition, ast.ClassDef)
    for known in (classes, hydrated):
        inherited = known[ClassId("pkg.base", "Base")]["dataclass_init_projection"]
        child = project_initializer_fields(child_definition, (inherited,), False, None)
        assert child is not None
        assert tuple(field.schema() for field in child) == expected(namespace["Child"])


def test_dataclass_cache_tag_does_not_capture_a_metadata_name() -> None:
    name_map = {"__dataclass_init_field__": ["field", True, False, False, 0]}
    assert _round_trip(name_map) == name_map


@pytest.mark.parametrize(
    "fields",
    [
        None,
        [],
        ["field", True, False, False],
        ["field", True, False, False, 0, 1],
        ["not a field", True, False, False, 0],
        [17, True, False, False, 0],
        ["field", 1, False, False, 0],
        ["field", True, 0, False, 0],
        ["field", True, False, None, 0],
        ["field", True, False, False, True],
        ["field", True, False, False, -1],
        ["field", True, False, False, 3],
    ],
)
def test_dataclass_cache_projection_rejects_malformed_fields(fields) -> None:
    with pytest.raises(ValueError, match="dataclass initializer field"):
        MC._decode_cached_json_value({"__dataclass_init_field__": fields})


_DATACLASS_SOURCE = (
    "from dataclasses import InitVar, dataclass, field\n"
    "@dataclass\n"
    "class Record:\n"
    "    name: str\n"
    "    count: int = 0\n"
    "    tags: list = field(default_factory=list)\n"
    "    scale: InitVar[int] = 1\n"
    "    ident: int = field(default=0, kw_only=True)\n"
    "    def __post_init__(self, scale):\n"
    "        self.count *= scale\n"
)


def test_dataclass_initializer_projection_round_trips_through_the_cache_codec() -> None:
    """HF-668 exports each dataclass's initializer projection as a class fact. The
    lowering cache must encode and decode it exactly: an unencodable fact switches the
    context digest off and crashes the persisted module-lowering write."""
    _generator, classes = _lowered_classes("pkg.records", _DATACLASS_SOURCE)
    record_id = ClassId("pkg.records", "Record")
    projection = classes[record_id]["dataclass_init_projection"]
    assert projection, "the dataclass transform recorded no initializer projection"
    fresh = _digest(classes)
    assert fresh is not None, "an unencodable projection would switch the cache off"
    hydrated = _round_trip(classes)
    assert hydrated[record_id]["dataclass_init_projection"] == projection
    assert _digest(hydrated) == fresh


def test_a_malformed_dataclass_projection_payload_is_refused() -> None:
    """A bad record must not decode into a plain dict; the lowering read path turns
    the ValueError into a cache miss."""
    import pytest

    for schema in (
        ["name", True, False, False, 3],
        ["name", True, False, False],
        [1, True, False, False, 0],
        "name",
    ):
        with pytest.raises(ValueError, match="malformed dataclass initializer field"):
            MC._decode_cached_json_value({"__dataclass_init_field__": schema})


def test_dataclass_initializer_projection_survives_class_cache_round_trip() -> None:
    _generator, classes = _lowered_classes(
        "pkg.records",
        "from dataclasses import dataclass, field\n"
        "@dataclass\n"
        "class Record:\n"
        "    x: int\n"
        "    y: int = field(default_factory=int, kw_only=True)\n"
        "    seed: int = 0\n",
    )
    record = ClassId("pkg.records", "Record")
    fields = classes[record]["dataclass_init_projection"]
    assert fields is not None
    assert [field.schema() for field in fields] == [
        ("x", True, False, False, 0),
        ("y", True, True, False, 2),
        ("seed", True, False, False, 1),
    ]
    hydrated = _round_trip(classes)
    assert isinstance(hydrated, dict)
    restored = hydrated[record]["dataclass_init_projection"]
    assert restored == fields
    assert all(type(field) is DataclassInitField for field in restored)
    assert _digest(hydrated) == _digest(classes)


def test_dataclass_projection_cache_tag_does_not_capture_name_maps() -> None:
    names = {"__dataclass_init_field__": ["x", True, False, False, 0]}
    assert _round_trip(names) == names


def test_dataclass_projection_cache_rejects_malformed_fields() -> None:
    malformed = [
        None,
        ["x", True, False, False],
        [1, True, False, False, 0],
        ["x", 1, False, False, 0],
        ["x", True, 0, False, 0],
        ["x", True, False, 0, 0],
        ["x", True, False, False, True],
        ["x", True, False, False, 3],
    ]
    for payload in malformed:
        with pytest.raises(ValueError, match="dataclass initializer field"):
            MC._decode_cached_json_value({"__dataclass_init_field__": payload})
