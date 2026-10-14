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

from molt.frontend import BuiltinClass, ClassId, MoltValue, SimpleTIRGenerator

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
    generator = SimpleTIRGenerator(module_name=module_name, known_classes=known)
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
