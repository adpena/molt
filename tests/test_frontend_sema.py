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
import pytest

from molt.frontend import SimpleTIRGenerator
from molt.frontend.sema import (
    ClassGraph,
    SemaResult,
    analyze_module,
    build_class_facts,
    build_class_graph,
    c3_merge,
    class_body_needs_block_exec,
    collect_module_class_names,
    collect_module_func_defaults,
    collect_module_func_kinds,
    function_contains_yield,
    reachable_base_names,
    static_class_bases,
    static_mro_names,
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


def test_collect_assigned_names_includes_walrus_targets() -> None:
    # Regression: walrus (:=) targets bind in the ENCLOSING scope. The set-valued
    # _collect_assigned_names dropped them (unlike _collect_assigned_names_ordered),
    # so the local was mis-seen as global/free, corrupting unbound-checks,
    # closure-cell boxing, and free-var classification.
    gen = SimpleTIRGenerator()

    names = gen._collect_assigned_names(
        ast.parse("if (x := len([1, 2, 3])) > 2:\n    y = x\n").body
    )
    assert "x" in names and "y" in names

    # Walrus in an assignment's value expression is also a binding.
    names2 = gen._collect_assigned_names(ast.parse("y = (a := 5) + 1\n").body)
    assert "a" in names2

    # A walrus inside a NESTED function is that function's local, not the outer
    # scope's — it must NOT leak (the collector stops at FunctionDef/Lambda).
    names3 = gen._collect_assigned_names(
        ast.parse("def g():\n    if (z := 1):\n        pass\ny = 2\n").body
    )
    assert "z" not in names3
    assert {"g", "y"} <= names3


def test_free_var_analysis_memoizes_nested_subtrees() -> None:
    mod = ast.parse(
        """
module_value = 1
def outer():
    outer_value = 2
    def mid():
        def leaf():
            return module_value + outer_value
        helper = lambda: module_value + outer_value
        return leaf, helper
    def sibling():
        return mid
    return mid, sibling
"""
    )
    outer = mod.body[1]
    assert isinstance(outer, ast.FunctionDef)
    mid = outer.body[1]
    assert isinstance(mid, ast.FunctionDef)
    leaf = mid.body[0]
    assert isinstance(leaf, ast.FunctionDef)
    helper_assign = mid.body[1]
    assert isinstance(helper_assign, ast.Assign)
    helper = helper_assign.value
    assert isinstance(helper, ast.Lambda)
    sibling = outer.body[2]
    assert isinstance(sibling, ast.FunctionDef)

    gen = SimpleTIRGenerator()
    authority = gen._lexical_dependencies()
    assert gen._collect_free_vars_raw(outer) == {"module_value"}
    assert set(authority.summaries) == {outer, mid, leaf, helper, sibling}
    assert authority.declaration_scans == 5
    visits = authority.node_visits

    gen.locals = {"module_value": object(), "outer_value": object()}
    assert gen._collect_free_vars(mid) == ["module_value", "outer_value"]
    assert gen._collect_free_vars_expr(helper) == ["module_value", "outer_value"]
    assert gen._collect_free_vars_raw(leaf) == {"module_value", "outer_value"}

    assert authority.declaration_scans == 5
    assert authority.node_visits == visits


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("lambda: external", {"external"}),
        ("lambda arg=default: arg + external", {"default", "external"}),
        ("lambda shadow: (lambda: shadow + external)", {"external"}),
        (
            "[external for shadow in inputs if predicate(shadow)]",
            {"external", "inputs", "predicate"},
        ),
        ("[(lambda: shadow + external) for shadow in inputs]", {"external", "inputs"}),
    ],
)
def test_annotation_dependency_transport_is_transitive_and_scope_correct(
    source: str, expected: set[str]
) -> None:
    from molt.compiler_analysis.python_lexical_scope import PythonDependencyAuthority

    authority = PythonDependencyAuthority(
        eager_annotations=False, future_annotations=False
    )
    expression = ast.parse(source, mode="eval").body
    projection = authority.project((expression,), implicit_class_cell=True)
    assert set(projection.lexical) == expected


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "body",
    [
        "def inner(default=(lambda: value)):\n    pass",
        "class Inner:\n    value = 0\n    type Alias = lambda: value",
        "type Alias = lambda: value",
        "return [(lambda: value) for value in inputs], value",
    ],
)
def test_lexical_dependency_and_storage_planning_share_authority(
    target: tuple[int, int], body: str
) -> None:
    gen = SimpleTIRGenerator(target_python=target)
    nested = ast.parse(
        "def middle():\n" + "\n".join("    " + line for line in body.splitlines())
    )
    middle = nested.body[0]
    assert isinstance(middle, ast.FunctionDef)
    # A comprehension target must not suppress the independent outer value.
    assert "value" in gen._collect_free_vars_raw(middle)
    if not body.startswith("return"):
        assert "value" in gen._collect_scope_cell_vars(middle.body, {"value"})


@pytest.mark.parametrize(
    "body",
    [
        "try:\n    pass\nexcept Error as bound:\n    return lambda: bound",
        "match subject:\n    case {'key': bound}:\n        return lambda: bound",
        "match subject:\n    case [*bound]:\n        return lambda: bound",
        "match subject:\n    case {**bound}:\n        return lambda: bound",
        "match subject:\n    case _ as bound:\n        return lambda: bound",
    ],
)
def test_string_field_declarations_are_not_outer_closure_dependencies(
    body: str,
) -> None:
    node = ast.parse(
        "def function():\n" + "\n".join("    " + line for line in body.splitlines())
    ).body[0]
    assert isinstance(node, ast.FunctionDef)
    gen = SimpleTIRGenerator()
    authority = gen._lexical_dependencies()
    assert "bound" in authority.declarations(node).bound
    assert "bound" not in gen._collect_free_vars_raw(node)
    assert gen._collect_scope_cell_vars(node.body, {"bound"}) == {"bound"}


def test_free_var_cache_keeps_nested_super_classcell_projection() -> None:
    mod = ast.parse(
        """
class Child:
    def method(self):
        def inner():
            return super().label()
        return inner
"""
    )
    cls = mod.body[0]
    assert isinstance(cls, ast.ClassDef)
    method = cls.body[0]
    assert isinstance(method, ast.FunctionDef)

    gen = SimpleTIRGenerator()
    gen.locals = {"__class__": object()}
    gen.boxed_locals = {"__class__": object()}

    assert gen._collect_free_vars_raw(method) == {"__class__", "super"}
    assert gen._collect_free_vars(method) == ["__class__"]
