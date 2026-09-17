from __future__ import annotations

import ast
from molt.compiler_analysis.python_lexical_scope import (
    PythonDependencyAuthority,
    PythonScopeDeclarations,
)
import gc
import weakref
from types import ModuleType
from concurrent.futures import ThreadPoolExecutor
from collections import Counter
from threading import Event

import pytest

from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis.python_binding_facts import (
    OTHER_IDENTITY,
    PythonIdentity,
    PythonMember,
    identity_fact_is_exact,
    identity_fact_may_be,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_source_bindings,
)
from molt.compiler_analysis.python_effects_generated import (
    PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS,
    effect_mask_satisfies_capability,
)
from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
)


def _last_call(source: str):
    index = analyze_python_source_bindings(source)
    assert index.calls
    return index.calls[-1]


@pytest.mark.parametrize("target", ["Alias = A", "del Alias", "(Alias := A)"])
def test_import_metadata_projection_is_independent_of_unrelated_deferred_return(
    target: str,
) -> None:
    source = (
        f"class A:\n    pass\nclass B(A):\n    pass\n{target}\nfrom . import child\n"
    )
    for suffix in ("", "def unrelated():\n    return 1\n"):
        current = source + suffix
        tree = ast.parse(current)
        index = analyze_python_source_bindings(
            current,
            policy=PythonBindingPolicy(
                module_name="pkg.entry", module_spec_name="pkg.entry"
            ),
        )
        request = next(node for node in tree.body if isinstance(node, ast.ImportFrom))
        states = index.module_import_flow.states_for(request)
        assert states
        assert {state.package.kind for state in states} == {"unknown"}


@pytest.mark.parametrize(
    ("source", "observed"),
    [
        ("import sys\n", [False]),
        ("import __future__\n", [False]),
        ("from __future__ import annotations\n", [False]),
        ("import foreign\n", [True]),
        ("def helper():\n    return locals()\n", [False]),
        ("helper = lambda: globals()\n", [False]),
        ("def helper(value=globals()):\n    pass\n", [True]),
        ("class Helper:\n    saved = locals()\n", [True]),
        (
            "class Helper:\n    value: int\n    def method(self):\n        return 1\n",
            [False],
        ),
        ("class Helper:\n    descriptor = unknown\n", [True]),
        ("class Helper(base):\n    pass\n", [True]),
        ("factory().value: int\n", [True]),
        ("view = locals()\n", [True]),
        ("aliases = (globals,)\n", [True]),
        ("globals = 0\nalias = globals\n", [False, False]),
        ("value = unknown\nvalue = 1\n", [False, True]),
        ("callback()\n", [True]),
        ("value = unknown\nvalue += 1\n", [False, True]),
        ("if False:\n    callback()\n", [False]),
        ("if True:\n    import sys\n", [False]),
    ],
)
def test_statement_namespace_observability_uses_executed_binding_facts(
    source: str, observed: list[bool]
) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    facts = [index.statement_fact(statement) for statement in tree.body]
    assert all(fact is not None for fact in facts)
    assert [
        index.module_namespace_may_be_observed(stmt) for stmt in tree.body
    ] == observed
    assert index.module_namespace_may_be_observed(tree) is any(observed)


def test_namespace_projection_preserves_original_expression_and_fails_closed() -> None:
    source = "if (globals,):\n    pass\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    conditional = tree.body[0]
    assert isinstance(conditional, ast.If)
    synthetic = ast.copy_location(ast.Expr(value=conditional.test), conditional)
    assert index.statement_fact(synthetic) is None
    assert index.module_namespace_may_be_observed(synthetic)
    assert index.module_namespace_may_be_observed(ast.Pass())


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future", [False, True])
def test_function_annotation_observation_respects_target_and_future(
    target: tuple[int, int], future: bool
) -> None:
    source = ("from __future__ import annotations\n" if future else "") + (
        "def helper(value: observer()) -> observer():\n    pass\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    eager = target < (3, 14) and not future
    assert index.module_namespace_may_be_observed(tree.body[-1]) is eager
    calls = [node for node in ast.walk(tree.body[-1]) if isinstance(node, ast.Call)]
    assert len(calls) == 2
    assert all((index.call_fact(call) is not None) is (not future) for call in calls)


@pytest.mark.parametrize(
    "source",
    [
        "value = unknown\nvalue += 1\nresult = len\n",
        "class C(**mapping, marker=len):\n    pass\n",
        "class C(*bases, marker=len):\n    pass\n",
    ],
)
def test_statement_callback_boundary_invalidates_later_binding_reads(
    source: str,
) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    read = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and node.id == "len"
    )
    fact = index.expression_fact(read)
    assert fact is not None and fact.binding_invalidated


@pytest.mark.parametrize(
    "source",
    [
        "def helper[globals](value: globals):\n    pass\n",
        "class Helper[globals](globals):\n    pass\n",
        "type Helper[globals] = globals\n",
    ],
)
def test_type_parameter_scopes_shadow_namespace_builtins(source: str) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    reads = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and node.id == "globals"
    ]
    assert reads
    for read in reads:
        fact = index.expression_fact(read)
        assert fact is not None
        assert fact.binding_is_bound
        assert not identity_fact_may_be(fact.identities, PythonIdentity.BUILTIN_GLOBALS)


@pytest.mark.parametrize("target", [(3, 12), (3, 13)])
@pytest.mark.parametrize("definition", ["def", "async def"])
def test_eager_annotation_walrus_shadows_module_before_definition(
    target: tuple[int, int], definition: str
) -> None:
    source = (
        "flag = False\n"
        "def outer():\n"
        "    before = flag\n"
        f"    {definition} inner(value: (flag := False)):\n"
        "        pass\n"
        "    return before\n"
    )
    tree = ast.parse(source)
    outer = tree.body[1]
    assert isinstance(outer, ast.FunctionDef)
    before = outer.body[0]
    assert isinstance(before, ast.Assign)
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    scope = next(scope for scope in index.scopes if scope.name == "outer")
    assert "flag" in scope.local_names
    fact = index.expression_fact(before.value)
    assert fact is not None
    assert fact.identities == python_binding_flow.UNBOUND_IDENTITY
    assert fact.result.truth is None
    assert fact.result.evaluation_required


@pytest.mark.parametrize("target", [(3, 12), (3, 13)])
def test_eager_nested_annotation_load_observes_later_closure_rebinding(
    target: tuple[int, int],
) -> None:
    source = (
        "def outer():\n"
        "    flag = False\n"
        "    def nested():\n"
        "        def inner(value: flag):\n"
        "            pass\n"
        "    flag = unknown\n"
        "    return nested\n"
    )
    tree = ast.parse(source)
    inner = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.FunctionDef) and node.name == "inner"
    )
    annotation = inner.args.args[0].annotation
    assert annotation is not None
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(annotation)
    assert fact is not None
    assert fact.result.truth is None


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future", [False, True])
@pytest.mark.parametrize("generic", [False, True])
def test_ast_only_annotation_declarations_respect_lexical_policy(
    target: tuple[int, int], future: bool, generic: bool
) -> None:
    # ast.parse intentionally accepts annotation walrus nodes that CPython's
    # subsequent compiler rejects for future/deferred/generic annotations.
    # This is a projection test, not a claim that those sources are executable.
    source = ("from __future__ import annotations\n" if future else "") + (
        "def outer():\n"
        f"    def inner{'[T]' if generic else ''}(value: (flag := False)):\n"
        "        pass\n"
    )
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    scope = next(scope for scope in index.scopes if scope.name == "outer")
    assert ("flag" in scope.local_names) is (
        target < (3, 14) and not future and not generic
    )


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("definition", ["def", "async def"])
@pytest.mark.parametrize("future", [False, True])
def test_generic_defaults_use_enclosing_scope_not_type_parameters(
    target: tuple[int, int], definition: str, future: bool
) -> None:
    source = ("from __future__ import annotations\n" if future else "") + (
        f"T = 'outer'\n{definition} helper[T](value=T, *, keyword=T):\n    return T\n"
    )
    tree = ast.parse(source)
    function = tree.body[-1]
    assert isinstance(function, (ast.FunctionDef, ast.AsyncFunctionDef))
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    for default in (*function.args.defaults, *function.args.kw_defaults):
        assert default is not None
        fact = index.expression_fact(default)
        assert fact is not None
        assert fact.static_value == "outer"
        assert index.scopes[fact.scope_id].kind == "module"
    returned = function.body[0]
    assert isinstance(returned, ast.Return) and returned.value is not None
    result = index.expression_fact(returned.value)
    assert result is not None and result.static_value is None


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("generic", [False, True])
def test_nested_annotation_dependencies_are_transitive(
    target: tuple[int, int], generic: bool
) -> None:
    source = (
        "def outer():\n"
        "    flag = False\n"
        "    def nested():\n"
        f"        def inner{'[T]' if generic else ''}(value: flag):\n"
        "            pass\n"
        "    flag = unknown\n"
        "    return nested\n"
    )
    tree = ast.parse(source)
    inner = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.FunctionDef) and node.name == "inner"
    )
    annotation = inner.args.args[0].annotation
    assert annotation is not None
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(annotation)
    assert fact is not None and fact.result.truth is None


def test_deep_closure_load_observes_rebinding_after_intermediate_definition() -> None:
    source = (
        "def outer():\n"
        "    flag = False\n"
        "    def nested():\n"
        "        def deeper():\n"
        "            return flag\n"
        "        return deeper\n"
        "    flag = unknown\n"
        "    return nested\n"
    )
    tree = ast.parse(source)
    read = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name)
        and node.id == "flag"
        and isinstance(node.ctx, ast.Load)
    )
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(read)
    assert fact is not None and fact.result.truth is None


@pytest.mark.parametrize(
    ("body", "lexical", "global_names"),
    [
        (
            "def inner(value):\n    def deep():\n        return value\n    return deep\n",
            set(),
            set(),
        ),
        (
            "def inner():\n    def lexical():\n        return value\n"
            "    def external():\n        global value\n        return value\n",
            {"value"},
            {"value"},
        ),
        (
            "class C:\n    value = False\n    def method(self):\n        return value\n",
            {"value"},
            set(),
        ),
        (
            "class C:\n    result = value\n    value = False\n",
            set(),
            {"value"},
        ),
        (
            "def inner():\n    nonlocal value\n    value = False\n",
            {"value"},
            set(),
        ),
        (
            "result = [item for item in values if predicate(item)]\n",
            {"values", "predicate"},
            set(),
        ),
        (
            "def inner[T](value=external):\n    return T\n",
            {"external"},
            set(),
        ),
    ],
)
def test_dependency_summary_retains_distinct_lexical_and_global_custody(
    body: str, lexical: set[str], global_names: set[str]
) -> None:
    source = "def outer():\n" + "".join(
        "    " + line + "\n" for line in body.splitlines()
    )
    node = ast.parse(source).body[0]
    assert isinstance(node, ast.FunctionDef)
    authority = PythonDependencyAuthority(
        eager_annotations=True, future_annotations=False
    )
    result = authority.summary(node).body
    assert result.lexical == lexical
    assert result.globals == global_names


@pytest.mark.parametrize("eager", [False, True])
@pytest.mark.parametrize("future", [False, True])
@pytest.mark.parametrize("generic", [False, True])
def test_dependency_summary_keeps_annotation_policy_separate_from_body(
    eager: bool, future: bool, generic: bool
) -> None:
    source = (
        "def outer():\n"
        f"    def inner{'[T]' if generic else ''}(value: annotation = default):\n"
        "        return body\n"
    )
    node = ast.parse(source).body[0]
    assert isinstance(node, ast.FunctionDef)
    authority = PythonDependencyAuthority(
        eager_annotations=eager and not future, future_annotations=future
    )
    result = authority.summary(node).body
    assert result.lexical == {"default", "body", *(() if future else ("annotation",))}
    assert not result.globals


@pytest.mark.parametrize("depth", [8, 16, 32])
def test_transitive_dependency_scopes_are_summarized_once(depth: int) -> None:
    source = "".join("    " * level + f"def f{level}():\n" for level in range(depth))
    source += "    " * depth + "return watched\n"
    tree = ast.parse(source)
    root = tree.body[0]
    assert isinstance(root, ast.FunctionDef)
    authority = PythonDependencyAuthority(
        eager_annotations=True, future_annotations=False
    )
    assert authority.summary(root).body.lexical == {"watched"}
    assert len(authority.summaries) == depth
    assert authority.declaration_scans == depth
    visits = authority.node_visits
    assert visits <= 12 * depth
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef):
            authority.summary(node)
            authority.declarations(node)
    assert authority.node_visits == visits
    assert authority.declaration_scans == depth


def test_eager_variable_annotation_observes_assigned_value() -> None:
    source = "globals: globals = 0\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    statement = tree.body[0]
    assert isinstance(statement, ast.AnnAssign)
    fact = index.expression_fact(statement.annotation)
    assert fact is not None and fact.binding_is_bound
    assert not identity_fact_may_be(fact.identities, PythonIdentity.BUILTIN_GLOBALS)
    assert not index.module_namespace_may_be_observed(statement)


@pytest.mark.parametrize("header", ["", "(Base)", "(metaclass=Meta)"])
@pytest.mark.parametrize("global_read", [False, True])
def test_class_preparation_callbacks_precede_body_name_reads(
    header: str, global_read: bool
) -> None:
    source = (
        "flag = False\n"
        f"class Subject{header}:\n"
        + ("    global flag\n" if global_read else "")
        + "    observed = flag\n"
    )
    tree = ast.parse(source)
    statement = tree.body[-1]
    assert isinstance(statement, ast.ClassDef)
    read = statement.body[-1]
    assert isinstance(read, ast.Assign)
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(read.value)
    assert fact is not None
    if header:
        assert fact.binding_invalidated
        assert not identity_fact_is_exact(fact.identities, PythonIdentity.STATIC_FALSE)
    else:
        assert identity_fact_is_exact(fact.identities, PythonIdentity.STATIC_FALSE)


def test_prepared_mapping_store_delete_do_not_prove_later_lookup_values() -> None:
    source = (
        "class Subject(metaclass=Meta):\n"
        "    value = 1\n"
        "    before = value\n"
        "    del value\n"
        "    after = value\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    subject = tree.body[0]
    assert isinstance(subject, ast.ClassDef)
    for statement in (subject.body[1], subject.body[3]):
        assert isinstance(statement, ast.Assign)
        fact = index.expression_fact(statement.value)
        assert fact is not None and fact.binding_invalidated
        assert fact.static_value is None
        assert fact.identities & OTHER_IDENTITY
        assert fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON


def test_prepared_namespace_uses_classderef_reads_but_not_deref_writes() -> None:
    scope = python_binding_flow._Scope(
        scope_id=1,
        parent=None,
        kind="class",
        name="Prepared",
        locals=frozenset({"local"}),
        globals=frozenset({"global_name"}),
        nonlocals=frozenset({"cell"}),
        slots={},
        activation_namespace_stable=True,
        dynamic_class_namespace=True,
    )
    assert scope.namespace_can_call("cell")
    assert not scope.namespace_can_call("cell", write=True)
    assert not scope.namespace_can_call("global_name")
    assert not scope.namespace_can_call("global_name", write=True)
    assert scope.namespace_can_call("local")
    assert scope.namespace_can_call("local", write=True)


def test_plain_class_preserves_explicit_module_binding_writes() -> None:
    source = "class Helper:\n    global len\n    len = 1\nresult = len\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    assignment = tree.body[-1]
    assert isinstance(assignment, ast.Assign)
    fact = index.expression_fact(assignment.value)
    assert fact is not None and fact.binding_is_bound
    assert fact.static_value == 1


@pytest.mark.parametrize("class_global", [False, True])
def test_annotation_namespace_owner_distinguishes_current_and_captured_typeparams(
    class_global: bool,
) -> None:
    analyzer = python_binding_flow._Analyzer(PythonBindingPolicy(), "scope-policy")
    declarations = PythonScopeDeclarations
    empty = frozenset()
    source = ast.parse("T = 0\nclass Prepared:\n    def method[T](value: T): pass\n")
    class_node = source.body[1]
    assert isinstance(class_node, ast.ClassDef)
    function_node = class_node.body[0]
    assert isinstance(function_node, ast.FunctionDef)
    annotation_node = function_node.args.args[0].annotation
    assert annotation_node is not None
    module = analyzer._new_scope(
        parent=None,
        source_node=source,
        kind="module",
        name="module",
        declarations=declarations(frozenset({"T"}), empty, empty),
    )
    analyzer.module_scope = module
    subject = analyzer._new_scope(
        parent=module,
        source_node=class_node,
        kind="class",
        name="Prepared",
        declarations=declarations(
            empty, frozenset({"T"}) if class_global else empty, empty
        ),
    )
    subject.dynamic_class_namespace = True
    parameters = analyzer._new_scope(
        parent=subject,
        source_node=function_node,
        kind="annotation",
        name="type parameters",
        declarations=declarations(frozenset({"T"}), empty, empty),
    )
    annotation = analyzer._new_scope(
        parent=parameters,
        source_node=annotation_node,
        kind="annotation",
        name="deferred annotation",
        declarations=declarations(empty, empty, empty),
    )
    assert parameters.class_namespace_owner() is subject
    assert annotation.class_namespace_owner() is subject
    assert not parameters.namespace_can_call("T")
    assert analyzer._slot_for_name(parameters, "T") == parameters.slots["T"]
    assert annotation.namespace_can_call("T") is (not class_global)
    expected_slot = module.slots["T"] if class_global else parameters.slots["T"]
    assert analyzer._slot_for_name(annotation, "T") == expected_slot
    for current in (parameters, annotation):
        assert not current.namespace_can_call("T", write=True)
        assert current.namespace_can_call("probe")
    function = analyzer._new_scope(
        parent=annotation,
        source_node=function_node,
        kind="function",
        name="ordinary function",
        declarations=declarations(empty, empty, empty),
    )
    assert function.class_namespace_owner() is None
    assert not function.namespace_can_call("probe")
    subject.dynamic_class_namespace = False
    assert not annotation.namespace_can_call("probe")
    assert annotation.namespace_can_call("probe", namespace_tainted=True)
    assert not parameters.namespace_can_call("T", namespace_tainted=True)
    assert annotation.namespace_can_call("T", namespace_tainted=True) is (
        not class_global
    )
    assert not annotation.namespace_can_call(
        "probe", write=True, namespace_tainted=True
    )


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("class_global", [False, True])
def test_generic_class_annotation_lookup_obeys_versioned_scope_policy(
    target: tuple[int, int],
    class_global: bool,
) -> None:
    source = (
        "T = False\n"
        "class Prepared(metaclass=Meta):\n"
        + ("    global T\n" if class_global else "")
        + "    def helper[T](value: T, other: probe):\n"
        "        pass\n"
    )
    function = next(
        node
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.FunctionDef) and node.name == "helper"
    )
    index = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(target_python=target),
    )
    value_annotation = function.args.args[0].annotation
    probe_annotation = function.args.args[1].annotation
    assert value_annotation is not None and probe_annotation is not None
    value = index.expression_fact(value_annotation)
    probe = index.expression_fact(probe_annotation)
    assert value is not None and probe is not None
    callback = python_binding_flow.EXECUTES_ARBITRARY_PYTHON
    # Deferred annotations can execute with foreign activation mappings. A
    # class-level global declaration bypasses classderef, not mapping callbacks.
    assert bool(value.effects & callback) is (target >= (3, 14))
    assert probe.effects & callback
    assert probe.static_value is None
    assert value.static_value is None


def test_prepared_class_nonlocal_stores_target_cell_but_reads_can_call_mapping() -> (
    None
):
    source = (
        "def outer():\n"
        "    cell = False\n"
        "    class Prepared(metaclass=Meta):\n"
        "        nonlocal cell\n"
        "        cell = False\n"
        "        observed = cell\n"
        "        del cell\n"
    )
    subject = next(
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ClassDef)
    )
    index = analyze_python_source_bindings(source)
    read = subject.body[2]
    assert isinstance(read, ast.Assign)
    fact = index.expression_fact(read.value)
    assert fact is not None and fact.static_value is None
    assert fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("generic_annotation", [False, True])
def test_initially_plain_class_namespace_key_mutation_enables_lookup_callbacks(
    target: tuple[int, int],
    generic_annotation: bool,
) -> None:
    source = (
        "flag = False\n"
        "class Owner:\n"
        "    global flag\n"
        "    locals()[custom_key] = 'stored'\n"
        "    flag = False\n"
        + (
            "    def helper[T](value: probe):\n        pass\n"
            if generic_annotation
            else "    observed = probe\n"
        )
        + "    after = flag\n"
    )
    tree = ast.parse(source)
    lookup = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and node.id == "probe"
    )
    index = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(target_python=target),
    )
    fact = index.expression_fact(lookup)
    assert fact is not None
    assert fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON
    if not generic_annotation or target < (3, 14):
        subject = tree.body[1]
        assert isinstance(subject, ast.ClassDef)
        after = subject.body[-1]
        assert isinstance(after, ast.Assign)
        after_fact = index.expression_fact(after.value)
        assert after_fact is not None
        assert not identity_fact_is_exact(
            after_fact.identities, PythonIdentity.STATIC_FALSE
        )


@pytest.mark.parametrize(
    ("source", "name", "expected"),
    [
        ("result = len\n", "len", [False]),
        ("from ext import *\nlen\n", "len", [True]),
        ("before = len\nfrom ext import *\nafter = len\n", "len", [False, True]),
        ("array = 1\nfrom ext import *\nresult = array\n", "array", [True]),
        ("from ext import *\narray = 1\nresult = array\n", "array", [True]),
        ("array = 1\ncallback()\nresult = array\n", "array", [True]),
        (
            "array = 1\nif flag:\n    from ext import *\nresult = array\n",
            "array",
            [True],
        ),
        ("from ext import *\ndef f(array):\n    return array\n", "array", [False]),
        (
            "from ext import *\ndef f(array):\n    def g():\n        return array\n    return g\n",
            "array",
            [False],
        ),
        (
            "from ext import *\ndef f():\n    global array\n    return array\n",
            "array",
            [True],
        ),
        ("from ext import *\nresult = [array for array in (1, 2)]\n", "array", [False]),
    ],
)
def test_name_binding_invalidation_is_source_ordered_and_scope_aware(
    source: str, name: str, expected: list[bool]
) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    reads = sorted(
        (
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.Name)
            and isinstance(node.ctx, ast.Load)
            and node.id == name
        ),
        key=lambda node: (node.lineno, node.col_offset),
    )
    facts = [index.expression_fact(node) for node in reads]
    assert all(fact is not None for fact in facts)
    assert [fact.binding_invalidated for fact in facts if fact is not None] == expected


@pytest.mark.parametrize(
    ("source", "bound"),
    [
        ("abs(-1)\n", False),
        ("def f(abs):\n    return abs(-1)\n", True),
        ("def f():\n    abs(-1)\n    abs = replacement\n", True),
        ("abs = replacement\nabs(-1)\n", True),
        ("abs = replacement\ndel abs\nabs(-1)\n", True),
        ("abs = 0\ndel abs\nabs(-1)\n", False),
    ],
)
def test_builtin_shadowing_uses_binding_authority(source: str, bound: bool) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    fact = index.expression_fact(call.func)
    assert fact is not None
    assert fact.binding_is_bound is bound


@pytest.mark.parametrize("scope_kind", ["module", "class"])
@pytest.mark.parametrize(
    ("name", "assigned", "builtin_identity"),
    [
        ("len", "'shadow'", PythonIdentity.BUILTIN_LEN),
        ("unknown_builtin_name", "()", None),
    ],
)
def test_partial_namespace_binding_projects_builtin_fallback_on_normal_load(
    scope_kind: str,
    name: str,
    assigned: str,
    builtin_identity: PythonIdentity | None,
) -> None:
    body = f"if flag is None:\n    {name} = {assigned}\nresult = {name}\n"
    source = (
        body
        if scope_kind == "module"
        else "class Owner:\n" + "".join(f"    {line}\n" for line in body.splitlines())
    )
    tree = ast.parse(source)
    owner = tree.body[0] if scope_kind == "class" else None
    assignment = owner.body[-1] if isinstance(owner, ast.ClassDef) else tree.body[-1]
    assert isinstance(assignment, ast.Assign)
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(assignment.value)
    assert fact is not None
    assert index.expression_result(assignment.value) == UNKNOWN_EXPRESSION_RESULT
    if builtin_identity is None:
        assert fact.identities & OTHER_IDENTITY
        assert fact.identities & int(PythonIdentity.UNBOUND)
    else:
        assert fact.identities & int(builtin_identity)
        assert not fact.identities & int(PythonIdentity.UNBOUND)


@pytest.mark.parametrize(
    ("source", "foreign_cell"),
    [
        (
            "def read(flag):\n"
            "    if flag is None:\n"
            "        value = ()\n"
            "    return value\n",
            False,
        ),
        (
            "def outer(flag):\n"
            "    if flag is None:\n"
            "        value = ()\n"
            "    def read():\n"
            "        return value\n"
            "    return read\n",
            True,
        ),
        (
            "def outer(flag):\n"
            "    if flag is None:\n"
            "        value = ()\n"
            "    def read():\n"
            "        nonlocal value\n"
            "        return value\n"
            "    return read\n",
            True,
        ),
        (
            "def read(flag):\n"
            "    if flag is None:\n"
            "        unknown_builtin_name = ()\n"
            "    return unknown_builtin_name\n",
            False,
        ),
    ],
)
def test_partial_lexical_cell_absence_raises_without_builtin_fallback(
    source: str,
    foreign_cell: bool,
) -> None:
    tree = ast.parse(source)
    returned = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Return)
        and isinstance(node.value, ast.Name)
        and node.value.id in {"value", "unknown_builtin_name"}
    )
    assert isinstance(returned.value, ast.Name)
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(returned.value)
    assert fact is not None
    result = index.expression_result(returned.value)
    assert fact.identities & int(PythonIdentity.UNBOUND)
    assert fact.name_lookup == "lexical"
    if foreign_cell:
        # FunctionType can supply an arbitrary compatible closure cell; the
        # lexical slot still must not fall through to builtin lookup.
        assert result == UNKNOWN_EXPRESSION_RESULT
        assert fact.identities & OTHER_IDENTITY
    else:
        assert result.kind == "tuple"
        assert result.items == ()
        assert not result.release_may_call
        # Coarse identity alternatives include OTHER for inert values. Normal
        # result shape remains exact; absence is a lexical exception, not a
        # callback-invalidated namespace lookup or builtin fallback.
        assert not fact.binding_invalidated


def test_class_preparation_and_nested_activation_widen_captured_payload() -> None:
    source = (
        "def outer():\n"
        "    value = ()\n"
        "    direct = value\n"
        "    class Inline:\n"
        "        observed = value\n"
        "    def nested():\n"
        "        return value\n"
        "    return direct, Inline, nested\n"
    )
    tree = ast.parse(source)
    reads = sorted(
        (
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.Name)
            and isinstance(node.ctx, ast.Load)
            and node.id == "value"
        ),
        key=lambda node: node.lineno,
    )
    assert len(reads) == 3
    index = analyze_python_source_bindings(source)
    direct_result, inline_result, nested_result = (
        index.expression_result(read) for read in reads
    )
    assert direct_result.kind == "tuple"
    assert direct_result.items == ()
    assert not direct_result.release_may_call
    # Foreign __build_class__/metaclass preparation can supply a mapping whose
    # value takes precedence over the enclosing activation's closure cell.
    assert inline_result == UNKNOWN_EXPRESSION_RESULT
    inline_fact = index.expression_fact(reads[1])
    assert inline_fact is not None and inline_fact.name_lookup == "class_lexical"
    assert nested_result == UNKNOWN_EXPRESSION_RESULT
    nested_fact = index.expression_fact(reads[2])
    assert nested_fact is not None
    assert nested_fact.identities & OTHER_IDENTITY
    # FunctionType may also supply a compatible empty cell, so a free read
    # retains its NameError path even when the source-created closure was bound.
    assert nested_fact.identities & int(PythonIdentity.UNBOUND)

    import builtins
    from types import FunctionType

    class PreparedNamespace(type):
        @classmethod
        def __prepare__(metaclass, name, bases):
            return {"value": "prepared namespace"}

    def build_class(body, name):
        return builtins.__build_class__(body, name, metaclass=PreparedNamespace)

    namespace = {}
    exec(source, namespace)
    rebound = FunctionType(
        namespace["outer"].__code__,
        {"__name__": "probe", "__builtins__": {"__build_class__": build_class}},
    )
    direct, inline, nested = rebound()
    assert direct == ()
    assert inline.observed == "prepared namespace"
    assert nested() == ()


@pytest.mark.parametrize(
    "expression", ["(value for _ in value)", "[value for _ in value]"]
)
def test_comprehension_activation_separates_first_iterator_from_captured_body(
    expression: str,
) -> None:
    source = f"def outer():\n    value = (None,)\n    return {expression}\n"
    tree = ast.parse(source)
    comprehension = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, (ast.GeneratorExp, ast.ListComp))
    )
    index = analyze_python_source_bindings(source)
    first = index.expression_result(comprehension.generators[0].iter)
    assert first.kind == "tuple" and first.length == 1
    body = index.expression_result(comprehension.elt)
    fact = index.expression_fact(comprehension.elt)
    assert fact is not None
    if isinstance(comprehension, ast.GeneratorExp):
        assert body == UNKNOWN_EXPRESSION_RESULT
        assert fact.identities & OTHER_IDENTITY
        assert fact.identities & int(PythonIdentity.UNBOUND)
    else:
        assert body == first
        assert not fact.binding_invalidated


def test_generator_target_is_current_activation_local_not_foreign_cell() -> None:
    source = "def outer():\n    value = ()\n    return (value for value in (None,))\n"
    tree = ast.parse(source)
    comprehension = next(
        node for node in ast.walk(tree) if isinstance(node, ast.GeneratorExp)
    )
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(comprehension.elt)
    assert fact is not None and fact.binding_is_bound
    assert not fact.identities & int(PythonIdentity.UNBOUND)
    assert not fact.binding_invalidated


def test_nonlocal_write_releases_foreign_cell_before_reading_its_new_value() -> None:
    source = (
        "def outer():\n"
        "    value = foreign\n"
        "    def nested():\n"
        "        nonlocal value\n"
        "        value = ()\n"
        "        return value\n"
        "    return nested\n"
    )
    tree = ast.parse(source)
    returned = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Return)
        and isinstance(node.value, ast.Name)
        and node.value.id == "value"
    )
    assert isinstance(returned.value, ast.Name)
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(returned.value)
    result = index.expression_result(returned.value)
    assert fact is not None
    assert fact.identities & OTHER_IDENTITY
    assert fact.binding_invalidated
    assert result == UNKNOWN_EXPRESSION_RESULT

    # STORE_DEREF publishes first; releasing the arbitrary old cell value can
    # overwrite it again. A strong write is not a callback-free lifetime proof.
    namespace = {"foreign": None}
    exec(source, namespace)
    nested = namespace["outer"]()
    cell = nested.__closure__[0]

    class ReplaceOnRelease:
        def __del__(self):
            cell.cell_contents = "reentered"

    cell.cell_contents = ReplaceOnRelease()
    assert nested() == "reentered"


@pytest.mark.parametrize("branch_count", [2, 3, 4])
def test_conditional_binding_join_preserves_clean_bound_and_pristine_unbound_paths(
    branch_count: int,
) -> None:
    pool = python_binding_flow._StatePool()
    pool.set_taint_domain(1)
    branches = [0]
    for _ in range(branch_count - 1):
        tainted = pool.taint_module_bindings(branches[-1])
        branches.append(pool.set_binding(tainted, 0, int(PythonIdentity.USER_FUNCTION)))
    joined = pool.join(*branches)
    assert pool.binding(joined, 0) == int(
        PythonIdentity.USER_FUNCTION | PythonIdentity.UNBOUND
    )
    assert pool._binding_resolution(joined, 0).clean
    tainted_unbound = pool.taint_module_bindings(0)
    unsafe_join = pool.join(*branches[1:], tainted_unbound)
    assert not pool._binding_resolution(unsafe_join, 0).clean


@pytest.mark.parametrize("branch_count", [2, 4])
def test_binding_result_join_treats_absence_as_no_normal_value(
    branch_count: int,
) -> None:
    pool = python_binding_flow._StatePool()
    exact = StaticExpressionResult.scalar("stable")
    bound = pool.set_binding(
        0,
        0,
        int(PythonIdentity.USER_FUNCTION),
        static_value="stable",
        result=exact,
    )
    absent = [0]
    for slot in range(1, branch_count - 1):
        absent.append(
            pool.set_binding(
                0,
                slot,
                int(PythonIdentity.INERT_VALUE),
                result=StaticExpressionResult.scalar(slot),
            )
        )

    joined = pool.join(bound, *absent)
    assert pool.binding(joined, 0) == int(
        PythonIdentity.USER_FUNCTION | PythonIdentity.UNBOUND
    )
    assert pool.static_value(joined, 0) == "stable"
    assert pool.result(joined, 0) == exact


@pytest.mark.parametrize("branch_count", [2, 4])
def test_binding_result_join_keeps_bound_unknown_alternatives_widening(
    branch_count: int,
) -> None:
    pool = python_binding_flow._StatePool()
    exact = StaticExpressionResult.scalar("stable")
    bound = pool.set_binding(
        0,
        0,
        int(PythonIdentity.USER_FUNCTION),
        static_value="stable",
        result=exact,
    )
    unknown = pool.set_binding(0, 0, OTHER_IDENTITY)
    branches = [bound, unknown]
    for slot in range(1, branch_count - 1):
        branches.append(
            pool.set_binding(
                0,
                slot,
                int(PythonIdentity.INERT_VALUE),
                result=StaticExpressionResult.scalar(slot),
            )
        )

    joined = pool.join(*branches)
    assert pool.static_value(joined, 0) is None
    assert pool.result(joined, 0) == UNKNOWN_EXPRESSION_RESULT


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("array = 0\nif flag:\n    result = array\n", [True]),
        ("array = 0\nif flag is not None:\n    result = array\n", [False]),
        ("array = 0\nresult = array if flag else 1\n", [True]),
        ("array = 0\nresult = flag and array\n", [True]),
        ("array = 0\nresult = flag or array\n", [True]),
        ("array = 0\nwhile flag:\n    result = array\n    break\n", [True]),
        ("array = 0\nassert flag, array\n", [True]),
        ("array = 0\nresult = [array for item in (1,) if flag]\n", [True]),
        (
            "array = 0\nmatch subject:\n    case _ if flag:\n        result = array\n",
            [True],
        ),
        ("if flag:\n    array = 1\nelse:\n    array = 2\nresult = array\n", [True]),
        ("result = (array := 1) if flag else (array := 2)\nresult = array\n", [True]),
        (
            "array = 0\nif flag is None:\n    array = 1\nelse:\n    array = 2\nresult = array\n",
            [False],
        ),
    ],
)
def test_truth_callbacks_precede_branch_consumers_and_not_clean_rebindings(
    source: str, expected: list[bool]
) -> None:
    index = analyze_python_source_bindings(source)
    reads = sorted(
        (
            node
            for node in ast.walk(ast.parse(source))
            if isinstance(node, ast.Name)
            and isinstance(node.ctx, ast.Load)
            and node.id == "array"
        ),
        key=lambda node: (node.lineno, node.col_offset),
    )
    facts = [index.expression_fact(node) for node in reads]
    assert all(fact is not None for fact in facts)
    assert [fact.binding_invalidated for fact in facts if fact is not None] == expected


def test_synthetic_node_key_cache_retains_identity_against_id_reuse() -> None:
    analyzer = python_binding_flow._Analyzer(PythonBindingPolicy(), "synthetic")
    first = ast.Name(id="first", lineno=1, col_offset=0)
    first_identity = id(first)
    first_key = analyzer._node_key(first)
    retained = weakref.ref(first)

    del first
    gc.collect()

    retained_node = retained()
    assert retained_node is not None
    assert retained_node in analyzer._node_keys
    second = ast.Name(id="second", lineno=2, col_offset=0)
    assert id(second) != first_identity
    assert analyzer._node_key(second) != first_key


def test_persistent_binding_tree_grows_and_joins_across_radix_boundaries() -> None:
    pool = python_binding_flow._StatePool()
    chunks_per_fixed_depth = 1 << (python_binding_flow._BINDING_TREE_SHIFT * 3)
    chunk_size = python_binding_flow._BINDING_CHUNK_SIZE
    below = (chunks_per_fixed_depth - 1) * chunk_size
    boundary = chunks_per_fixed_depth * chunk_size
    above = (chunks_per_fixed_depth + 1) * chunk_size
    import_module = int(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    dunder_import = int(PythonIdentity.BUILTINS_IMPORT)

    left = pool.set_binding(0, below, import_module)
    left = pool.set_binding(left, boundary, dunder_import)
    right = pool.set_binding(0, above, import_module)
    joined = pool.join(left, right)

    assert pool.binding(left, below) == import_module
    assert pool.binding(left, boundary) == dunder_import
    assert pool.binding(right, above) == import_module
    assert pool.binding(joined, below) & import_module
    assert pool.binding(joined, boundary) & dunder_import
    assert pool.binding(joined, above) & import_module


def test_binding_history_diff_visits_only_changed_trie_branches() -> None:
    pool = python_binding_flow._StatePool()
    chunk_size = python_binding_flow._BINDING_CHUNK_SIZE
    far_chunk = (1 << (python_binding_flow._BINDING_TREE_SHIFT * 4)) - 1
    far_slot = far_chunk * chunk_size
    import_module = int(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    dunder_import = int(PythonIdentity.BUILTINS_IMPORT)
    base = pool.set_binding(0, 0, import_module)
    base = pool.set_binding(base, far_slot, import_module)
    left = pool.set_binding(base, 0, dunder_import)
    right = pool.set_binding(base, far_slot, dunder_import)
    joined = pool.join(left, right)

    assert pool.changed_slots_between(base, joined) == (0, far_slot)
    assert pool.structural_diff_shared_skips > 0
    assert pool.structural_diff_node_visits < far_chunk.bit_length() * 4


def _identity_on_line(source: str, line: int, identity: PythonIdentity) -> bool:
    index = analyze_python_source_bindings(source)
    return any(
        fact.node.lineno == line and identity_fact_is_exact(fact.identities, identity)
        for fact in index.expressions
    )


def test_alias_chain_preserves_exact_import_module_identity() -> None:
    call = _last_call(
        "import importlib as loader\nload = loader.import_module\nload('pkg.leaf')\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert not call.callee_identities & OTHER_IDENTITY


@pytest.mark.parametrize(
    "source",
    (
        "def deferred():\n"
        "    load('pkg.module_late')\n"
        "from importlib import import_module as load\n",
        "def outer():\n"
        "    def deferred():\n"
        "        load('pkg.enclosing_late')\n"
        "    from importlib import import_module as load\n",
        "def outer():\n"
        "    class DeferredOwner:\n"
        "        def load_later(self):\n"
        "            load('pkg.class_enclosing_late')\n"
        "    from importlib import import_module as load\n",
        "from importlib import import_module as load\n"
        "def outer():\n"
        "    load = print\n"
        "    def deferred():\n"
        "        global load\n"
        "        load('pkg.global')\n",
    ),
)
def test_deferred_import_aliases_use_future_canonical_scope_state(
    source: str,
) -> None:
    call = _last_call(source)
    assert call.possible_import_call_kinds() == ("import_module",)


def test_local_parameter_does_not_inherit_outer_import_identity() -> None:
    call = _last_call(
        "from importlib import import_module as load\n"
        "def deferred(load):\n"
        "    load('pkg.not_imported')\n"
    )
    assert call.possible_import_call_kinds() == ()


@pytest.mark.parametrize(
    "source",
    [
        "import importlib.util as util\nutil.find_spec('pkg.leaf')\n",
        "import importlib\nimportlib.util.find_spec('pkg.leaf')\n",
        "from importlib.util import find_spec as find\nfind('pkg.leaf')\n",
    ],
)
def test_find_spec_forms_share_exact_identity(source: str) -> None:
    call = _last_call(source)
    assert call.callee_is(PythonIdentity.IMPORTLIB_FIND_SPEC)


def test_find_spec_member_rebinding_invalidates_all_aliases() -> None:
    call = _last_call(
        "import importlib.util as util\n"
        "alias = util\n"
        "alias.find_spec = replacement\n"
        "util.find_spec('pkg.leaf')\n"
    )
    assert call.callee_identities == OTHER_IDENTITY
    assert call.definitely_invalidated_members_after & int(PythonMember.UTIL_FIND_SPEC)


def test_intrinsic_require_alias_has_one_exact_binding_identity() -> None:
    index = analyze_python_source_bindings(
        "from _intrinsics import require_intrinsic as require\n"
        "require('molt_demo', globals())\n"
    )
    call = next(fact for fact in index.calls if fact.node.col_offset == 0)
    assert call.callee_is(PythonIdentity.INTRINSICS_REQUIRE)


@pytest.mark.parametrize(
    "conditional",
    [
        "if flag:\n    importlib = replacement\n",
        "for item in items:\n    importlib = replacement\n",
        "while flag:\n    importlib = replacement\n",
        "try:\n    importlib = replacement\nexcept Exception:\n    pass\n",
        "match token:\n    case 1:\n        importlib = replacement\n",
    ],
)
def test_control_flow_join_retains_both_canonical_and_shadowed_identity(
    conditional: str,
) -> None:
    call = _last_call(
        f"import importlib\n{conditional}importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    assert not call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


def test_statically_dead_branch_does_not_manufacture_possible_shadow() -> None:
    call = _last_call(
        "import importlib\n"
        "if False:\n"
        "    importlib = replacement\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    "expression",
    [
        "False and importlib.import_module('pkg.dead')",
        "True or importlib.import_module('pkg.dead')",
    ],
)
def test_boolean_short_circuit_does_not_index_dead_call(expression: str) -> None:
    index = analyze_python_source_bindings(f"import importlib\n{expression}\n")
    assert index.calls == ()


def test_unconditional_member_rebinding_removes_canonical_identity() -> None:
    call = _last_call(
        "import importlib\n"
        "importlib.import_module = replacement\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_identities == OTHER_IDENTITY
    assert call.definitely_invalidated_members_after & int(
        PythonMember.IMPORTLIB_IMPORT_MODULE
    )


def test_conditional_member_rebinding_is_possible_not_definite() -> None:
    call = _last_call(
        "import importlib\n"
        "if flag:\n"
        "    importlib.import_module = replacement\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    assert call.maybe_invalidated_members_after & int(
        PythonMember.IMPORTLIB_IMPORT_MODULE
    )
    assert not call.definitely_invalidated_members_after & int(
        PythonMember.IMPORTLIB_IMPORT_MODULE
    )


def test_function_parameter_shadows_outer_importlib_for_whole_scope() -> None:
    call = _last_call(
        "import importlib\n"
        "def load(importlib):\n"
        "    return importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_identities == OTHER_IDENTITY


def test_deferred_function_retains_import_dependency_not_activation_identity() -> None:
    call = _last_call(
        "import importlib\n"
        "def load():\n"
        "    return importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    immediate = _last_call("import importlib\nimportlib.import_module('pkg.leaf')\n")
    assert immediate.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("receiver().attribute += rhs()\n", ["receiver", "rhs"]),
        ("receiver()[index()] += rhs()\n", ["receiver", "index", "rhs"]),
    ],
)
def test_augmented_member_assignment_evaluates_target_once(
    source: str, expected: list[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    observed: list[str] = []
    original = python_binding_flow._Analyzer.eval_expr

    def record(
        analyzer: python_binding_flow._Analyzer,
        node: ast.expr,
        state_id: int,
        scope: python_binding_flow._Scope,
    ) -> python_binding_flow._ExpressionResult:
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
            observed.append(node.func.id)
        return original(analyzer, node, state_id, scope)

    monkeypatch.setattr(python_binding_flow._Analyzer, "eval_expr", record)
    analyzer = python_binding_flow._Analyzer(PythonBindingPolicy(), "target-custody")
    analyzer.analyze(ast.parse(source))
    assert observed == expected


def test_nested_closure_retains_import_dependency_not_foreign_import_identity() -> None:
    call = _last_call(
        "def outer():\n"
        "    import importlib\n"
        "    def load():\n"
        "        return importlib.import_module('pkg.leaf')\n"
        "    return load\n"
    )
    # The closure retains the acquired object, but the outer function's import
    # hook belongs to its activation and need not return canonical importlib.
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_global_binding_obeys_source_order_inside_function() -> None:
    call = _last_call(
        "import importlib\n"
        "def load(flag):\n"
        "    global importlib\n"
        "    if flag:\n"
        "        importlib = replacement\n"
        "    return importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_nonlocal_binding_obeys_source_order_inside_nested_function() -> None:
    call = _last_call(
        "def outer():\n"
        "    import importlib\n"
        "    def load(flag):\n"
        "        nonlocal importlib\n"
        "        if flag:\n"
        "            importlib = replacement\n"
        "        return importlib.import_module('pkg.leaf')\n"
        "    return load\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_module_spec_alias_and_constructor_result_are_tracked() -> None:
    call = _last_call(
        "from importlib.machinery import ModuleSpec as Spec\n"
        "Alias = Spec\n"
        "value = Alias('pkg.leaf', None)\n"
    )
    assert call.callee_is(PythonIdentity.MODULE_SPEC_CLASS)
    assert identity_fact_is_exact(
        call.result_identities, PythonIdentity.MODULE_SPEC_INSTANCE
    )


def test_module_spec_member_mutation_invalidates_all_aliases() -> None:
    call = _last_call(
        "import importlib.machinery as machinery\n"
        "alias = machinery\n"
        "alias.ModuleSpec = replacement\n"
        "value = machinery.ModuleSpec('pkg.leaf', None)\n"
    )
    assert call.callee_identities == OTHER_IDENTITY
    assert call.definitely_invalidated_members_after & int(
        PythonMember.MACHINERY_MODULE_SPEC
    )


def test_globals_module_and_frame_capabilities_share_exact_identities() -> None:
    source = (
        "import sys\n"
        "import inspect\n"
        "global_map = globals()\n"
        "module = sys.modules[__name__]\n"
        "frame = inspect.currentframe()\n"
        "frame_globals = frame.f_globals\n"
        "def local():\n"
        "    pass\n"
        "function_globals = local.__globals__\n"
    )
    assert _identity_on_line(source, 3, PythonIdentity.CURRENT_GLOBALS)
    assert _identity_on_line(source, 4, PythonIdentity.CURRENT_MODULE)
    assert _identity_on_line(source, 5, PythonIdentity.CURRENT_FRAME)
    assert _identity_on_line(source, 6, PythonIdentity.CURRENT_GLOBALS)
    assert _identity_on_line(source, 9, PythonIdentity.CURRENT_GLOBALS)


def test_setattr_and_exec_invalidate_import_identity_without_losing_possibility() -> (
    None
):
    setattr_call = _last_call(
        "import importlib\n"
        "setattr(importlib, 'import_module', replacement)\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert setattr_call.callee_identities == OTHER_IDENTITY

    exec_call = _last_call(
        "import importlib\nexec(source)\nimportlib.import_module('pkg.leaf')\n"
    )
    assert exec_call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert exec_call.callee_identities & OTHER_IDENTITY


@pytest.mark.parametrize(
    "mutation",
    [
        "globals()['importlib'] = replacement\n",
        "vars()['importlib'] = replacement\n",
        (
            "import inspect\n"
            "inspect.currentframe().f_globals['importlib'] = replacement\n"
        ),
    ],
)
def test_global_and_frame_reflection_uses_definite_replacement(mutation: str) -> None:
    call = _last_call(
        f"import importlib\n{mutation}importlib.import_module('pkg.leaf')\n"
    )
    direct = _last_call(
        "import importlib\nimportlib = replacement\nimportlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_identities == direct.callee_identities == OTHER_IDENTITY


def test_builtins_import_alias_and_member_mutation_share_one_authority() -> None:
    exact = _last_call("from builtins import __import__ as load\nload('pkg.leaf')\n")
    assert exact.callee_is(PythonIdentity.BUILTINS_IMPORT)

    mutated = _last_call(
        "import builtins\nbuiltins.__import__ = replacement\n__import__('pkg.leaf')\n"
    )
    assert mutated.callee_identities == OTHER_IDENTITY


def test_import_hook_mutation_downgrades_later_standard_import_to_possible() -> None:
    call = _last_call(
        "import sys\n"
        "sys.meta_path = hooks\n"
        "import importlib\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    assert call.maybe_invalidated_members_after & int(PythonMember.IMPORT_HOOKS)


def test_reference_release_callbacks_poison_exposed_global_bindings() -> None:
    call = _last_call(
        "import importlib\n"
        "owned = arbitrary\n"
        "owned = 1\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_import_calls_fail_preserves_import_state_capability() -> None:
    call = _last_call(
        "from importlib import import_module\nimport_module('pkg.leaf')\n"
    )
    assert not effect_mask_satisfies_capability(
        call.effects, PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS
    )


def test_noncanonical_import_policy_exposes_possible_identity() -> None:
    index = analyze_python_source_bindings(
        "import importlib\nimportlib.import_module('pkg.leaf')\n",
        policy=PythonBindingPolicy(standard_imports_are_canonical=False),
    )
    call = index.calls[-1]
    assert identity_fact_may_be(
        call.callee_identities, PythonIdentity.IMPORTLIB_IMPORT_MODULE
    )
    assert call.callee_identities & OTHER_IDENTITY


def test_target_python_gates_eager_annotation_effects() -> None:
    source = (
        "from importlib import import_module\n"
        "value: import_module('pkg.annotation') = None\n"
    )
    eager = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=(3, 13))
    )
    deferred = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=(3, 14))
    )
    assert len(eager.calls) == 1
    assert len(deferred.calls) == 1
    assert eager.scopes[eager.calls[0].scope_id].kind == "module"
    assert deferred.scopes[deferred.calls[0].scope_id].kind == "annotation"
    statement = ast.parse(source).body[-1]
    assert eager.module_namespace_may_be_observed(statement)
    assert not deferred.module_namespace_may_be_observed(statement)


def test_content_cache_is_single_flight_and_filename_independent() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"

    def analyze(index: int):
        return analyze_python_source_bindings(source, filename=f"module_{index}.py")

    with ThreadPoolExecutor(max_workers=8) as executor:
        indexes = list(executor.map(analyze, range(32)))
    assert all(index is indexes[0] for index in indexes)


def test_binding_cache_evicts_fifo_in_constant_time_authority() -> None:
    cache = python_binding_flow._BindingIndexCache(max_entries=2)
    indexes = {
        name: python_binding_flow.analyze_python_bindings(
            ast.parse(f"value = {ordinal}\n"),
            source_digest=name,
        )
        for ordinal, name in enumerate(("a", "b", "c"))
    }
    calls: Counter[str] = Counter()

    def fetch(name: str):
        def compute():
            calls[name] += 1
            return indexes[name]

        return cache.get_or_compute((name,), compute)

    assert fetch("a") is indexes["a"]
    assert fetch("b") is indexes["b"]
    assert fetch("c") is indexes["c"]
    assert fetch("b") is indexes["b"]
    assert fetch("a") is indexes["a"]
    assert calls == Counter(a=2, b=1, c=1)


def test_loop_fixpoint_does_not_conflate_identical_storage_with_tainted_binding() -> (
    None
):
    states = python_binding_flow._StatePool()
    exact = states.set_binding(0, 0, int(PythonIdentity.IMPORTLIB_MODULE))
    tainted = states.taint_slots(exact, 1)
    loop_header = states.join(exact, tainted)

    assert states.binding(exact, 0) == int(PythonIdentity.IMPORTLIB_MODULE)
    assert states.binding(loop_header, 0) & OTHER_IDENTITY
    assert not states.equivalent(exact, loop_header)


def test_binding_cache_single_flight_exception_wakes_waiters_and_recovers() -> None:
    cache = python_binding_flow._BindingIndexCache(max_entries=2)
    index = python_binding_flow.analyze_python_bindings(
        ast.parse("value = 1\n"),
        source_digest="recovered",
    )
    entered = Event()
    release = Event()
    waiter_started = Event()
    waiter_returned = Event()
    attempts = 0

    def compute():
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            entered.set()
            assert release.wait(5)
            raise ValueError("first analysis failed")
        return index

    def fetch():
        return cache.get_or_compute(("shared",), compute)

    def wait_for_shared_result():
        waiter_started.set()
        try:
            return fetch()
        finally:
            waiter_returned.set()

    with ThreadPoolExecutor(max_workers=2) as executor:
        owner = executor.submit(fetch)
        assert entered.wait(5)
        waiter = executor.submit(wait_for_shared_result)
        assert waiter_started.wait(5)
        assert not waiter_returned.wait(0.05)
        release.set()
        failures: list[ValueError] = []
        for future in (owner, waiter):
            with pytest.raises(ValueError, match="first analysis failed") as caught:
                future.result()
            failures.append(caught.value)

    assert failures[0] is not failures[1]
    assert failures[0].__traceback__ is not failures[1].__traceback__

    assert attempts == 1
    assert fetch() is index
    assert attempts == 2


def test_policy_context_is_part_of_cache_and_index_identity() -> None:
    source = "import importlib\n"
    linux = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(
            target_sys_platform="linux",
            module_name="pkg.mod",
            module_spec_name="pkg.mod",
            module_is_package=False,
            module_execution_kind="imported",
        ),
    )
    windows = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(
            target_sys_platform="win32",
            module_name="pkg.mod",
            module_spec_name="pkg.mod",
            module_is_package=False,
            module_execution_kind="imported",
        ),
    )

    assert linux is not windows
    assert linux.target_sys_platform == "linux"
    assert windows.target_sys_platform == "win32"
    assert linux.module_name == "pkg.mod"
    assert linux.module_execution_kind == "imported"


def test_reparse_query_uses_stable_source_keys_not_ast_identity() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"
    index = analyze_python_source_bindings(source)
    reparsed_call = next(
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.Call)
    )
    fact = index.call_fact(reparsed_call)
    assert fact is not None
    assert fact.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    reparsed_callee = reparsed_call.func
    callee_fact = index.expression_fact(reparsed_callee)
    assert callee_fact is not None
    assert identity_fact_is_exact(
        callee_fact.identities,
        PythonIdentity.IMPORTLIB_IMPORT_MODULE,
    )


def test_ast_cache_identity_includes_spans_used_by_fact_lookup() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"
    shifted_source = "\n    \n" + source
    tree = ast.parse(source)
    shifted_tree = ast.parse(shifted_source)

    assert python_binding_flow.python_ast_digest(
        tree
    ) != python_binding_flow.python_ast_digest(shifted_tree)
    index = python_binding_flow.analyze_python_bindings(
        tree, source_digest=python_binding_flow.python_ast_digest(tree)
    )
    shifted_index = python_binding_flow.analyze_python_bindings(
        shifted_tree,
        source_digest=python_binding_flow.python_ast_digest(shifted_tree),
    )
    call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    shifted_call = next(
        node for node in ast.walk(shifted_tree) if isinstance(node, ast.Call)
    )

    assert index is not shifted_index
    assert index.call_fact(call) is not None
    assert shifted_index.call_fact(shifted_call) is not None
    assert index.call_fact(shifted_call) is None


def test_default_parameter_retains_possible_dunder_import_identity() -> None:
    source = (
        "def load(name, importer=__import__):\n"
        "    return importer(name)\n"
        "load('pkg.leaf')\n"
    )
    index = analyze_python_source_bindings(source)
    importer_call = next(fact for fact in index.calls if fact.node.lineno == 2)

    assert importer_call.callee_may_be(PythonIdentity.BUILTINS_IMPORT)


@pytest.mark.parametrize(
    "source",
    [
        "import importlib\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    importlib = replacement\nimportlib.import_module('live')\n",
        "import importlib\nimport typing as t\nif t.TYPE_CHECKING:\n    importlib = replacement\nimportlib.import_module('live')\n",
        "import importlib\nimport typing_extensions as t\nif t.TYPE_CHECKING:\n    importlib = replacement\nimportlib.import_module('live')\n",
    ],
)
def test_type_checking_dead_branches_share_binding_authority(source: str) -> None:
    call = _last_call(source)
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    "test",
    ["[*()]", "(*(),)", "{*()}", "{**{}}", "2 == True", "[1] == True", "1 is True"],
)
def test_shared_literal_result_drives_source_ordered_branch_bindings(test: str):
    source = f"if {test}:\n    from dead import *\nelse:\n    import importlib\nimportlib.import_module('live')\n"
    call = _last_call(source)
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    "source",
    [
        "from typing import TYPE_CHECKING\nmutate()\nif TYPE_CHECKING:\n    pass\n",
        "import typing\nimport mutator\nif typing.TYPE_CHECKING:\n    pass\n",
        "import sys\nsys.platform = replacement\nif sys.platform == 'win32':\n    pass\n",
        "def method(TYPE_CHECKING):\n    if TYPE_CHECKING:\n        return 1\n",
        "import sys\ndef method(sys):\n    if sys.platform == 'win32':\n        return 1\n",
        "if sys.platform == 'win32':\n    pass\nimport sys\n",
        "class C:\n    TYPE_CHECKING = False\n    def method(self):\n        if TYPE_CHECKING:\n            return 1\n",
    ],
)
def test_reentrant_unbound_and_lexical_names_never_become_static_gates(source: str):
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_sys_platform="win32")
    )
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.If):
            assert index.expression_result(node.test).truth is None


def test_class_load_name_uses_module_fallback_before_local_assignment():
    source = "from typing import TYPE_CHECKING\nclass C:\n    if TYPE_CHECKING:\n        pass\n    TYPE_CHECKING = True\n"
    index = analyze_python_source_bindings(source)
    node = next(
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.If)
    )
    assert index.expression_result(node.test).truth is False


def test_deferred_annotation_class_namespace_is_not_a_captured_constant():
    source = "class C:\n    flag = False\n    type Alias = int if flag else str\nC.flag = True\n"
    index = analyze_python_source_bindings(source)
    test = next(
        node.test for node in ast.walk(ast.parse(source)) if isinstance(node, ast.IfExp)
    )
    assert index.expression_result(test).truth is None


@pytest.mark.parametrize(
    "display",
    ["{key: 1}", "{key}", "{**mapping}", "{key: 1, **{}, 2: 3}", "{key, *(), 3}"],
)
def test_container_callback_boundaries_invalidate_following_import_specialization(
    display: str,
):
    call = _last_call(
        "import importlib\n" + display + "\nimportlib.import_module('live')\n"
    )
    assert not call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


def test_mapping_unpack_invalidates_before_later_display_expressions():
    call = _last_call(
        "import importlib\nvalue = {**mapping, 0: importlib.import_module('live')}\n"
    )
    assert not call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    "expression",
    [
        "f(**{key: 0}, set=(x := 1), **{'seen': x})",
        "f(**{key: 0}, **{'set': (x := 1)}, **{'seen': x})",
        "{key: 0, **{}, 'set': (x := 1), **{'seen': x}}",
        "{key: 0, **{'set': (x := 1)}, **{'seen': x}}",
        "{key, *(), (x := 1), *(x,)}",
        "{key, *((x := 1),), *(x,)}",
    ],
)
def test_accumulated_keys_invalidate_after_later_insertion(expression: str) -> None:
    source = f"x = 0\nresult = {expression}\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    reads = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name)
        and node.id == "x"
        and isinstance(node.ctx, ast.Load)
    ]
    assert len(reads) == 1
    fact = index.expression_fact(reads[0])
    assert fact is not None and fact.binding_invalidated


@pytest.mark.parametrize(
    "expression, collision",
    [
        ("f(**{key: 0}, set=(x := 1), **{'seen': x})", "'set'"),
        ("f(**{key: 0}, **{'set': (x := 1)}, **{'seen': x})", "'set'"),
        ("{key: 0, **{}, 'set': (x := 1), **{'seen': x}}", "'set'"),
        ("{key, *(), (x := 1), *(x,)}", "1"),
    ],
)
def test_existing_key_collision_oracle_rebinds_before_next_operand(
    expression: str, collision: str
) -> None:
    namespace: dict[str, object] = {}
    exec(
        "class Key(str):\n"
        f"    def __hash__(self): return hash({collision})\n"
        "    def __eq__(self, other):\n"
        "        global x\n"
        "        x = 2\n"
        "        return False\n"
        "def f(**kwargs): return kwargs\n"
        "key = Key('other')\n"
        "x = 0\n"
        f"result = {expression}\n",
        namespace,
    )
    result = namespace["result"]
    assert namespace["x"] == 2
    if isinstance(result, dict):
        assert result["seen"] == 2
    else:
        assert isinstance(result, set) and 2 in result


@pytest.mark.parametrize(
    "expression",
    [
        "f(**{'other': 0}, set=(x := 1), **{'seen': x})",
        "f(**{'other': 0}, **{'set': (x := 1)}, **{'seen': x})",
        "{'other': 0, **{}, 'set': (x := 1), **{'seen': x}}",
        "{0, *(), (x := 1), *(x,)}",
    ],
)
def test_inert_or_empty_key_insertions_preserve_binding_facts(expression: str) -> None:
    source = f"x = 0\nresult = {expression}\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    read = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name)
        and node.id == "x"
        and isinstance(node.ctx, ast.Load)
    )
    fact = index.expression_fact(read)
    assert fact is not None and not fact.binding_invalidated


@pytest.mark.parametrize(
    "replacement",
    [
        "x = 1",
        "x: int = 1",
        "(x := 1)",
        "x, other = (1, 0)",
        "del x",
        "globals()['x'] = 1",
        "del globals()['x']",
    ],
)
def test_binding_replacement_finalizer_can_rewrite_or_resurrect_name(
    replacement: str,
) -> None:
    source = (
        "class Finalizer:\n"
        "    def __del__(self):\n"
        "        global x\n"
        "        x = 2\n"
        "x = Finalizer()\n"
        f"{replacement}\n"
        "seen = x\n"
    )
    namespace: dict[str, object] = {}
    exec(source, namespace)
    assert namespace["seen"] == 2
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    statement = tree.body[-1]
    assert isinstance(statement, ast.Assign)
    fact = index.expression_fact(statement.value)
    assert fact is not None and fact.binding_invalidated
    assert fact.static_value is None


@pytest.mark.parametrize("replacement", ["x = 1", "x: int = 1", "(x := 1)"])
def test_proven_inert_binding_replacement_preserves_new_value(replacement: str) -> None:
    source = f"x = 0\n{replacement}\nseen = x\n"
    index = analyze_python_source_bindings(source)
    statement = ast.parse(source).body[-1]
    assert isinstance(statement, ast.Assign)
    fact = index.expression_fact(statement.value)
    assert fact is not None and not fact.binding_invalidated and fact.static_value == 1


@pytest.mark.parametrize(
    "identity",
    [
        PythonIdentity.STATIC_FALSE,
        PythonIdentity.CURRENT_GLOBALS,
    ],
)
def test_unbound_release_alternative_is_neutral(identity: PythonIdentity) -> None:
    alternatives = int(identity | PythonIdentity.UNBOUND)
    assert not python_binding_flow._identity_can_release(alternatives)
    assert python_binding_flow._identity_can_release(alternatives | OTHER_IDENTITY)


@pytest.mark.parametrize("identity", list(PythonIdentity))
def test_release_identity_table_requires_retained_owner_proof(
    identity: PythonIdentity,
) -> None:
    rooted = {
        PythonIdentity.UNBOUND,
        PythonIdentity.STATIC_FALSE,
        PythonIdentity.CURRENT_GLOBALS,
    }
    assert python_binding_flow._identity_can_release(int(identity)) is (
        identity not in rooted
    )
    assert python_binding_flow._identity_can_release(
        int(identity | PythonIdentity.UNBOUND)
    ) is (identity not in rooted)


@pytest.mark.parametrize("owner_kind", ["module", "function"])
def test_detached_reference_owner_runs_weakref_callback(owner_kind: str) -> None:
    events: list[str] = []
    owner = ModuleType("detached_owner") if owner_kind == "module" else lambda: None
    registry = {"owner": owner}
    watch = weakref.ref(owner, lambda unused: events.append("released"))
    # Eviction does not change the private alias's identity, but does change
    # whether its next release is the last one. No global interpreter mutation.
    del registry["owner"]
    assert events == []
    owner = None
    assert watch() is None and events == ["released"]


@pytest.mark.parametrize("owner", ["locals()", "inspect.currentframe()"])
@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_captured_namespace_owner_release_can_finalize_contents(
    owner: str,
    target: tuple[int, int],
) -> None:
    source = (
        "import inspect\n"
        "def capture(value):\n"
        f"    kept = {owner}\n"
        "    def release():\n"
        "        nonlocal kept\n"
        "        kept = None\n"
        "    return release\n"
    )
    events: list[str] = []

    class Payload:
        def __del__(self) -> None:
            events.append("released")

    namespace: dict[str, object] = {}
    exec(source, namespace)
    release = namespace["capture"](Payload())
    assert events == []
    release()
    assert events == ["released"]
    # The host oracle proves its own CPython version. Policy projections must
    # all retain this lifetime possibility, including 3.12's cached locals.
    tree = ast.parse(source)
    statement = tree.body[1].body[1].body[1]
    assert isinstance(statement, ast.Assign)
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.statement_fact(statement)
    assert fact is not None and fact.effects & python_binding_flow.RUNS_FINALIZER


@pytest.mark.parametrize(
    "publication",
    ["import ext as target", "from ext import target"],
)
def test_import_publication_releases_callback_populated_target(
    publication: str,
) -> None:
    # The import hook receives the importing namespace and can populate even a
    # previously absent target. STORE_NAME publishes the import first, then the
    # replaced value's finalizer can rewrite it; the import SSA value is stale.
    namespace: dict[str, object] = {}

    class Previous:
        def __del__(self) -> None:
            namespace["target"] = "reentered"

    class Export:
        target = "imported"

    def importing(name, globals, locals, fromlist=(), level=0):
        assert globals is namespace
        namespace["target"] = Previous()
        return Export()

    namespace["__builtins__"] = {"__import__": importing}
    source = f"{publication}\nseen = target\n"
    exec(source, namespace)
    assert namespace["seen"] == "reentered"
    statement = ast.parse(source).body[-1]
    assert isinstance(statement, ast.Assign)
    fact = analyze_python_source_bindings(source).expression_fact(statement.value)
    assert fact is not None and fact.binding_invalidated and fact.static_value is None


def test_import_installed_finalizers_rewrite_later_static_bindings() -> None:
    namespace: dict[str, object] = {}

    class Previous:
        def __init__(self, name: str, value: object) -> None:
            self.name = name
            self.value = value

        def __del__(self) -> None:
            namespace[self.name] = self.value

    class Export:
        pass

    def importing(name, globals, locals, fromlist=(), level=0):
        assert globals is namespace
        globals["TYPE_CHECKING"] = Previous("TYPE_CHECKING", True)
        globals["MODULE_NAME"] = Previous("MODULE_NAME", "reentered")
        return Export()

    namespace["__builtins__"] = {"__import__": importing}
    source = (
        "import ext\n"
        "TYPE_CHECKING = False\n"
        "MODULE_NAME = 'warnings'\n"
        "seen = (TYPE_CHECKING, MODULE_NAME)\n"
    )
    exec(source, namespace)
    assert namespace["seen"] == (True, "reentered")

    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    statement = tree.body[-1]
    assert isinstance(statement, ast.Assign)
    assert isinstance(statement.value, ast.Tuple)
    reads = statement.value.elts
    assert all(isinstance(read, ast.Name) for read in reads)
    facts = [index.expression_fact(read) for read in reads]
    assert all(
        fact is not None and fact.binding_invalidated and fact.static_value is None
        for fact in facts
    )
    assert index.expression_result(reads[0]).truth is None


def test_absent_binding_is_pristine_until_its_namespace_is_exposed() -> None:
    pool = python_binding_flow._StatePool()
    pool.set_taint_domain(1)
    assert pool._binding_resolution(0, 0).clean
    assert pool.binding(0, 0) == int(PythonIdentity.UNBOUND)
    exposed = pool.taint_module_bindings(0)
    assert not pool._binding_resolution(exposed, 0).clean
    assert pool.binding(exposed, 0) == int(PythonIdentity.UNBOUND) | OTHER_IDENTITY
    # A private fast-local slot is not part of the exposed namespace.
    assert pool._binding_resolution(exposed, 1).clean
    assert pool.binding(exposed, 1) == int(PythonIdentity.UNBOUND)


def test_deferred_history_retains_insertion_into_previously_absent_namespace() -> None:
    pool = python_binding_flow._StatePool()
    pool.set_taint_domain(1)
    exposed = pool.taint_module_bindings(0)
    history = python_binding_flow._HistorySummary.build(pool, [0, exposed])
    assert history.binding(pool, 0, 0) == int(PythonIdentity.UNBOUND) | OTHER_IDENTITY
    assert history.binding(pool, 0, 1) == int(PythonIdentity.UNBOUND)


@pytest.mark.parametrize(
    "replacement",
    [
        "if flag:\n    x = 1\nelse:\n    x = 3\n",
        "result = (x := 1) if flag else (x := 3)\n",
    ],
)
def test_condition_callback_can_install_old_finalizable_binding(
    replacement: str,
) -> None:
    source = (
        "class Finalizer:\n"
        "    def __del__(self):\n"
        "        global x\n"
        "        x = 2\n"
        "class Flag:\n"
        "    def __bool__(self):\n"
        "        global x\n"
        "        x = Finalizer()\n"
        "        return True\n"
        "flag = Flag()\n" + replacement + "seen = x\n"
    )
    namespace: dict[str, object] = {}
    exec(source, namespace)
    assert namespace["seen"] == 2
    index = analyze_python_source_bindings(source)
    statement = ast.parse(source).body[-1]
    assert isinstance(statement, ast.Assign)
    fact = index.expression_fact(statement.value)
    assert fact is not None and fact.binding_invalidated


def test_future_import_purity_still_requires_canonical_import_policy() -> None:
    source = "from __future__ import annotations\n"
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(standard_imports_are_canonical=False)
    )
    assert index.module_namespace_may_be_observed(ast.parse(source).body[0])


def test_import_star_can_export_a_fresh_finalizable_binding() -> None:
    namespace: dict[str, object] = {}

    class Finalizer:
        def __del__(self) -> None:
            namespace["array"] = 2

    class Export:
        __all__ = ("array",)

        def __getattr__(self, name: str) -> object:
            if name == "array":
                return Finalizer()
            raise AttributeError(name)

    namespace["__builtins__"] = {"__import__": lambda *args: Export()}
    exec("from ext import *\narray = 1\nseen = array\n", namespace)
    assert namespace["seen"] == 2


@pytest.mark.parametrize(
    "source",
    [
        "def run():\n"
        "    class Finalizer:\n"
        "        def __del__(self):\n"
        "            nonlocal x\n"
        "            x = 2\n"
        "    x = Finalizer()\n"
        "    x = 1\n"
        "    return x\n"
        "seen = run()\n",
        "class Finalizer:\n"
        "    def __del__(self):\n"
        "        namespace['x'] = 2\n"
        "class Holder:\n"
        "    global namespace\n"
        "    namespace = locals()\n"
        "    x = Finalizer()\n"
        "    x = 1\n"
        "    seen = x\n"
        "seen = Holder.seen\n",
    ],
)
def test_callback_exposed_binding_storage_tracks_reentrant_finalizers(
    source: str,
) -> None:
    namespace: dict[str, object] = {}
    exec(source, namespace)
    assert namespace["seen"] == 2
    index = analyze_python_source_bindings(source)
    read = next(
        node
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.Name)
        and node.id == "x"
        and isinstance(node.ctx, ast.Load)
    )
    fact = index.expression_fact(read)
    assert fact is not None and fact.binding_invalidated and fact.static_value is None


def test_uncaptured_fast_local_is_not_callback_exposed_storage() -> None:
    source = "def run(x):\n    x = 1\n    return x\n"
    index = analyze_python_source_bindings(source)
    read = next(
        node
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.Name)
        and node.id == "x"
        and isinstance(node.ctx, ast.Load)
    )
    fact = index.expression_fact(read)
    assert fact is not None and not fact.binding_invalidated and fact.static_value == 1


@pytest.mark.parametrize(
    "arguments, stable",
    [
        ("*items, key=importlib.import_module('live')", True),
        ("0, *items, key=importlib.import_module('live')", False),
        ("*items, *(), key=importlib.import_module('live')", False),
        ("**mapping, key=importlib.import_module('live')", False),
    ],
)
def test_call_expansion_effects_follow_shared_schedule(arguments: str, stable: bool):
    index = analyze_python_source_bindings("import importlib\nf(" + arguments + ")\n")
    call = (
        next(
            call
            for call in index.calls
            if call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
        )
        if stable
        else None
    )
    assert (call is not None) == stable
    if not stable:
        assert not any(
            call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
            for call in index.calls
        )


def test_iteration_callbacks_precede_body_and_comprehension_consumers():
    for source in [
        "import importlib\nfor item in items:\n    importlib.import_module('live')\n",
        "import importlib\nvalue = [importlib.import_module('live') for item in items]\n",
    ]:
        assert not _last_call(source).callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize("operator", ["==", "!=", "<", "in", "not in"])
def test_comparison_callbacks_precede_later_chain_consumers(operator: str):
    source = (
        "import importlib\n"
        f"probe {operator} operand == importlib.import_module('live')\n"
    )
    assert not _last_call(source).callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    "prefix", ["False is True", "True is not True", "0 == 1", "1 != 1"]
)
def test_known_false_comparison_stops_before_unreachable_chain_operand(prefix: str):
    source = "import importlib\n" + prefix + " == importlib.import_module('dead')\n"
    index = analyze_python_source_bindings(source)
    assert index.calls == ()


def test_comparison_truth_callbacks_invalidate_later_static_member_reads():
    source = "import typing\nprobe == 0 == typing.TYPE_CHECKING\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    member = next(node for node in ast.walk(tree) if isinstance(node, ast.Attribute))
    assert index.expression_result(member).truth is None


def test_closed_comparison_chain_preserves_following_import_specialization():
    source = (
        "import importlib\nFalse is False is False\nimportlib.import_module('live')\n"
    )
    assert _last_call(source).callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize("module", ["typing", "typing_extensions"])
def test_static_false_member_preserves_walrus_owner_evaluation(module: str):
    source = (
        f"import {module} as typing\nif (alias := typing).TYPE_CHECKING:\n    pass\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    node = next(node for node in ast.walk(tree) if isinstance(node, ast.If))
    result = index.expression_result(node.test)
    assert result.truth is False
    assert result.evaluation_required


@pytest.mark.parametrize(
    ("source", "target", "owners"),
    [
        ("class C:\n    type Alias = value\n", (3, 12), {"C"}),
        ("class C:\n    if flag:\n        type Alias = value\n", (3, 12), {"C"}),
        ("class C:\n    type Alias[T: value] = T\n", (3, 12), {"C"}),
        ("class C:\n    def method(value: injected): pass\n", (3, 12), set()),
        ("class C:\n    def method(value: injected): pass\n", (3, 14), {"C"}),
        ("class C:\n    value: injected\n", (3, 14), {"C"}),
        ("class C:\n    type Alias = lambda: value\n", (3, 13), set()),
        ("class C:\n    type Alias = [value for item in items]\n", (3, 13), {"C"}),
        ("class C:\n    def method[T](value: injected): pass\n", (3, 12), set()),
        ("class C:\n    def method[T: injected](): pass\n", (3, 12), {"C"}),
        (
            "class Outer:\n    class Inner:\n        type Alias = value\n",
            (3, 12),
            {"Inner"},
        ),
        (
            "class Outer:\n    marker = int\n    class Inner[T: marker]: pass\n",
            (3, 12),
            {"Outer"},
        ),
        ("class C:\n    def method():\n        type Alias = value\n", (3, 12), set()),
        (
            "from __future__ import annotations\nclass C:\n    value: injected\n",
            (3, 14),
            set(),
        ),
    ],
)
def test_class_annotation_storage_owner_is_a_whole_body_binding_fact(
    source, target, owners
):
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    classes = [
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ClassDef)
    ]
    assert {
        node.name for node in classes if index.class_annotation_namespace_required(node)
    } == owners
