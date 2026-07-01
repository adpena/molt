"""Unit tests for the frontend Bind/Sema phase (doc 44 §F2b).

These exercise the sema/ free functions in ISOLATION on bare ASTs — the
testability-in-isolation win doc 44 §5.5 names: no SimpleTIRGenerator (150
fields) need be constructed to test the static class graph, const environment,
or function metadata.

They also pin the F2b additive-shim contract: that _populate_sema_state fills
the existing god-object dicts from SemaResult so the walk stays byte-identical.
"""

from __future__ import annotations

import ast

from molt.frontend import SimpleTIRGenerator
from molt.frontend.sema import (
    ClassFacts,
    ClassGraph,
    SemaResult,
    analyze_module,
    build_class_facts,
    build_class_graph,
    c3_merge,
    class_facts_with_super_fold_sound_methods,
    class_body_needs_block_exec,
    collect_module_class_names,
    collect_module_func_defaults,
    collect_module_func_kinds,
    function_contains_yield,
    reachable_base_names,
    static_class_bases,
    static_method_owner_after,
    static_mro_names,
    super_fold_is_sound,
    visible_subclasses_of,
)
from molt.frontend.sema.constenv import collect_module_const_dicts


# ---------------------------------------------------------------------------
# class graph
# ---------------------------------------------------------------------------


def test_class_graph_simple_bases() -> None:
    mod = ast.parse("class A(B, C): pass\n")
    g = build_class_graph(mod)
    assert g.bases_by_class == {"A": [["B", "C"]]}
    assert g.subclassed_names == {"B", "C"}


def test_class_graph_no_bases_defaults_to_object() -> None:
    g = build_class_graph(ast.parse("class A: pass\n"))
    assert g.bases_by_class == {"A": [["object"]]}
    assert g.subclassed_names == set()


def test_class_graph_keyword_base_is_opaque() -> None:
    g = build_class_graph(ast.parse("class A(B, metaclass=M): pass\n"))
    # any keyword -> the whole definition is opaque (un-foldable MRO)
    assert g.bases_by_class == {"A": [["<opaque>"]]}
    # metaclass name and base name are both recorded as referenced
    assert g.subclassed_names == {"B", "M"}


def test_class_graph_non_name_base_is_opaque() -> None:
    g = build_class_graph(ast.parse("class A(mod.Base): pass\n"))
    assert g.bases_by_class == {"A": [["<opaque>"]]}
    # dotted base records every attr segment and the root name
    assert g.subclassed_names == {"mod", "Base"}


def test_class_graph_multiple_definitions_retained() -> None:
    src = "if x:\n    class A(B): pass\nelse:\n    class A(C): pass\n"
    g = build_class_graph(ast.parse(src))
    assert g.bases_by_class == {"A": [["B"], ["C"]]}
    assert g.subclassed_names == {"B", "C"}


def test_class_graph_includes_nested_and_function_local_classes() -> None:
    src = (
        "class Outer:\n"
        "    class Inner(Base): pass\n"
        "def f():\n"
        "    class Local(LB): pass\n"
    )
    g = build_class_graph(ast.parse(src))
    assert g.bases_by_class == {
        "Outer": [["object"]],
        "Inner": [["Base"]],
        "Local": [["LB"]],
    }
    assert g.subclassed_names == {"Base", "LB"}


def _method_classes(*rows: tuple[str, set[str]]) -> dict[str, dict[str, object]]:
    return {
        name: {"methods": {method: object() for method in methods}}
        for name, methods in rows
    }


def _class_facts(
    methods: dict[str, set[str]],
    attrs: dict[str, set[str]] | None = None,
    ambiguous: set[str] | None = None,
    opaque: set[str] | None = None,
) -> ClassFacts:
    return ClassFacts(
        method_names_by_class={
            name: frozenset(method_names) for name, method_names in methods.items()
        },
        attr_names_by_class={
            name: frozenset(attr_names) for name, attr_names in (attrs or {}).items()
        },
        opaque_member_class_names=frozenset(opaque or set()),
        ambiguous_class_names=frozenset(ambiguous or set()),
        block_exec_class_nodes=frozenset(),
        super_fold_sound_methods_by_class={},
    )


def test_class_facts_collect_methods_and_attr_blockers() -> None:
    src = (
        "class C:\n"
        "    x = 1\n"
        "    y: int = 2\n"
        "    z: int\n"
        "    def f(self): pass\n"
        "    async def g(self): pass\n"
        "    @f.setter\n"
        "    def f(self, value): pass\n"
    )
    facts = build_class_facts(ast.parse(src))
    assert facts.method_names_by_class == {"C": frozenset({"f", "g"})}
    assert facts.attr_names_by_class == {"C": frozenset({"x", "y"})}
    assert facts.opaque_member_class_names == frozenset()
    assert facts.block_exec_class_nodes == frozenset()
    assert facts.super_fold_sound_methods_by_class == {}
    assert facts.ambiguous_class_names == frozenset()


def test_class_facts_track_final_binding_and_ambiguous_defs() -> None:
    src = (
        "class A:\n"
        "    def f(self): pass\n"
        "    f = 1\n"
        "    def g(self): pass\n"
        "    del g\n"
        "    import math as m\n"
        "    class Nested: pass\n"
        "class A:\n"
        "    def h(self): pass\n"
    )
    facts = build_class_facts(ast.parse(src))
    assert facts.method_names_by_class == {
        "A": frozenset({"h"}),
        "Nested": frozenset(),
    }
    assert facts.attr_names_by_class == {
        "A": frozenset(),
        "Nested": frozenset(),
    }
    assert facts.opaque_member_class_names == frozenset({"A"})
    assert facts.ambiguous_class_names == frozenset({"A"})


def test_class_facts_mark_dynamic_or_decorated_member_surfaces_opaque() -> None:
    src = (
        "def deco(x): return x\n"
        "@deco\n"
        "class DecoratedClass:\n"
        "    def f(self): pass\n"
        "class ControlFlow:\n"
        "    if FLAG:\n"
        "        def f(self): pass\n"
        "class DecoratedMethod:\n"
        "    @deco\n"
        "    def f(self): pass\n"
    )
    facts = build_class_facts(ast.parse(src))
    assert facts.opaque_member_class_names == frozenset(
        {"DecoratedClass", "ControlFlow", "DecoratedMethod"}
    )


def test_class_body_needs_block_exec_tracks_non_straight_line_bodies() -> None:
    mod = ast.parse(
        "class Simple:\n"
        "    x = 1\n"
        "    def f(self): pass\n"
        "class Looped:\n"
        "    for i in range(2):\n"
        "        x = i\n"
        "class Destructured:\n"
        "    a, b = pair\n"
    )
    simple, looped, destructured = mod.body
    assert isinstance(simple, ast.ClassDef)
    assert isinstance(looped, ast.ClassDef)
    assert isinstance(destructured, ast.ClassDef)
    assert not class_body_needs_block_exec(simple.body)
    assert class_body_needs_block_exec(looped.body)
    assert class_body_needs_block_exec(destructured.body)
    facts = build_class_facts(mod)
    assert facts.block_exec_class_nodes == frozenset({id(looped), id(destructured)})


def test_static_method_owner_after_fails_closed_on_unknown_owner() -> None:
    facts = _class_facts({"Base": {"f"}, "Child": set()})
    assert (
        static_method_owner_after(facts, {}, ["Child", "Base", "object"], "Child", "f")
        == "Base"
    )
    assert (
        static_method_owner_after(facts, {}, ["Child", "Missing", "Base"], "Child", "f")
        is None
    )


def test_static_method_owner_after_uses_imported_classes_after_local_facts() -> None:
    facts = _class_facts({"Child": set()})
    imported = _method_classes(("ImportedBase", {"f"}))
    assert (
        static_method_owner_after(
            facts, imported, ["Child", "ImportedBase", "object"], "Child", "f"
        )
        == "ImportedBase"
    )


def test_static_method_owner_after_rejects_class_attr_interposition() -> None:
    facts = _class_facts({"Child": set(), "Base": {"f"}}, {"Blocker": {"f"}})
    assert (
        static_method_owner_after(
            facts, {}, ["Child", "Blocker", "Base", "object"], "Child", "f"
        )
        is None
    )


def test_static_method_owner_after_rejects_ambiguous_local_class() -> None:
    facts = _class_facts(
        {"Child": set(), "Base": {"f"}},
        ambiguous={"Base"},
    )
    assert (
        static_method_owner_after(facts, {}, ["Child", "Base", "object"], "Child", "f")
        is None
    )


def test_static_method_owner_after_rejects_opaque_member_facts() -> None:
    facts = _class_facts(
        {"Child": set(), "Base": {"f"}},
        opaque={"Base"},
    )
    assert (
        static_method_owner_after(facts, {}, ["Child", "Base", "object"], "Child", "f")
        is None
    )


def test_c3_merge_computes_diamond_linearization_tail() -> None:
    assert c3_merge(
        [
            ["Left", "Base", "object"],
            ["Right", "Base", "object"],
            ["Left", "Right"],
        ]
    ) == ["Left", "Right", "Base", "object"]


def test_static_class_bases_fail_closed_for_ambiguous_or_opaque_defs() -> None:
    classes: dict[str, dict[str, object]] = {
        "Imported": {"bases": ["object"]},
        "Dynamic": {"bases": ["object"], "dynamic": True},
    }
    graph = ClassGraph(
        bases_by_class={
            "Local": [["Base"]],
            "Ambiguous": [["A"], ["B"]],
            "Opaque": [["<opaque>"]],
        },
        subclassed_names={"Base", "A", "B"},
    )
    assert static_class_bases(graph, classes, "object") == ["object"]
    assert static_class_bases(graph, classes, "Local") == ["Base"]
    assert static_class_bases(graph, classes, "Imported") == ["object"]
    assert static_class_bases(graph, classes, "Ambiguous") is None
    assert static_class_bases(graph, classes, "Opaque") is None
    assert static_class_bases(graph, classes, "Dynamic") is None


def test_static_mro_names_and_reachability_share_class_graph_authority() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Left": [["Base"]],
            "Right": [["Base"]],
            "Final": [["Left", "Right"]],
        },
        subclassed_names={"Base", "Left", "Right"},
    )
    assert static_mro_names(graph, {}, "Final") == [
        "Final",
        "Left",
        "Right",
        "Base",
        "object",
    ]
    assert reachable_base_names(graph, "Final") == {
        "Final",
        "Left",
        "Right",
        "Base",
        "object",
    }


def test_visible_subclasses_fails_closed_when_candidate_mro_is_uncertain() -> None:
    graph = ClassGraph(
        bases_by_class={"Base": [["<opaque>"]], "MaybeChild": [["Base"]]},
        subclassed_names={"Base"},
    )
    assert visible_subclasses_of(graph, "Base", {}) is None


def test_visible_subclasses_uses_static_mro() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Mid": [["Base"]],
            "Leaf": [["Mid"]],
        },
        subclassed_names={"Base", "Mid"},
    )
    assert visible_subclasses_of(graph, "Base", {}) == ["Mid", "Leaf"]


def test_super_fold_sound_for_linear_visible_hierarchy() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Mid": [["Base"]],
            "Leaf": [["Mid"]],
        },
        subclassed_names={"Base", "Mid"},
    )
    assert super_fold_is_sound(
        "Mid",
        "f",
        class_facts=_class_facts({"Base": {"f"}, "Mid": {"g"}, "Leaf": set()}),
        imported_classes={},
        class_graph=graph,
        module_name="__main__",
        entry_module=None,
    )


def test_super_fold_rejects_diamond_subclass_interposition() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Left": [["Base"]],
            "Right": [["Base"]],
            "Final": [["Left", "Right"]],
        },
        subclassed_names={"Base", "Left", "Right"},
    )
    assert not super_fold_is_sound(
        "Left",
        "who",
        class_facts=_class_facts(
            {"Base": {"who"}, "Right": {"who"}, "Left": set(), "Final": set()}
        ),
        imported_classes={},
        class_graph=graph,
        module_name="__main__",
        entry_module=None,
    )


def test_super_fold_rejects_non_method_class_attr_interposition() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Left": [["Base"]],
            "Right": [["Base"]],
            "Final": [["Left", "Right"]],
        },
        subclassed_names={"Base", "Left", "Right"},
    )
    assert not super_fold_is_sound(
        "Left",
        "who",
        class_facts=_class_facts(
            {"Base": {"who"}, "Left": set(), "Right": set(), "Final": set()},
            {"Right": {"who"}},
        ),
        imported_classes={},
        class_graph=graph,
        module_name="__main__",
        entry_module=None,
    )


def test_super_fold_rejects_non_entry_module() -> None:
    graph = ClassGraph(
        bases_by_class={"Base": [["object"]], "Child": [["Base"]]},
        subclassed_names={"Base"},
    )
    assert not super_fold_is_sound(
        "Child",
        "f",
        class_facts=_class_facts({"Base": {"f"}, "Child": set()}),
        imported_classes={},
        class_graph=graph,
        module_name="library_mod",
        entry_module="__main__",
    )


def test_super_fold_rejects_dynamic_class_body_interposer() -> None:
    src = (
        "class Base:\n"
        "    def who(self): pass\n"
        "class Left(Base):\n"
        "    pass\n"
        "class Right(Base):\n"
        "    if FLAG:\n"
        "        def who(self): pass\n"
        "class Final(Left, Right):\n"
        "    pass\n"
    )
    module = ast.parse(src)
    assert not super_fold_is_sound(
        "Left",
        "who",
        class_facts=build_class_facts(module),
        imported_classes={},
        class_graph=build_class_graph(module),
        module_name="__main__",
        entry_module=None,
    )


def test_super_fold_rejects_decorated_method_interposer() -> None:
    src = (
        "class Base:\n"
        "    def who(self): pass\n"
        "class Left(Base):\n"
        "    pass\n"
        "class Right(Base):\n"
        "    @decorator\n"
        "    def who(self): pass\n"
        "class Final(Left, Right):\n"
        "    pass\n"
    )
    module = ast.parse(src)
    assert not super_fold_is_sound(
        "Left",
        "who",
        class_facts=build_class_facts(module),
        imported_classes={},
        class_graph=build_class_graph(module),
        module_name="__main__",
        entry_module=None,
    )


def test_class_facts_precompute_super_fold_sound_methods() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Mid": [["Base"]],
            "Leaf": [["Mid"]],
        },
        subclassed_names={"Base", "Mid"},
    )
    facts = _class_facts({"Base": {"f"}, "Mid": {"g"}, "Leaf": set()})
    enriched = class_facts_with_super_fold_sound_methods(
        class_graph=graph,
        class_facts=facts,
        imported_classes={},
        module_name="__main__",
        entry_module=None,
    )
    assert enriched.super_fold_sound_methods_by_class == {
        "Base": frozenset(),
        "Mid": frozenset({"f"}),
        "Leaf": frozenset({"f", "g"}),
    }


def test_class_facts_precompute_rejects_diamond_super_fold() -> None:
    graph = ClassGraph(
        bases_by_class={
            "Base": [["object"]],
            "Left": [["Base"]],
            "Right": [["Base"]],
            "Final": [["Left", "Right"]],
        },
        subclassed_names={"Base", "Left", "Right"},
    )
    facts = _class_facts(
        {"Base": {"who"}, "Right": {"who"}, "Left": set(), "Final": set()}
    )
    enriched = class_facts_with_super_fold_sound_methods(
        class_graph=graph,
        class_facts=facts,
        imported_classes={},
        module_name="__main__",
        entry_module=None,
    )
    assert "who" not in enriched.super_fold_sound_methods_by_class["Left"]


# ---------------------------------------------------------------------------
# const environment
# ---------------------------------------------------------------------------


def test_const_dicts_string_keyed_constant_values() -> None:
    src = 'SLOTS = {"slots": True, "frozen": False, "n": 3, "x": None}\n'
    assert collect_module_const_dicts(ast.parse(src)) == {
        "SLOTS": {"slots": True, "frozen": False, "n": 3, "x": None}
    }


def test_const_dicts_rejects_nonstring_key() -> None:
    assert collect_module_const_dicts(ast.parse("D = {1: 2}\n")) == {}


def test_const_dicts_rejects_nonconst_value() -> None:
    assert collect_module_const_dicts(ast.parse('D = {"k": f()}\n')) == {}


def test_const_dicts_scans_version_gated_if_blocks() -> None:
    src = (
        "import sys\n"
        "if sys.version_info >= (3, 10):\n"
        '    SLOTS = {"slots": True}\n'
        "else:\n"
        '    SLOTS = {"slots": False}\n'
    )
    # both branches are scanned; else overwrites then-branch (source order)
    assert collect_module_const_dicts(ast.parse(src)) == {"SLOTS": {"slots": False}}


def test_const_dicts_rejects_multi_target_assign() -> None:
    assert collect_module_const_dicts(ast.parse('A = B = {"k": 1}\n')) == {}


# ---------------------------------------------------------------------------
# function metadata
# ---------------------------------------------------------------------------


def test_func_kinds_sync_async_gen() -> None:
    src = (
        "def s(): return 1\n"
        "async def a(): pass\n"
        "async def ag():\n    yield 1\n"
        "def g():\n    yield 1\n"
        "def gf():\n    yield from range(3)\n"
    )
    assert collect_module_func_kinds(ast.parse(src)) == {
        "s": "sync",
        "a": "async",
        "g": "gen",
        "gf": "gen",
        "ag": "asyncgen",
    }


def test_func_contains_yield_does_not_descend_into_nested_def() -> None:
    # a yield inside a NESTED function does not make the outer a generator
    src = "def outer():\n    def inner():\n        yield 1\n    return inner\n"
    fn = ast.parse(src).body[0]
    assert function_contains_yield(fn) is False


def test_func_contains_yield_ignores_lambda_body() -> None:
    src = "def outer():\n    f = lambda: (yield)\n    return 1\n"
    fn = ast.parse(src).body[0]
    # CPython parses (yield) in a lambda as the lambda's own generator; the
    # scanner skips Lambda bodies, so outer is NOT a generator.
    assert function_contains_yield(fn) is False


def test_class_names_top_level_only() -> None:
    src = "class A: pass\nclass B: pass\ndef f():\n    class Local: pass\n"
    assert collect_module_class_names(ast.parse(src)) == {"A", "B"}


def test_func_defaults_param_and_default_shape() -> None:
    # a, b are positional-only (before /); c is pos-or-kw; d is kw-only.
    src = "def f(a, b=1, /, c=2, *, d=3): return a\n"
    out = collect_module_func_defaults(ast.parse(src))
    assert out["f"] == {
        "params": 4,
        "defaults": [
            {"const": True, "value": 1},
            {"const": True, "value": 2},
            {"const": True, "value": 3, "kwonly": True, "name": "d"},
        ],
        "posonly": 2,
        "kwonly": 1,
        "kind": "sync",
        "has_decorators": False,
    }


def test_func_defaults_vararg_marker() -> None:
    src = "def f(*args, **kw): return 1\n"
    assert collect_module_func_defaults(ast.parse(src)) == {
        "f": {"has_vararg": True, "kind": "sync", "has_decorators": False}
    }


def test_func_defaults_nonconst_default_is_marked() -> None:
    src = "def f(a=[]): return a\n"
    out = collect_module_func_defaults(ast.parse(src))
    assert out["f"]["defaults"] == [{"const": False}]


def test_func_defaults_carry_kind_and_decorator_shape() -> None:
    src = (
        "import contextlib\n"
        "@contextlib.contextmanager\n"
        "def cm(label):\n"
        "    yield label\n"
        "async def agen(value):\n"
        "    yield value\n"
    )
    out = collect_module_func_defaults(ast.parse(src))
    assert out["cm"]["kind"] == "gen"
    assert out["cm"]["has_decorators"] is True
    assert out["agen"]["kind"] == "asyncgen"
    assert out["agen"]["has_decorators"] is False


# ---------------------------------------------------------------------------
# SemaResult aggregate + immutability
# ---------------------------------------------------------------------------


def test_analyze_module_aggregates_all_families() -> None:
    src = (
        'SLOTS = {"slots": True}\n'
        "def f(a, b=1): return a\n"
        "async def g(): pass\n"
        "class A(B):\n    def m(self): pass\n    k = 1\n"
    )
    r = analyze_module(ast.parse(src))
    assert isinstance(r, SemaResult)
    assert r.const_dicts == {"SLOTS": {"slots": True}}
    assert r.function_meta.declared_funcs == {"f": "sync", "g": "async"}
    assert r.function_meta.declared_classes == {"A"}
    assert r.class_graph.bases_by_class == {"A": [["B"]]}
    assert r.class_graph.subclassed_names == {"B"}
    assert r.class_facts.method_names_by_class == {"A": frozenset({"m"})}
    assert r.class_facts.attr_names_by_class == {"A": frozenset({"k"})}
    assert r.class_facts.opaque_member_class_names == frozenset()
    assert r.class_facts.block_exec_class_nodes == frozenset()
    assert r.class_facts.super_fold_sound_methods_by_class == {}
    assert r.function_meta.defaults["f"]["params"] == 2


def test_sema_result_is_frozen() -> None:
    import dataclasses

    import pytest

    r = analyze_module(ast.parse("x = 1\n"))
    with pytest.raises(dataclasses.FrozenInstanceError):
        r.const_dicts = {}  # type: ignore[misc]
    with pytest.raises(dataclasses.FrozenInstanceError):
        r.class_graph.subclassed_names = set()  # type: ignore[misc]
    with pytest.raises(dataclasses.FrozenInstanceError):
        r.class_facts.method_names_by_class = {}  # type: ignore[misc]
    with pytest.raises(dataclasses.FrozenInstanceError):
        r.function_meta.declared_funcs = {}  # type: ignore[misc]


# ---------------------------------------------------------------------------
# populate-shim contract (the F2b additive-shim invariant)
# ---------------------------------------------------------------------------


def test_populate_sema_state_fills_god_object_dicts_from_result() -> None:
    src = (
        'SLOTS = {"slots": True}\n'
        "def f(a, b=2): return a\n"
        "def g():\n    yield 1\n"
        "class A(B):\n    def m(self): pass\n"
    )
    mod = ast.parse(src)
    gen = SimpleTIRGenerator()
    sema = gen._populate_sema_state(mod)

    # the shim aliases remaining walk-time compatibility dicts into existing state
    assert gen.module_const_dicts == {"SLOTS": {"slots": True}}
    assert gen.module_const_dicts is sema.const_dicts
    assert gen.module_declared_funcs == {"f": "sync", "g": "gen"}
    assert gen.module_declared_funcs is sema.function_meta.declared_funcs
    assert gen.module_declared_classes == {"A"}
    assert gen.module_declared_classes is sema.function_meta.declared_classes
    assert sema.class_graph.bases_by_class == {"A": [["B"]]}
    assert sema.class_graph.subclassed_names == {"B"}
    assert not hasattr(gen, "module_class_bases")
    assert not hasattr(gen, "module_subclassed_names")
    assert gen._sema.class_facts.method_names_by_class == {"A": frozenset({"m"})}
    assert gen._sema.class_facts.block_exec_class_nodes == frozenset()
    assert gen._sema.class_facts.super_fold_sound_methods_by_class == {"A": frozenset()}
    assert gen.module_func_defaults["f"]["params"] == 2
    assert gen._sema is sema


def test_populate_sema_state_honors_known_func_defaults_override() -> None:
    # When known_func_defaults supplies the module, the override wins over the
    # AST-derived defaults (the runtime-input semantics preserved by the shim).
    override = {
        "mymod": {"f": {"params": 99, "defaults": [], "posonly": 0, "kwonly": 0}}
    }
    gen = SimpleTIRGenerator(module_name="mymod", known_func_defaults=override)
    gen._populate_sema_state(ast.parse("def f(a, b=1): return a\n"))
    assert gen.module_func_defaults == override["mymod"]
