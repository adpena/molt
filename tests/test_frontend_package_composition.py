"""Regression tests for the frontend visitor-mixin package decomposition (F1).

The SimpleTIRGenerator god-class was decomposed (move-only) into a package of
visitor/lowering mixins composed via MRO. These tests pin the invariants that
make that decomposition correct and keep it from silently regressing:

  * the historical public import surface (``from molt.frontend import ...``) is
    preserved exactly;
  * every extracted mixin is present in the SimpleTIRGenerator MRO and its
    methods resolve on the assembled class;
  * the shared data types live in the ``_types`` leaf (no ``__init__`` <-> mixin
    import cycle);
  * compile_to_tir still produces deterministic IR for a representative corpus.

If a future change moves a method into a new mixin, add that mixin to
EXPECTED_MIXINS; if it re-inlines the class, these tests will flag the loss of
the decomposition.
"""

from __future__ import annotations

import importlib
import json
from pathlib import Path

import molt.frontend as frontend
from molt.frontend import (
    MoltOp,
    MoltValue,
    SimpleTIRGenerator,
    compile_to_tir,
)

ROOT = Path(__file__).resolve().parents[1]

# The public names that external consumers (cli.py, debug/ir.py, tests, tools)
# import from molt.frontend. This is the backward-compatibility contract.
PUBLIC_SURFACE = [
    "MoltValue",
    "MoltOp",
    "SimpleTIRGenerator",
    "compile_to_tir",
    "SCCPResult",
    "LoopBoundFact",
    "ClassInfo",
    "FuncInfo",
    "MethodInfo",
    "ActiveException",
    "BuiltinFuncSpec",
    "TryScope",
    "CompatibilityError",
    "CompatibilityReporter",
    "FallbackPolicy",
    "CFGGraph",
    "ControlMaps",
    "build_cfg",
    "normalize_type_hint",
]

# Mixins extracted from SimpleTIRGenerator (extend as more families move).
EXPECTED_MIXINS = [
    "SerializationMixin",
    "PatternMatchMixin",
]


def test_public_import_surface_preserved() -> None:
    """Every historically-importable name is still importable from the package."""
    for name in PUBLIC_SURFACE:
        assert hasattr(frontend, name), f"molt.frontend lost public name: {name}"


def test_shared_types_live_in_leaf_module() -> None:
    """Shared dataclasses come from the _types leaf, not __init__ (cycle break)."""
    assert MoltValue.__module__ == "molt.frontend._types"
    assert MoltOp.__module__ == "molt.frontend._types"
    # The leaf must never import back into __init__ at runtime.
    types_src = (ROOT / "src" / "molt" / "frontend" / "_types.py").read_text()
    assert "from molt.frontend import" not in types_src.split("if TYPE_CHECKING")[0]


def test_mixins_present_in_mro() -> None:
    """Each extracted mixin must be in the SimpleTIRGenerator MRO."""
    mro_names = {cls.__name__ for cls in SimpleTIRGenerator.__mro__}
    for mixin in EXPECTED_MIXINS:
        assert mixin in mro_names, f"{mixin} missing from SimpleTIRGenerator MRO"
    # ast.NodeVisitor must remain the dispatch base.
    assert "NodeVisitor" in mro_names


def test_moved_methods_resolve_on_class() -> None:
    """Representative methods from each mixin resolve on the assembled class."""
    # serialization
    assert hasattr(SimpleTIRGenerator, "map_ops_to_json")
    assert hasattr(SimpleTIRGenerator, "_scalarize_string_split_fields_json")
    # pattern_match
    assert hasattr(SimpleTIRGenerator, "visit_Match")
    assert hasattr(SimpleTIRGenerator, "_emit_match_class")
    assert hasattr(SimpleTIRGenerator, "_validate_match_pattern")


def test_mixin_modules_import_standalone() -> None:
    """Mixin modules import without triggering an __init__ <-> mixin cycle."""
    for mod in (
        "molt.frontend._types",
        "molt.frontend.lowering.serialization",
        "molt.frontend.visitors.pattern_match",
    ):
        assert importlib.import_module(mod) is not None


def test_compile_to_tir_deterministic_with_match() -> None:
    """A match-statement program compiles deterministically through the mixins."""
    source = (
        "def classify(x):\n"
        "    match x:\n"
        "        case [a, b]:\n"
        "            return a + b\n"
        "        case {'k': v}:\n"
        "            return v\n"
        "        case int():\n"
        "            return x\n"
        "        case _:\n"
        "            return None\n"
    )
    ir_a = json.dumps(compile_to_tir(source), sort_keys=True)
    ir_b = json.dumps(compile_to_tir(source), sort_keys=True)
    assert ir_a == ir_b
    # The match lowering must have produced ops (sanity: the mixin ran).
    assert "classify" in ir_a
