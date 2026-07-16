"""Structural teeth for the runtime-native `_weakref.ReferenceType` authority."""

from __future__ import annotations

import ast
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_low_level_weakref_module_owns_native_type_facade_without_cycle() -> None:
    low_level = (ROOT / "src/molt/stdlib/_weakref.py").read_text(encoding="utf-8")
    high_level = (ROOT / "src/molt/stdlib/weakref.py").read_text(encoding="utf-8")
    assert "from weakref import" not in low_level
    assert 'molt_weakref_reference_type")()' in low_level
    assert "from _weakref import ReferenceType" in high_level
    assert "class ReferenceType" not in high_level
    assert "class _ReferenceTypeMeta" not in high_level


def test_high_level_weakref_only_subclasses_native_reference_type() -> None:
    path = ROOT / "src/molt/stdlib/weakref.py"
    module = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
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
            if not path.is_file() or path.suffix not in {".py", ".rs", ".toml", ".json"}:
                continue
            text = path.read_text(encoding="utf-8", errors="ignore")
            if any(token in text for token in forbidden):
                survivors.append(path.relative_to(ROOT).as_posix())
    assert survivors == []
