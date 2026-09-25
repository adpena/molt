"""Structural teeth for the runtime-native `_weakref.ReferenceType` authority."""

from __future__ import annotations

import ast
from pathlib import Path

from molt.stdlib_intrinsic_policy import (
    STDLIB_PROBE_INTRINSIC,
    intrinsic_names_from_source,
)
from tools.stdlib_full_coverage_manifest import (
    STDLIB_FULLY_COVERED_MODULES,
    STDLIB_REQUIRED_INTRINSICS_BY_MODULE,
)


ROOT = Path(__file__).resolve().parents[1]


def test_low_level_weakref_module_owns_native_type_facade_without_cycle() -> None:
    low_level = (ROOT / "src/molt/stdlib/_weakref.py").read_text(encoding="utf-8")
    high_level = (ROOT / "src/molt/stdlib/weakref.py").read_text(encoding="utf-8")
    assert "from weakref import" not in low_level
    assert 'molt_weakref_reference_type")()' in low_level
    delegated = {
        (alias.name, alias.asname or alias.name)
        for node in ast.parse(high_level).body
        if isinstance(node, ast.ImportFrom)
        and node.level == 0
        and node.module == "_weakref"
        for alias in node.names
    }
    assert delegated == {
        (name, name)
        for name in ("ReferenceType", "ref", "getweakrefcount", "getweakrefs")
    }
    assert intrinsic_names_from_source(low_level) == {
        "molt_weakref_reference_type",
        "molt_weakref_count",
        "molt_weakref_refs",
    }
    assert any(
        isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "ref"
            for target in node.targets
        )
        and isinstance(node.value, ast.Name)
        and node.value.id == "ReferenceType"
        for node in ast.parse(low_level).body
    )
    assert "class ReferenceType" not in high_level
    assert "class _ReferenceTypeMeta" not in high_level


def test_weakref_full_coverage_contract_tracks_only_facade_intrinsic_loads() -> None:
    source = (ROOT / "src/molt/stdlib/weakref.py").read_text(encoding="utf-8")
    required = set(STDLIB_REQUIRED_INTRINSICS_BY_MODULE["weakref"])
    assert required == intrinsic_names_from_source(source) - {STDLIB_PROBE_INTRINSIC}
    assert required.isdisjoint(
        {
            "molt_weakref_count",
            "molt_weakref_refs",
            "molt_weakref_reference_type",
            "molt_weakref_find_nocallback",
            "molt_weakref_register",
        }
    )
    assert "weakref" in STDLIB_FULLY_COVERED_MODULES
    assert "_weakref" not in STDLIB_FULLY_COVERED_MODULES


def test_high_level_weakref_preserves_keyed_ref_without_shadow_hash_state() -> None:
    path = ROOT / "src/molt/stdlib/weakref.py"
    source = path.read_text(encoding="utf-8")
    module = ast.parse(source, filename=str(path))
    keyed = [
        node
        for node in module.body
        if isinstance(node, ast.ClassDef) and node.name == "KeyedRef"
    ]
    assert len(keyed) == 1
    assert [base.id for base in keyed[0].bases if isinstance(base, ast.Name)] == [
        "ReferenceType"
    ]
    methods = {
        node.name
        for node in keyed[0].body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    assert {"__new__", "__init__"} <= methods
    assert "self.key = key" in source
    assert "ReferenceType.__init__(self, ob, callback)" in source
    assert "value_ref = KeyedRef(value, self._remove, key)" in source
    assert "._hash =" not in source


def test_deleted_trusted_class_transport_lane_has_no_survivors() -> None:
    forbidden = (
        "trusted_class_spec",
        "class_set_instance_kind_trusted",
        "TrustedClassSpec",
        "TRUSTED_CLASS_WEAKREF",
    )
    roots = [ROOT / "src", ROOT / "runtime", ROOT / "tools"]
    survivors: list[str] = []
    for root in roots:
        for path in root.rglob("*"):
            if not path.is_file() or path.suffix not in {
                ".py",
                ".rs",
                ".toml",
                ".json",
            }:
                continue
            text = path.read_text(encoding="utf-8", errors="ignore")
            if any(token in text for token in forbidden):
                survivors.append(path.relative_to(ROOT).as_posix())
    assert survivors == []


def test_class_shaped_attribute_authority_is_generated_not_type_listed() -> None:
    object_model = (ROOT / "runtime/molt-runtime/src/object/mod.rs").read_text(
        encoding="utf-8"
    )
    mutation = (
        ROOT / "runtime/molt-runtime/src/builtins/attributes/mutation.rs"
    ).read_text(encoding="utf-8")
    assert "heap_shape_policy(type_id) == Some(HeapShapePolicy::Class)" in object_model
    assert "heap_kind_has_class_shape(object_type_id(ptr))" in object_model
    assert mutation.count("heap_kind_has_class_shape(type_id)") >= 2
    attr = (ROOT / "runtime/molt-runtime/src/builtins/attr.rs").read_text(
        encoding="utf-8"
    )
    attributes = (ROOT / "runtime/molt-runtime/src/builtins/attributes.rs").read_text(
        encoding="utf-8"
    )
    accessors = (ROOT / "runtime/molt-runtime/src/object/accessors.rs").read_text(
        encoding="utf-8"
    )
    assert "fn class_instance_layout_attr_allowed" in attr
    assert "class_instance_layout_attr_allowed(_py, class_ptr, attr_bits)" in attr
    # Default lookup delegates to the same raw object authority as explicit
    # object.__getattribute__; the retired result IC must not duplicate it.
    assert "return classed_default_attr_lookup(_py, obj_ptr, attr_bits)" in attributes
    assert "object_attr_lookup_raw(_py, obj_ptr, attr_bits)" in attributes
    assert "class_instance_layout_attr_allowed" not in attributes
    assert "attr_ic_class_key" not in accessors
    assert "super::heap_kind_has_class_shape((*header).type_id)" in accessors
    assert "!crate::object::heap_kind_has_class_shape(type_id)" in attr


def test_callback_descriptor_is_published_once_in_reference_type_dictionary() -> None:
    attr = (ROOT / "runtime/molt-runtime/src/builtins/attr.rs").read_text(
        encoding="utf-8"
    )
    classes = (ROOT / "runtime/molt-runtime/src/builtins/classes.rs").read_text(
        encoding="utf-8"
    )
    assert "pub(crate) fn install_weakref_callback_descriptor" in attr
    assert "install_weakref_callback_descriptor(&py)" in classes
    assert 'attr_name.as_deref() == Some("__callback__")' not in attr
