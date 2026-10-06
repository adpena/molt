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
from dataclasses import fields, replace
from threading import Event

import pytest

from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis.python_binding_facts import PythonMember
from molt.compiler_analysis.python_value_identity import (
    OTHER_IDENTITY,
    PythonIdentity,
    identity_fact_is_exact,
    identity_fact_may_be,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingFlowPolicy,
    PythonBindingPolicy,
    analyze_python_source_bindings,
)
from molt.compiler_analysis.python_effects_generated import (
    PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS,
    effect_mask_satisfies_capability,
)
from molt.compiler_analysis.python_source_keys import (
    python_ast_digest,
    python_source_digest,
)
from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
)


def _has_unknown_shape(result: StaticExpressionResult) -> bool:
    # Provenance is orthogonal to shape; preserve all prior shape/lifetime checks.
    from dataclasses import replace

    pending = [result]
    while pending:
        current = pending.pop()
        if (
            replace(
                current,
                identities=UNKNOWN_EXPRESSION_RESULT.identities,
                exposes_module_globals=False,
                element_result=None,
            )
            != UNKNOWN_EXPRESSION_RESULT
        ):
            return False
        if current.element_result is not None:
            pending.append(current.element_result)
    return True


def _last_call(source: str):
    index = analyze_python_source_bindings(source)
    assert index.calls
    return index.calls[-1]


@pytest.mark.parametrize(
    "method, arguments", [("__setitem__", "'key', None"), ("__delitem__", "'key'")]
)
def test_deferred_globals_method_preserves_possible_not_exact_identity(
    method, arguments
):
    source = f"def mutate():\n    globals().{method}({arguments})\n"
    index = analyze_python_source_bindings(source)
    call_node = ast.parse(source).body[0].body[0].value
    call = index.call_fact(call_node)
    identity = (
        PythonIdentity.GLOBALS_SETITEM
        if method == "__setitem__"
        else PythonIdentity.GLOBALS_DELITEM
    )
    assert call is not None and call.callee_may_be(identity)
    assert not call.callee_is(identity)
    assert call.effects & PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS


@pytest.mark.parametrize(
    "expression",
    [
        "(value, (value := replacement))",
        "[value, (value := replacement)]",
        "{value: (value := replacement)}",
        "consume(value, (value := replacement))",
        "consume(first=value, second=(value := replacement))",
        "value + (value := replacement)",
        "value[(value := replacement)]",
        "value is (value := replacement)",
        "(value if condition else replacement, (value := replacement))",
        "(value or replacement, (value := replacement))",
        "(value, [(value := item) for item in source])",
    ],
)
def test_pending_expression_read_captures_resolved_binding(expression: str) -> None:
    source = (
        "def f(value, replacement, condition, source, consume):\n"
        f"    return {expression}\n"
    )
    facts = analyze_python_source_bindings(source)
    reads = [
        node
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.Name)
        and node.id == "value"
        and isinstance(node.ctx, ast.Load)
    ]
    assert reads
    assert all(facts.expression_fact(node).binding_capture_required for node in reads)


@pytest.mark.parametrize(
    "expression",
    [
        "(value, replacement)",
        "((value := replacement), value)",
        "(value, lambda value: (value := replacement))",
        "(value, [value for value in source])",
    ],
)
def test_stable_or_different_storage_does_not_capture_name(expression: str) -> None:
    source = f"def f(value, replacement, source):\n    return {expression}\n"
    facts = analyze_python_source_bindings(source)
    outer = ast.parse(source).body[0]
    reads = [
        node
        for node in ast.walk(outer.body[0].value)
        if isinstance(node, ast.Name)
        and node.id == "value"
        and isinstance(node.ctx, ast.Load)
    ]
    # The earliest source read belongs to the enclosing function; equal text
    # in an isolated comprehension or lambda denotes a different slot.
    read = min(reads, key=lambda node: (node.lineno, node.col_offset))
    assert not facts.expression_fact(read).binding_capture_required


@pytest.mark.parametrize(
    "target, expected", [((3, 12), False), ((3, 13), True), ((3, 14), True)]
)
@pytest.mark.parametrize(
    "boundary", ["callback()", "callback and other", "callback.member"]
)
def test_live_frame_write_boundary_requires_expression_capture(
    target, expected, boundary
) -> None:
    source = f"def f(value, callback, other):\n    return (value, {boundary})\n"
    facts = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    read = ast.parse(source).body[0].body[0].value.elts[0]
    assert facts.expression_fact(read).binding_capture_required is expected


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "boundary",
    [
        "callback()",
        "callback.member",
        "callback and other",
        "callback == other",
        "tuple(callback)",
        "with callback:\n        pass",
        "other = None",
    ],
)
def test_frame_callbacks_expire_local_value_facts(target, boundary) -> None:
    source = (
        f"def f(callback, other):\n    value = 17\n    {boundary}\n    return value\n"
    )
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    read = ast.parse(source).body[0].body[-1].value
    fact = index.expression_fact(read)
    assert fact is not None
    assert fact.binding_invalidated is (target >= (3, 13))
    assert (_has_unknown_shape(fact.result)) is (target >= (3, 13))


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_frame_callback_can_replace_saved_callable_identity(target) -> None:
    source = (
        "def f(callback):\n    callee = len\n    callback()\n    return callee(())\n"
    )
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    call = ast.parse(source).body[0].body[-1].value
    fact = index.expression_fact(call.func)
    assert fact is not None
    assert fact.binding_invalidated is (target >= (3, 13))


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("body", ["1 + 2", "value = 23"])
def test_frame_local_facts_survive_inert_operations_and_fresh_stores(
    target, body
) -> None:
    source = f"def f(callback):\n    value = 17\n    {body}\n    return value\n"
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(ast.parse(source).body[0].body[-1].value)
    assert fact is not None and not fact.binding_invalidated
    assert not _has_unknown_shape(fact.result)


@pytest.mark.parametrize("target", [(3, 13), (3, 14)])
def test_frame_callback_store_cannot_hide_displaced_finalizer_reentry(target) -> None:
    # The callback may install a finalizable object. Replacing it publishes
    # 17, then runs that object's finalizer, which can replace the binding again.
    source = "def f(callback):\n    value = 1\n    callback()\n    value = 17\n    return value\n"
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(ast.parse(source).body[0].body[-1].value)
    assert fact is not None and fact.binding_invalidated
    assert _has_unknown_shape(fact.result)


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
    analyzer = python_binding_flow._Analyzer(PythonBindingFlowPolicy(), "scope-policy")
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
    assert _has_unknown_shape(index.expression_result(assignment.value))
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
        assert _has_unknown_shape(result)
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
    assert _has_unknown_shape(inline_result)
    inline_fact = index.expression_fact(reads[1])
    assert inline_fact is not None and inline_fact.name_lookup == "class_lexical"
    assert _has_unknown_shape(nested_result)
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
        assert _has_unknown_shape(body)
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
    assert _has_unknown_shape(result)

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
        tainted = pool.taint_exposed_bindings(branches[-1])
        branches.append(pool.set_binding(tainted, 0, int(PythonIdentity.USER_FUNCTION)))
    joined = pool.join(*branches)
    assert pool.binding(joined, 0) == int(
        PythonIdentity.USER_FUNCTION | PythonIdentity.UNBOUND
    )
    assert pool._binding_resolution(joined, 0).clean
    tainted_unbound = pool.taint_exposed_bindings(0)
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
    assert _has_unknown_shape(pool.result(joined, 0))


def test_callback_capable_method_keeps_binding_invalidated() -> None:
    source = "items = [1]\nitems.extend(source)\nresult = items\n"
    tree = ast.parse(source)
    result_statement = tree.body[-1]
    assert isinstance(result_statement, ast.Assign)
    fact = analyze_python_source_bindings(source).expression_fact(
        result_statement.value
    )

    assert fact is not None
    assert fact.binding_invalidated
    assert _has_unknown_shape(fact.result)


def test_append_retains_nested_argument_until_receiver_drop() -> None:
    source = (
        "def run(payload):\n"
        "    items = []\n"
        "    items.append([payload])\n"
        "    items\n"
        "    payload = None\n"
        "    items = None\n"
    )
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    append_statement = function.body[1]
    result_statement = function.body[-3]
    drop_statement = function.body[-1]
    assert isinstance(append_statement, ast.Expr)
    assert isinstance(append_statement.value, ast.Call)
    assert isinstance(result_statement, ast.Expr)
    assert isinstance(drop_statement, ast.Assign)
    index = analyze_python_source_bindings(source)
    call_fact = index.call_fact(append_statement.value)
    result_fact = index.expression_fact(result_statement.value)
    drop_fact = index.statement_fact(drop_statement)

    assert call_fact is not None
    assert not call_fact.cleanup_effects & python_binding_flow.RUNS_FINALIZER
    assert result_fact is not None
    assert not result_fact.binding_invalidated
    assert result_fact.result.kind == "list"
    assert result_fact.result.release_may_call
    assert drop_fact is not None
    assert drop_fact.effects & python_binding_flow.RUNS_FINALIZER


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
    analyzer = python_binding_flow._Analyzer(PythonBindingFlowPolicy(), "synthetic")
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
    analyzer = python_binding_flow._Analyzer(
        PythonBindingFlowPolicy(), "target-custody"
    )
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
    cache = python_binding_flow._BindingCache(max_entries=2)
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


def test_loop_join_collapses_owner_tokens_without_semantic_fixpoint_churn() -> None:
    states = python_binding_flow._StatePool()
    result = StaticExpressionResult(
        kind="list", element_result=StaticExpressionResult.scalar(1)
    )
    initial = states.set_binding(0, 0, OTHER_IDENTITY, result=result, owner_token=1)
    backedge = states.set_binding(
        initial, 0, OTHER_IDENTITY, result=result, owner_token=2
    )

    loop_header = states.join(initial, backedge)
    stabilized = states.join(loop_header, backedge)

    assert states.owner_token(loop_header, 0) == 0
    assert states.owner_token(stabilized, 0) == 0
    assert states.changed_slots_between(initial, loop_header) == ()
    assert states.slot_updated_between(initial, loop_header, 0)
    assert states.equivalent(initial, backedge)
    assert not states.owner_tokens_equal(initial, backedge)
    assert states.equivalent(loop_header, stabilized)
    assert states.owner_tokens_equal(loop_header, stabilized)


def test_recorded_same_shape_store_remains_a_transition_after_owner_collapse() -> None:
    states = python_binding_flow._StatePool()
    result = StaticExpressionResult(
        kind="list", element_result=StaticExpressionResult.scalar("value")
    )
    original = states.set_binding(0, 0, OTHER_IDENTITY, result=result)
    rewritten = states.set_bindings(
        original,
        ((0, OTHER_IDENTITY, None, result, 0),),
        record_writes=True,
    )

    assert rewritten != original
    assert states.changed_slots_between(original, rewritten) == ()
    assert states.slot_updated_between(original, rewritten, 0)


def test_binding_cache_single_flight_exception_wakes_waiters_and_recovers() -> None:
    cache = python_binding_flow._BindingCache(max_entries=2)
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


@pytest.mark.parametrize(
    "source",
    [
        "value = 1\n",
        "from . import child\n",
        "__package__ = 'override'\nfrom . import child\n",
        "del __spec__\nfrom . import child\n",
        "(__name__ := 'other.mod')\nfrom . import child\n",
        "__path__, value = producer()\nfrom . import child\n",
        "import sys\nsys.modules[__name__].__package__ = 'other'\nfrom . import child\n",
        "value = arbitrary\nvalue = 1\nfrom . import child\n",
        "del value\nfrom . import child\n",
        "receiver().field = value\nfrom . import child\n",
        "class A: pass\nclass B(A): pass\n(Alias := A)\nfrom . import child\n",
    ],
)
@pytest.mark.parametrize("version", [(3, 12), (3, 13), (3, 14)])
def test_context_projections_share_complete_facts_and_preserve_import_flow(
    source: str, version: tuple[int, int], monkeypatch: pytest.MonkeyPatch
) -> None:
    flow = python_binding_flow
    monkeypatch.setattr(flow, "_CORE_CACHE", flow._BindingCache(max_entries=2))
    tree = ast.parse(source)
    digest = python_source_digest(source)
    policy = PythonBindingPolicy(target_python=version)
    core = flow.analyze_python_binding_facts(
        tree, source_digest=digest, policy=policy.flow_policy()
    )
    contexts = (
        policy,
        replace(policy, module_name="pkg.mod", module_spec_name="pkg.mod"),
        replace(
            policy, module_name="pkg", module_spec_name="pkg", module_is_package=True
        ),
        replace(policy, module_name="__main__", module_execution_kind="script"),
        replace(
            policy,
            module_name="__main__",
            module_spec_name="pkg.mod",
            module_execution_kind="module",
        ),
    )
    for context in contexts:
        index = flow.analyze_python_bindings(tree, source_digest=digest, policy=context)
        assert index is flow.analyze_python_bindings(
            tree, source_digest=digest, policy=context
        )
        # Include the four lookup maps, telemetry, and annotation/storage facts.
        for item in fields(core):
            assert getattr(index, item.name) is getattr(core, item.name)
        fresh = flow._Analyzer(context.flow_policy(), digest).analyze(ast.parse(source))
        expected = flow._project_binding_index(
            fresh, context, lambda: ast.parse(source)
        )
        assert index == expected
    assert flow.python_binding_core_computations() == 1


def test_flow_policy_dimensions_cannot_share_a_fixpoint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    flow = python_binding_flow
    monkeypatch.setattr(flow, "_CORE_CACHE", flow._BindingCache())
    source = (
        "import sys\nfrom importlib import import_module\n"
        "if sys.platform == 'win32':\n    selected = 1\nelse:\n    selected = 2\n"
        "def deferred():\n    return import_module('pkg.child')\n"
        "annotation: import_module('pkg.annotation') = None\n"
    )
    base = PythonBindingPolicy()
    policies = (
        base,
        replace(base, target_python=(3, 13)),
        replace(base, target_python=(3, 14)),
        replace(base, target_sys_platform="win32"),
        replace(base, target_sys_platform="darwin"),
        replace(base, target_sys_platform="linux"),
        replace(base, analyze_deferred_bodies=False),
        replace(base, standard_imports_are_canonical=False),
    )
    indexes = [analyze_python_source_bindings(source, policy=p) for p in policies]
    assert flow.python_binding_core_computations() == len(policies)
    assert len({id(index.calls) for index in indexes}) == len(policies)
    assert len(indexes[6].calls) < len(indexes[0].calls)
    assert indexes[2].scopes[indexes[2].calls[-1].scope_id].kind == "annotation"
    with pytest.raises(TypeError, match="flow-only policy"):
        flow.analyze_python_binding_facts(
            ast.parse(source), source_digest="wrong-policy-type", policy=base
        )


def test_context_cache_lifetime_is_owned_and_bounded_by_core(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    flow = python_binding_flow
    cache = flow._BindingCache(max_entries=2)
    monkeypatch.setattr(flow, "_CORE_CACHE", cache)
    tree = ast.parse("value = 1\n")
    weak_tree = weakref.ref(tree)
    for number in range(20):
        index = flow.analyze_python_bindings(
            tree,
            source_digest="contexts",
            policy=PythonBindingPolicy(module_name=str(number)),
        )
    analysis = next(iter(cache._ready.values()))
    assert len(analysis.projections._ready) == 8
    assert flow.python_binding_core_computations() == 1
    for digest in ("second", "third"):
        flow.analyze_python_bindings(tree, source_digest=digest)
    assert len(cache._ready) == 2
    assert all(entry is not analysis for entry in cache._ready.values())
    # Cached facts, flows and completed single-flight closures retain no AST.
    del tree, analysis, index
    gc.collect()
    assert weak_tree() is None


def test_source_factory_is_lazy_and_projection_failure_does_not_poison_core(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    flow = python_binding_flow
    monkeypatch.setattr(flow, "_CORE_CACHE", flow._BindingCache())
    parse = ast.parse
    parses = 0

    def record_parse(*args, **kwargs):
        nonlocal parses
        parses += 1
        return parse(*args, **kwargs)

    monkeypatch.setattr(ast, "parse", record_parse)
    for name in ("one", "two", "one"):
        analyze_python_source_bindings(
            "pass\n", policy=PythonBindingPolicy(module_name=name)
        )
    assert parses == 1
    from molt.compiler_analysis import python_imports

    project = python_imports._analyze_module_import_flow_uncached
    attempts = 0

    def fail_once(*args, **kwargs):
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise ValueError("projection interrupted")
        return project(*args, **kwargs)

    monkeypatch.setattr(
        python_imports, "_analyze_module_import_flow_uncached", fail_once
    )
    with pytest.raises(ValueError, match="projection interrupted"):
        analyze_python_source_bindings("__package__ = 'pkg'\nfrom . import child\n")
    index = analyze_python_source_bindings("__package__ = 'pkg'\nfrom . import child\n")
    assert index.module_import_flow
    assert attempts == 2
    assert parses == 3
    assert flow.python_binding_core_computations() == 2


def test_concurrent_contexts_share_one_single_flight_core(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    flow = python_binding_flow
    monkeypatch.setattr(flow, "_CORE_CACHE", flow._BindingCache())
    entered, release = Event(), Event()
    analyze = flow._Analyzer.analyze

    def blocked(self, tree):
        entered.set()
        assert release.wait(5)
        return analyze(self, tree)

    monkeypatch.setattr(flow._Analyzer, "analyze", blocked)
    source = "__package__ = 'pkg'\nfrom . import child\n"

    def fetch(number):
        return analyze_python_source_bindings(
            source, policy=PythonBindingPolicy(module_name=f"pkg.mod{number % 4}")
        )

    with ThreadPoolExecutor(max_workers=8) as executor:
        futures = [executor.submit(fetch, number) for number in range(24)]
        assert entered.wait(5)
        release.set()
        indexes = [future.result() for future in futures]
    assert flow.python_binding_core_computations() == 1
    for number, index in enumerate(indexes):
        assert index is indexes[number % 4]
        assert index.calls is indexes[0].calls
        assert index._statement_lookup is indexes[0]._statement_lookup


def test_completed_assignment_effects_do_not_alias_mutable_analyzer() -> None:
    flow = python_binding_flow
    analyzer = flow._Analyzer(PythonBindingFlowPolicy(), "frozen-assignment-effects")
    analysis = analyzer.analyze(ast.parse("value = arbitrary\nvalue = 1\n"))
    assert analysis.module_import_flow_required
    expected = dict(analysis.assignment_effects)
    assert expected
    analyzer.assignment_effects.clear()
    assert dict(analysis.assignment_effects) == expected
    with pytest.raises(TypeError):
        analysis.assignment_effects[next(iter(expected))] = 0


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

    assert python_ast_digest(tree) != python_ast_digest(shifted_tree)
    index = python_binding_flow.analyze_python_bindings(
        tree, source_digest=python_ast_digest(tree)
    )
    shifted_index = python_binding_flow.analyze_python_bindings(
        shifted_tree,
        source_digest=python_ast_digest(shifted_tree),
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
    exposed = pool.taint_exposed_bindings(0)
    assert not pool._binding_resolution(exposed, 0).clean
    assert pool.binding(exposed, 0) == int(PythonIdentity.UNBOUND) | OTHER_IDENTITY
    # A private fast-local slot is not part of the exposed namespace.
    assert pool._binding_resolution(exposed, 1).clean
    assert pool.binding(exposed, 1) == int(PythonIdentity.UNBOUND)


def test_deferred_history_retains_insertion_into_previously_absent_namespace() -> None:
    pool = python_binding_flow._StatePool()
    pool.set_taint_domain(1)
    exposed = pool.taint_exposed_bindings(0)
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


@pytest.mark.parametrize(
    "expression,expected",
    [("-value", -1), ("+value", 1), ("~value", -2), ("-(-value)", 1)],
)
def test_numeric_unary_binding_retains_exact_value_without_callback_taint(
    expression: str,
    expected: int,
) -> None:
    source = f"value = 1\nresult = {expression}\ncopy = result\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    for node in (tree.body[1].value, tree.body[2].value):
        fact = index.expression_fact(node)
        assert fact is not None
        assert type(fact.static_value) is int and fact.static_value == expected
        assert fact.result.value_known and fact.result.value == expected
        assert not fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON


@pytest.mark.parametrize(
    "prefix", ["value = 1\ncallback()\n", "value = Number(1)\n", "value = unknown\n"]
)
def test_numeric_unary_binding_does_not_resurrect_expired_or_overloaded_values(
    prefix: str,
) -> None:
    source = prefix + "result = -value\n"
    tree = ast.parse(source)
    fact = analyze_python_source_bindings(source).expression_fact(tree.body[-1].value)
    assert fact is not None and fact.static_value is None
    assert not fact.result.value_known
    assert fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON


@pytest.mark.parametrize(
    "builtin,identity",
    [
        ("globals", PythonIdentity.CURRENT_GLOBALS),
        ("locals", PythonIdentity.CURRENT_LOCALS),
        ("vars", PythonIdentity.CURRENT_LOCALS),
    ],
)
def test_deferred_namespace_builtin_retains_possible_but_not_exact_result(
    builtin, identity
):
    source = f"def read():\n    return {builtin}()\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    call = tree.body[0].body[0].value
    fact = index.expression_fact(call)
    assert fact is not None
    assert fact.identities & int(identity)
    assert fact.identities & OTHER_IDENTITY
    assert fact.identities != int(identity)
    assert fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON


@pytest.mark.parametrize("builtin", ["globals", "locals", "vars"])
def test_module_namespace_builtin_keeps_exact_current_globals_result(builtin):
    source = f"namespace = {builtin}()\n"
    tree = ast.parse(source)
    fact = analyze_python_source_bindings(source).expression_fact(tree.body[0].value)
    assert fact is not None and fact.identities == int(PythonIdentity.CURRENT_GLOBALS)
    assert fact.result.kind == "dict"


def test_invalid_globals_call_does_not_publish_namespace_provenance():
    source = "namespace = globals(1)\n"
    tree = ast.parse(source)
    fact = analyze_python_source_bindings(source).expression_fact(tree.body[0].value)
    assert fact is not None
    assert not fact.identities & int(PythonIdentity.CURRENT_GLOBALS)


def test_cpython_function_globals_are_activation_owned_not_lexical_module_owned():
    from types import FunctionType

    def read_namespace():
        return globals()

    foreign = {"__package__": "foreign.pkg"}
    rebound = FunctionType(read_namespace.__code__, foreign)
    assert rebound() is foreign
    assert rebound()["__package__"] == "foreign.pkg"


@pytest.mark.parametrize(
    "mutation",
    [
        "owner['safe_alias'] = 1",
        "del owner['safe_alias']",
        "owner['safe_alias'] += 1",
        "owner.__setitem__('safe_alias', 1)",
        "put = owner.__setitem__; put('safe_alias', 1)",
        "owner.__delitem__('safe_alias')",
        "remove = owner.__delitem__; remove('safe_alias')",
    ],
)
def test_exact_activation_namespace_requires_dict_receiver_for_mutation(mutation):
    source = (
        "def mutate():\n"
        "    def local(): pass\n"
        "    owner = local.__globals__\n"
        f"    {mutation}\n"
    )
    tree = ast.parse(source)
    body = tree.body[0].body
    index = analyze_python_source_bindings(source)
    receiver = index.expression_fact(body[1].value)
    assert receiver is not None
    assert receiver.identities == int(PythonIdentity.CURRENT_GLOBALS)
    assert receiver.result.kind != "dict"
    mutation_fact = index.statement_fact(body[-1])
    assert mutation_fact is not None
    assert mutation_fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute) and node.attr in {
            "__setitem__",
            "__delitem__",
        }:
            fact = index.expression_fact(node)
            identity = (
                PythonIdentity.GLOBALS_SETITEM
                if node.attr == "__setitem__"
                else PythonIdentity.GLOBALS_DELITEM
            )
            # Namespace provenance retains the canonical method as a possible
            # source-discovery alternative, never exact dict/callable proof.
            assert fact is not None
            assert fact.identities == (int(identity) | OTHER_IDENTITY)
            assert fact.effects & python_binding_flow.INVOKES_DESCRIPTOR
    for call in index.calls:
        assert not call.callee_is(PythonIdentity.GLOBALS_SETITEM)
        assert not call.callee_is(PythonIdentity.GLOBALS_DELITEM)
        assert call.invocation_effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON
        assert call.cleanup_effects & python_binding_flow.RUNS_FINALIZER
        assert not call.callee_retention_safe


@pytest.mark.parametrize(
    "mutation,expected",
    [
        ("read_namespace()['item'] = 7", ("set", "item", 7)),
        ("del read_namespace()['item']", ("del", "item")),
        ("read_namespace()['item'] += 1", ("set", "item", 8)),
        ("read_namespace().__setitem__('item', 7)", ("set", "item", 7)),
        (
            "put = read_namespace().__setitem__; put('item', 7)",
            ("set", "item", 7),
        ),
        ("read_namespace().__delitem__('item')", ("del", "item")),
        (
            "remove = read_namespace().__delitem__; remove('item')",
            ("del", "item"),
        ),
    ],
)
def test_cpython_captured_globals_callable_can_return_dict_subclass(mutation, expected):
    from types import FunctionType

    events = []

    class Receiver(dict):
        def __setitem__(self, key, value):
            events.append(("set", key, value))

        def __delitem__(self, key):
            events.append(("del", key))

    namespace = {}
    exec("def mutate(read_namespace=globals):\n    " + mutation + "\n", namespace)
    original = namespace["mutate"]
    receiver = Receiver(item=7)
    rebound = FunctionType(original.__code__, receiver, argdefs=original.__defaults__)
    rebound()
    assert rebound.__globals__ is receiver
    assert original.__defaults__ == (globals,)
    assert events == [expected]
    assert dict.__getitem__(receiver, "item") == 7


def test_cpython_globals_subclass_descriptor_retains_method_and_argument_callbacks():
    from types import FunctionType

    events = []

    class Payload:
        def __del__(self):
            events.append("payload released")

    class Method:
        def __call__(self, key, value):
            events.append("invoke")

        def __del__(self):
            events.append("method released")

    class Receiver(dict):
        @property
        def __setitem__(self):
            events.append("lookup")
            return Method()

    def mutate(read_namespace=globals, make_payload=Payload, record=events.append):
        read_namespace().__setitem__("item", make_payload())
        record("after")

    receiver = Receiver()
    rebound = FunctionType(mutate.__code__, receiver, argdefs=mutate.__defaults__)
    rebound()
    assert events[:2] == ["lookup", "invoke"]
    assert events.index("payload released") < events.index("after")
    assert events.index("method released") < events.index("after")
    assert "item" not in receiver


@pytest.mark.parametrize(
    "source,stable",
    [
        ("owner = (lambda: None).__globals__\n", True),
        ("class Local:\n    owner = (lambda: None).__globals__\n", True),
        (
            "class Outer:\n    class Inner:\n"
            "        owner = (lambda: None).__globals__\n",
            True,
        ),
        ("class Local[T]:\n    owner = (lambda: None).__globals__\n", True),
        ("owners = [(lambda: None).__globals__ for item in (0,)]\n", True),
        ("owners = ((lambda: None).__globals__ for item in (0,))\n", False),
        ("def deferred():\n    return (lambda: None).__globals__\n", False),
        (
            "def deferred():\n    class Local:\n"
            "        owner = (lambda: None).__globals__\n",
            False,
        ),
        (
            "def deferred():\n"
            "    return [(lambda: None).__globals__ for item in (0,)]\n",
            False,
        ),
        ("deferred = lambda: (lambda: None).__globals__\n", False),
        ("type Deferred = (lambda: None).__globals__\n", False),
    ],
)
def test_globals_dict_result_uses_activation_owner_across_eager_scopes(source, stable):
    tree = ast.parse(source)
    receiver = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Attribute) and node.attr == "__globals__"
    )
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(receiver)
    assert fact is not None
    assert fact.identities == int(PythonIdentity.CURRENT_GLOBALS)
    assert (fact.result.kind == "dict") is stable


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_annotation_globals_dict_result_obeys_eager_or_deferred_activation(target):
    source = "def annotated(value: (lambda: None).__globals__): pass\n"
    tree = ast.parse(source)
    receiver = tree.body[0].args.args[0].annotation
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(receiver)
    assert fact is not None
    assert fact.identities == int(PythonIdentity.CURRENT_GLOBALS)
    assert (fact.result.kind == "dict") is (target < (3, 14))


def test_cpython_class_local_mapping_does_not_replace_activation_globals_dict():
    class ClassNamespace(dict):
        pass

    namespace = {"ClassNamespace": ClassNamespace}
    exec(
        "class Meta(type):\n"
        "    @classmethod\n"
        "    def __prepare__(cls, name, bases): return ClassNamespace()\n"
        "class Local(metaclass=Meta):\n"
        "    local_mapping = locals()\n"
        "    global_mapping = globals()\n"
        "eager_mappings = [globals() for item in (0,)]\n",
        namespace,
    )
    local = namespace["Local"]
    assert type(local.local_mapping) is ClassNamespace
    assert type(local.global_mapping) is dict
    assert local.global_mapping is namespace
    assert namespace["eager_mappings"][0] is namespace


def test_cpython_deferred_eager_scopes_inherit_functiontype_globals_subclass():
    from types import FunctionType

    class ActivationNamespace(dict):
        pass

    def outer(read_namespace=globals):
        class Local:
            namespace = read_namespace()

        return Local.namespace, [read_namespace() for item in (0,)][0]

    receiver = ActivationNamespace()
    rebound = FunctionType(outer.__code__, receiver, argdefs=outer.__defaults__)
    class_namespace, comprehension_namespace = rebound()
    assert class_namespace is receiver
    assert comprehension_namespace is receiver


@pytest.mark.parametrize(
    "name,identity",
    [
        ("globals", PythonIdentity.BUILTIN_GLOBALS),
        ("locals", PythonIdentity.BUILTIN_LOCALS),
        ("vars", PythonIdentity.BUILTIN_VARS),
        ("setattr", PythonIdentity.BUILTIN_SETATTR),
        ("eval", PythonIdentity.BUILTIN_EVAL),
        ("exec", PythonIdentity.BUILTIN_EXEC),
    ],
)
@pytest.mark.parametrize("acquisition", ["bare", "member", "from_import"])
def test_builtin_acquisition_forms_share_identity_and_replacement_guard(
    name, identity, acquisition
):
    acquire = (
        f"captured = {name}\n"
        if acquisition == "bare"
        else f"captured = builtins.{name}\n"
        if acquisition == "member"
        else f"from builtins import {name} as imported\ncaptured = imported\n"
    )
    for replacement in ("", f"builtins.{name} = replacement\n"):
        source = "import builtins\n" + replacement + acquire
        tree = ast.parse(source)
        fact = analyze_python_source_bindings(source).expression_fact(
            tree.body[-1].value
        )
        assert fact is not None
        if replacement:
            assert not fact.identities & int(identity)
        else:
            assert fact.identities == int(identity)


@pytest.mark.parametrize("name", ["globals", "locals", "vars"])
@pytest.mark.parametrize("acquisition", ["member", "from_import"])
def test_imported_namespace_builtins_preserve_eager_and_deferred_result_provenance(
    name, acquisition
):
    acquire = (
        "import builtins\n" + f"namespace = builtins.{name}\n"
        if acquisition == "member"
        else f"from builtins import {name} as namespace\n"
    )
    for deferred in (False, True):
        body = acquire + "result = namespace()\n"
        source = (
            "def read():\n"
            + "".join("    " + line + "\n" for line in body.splitlines())
            if deferred
            else body
        )
        tree = ast.parse(source)
        statements = tree.body[0].body if deferred else tree.body
        fact = analyze_python_source_bindings(source).expression_fact(
            statements[-1].value
        )
        expected = (
            PythonIdentity.CURRENT_LOCALS
            if deferred and name != "globals"
            else PythonIdentity.CURRENT_GLOBALS
        )
        assert fact is not None and fact.identities & int(expected)
        if deferred:
            assert fact.identities & OTHER_IDENTITY
            assert fact.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON
            assert fact.result.kind != "dict"
        else:
            assert fact.identities == int(expected)
            assert fact.result.kind == "dict"


@pytest.mark.parametrize(
    "module,member,identity",
    [
        ("inspect", "currentframe", PythonIdentity.INSPECT_CURRENTFRAME),
        ("importlib", "import_module", PythonIdentity.IMPORTLIB_IMPORT_MODULE),
        ("importlib.util", "find_spec", PythonIdentity.IMPORTLIB_FIND_SPEC),
        ("importlib.machinery", "ModuleSpec", PythonIdentity.MODULE_SPEC_CLASS),
        ("typing", "TYPE_CHECKING", PythonIdentity.STATIC_FALSE),
        ("builtins", "globals", PythonIdentity.BUILTIN_GLOBALS),
    ],
)
@pytest.mark.parametrize("boundary", ["callback()", "owner.MEMBER = replacement"])
def test_from_import_member_acquisition_observes_callback_and_replacement_guards(
    module, member, identity, boundary
):
    source = (
        f"import {module} as owner\n"
        + boundary.replace("MEMBER", member)
        + f"\nfrom {module} import {member} as acquired\nvalue = acquired\n"
    )
    tree = ast.parse(source)
    fact = analyze_python_source_bindings(source).expression_fact(tree.body[-1].value)
    assert fact is not None and fact.identities & OTHER_IDENTITY
    if boundary == "callback()":
        assert fact.identities & int(identity)
    else:
        assert not fact.identities & int(identity)


@pytest.mark.parametrize(
    "acquire,callee",
    [
        ("import inspect", "inspect.currentframe"),
        ("from inspect import currentframe as capture", "capture"),
    ],
)
def test_possible_currentframe_transports_globals_without_callback_elision(
    acquire, callee
):
    source = f"def read():\n    {acquire}\n    return {callee}().f_globals\n"
    tree = ast.parse(source)
    expression = tree.body[0].body[-1].value
    index = analyze_python_source_bindings(source)
    frame = index.expression_fact(expression.value)
    namespace = index.expression_fact(expression)
    assert frame is not None
    assert frame.identities & int(PythonIdentity.CURRENT_FRAME)
    assert frame.identities & OTHER_IDENTITY
    assert frame.effects & python_binding_flow.EXECUTES_ARBITRARY_PYTHON
    assert namespace is not None
    assert namespace.identities & int(PythonIdentity.CURRENT_GLOBALS)
    assert namespace.identities & OTHER_IDENTITY
    assert namespace.result.kind != "dict"
    call = index.call_fact(expression.value)
    assert call is not None and not call.callee_retention_safe


@pytest.mark.parametrize(
    "source",
    [
        "import inspect\nvalue = inspect.currentframe(1).f_globals\n",
        "import inspect\nvalue = inspect.currentframe(unexpected=1).f_globals\n",
        "import inspect\nvalue = inspect.currentframe().__globals__\n",
        "value = (lambda: None).f_globals\n",
    ],
)
def test_namespace_provenance_requires_valid_frame_call_and_matching_member(source):
    tree = ast.parse(source)
    fact = analyze_python_source_bindings(source).expression_fact(tree.body[-1].value)
    assert fact is not None
    assert not fact.identities & int(PythonIdentity.CURRENT_GLOBALS)


@pytest.mark.parametrize(
    "module_name,member", [("builtins", "globals"), ("inspect", "currentframe")]
)
def test_cpython_import_callback_can_replace_member_of_canonical_module(
    module_name, member
):
    import builtins
    import inspect

    module = builtins if module_name == "builtins" else inspect
    original_member = getattr(module, member)
    original_import = builtins.__import__
    marker = object()
    returned_modules = []

    def import_with_mutation(name, globals=None, locals=None, fromlist=(), level=0):
        imported = original_import(name, globals, locals, fromlist, level)
        if name == module_name:
            module.__dict__[member] = lambda: marker
            returned_modules.append(imported)
        return imported

    namespace = {
        "__builtins__": dict(builtins.__dict__, __import__=import_with_mutation)
    }
    try:
        exec(
            f"from {module_name} import {member} as capture\nvalue = capture()\n",
            namespace,
        )
    finally:
        module.__dict__[member] = original_member
    assert returned_modules == [module]
    assert namespace["value"] is marker


@pytest.mark.parametrize("branch_count", [2, 4])
@pytest.mark.parametrize("taint_before_domain", [False, True])
def test_metadata_storage_preserves_deleted_slot_presence_across_joins(
    branch_count: int, taint_before_domain: bool
) -> None:
    pool = python_binding_flow._StatePool()
    deleted = pool.set_binding(0, 0, int(PythonIdentity.UNBOUND))
    if taint_before_domain:
        deleted = pool.taint_slots(deleted, 1)
    branches = [deleted, 0]
    for slot in range(1, branch_count - 1):
        branches.append(pool.set_binding(0, slot, int(PythonIdentity.INERT_VALUE)))
    joined = pool.join(*branches)
    pool.set_taint_domain(1)
    assert not pool._binding_resolution(0, 0).present
    for state in (deleted, joined):
        resolution = pool._binding_resolution(state, 0)
        assert resolution.present
        assert resolution.public().present
        assert resolution.clean is not taint_before_domain


@pytest.mark.parametrize("prefix,pristine", [("", True), ("del __package__\n", False)])
def test_metadata_name_and_invocation_share_loader_pristine_proof(prefix, pristine):
    source = prefix + "__import__('child', {'__package__': __package__}, level=1)\n"
    facts = analyze_python_source_bindings(source)
    tree = ast.parse(source)
    call = tree.body[-1].value
    name = next(
        node
        for node in ast.walk(call)
        if isinstance(node, ast.Name) and node.id == "__package__"
    )
    read = facts.expression_fact(name)
    invocation = facts.call_fact(call)
    assert read is not None and invocation is not None
    assert read.module_metadata.admits_loader_borrow("__package__") is pristine
    assert (
        invocation.module_metadata_at_invocation.admits_loader_borrow("__package__")
        is pristine
    )


def test_import_argument_deletion_keeps_read_pristine_but_invalidates_borrow():
    source = "__import__('child', {'__package__': __package__}, globals().__delitem__('__package__'), [], 1)"
    facts = analyze_python_source_bindings(source)
    call = ast.parse(source).body[0].value
    name = call.args[1].values[0]
    read = facts.expression_fact(name)
    invocation = facts.call_fact(call)
    assert read is not None and invocation is not None
    assert read.module_metadata.admits_loader_borrow("__package__")
    assert not invocation.module_metadata_at_invocation.admits_loader_borrow(
        "__package__"
    )


@pytest.mark.parametrize(
    "prefix,admitted",
    [("", True), ("items.attribute\n", False), ("from . import sibling\n", False)],
)
def test_relative_statement_and_current_globals_call_share_entry_custody(
    prefix, admitted
):
    statement_source = prefix + "from . import child\n"
    call_source = prefix + "__import__('child', globals(), level=1)\n"
    statement = ast.parse(statement_source).body[-1]
    call = ast.parse(call_source).body[-1].value
    statement_fact = analyze_python_source_bindings(statement_source).statement_fact(
        statement
    )
    call_fact = analyze_python_source_bindings(call_source).call_fact(call)
    assert statement_fact is not None and call_fact is not None
    assert statement_fact.module_metadata_at_entry.admits_current_namespace is admitted
    assert call_fact.module_metadata_at_invocation.admits_current_namespace is admitted


def test_metadata_proof_is_not_computed_for_unrelated_calls_or_statements():
    from molt.compiler_analysis.python_binding_facts import NO_MODULE_METADATA_PROOF

    facts = analyze_python_source_bindings("value = len((1, 2))\n")
    assert all(f.module_metadata is NO_MODULE_METADATA_PROOF for f in facts.expressions)
    assert all(
        f.module_metadata_at_invocation is NO_MODULE_METADATA_PROOF for f in facts.calls
    )
    assert all(
        f.module_metadata_at_entry is NO_MODULE_METADATA_PROOF for f in facts.statements
    )


def test_explicit_unbound_storage_is_not_an_absent_state_or_history_noop():
    pool = python_binding_flow._StatePool()
    deleted = pool.set_binding(0, 0, int(PythonIdentity.UNBOUND))
    assert deleted != 0
    assert pool.binding(deleted, 0) == pool.binding(0, 0)
    assert not pool.equivalent(0, deleted)
    assert pool.changed_slots_between(0, deleted) == (0,)
    assert pool.set_binding(deleted, 0, int(PythonIdentity.UNBOUND)) == deleted


@pytest.mark.parametrize("discovery_first", [False, True])
@pytest.mark.parametrize(
    "source, expected_transfers",
    [
        ("value = 1\nfrom . import child\n", [True]),
        ("import os\n__package__ = 'pkg.alt'\nfrom . import child\n", [False, True]),
    ],
)
def test_source_discovery_reuses_single_flight_strict_context_projection(
    discovery_first: bool,
    source: str,
    expected_transfers: list[bool],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.compiler_analysis import python_imports

    monkeypatch.setattr(
        python_binding_flow, "_CORE_CACHE", python_binding_flow._BindingCache()
    )
    original = python_imports._analyze_module_import_flow_uncached
    transfers = []

    def transfer(*args, **kwargs):
        transfers.append(kwargs.get("source_discovery", False))
        return original(*args, **kwargs)

    monkeypatch.setattr(
        python_imports, "_analyze_module_import_flow_uncached", transfer
    )
    strict_policy = PythonBindingPolicy(module_name="pkg.entry")
    source_policy = replace(strict_policy, include_import_discovery=True)
    policies = (
        (source_policy, strict_policy)
        if discovery_first
        else (strict_policy, source_policy)
    )
    for policy in policies:
        first = analyze_python_source_bindings(source, policy=policy)
        assert first is analyze_python_source_bindings(source, policy=policy)
    strict = analyze_python_source_bindings(source, policy=strict_policy)
    discovery = analyze_python_source_bindings(source, policy=source_policy)
    assert strict.expressions is discovery.expressions
    assert strict.module_import_flow.states_by_node == (
        discovery.module_import_flow.states_by_node
    )
    assert transfers == expected_transfers
    assert python_binding_flow.python_binding_core_computations() == 1


@pytest.mark.parametrize(
    "select",
    [
        "load = (__import__,)[0]",
        "box = (__import__,)\nload = box[0]",
        "(load,) = (__import__,)",
        "(load,) = (*(__import__,),)",
        "*loads, = (__import__,)\nload = loads[0]",
        "head, *loads = (None, __import__)\nload = loads[0]",
        "load = [__import__].pop()",
        "load = list((__import__,))[0]",
        "(load,) = set((__import__,))",
        "(load,) = tuple(frozenset((__import__,)))",
        "load = {}.get('absent', __import__)",
        "load = {}.pop('absent', __import__)",
        "load = {}.setdefault('absent', __import__)",
        "for load in (__import__,):\n    pass",
    ],
)
def test_selected_importer_identity_matches_cpython_object(select: str) -> None:
    import builtins

    source = select + "\nloaded = load('math')\n"
    namespace = {}
    exec(compile(source, "<selected-importer-oracle>", "exec"), namespace)
    assert namespace["load"] is builtins.__import__
    assert namespace["loaded"].__name__ == "math"
    index = analyze_python_source_bindings(source)
    call = ast.parse(source).body[-1].value
    fact = index.call_fact(call)
    assert fact is not None
    assert fact.callee_may_be(PythonIdentity.BUILTINS_IMPORT)
    assert "dunder_import" in fact.possible_import_call_kinds()
    callee = index.expression_fact(call.func)
    assert callee is not None and callee.result.identities == callee.identities


@pytest.mark.parametrize("method", ["get", "pop", "setdefault"])
def test_unknown_dictionary_default_retains_importer_alternative(method: str) -> None:
    source = f"mapping = {{'present': None}}\nload = mapping.{method}('absent', __import__)\nloaded = load('math')\n"
    namespace = {}
    exec(compile(source, "<default-selection-oracle>", "exec"), namespace)
    assert namespace["loaded"].__name__ == "math"
    fact = analyze_python_source_bindings(source).calls[-1]
    assert fact.callee_may_be(PythonIdentity.BUILTINS_IMPORT)
    assert not fact.callee_is(PythonIdentity.BUILTINS_IMPORT)


@pytest.mark.parametrize(
    "display", ["(globals(),)", "[globals()]", "{'namespace': globals()}"]
)
def test_stored_aggregate_transports_namespace_exposure(display: str) -> None:
    source = f"box = {display}\nalias = box\nconsume(alias)\n"
    namespace = {"consume": lambda value: None}
    exec(compile(source, "<namespace-publication-oracle>", "exec"), namespace)
    observed = namespace["alias"]
    assert (
        observed["namespace"] if isinstance(observed, dict) else observed[0]
    ) is namespace
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    argument = index.expression_fact(tree.body[-1].value.args[0])
    assert argument is not None and argument.exposes_module_globals
    assert argument.result.exposes_module_globals


def test_selected_globals_identity_does_not_become_ordinary_dict_identity() -> None:
    source = "selected = (globals(),)[0]\n"
    namespace = {}
    exec(source, namespace)
    assert namespace["selected"] is namespace
    fact = analyze_python_source_bindings(source).expression_fact(
        ast.parse(source).body[0].value
    )
    assert fact is not None and identity_fact_is_exact(
        fact.identities, PythonIdentity.CURRENT_GLOBALS
    )
    assert fact.result.kind == "dict" and fact.exposes_module_globals
    ordinary = "selected = ({},)[0]\n"
    fact = analyze_python_source_bindings(ordinary).expression_fact(
        ast.parse(ordinary).body[0].value
    )
    assert fact is not None and not fact.identities & int(
        PythonIdentity.CURRENT_GLOBALS
    )


def test_selected_mutable_result_expires_contents_after_sibling_retirement() -> None:
    source = (
        "inner = [__import__]\n"
        "class Retired:\n"
        "    def __del__(self):\n"
        "        inner[0] = lambda name: 'replacement'\n"
        "selected = (inner, Retired())[0]\n"
        "load = selected[0]\n"
        "loaded = load('math')\n"
    )
    namespace = {}
    exec(compile(source, "<selection-retirement-oracle>", "exec"), namespace)
    assert namespace["loaded"] == "replacement"
    index = analyze_python_source_bindings(source)
    selected = index.expression_fact(ast.parse(source).body[2].value)
    assert selected is not None and selected.result.items is None
    possible = selected.result.element_result
    assert possible is not None and _has_unknown_shape(possible)
    assert possible.identities & OTHER_IDENTITY
    assert possible.release_may_call and not possible.fresh_container
    assert not index.calls[-1].callee_is(PythonIdentity.BUILTINS_IMPORT)


def test_selected_importlib_callable_survives_module_binding_replacement() -> None:
    source = (
        "import importlib\n"
        "load = (importlib.import_module,)[0]\n"
        "importlib = None\n"
        "loaded = load('math')\n"
    )
    namespace = {}
    exec(compile(source, "<captured-importlib-oracle>", "exec"), namespace)
    assert namespace["loaded"].__name__ == "math"
    fact = analyze_python_source_bindings(source).calls[-1]
    assert fact.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert "import_module" in fact.possible_import_call_kinds()


def test_selected_deferred_globals_keeps_foreign_dict_protocol() -> None:
    import builtins
    from types import FunctionType

    source = "def mutate():\n    (globals(),)[0].__setitem__('key', None)\n"
    namespace = {}
    exec(compile(source, "<selected-foreign-globals-oracle>", "exec"), namespace)
    events = []

    class ForeignGlobals(dict):
        def __setitem__(self, key, value):
            events.append(key)
            super().__setitem__(key, value)

    foreign = ForeignGlobals(__builtins__=builtins.__dict__)
    FunctionType(namespace["mutate"].__code__, foreign)()
    assert events == ["key"]
    node = ast.parse(source).body[0].body[0].value
    index = analyze_python_source_bindings(source)
    receiver = index.expression_fact(node.func.value)
    call = index.call_fact(node)
    assert receiver is not None and receiver.identities & int(
        PythonIdentity.CURRENT_GLOBALS
    )
    assert receiver.result.kind != "dict"
    assert call is not None and call.callee_may_be(PythonIdentity.GLOBALS_SETITEM)
    assert not call.callee_is(PythonIdentity.GLOBALS_SETITEM)
    assert call.effects & PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS


@pytest.mark.parametrize(
    ("expression", "exposes_globals"),
    [
        ("globals", False),
        ("globals().__setitem__", False),
        ("globals().__delitem__", False),
        ("(globals,)", False),
        ("[globals]", False),
        ("{'callable': globals}", False),
        ("(globals(), 7)[1]", False),
        ("globals()", True),
        ("(globals(),)", True),
        ("[globals(), *unknown]", True),
        ("(*unknown, globals())", True),
        ("{'namespace': globals(), **unknown}", True),
    ],
)
def test_namespace_evaluation_event_is_distinct_from_result_exposure(
    expression: str, exposes_globals: bool
) -> None:
    source = f"alias = {expression}\n"
    namespace = {"unknown": {}, "__package__": "pkg"}
    exec(compile(source, "<namespace-event-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "pkg"
    alias = namespace["alias"]
    contents = alias.values() if type(alias) is dict else alias
    contains_globals = alias is namespace or (
        type(alias) in {tuple, list, dict}
        and any(value is namespace for value in contents)
    )
    assert contains_globals is exposes_globals
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(tree.body[0].value)
    assert fact is not None and fact.module_namespace_observable
    assert index.module_namespace_may_be_observed(tree.body[0])
    assert fact.exposes_module_globals is exposes_globals
    assert fact.result.exposes_module_globals is exposes_globals


def test_generator_value_exposure_is_deferred_without_yield_value_guarantees() -> None:
    source = "values = (globals() for _ in (0,))\nalias = values\nconsume(alias)\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    expression = tree.body[0].value
    generator = index.expression_fact(expression)
    payload = index.expression_fact(expression.elt)
    argument = index.expression_fact(tree.body[-1].value.args[0])
    assert generator is not None and payload is not None and argument is not None
    assert not generator.module_namespace_observable
    assert payload.module_namespace_observable
    assert generator.exposes_module_globals and argument.exposes_module_globals
    yielded = generator.result.element_result
    assert yielded is not None and yielded.exposes_module_globals
    assert _has_unknown_shape(yielded) and yielded.identities == OTHER_IDENTITY
    assert generator.result.kind == "unknown" and generator.result.release_may_call


def test_deferred_generator_does_not_capture_stale_importer_identity() -> None:
    source = (
        "load = __import__\n"
        "values = (load for _ in (0,))\n"
        "load = lambda name: 'replacement'\n"
        "for selected in values:\n"
        "    loaded = selected('math')\n"
    )
    namespace = {}
    exec(compile(source, "<deferred-yield-oracle>", "exec"), namespace)
    assert namespace["loaded"] == "replacement"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    iteration = index.iteration_fact(tree.body[-1])
    assert iteration is not None
    yielded = iteration.element_result
    assert _has_unknown_shape(yielded)
    assert yielded.identities == OTHER_IDENTITY
    call = index.call_fact(tree.body[-1].body[0].value)
    assert call is not None and not call.callee_is(PythonIdentity.BUILTINS_IMPORT)


@pytest.mark.parametrize("iteration", [False, True])
def test_unknown_binding_and_iteration_keep_namespace_exposure_only(
    iteration: bool,
) -> None:
    tail = (
        "for value in values:\n    consume(value)\n"
        if iteration
        else "consume(values)\n"
    )
    source = "values = (globals(),)\ncallback()\n" + tail
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    call = tree.body[-1].body[0].value if iteration else tree.body[-1].value
    argument = index.expression_fact(call.args[0])
    assert argument is not None and argument.exposes_module_globals
    assert _has_unknown_shape(argument.result)
    assert argument.result.release_may_call
    assert not identity_fact_is_exact(
        argument.identities, PythonIdentity.CURRENT_GLOBALS
    )


@pytest.mark.parametrize(
    "source",
    [
        "box = ()\ndef send():\n    consume(box)\nbox = (globals(),)\nsend()\n",
        "def outer():\n    box = (globals(),)\n    def send():\n        consume(box)\n"
        "    return send\nsend = outer()\nsend()\n",
    ],
)
def test_deferred_capture_retains_namespace_exposure_without_value_proof(
    source: str,
) -> None:
    def consume(box):
        box[0]["__package__"] = "changed.pkg"

    namespace = {"__package__": "pkg", "consume": consume}
    exec(compile(source, "<deferred-exposure-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "changed.pkg"
    tree = ast.parse(source)
    call = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "consume"
    )
    index = analyze_python_source_bindings(source)
    argument = index.expression_fact(call.args[0])
    assert argument is not None and argument.exposes_module_globals
    assert _has_unknown_shape(argument.result) and argument.result.release_may_call


@pytest.mark.parametrize(
    "generators",
    [
        "for _ in (0, 1)",
        "for _ in (0, 1) for inner in (0,)",
        "for _ in (0,) for inner in (0, 1)",
    ],
)
def test_comprehension_backedges_join_values_and_importer_identity(
    generators: str,
) -> None:
    import builtins

    source = (
        "y = 0\n"
        f"values = [(y, (y := 1))[0] {generators}]\n"
        "load = None\n"
        f"calls = [(load, (load := __import__))[0] {generators}]\n"
        "loaded = calls[1]('math')\n"
    )
    namespace = {}
    exec(compile(source, "<comprehension-backedge-oracle>", "exec"), namespace)
    assert namespace["values"] == [0, 1]
    assert namespace["calls"] == [None, builtins.__import__]
    assert namespace["loaded"].sqrt(81) == 9.0
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    values = index.expression_result(tree.body[1].value)
    assert values.element_result is not None
    assert values.element_result.truth is None
    assert not values.element_result.value_known
    call = index.call_fact(tree.body[-1].value)
    assert call is not None and call.callee_may_be(PythonIdentity.BUILTINS_IMPORT)
    assert "dunder_import" in call.possible_import_call_kinds()


@pytest.mark.parametrize(
    ("comprehension", "expected", "after"),
    [
        ("[(y := 1) for _ in ()]", [], 0),
        ("[(y := 1) for _ in (0, 1) if False]", [], 0),
        ("[(y := 1) for _ in (0, 1) for inner in ()]", [], 0),
        ("[y for _ in (0, 1) if (y, (y := 1))[0]]", [1], 1),
        ("[(y := 1) for _ in (0,) for inner in (0, 1) if False]", [], 0),
    ],
)
def test_comprehension_zero_paths_and_filter_backedges(
    comprehension: str, expected: list[int], after: int
) -> None:
    source = f"y = 0\nvalues = {comprehension}\nafter = y\n"
    namespace = {}
    exec(compile(source, "<comprehension-control-oracle>", "exec"), namespace)
    assert namespace["values"] == expected and namespace["after"] == after
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    fact = index.expression_result(tree.body[-1].value)
    if after == 0:
        assert fact.value_known and fact.value == 0
    else:
        # A may-execute iteration may conservatively retain the zero path, but
        # it cannot claim the rejected first filter is false on every visit.
        condition = tree.body[1].value.generators[0].ifs[0]
        assert index.expression_result(condition).truth is None


def test_generator_body_writes_do_not_enter_creation_state() -> None:
    source = "y = 0\nvalues = ((y := 1) for _ in (0, 1))\nafter = y\n"
    namespace = {}
    exec(compile(source, "<deferred-comprehension-oracle>", "exec"), namespace)
    assert namespace["after"] == 0
    assert list(namespace["values"]) == [1, 1]
    assert namespace["y"] == 1
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    fact = index.expression_result(tree.body[-1].value)
    assert fact.value_known and fact.value == 0


def test_comprehension_clause_facts_remain_distinct_after_reparse() -> None:
    from molt.compiler_analysis.python_binding_facts import PythonNodeKey
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = (
        "seen = []\n"
        "first = [seen.append('dead') for a in (0,) for b in ()]\n"
        "second = [c for c in (1,)]\n"
    )
    namespace = {}
    exec(compile(source, "<distinct-comprehension-clauses>", "exec"), namespace)
    assert namespace["seen"] == [] and namespace["second"] == [1]
    parsed = ast.parse(source)
    reparsed = ast.parse(source)
    clauses = [node for node in ast.walk(parsed) if isinstance(node, ast.comprehension)]
    reparsed_clauses = [
        node for node in ast.walk(reparsed) if isinstance(node, ast.comprehension)
    ]
    keys = [python_node_source_key(node) for node in clauses]
    assert len(set(keys)) == len(clauses)
    assert keys == [python_node_source_key(node) for node in reparsed_clauses]
    index = analyze_python_source_bindings(source)
    by_target = {
        node.target.id: index.iteration_fact(node) for node in reparsed_clauses
    }
    assert by_target["a"] is not None and not by_target["a"].empty
    assert by_target["b"] is not None and by_target["b"].empty
    assert by_target["c"] is not None and not by_target["c"].empty
    for node in clauses:
        assert PythonNodeKey.from_node(node) != PythonNodeKey.from_node(node.target)


@pytest.mark.parametrize(
    "selection",
    [
        "load = box[0]\n",
        "load, = box\n",
        "for load in box:\n    pass\n",
    ],
)
def test_callback_retains_possible_importer_without_exact_call_proof(
    selection: str,
) -> None:
    source = f"box = [__import__]\ncallback()\n{selection}loaded = load('math')\n"
    namespace = {"callback": lambda: None}
    exec(compile(source, "<possible-importer-retention>", "exec"), namespace)
    assert namespace["loaded"].sqrt(81) == 9.0
    index = analyze_python_source_bindings(source)
    call = index.call_fact(ast.parse(source).body[-1].value)
    assert call is not None and call.callee_may_be(PythonIdentity.BUILTINS_IMPORT)
    assert not call.callee_is(PythonIdentity.BUILTINS_IMPORT)
    assert not call.callee_elision_safe
    if selection.startswith("load,"):
        from molt.compiler_analysis.python_effects_generated import (
            INVOKES_ITERATION_CALLBACK,
        )

        assignment = ast.parse(source).body[2]
        fact = index.statement_fact(assignment)
        assert fact is not None and fact.effects & INVOKES_ITERATION_CALLBACK


def test_deferred_result_provenance_survives_publication_join_and_widening():
    from molt.compiler_analysis.static_truth import (
        DeferredExecution,
        ExpressionSequenceItem,
        StaticExpressionResult,
        UNKNOWN_EXPRESSION_RESULT,
        expression_result_for_publication,
        expression_result_without_value_facts,
        iterable_element_result,
        join_static_expression_results,
        static_subscription_shape,
    )

    reference = DeferredExecution((1, 0, 1, 20, "GeneratorExp"), "resume")
    deferred = StaticExpressionResult(deferred=frozenset({reference}))
    stored = StaticExpressionResult(
        kind="tuple", items=(ExpressionSequenceItem(deferred),)
    )
    published = expression_result_for_publication(stored)
    assert not published.deferred
    assert reference in published.exposed_deferred
    element = iterable_element_result(published)
    assert element is not None and reference in element.deferred
    widened = expression_result_without_value_facts(published)
    selected = static_subscription_shape(widened, UNKNOWN_EXPRESSION_RESULT).result
    assert reference in selected.deferred
    joined = join_static_expression_results((selected, UNKNOWN_EXPRESSION_RESULT))
    assert reference in joined.deferred
    assert joined != UNKNOWN_EXPRESSION_RESULT
    assert len({joined, UNKNOWN_EXPRESSION_RESULT}) == 2


def test_conditional_callable_join_and_hook_facts_use_result_algebra():
    from molt.compiler_analysis.static_truth import (
        DeferredExecution,
        expression_result_without_value_facts,
        join_static_expression_results,
        static_expression_result,
    )

    first = DeferredExecution((1, 0, 2, 4, "FunctionDef"), "call")
    second = DeferredExecution((4, 0, 5, 4, "FunctionDef"), "call")
    candidates = {
        "a": StaticExpressionResult(
            deferred=frozenset({first}),
            deferred_complete=True,
            attribute_hooks=frozenset(),
        ),
        "b": StaticExpressionResult(
            deferred=frozenset({second}),
            deferred_complete=True,
            attribute_hooks=frozenset(),
        ),
    }
    expression = ast.parse("a if condition else b", mode="eval").body
    result = static_expression_result(
        expression,
        fact_result=lambda node: (
            candidates.get(node.id) if isinstance(node, ast.Name) else None
        ),
    )
    assert result.deferred == frozenset({first, second})
    assert result.deferred_complete and result.attribute_hooks == frozenset()
    widened = expression_result_without_value_facts(result)
    assert widened.deferred == result.deferred
    assert not widened.deferred_complete and widened.attribute_hooks is None
    joined = join_static_expression_results((result, UNKNOWN_EXPRESSION_RESULT))
    assert joined.deferred == result.deferred and not joined.deferred_complete
    assert joined.attribute_hooks is None
    assert StaticExpressionResult(
        identities=int(PythonIdentity.CURRENT_MODULE)
    ).exposes_module_globals


@pytest.mark.parametrize(
    ("method", "operation"),
    [
        (
            "__hash__(self):\n        namespace['__package__'] = 'other'\n        return 0",
            "{Change()}",
        ),
        (
            "__getattr__(self, name):\n        namespace['__package__'] = 'other'\n        return 1",
            "Change().missing",
        ),
        ("__del__(self):\n        namespace['__package__'] = 'other'", "Change()"),
        (
            "__setitem__(self, key, value):\n        namespace['__package__'] = 'other'",
            "target[0] = 1",
        ),
        (
            "__delitem__(self, key):\n        namespace['__package__'] = 'other'",
            "del target[0]",
        ),
    ],
)
def test_implicit_operation_fact_carries_its_executed_values(method, operation):
    from molt.compiler_analysis.python_effects_generated import WRITES_MODULE_METADATA

    source = (
        "namespace = globals()\nclass Change:\n    def "
        + method
        + "\ntarget = Change()\n__package__ = 'pkg'\n"
        + operation
        + "\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    node = tree.body[-1]
    fact = index.statement_fact(node)
    effects = fact.module_metadata_effects if fact is not None else 0
    if isinstance(node, ast.Expr):
        expression = index.expression_fact(node.value)
        assert expression is not None
        effects |= expression.module_metadata_effects
    assert effects & WRITES_MODULE_METADATA


@pytest.mark.parametrize("analyze_bodies", [False, True])
def test_unanalyzed_class_methods_keep_unknown_escaped_execution(analyze_bodies):
    from molt.compiler_analysis.python_effects_generated import WRITES_MODULE_METADATA

    source = (
        "class Stored:\n    def apply(self): pass\nvalue = Stored()\nvalue.apply()\n"
    )
    index = python_binding_flow.analyze_python_binding_facts(
        ast.parse(source),
        source_digest=python_source_digest(source),
        policy=PythonBindingFlowPolicy(analyze_deferred_bodies=analyze_bodies),
    )
    fact = index.expression_fact(ast.parse(source).body[-1].value)
    assert fact is not None
    assert bool(fact.module_metadata_effects & WRITES_MODULE_METADATA) is (
        not analyze_bodies
    )


def test_deferred_kind_uses_cached_lexical_body_summary():
    source = "def outer():\n    def nested():\n        yield 1\n    return nested\n"
    definition = ast.parse(source).body[0]
    authority = PythonDependencyAuthority(
        eager_annotations=True, future_annotations=False
    )
    summary = authority.summary(definition)
    assert not summary.contains_yield
    assert authority.summary(definition.body[0]).contains_yield
    visits = authority.node_visits
    for _ in range(50):
        assert authority.summary(definition) is summary
    assert authority.node_visits == visits
