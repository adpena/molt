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

import ast
import importlib
import inspect
import json
from pathlib import Path

import molt.frontend as frontend
from molt.frontend import (
    MoltOp,
    MoltValue,
    SimpleTIRGenerator,
    compile_to_tir,
)
from molt.frontend._protocol import _GeneratorProtocol

ROOT = Path(__file__).resolve().parents[1]
FRONTEND_DIR = ROOT / "src" / "molt" / "frontend"

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
    "CallVisitorMixin",
    "ClassDefVisitorMixin",
    "ComprehensionMixin",
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
    # calls (phase 2)
    assert hasattr(SimpleTIRGenerator, "visit_Call")
    assert hasattr(SimpleTIRGenerator, "_emit_call_args_builder")
    assert hasattr(SimpleTIRGenerator, "_fold_bare_super_static")
    # classes (phase 2)
    assert hasattr(SimpleTIRGenerator, "visit_ClassDef")
    assert hasattr(SimpleTIRGenerator, "_compute_method_closure")
    assert hasattr(SimpleTIRGenerator, "_extract_inline_init_assigns")


def test_mixin_modules_import_standalone() -> None:
    """Mixin modules import without triggering an __init__ <-> mixin cycle."""
    for mod in (
        "molt.frontend._types",
        "molt.frontend.lowering.serialization",
        "molt.frontend.visitors.pattern_match",
        "molt.frontend.visitors.calls",
        "molt.frontend.visitors.classes",
    ):
        assert importlib.import_module(mod) is not None


# ---------------------------------------------------------------------------
# Protocol-drift guard (phase-1 reviewer finding)
#
# Each mixin annotates ``self`` as ``_GeneratorProtocol`` under TYPE_CHECKING so
# cross-mixin ``self.<method>`` / ``self.<attr>`` references type-check across
# files.  That guarantee only holds while the Protocol is a SUPERSET of the
# assembled generator's real surface.  If a method moves into a mixin but the
# Protocol is not regenerated (tmp/gen_protocol.py), the moved method — and
# every sibling-mixin call to it — silently loses static checking.  These tests
# fail the moment the Protocol and the assembled class diverge.
# ---------------------------------------------------------------------------

# Names provided by ast.NodeVisitor (the dispatch base) / object are NOT part of
# the generator's own surface and are excluded from the coverage contract.
_BUILTIN_NAMES = set(dir(ast.NodeVisitor)) | set(dir(object))


def _protocol_methods() -> set[str]:
    return {
        name
        for name, val in vars(_GeneratorProtocol).items()
        if callable(val) and not name.startswith("__")
    }


def _protocol_attrs() -> set[str]:
    return set(getattr(_GeneratorProtocol, "__annotations__", {}))


def _assembled_class_methods() -> set[str]:
    """Public methods contributed by SimpleTIRGenerator + all its mixins.

    Excludes dunder methods and anything inherited from ast.NodeVisitor/object
    (the dispatch base and Python builtins, which the Protocol does not model).
    """
    names: set[str] = set()
    for klass in SimpleTIRGenerator.__mro__:
        if klass in (object, ast.NodeVisitor):
            continue
        for name, val in vars(klass).items():
            if name.startswith("__"):
                continue
            if callable(val):
                names.add(name)
    return names - _BUILTIN_NAMES


def _assembled_class_attrs() -> set[str]:
    """Instance attributes (``self.x = ...`` in __init__) + class-level
    annotated vars across the assembled class and its mixins."""
    import textwrap

    attrs: set[str] = set()
    init_src = textwrap.dedent(inspect.getsource(SimpleTIRGenerator.__init__))
    for node in ast.walk(ast.parse(init_src)):
        if (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id == "self"
            and isinstance(node.ctx, ast.Store)
        ):
            attrs.add(node.attr)
    for klass in SimpleTIRGenerator.__mro__:
        if klass is object:
            continue
        attrs.update(getattr(klass, "__annotations__", {}))
    return attrs - _BUILTIN_NAMES


def _discover_mixin_classes() -> dict[str, type]:
    """Auto-discover every *Mixin class under visitors/ and lowering/ so this
    guard automatically covers new mixins added in later extraction phases."""
    found: dict[str, type] = {}
    for sub in ("visitors", "lowering"):
        pkg_dir = FRONTEND_DIR / sub
        for path in sorted(pkg_dir.glob("*.py")):
            if path.name == "__init__.py":
                continue
            mod = importlib.import_module(f"molt.frontend.{sub}.{path.stem}")
            for name, obj in vars(mod).items():
                if (
                    isinstance(obj, type)
                    and name.endswith("Mixin")
                    and obj.__module__ == mod.__name__
                ):
                    found[name] = obj
    return found


def test_protocol_covers_full_class_method_surface() -> None:
    """_GeneratorProtocol must declare every method the assembled class exposes.

    A missing entry means the Protocol drifted from the class (gen_protocol.py
    was not re-run after a move), so sibling-mixin ``self.<method>`` calls no
    longer type-check.
    """
    missing = _assembled_class_methods() - _protocol_methods()
    assert not missing, (
        "Protocol drift: methods on SimpleTIRGenerator missing from "
        f"_GeneratorProtocol (re-run tmp/gen_protocol.py): {sorted(missing)}"
    )


def test_protocol_covers_full_class_attr_surface() -> None:
    """_GeneratorProtocol must declare every instance/class attribute the
    assembled generator sets, so cross-mixin ``self.<attr>`` reads type-check."""
    missing = _assembled_class_attrs() - _protocol_attrs()
    assert not missing, (
        "Protocol drift: attributes on SimpleTIRGenerator missing from "
        f"_GeneratorProtocol (re-run tmp/gen_protocol.py): {sorted(missing)}"
    )


def test_every_mixin_method_is_on_protocol() -> None:
    """Per-mixin view of the same contract, for precise failure attribution.

    Each extracted mixin's own (non-dunder) methods must all appear on the
    Protocol — every mixin annotates ``self`` as the Protocol, so any of its
    methods that is absent is invisible to every other mixin's static checks.
    """
    proto = _protocol_methods()
    mixins = _discover_mixin_classes()
    assert mixins, "no *Mixin classes discovered under visitors/ or lowering/"
    drift: dict[str, list[str]] = {}
    for mixin_name, mixin_cls in sorted(mixins.items()):
        own = {
            name
            for name, val in vars(mixin_cls).items()
            if callable(val) and not name.startswith("__")
        }
        missing = sorted(own - proto)
        if missing:
            drift[mixin_name] = missing
    assert not drift, (
        "Protocol drift: mixin methods missing from _GeneratorProtocol "
        f"(re-run tmp/gen_protocol.py): {drift}"
    )


def test_discovered_mixins_match_expected() -> None:
    """The auto-discovered mixin set must equal EXPECTED_MIXINS, so adding a
    mixin without registering it (or vice versa) is caught."""
    discovered = set(_discover_mixin_classes())
    assert discovered == set(EXPECTED_MIXINS), (
        f"mixin registry drift: discovered={sorted(discovered)} "
        f"expected={sorted(EXPECTED_MIXINS)}"
    )


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
