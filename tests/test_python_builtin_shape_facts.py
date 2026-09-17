from __future__ import annotations

import ast

import pytest

from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis.python_binding_facts import (
    BUILTIN_SHAPE_IDENTITIES,
    OTHER_IDENTITY,
    PythonIdentity,
    PythonIterationFact,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_source_bindings,
)
from molt.compiler_analysis.python_builtin_shapes import (
    BUILTIN_SHAPE_NAMES,
    builtin_call_shape,
    builtin_method_call_shape,
    builtin_open_result,
)
from molt.compiler_analysis.python_effects_generated import (
    ALLOCATES,
    EXECUTES_ARBITRARY_PYTHON,
    INVOKES_ITERATION_CALLBACK,
    RAISES,
    RELEASES_REFERENCE,
    RUNS_FINALIZER,
    RUNS_WEAKREF_CALLBACK,
)
from molt.compiler_analysis.static_truth import (
    ExpressionKind,
    ExpressionSequenceItem,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    expression_result_for_publication,
    expression_result_without_mutable_contents,
    join_static_expression_results,
    static_comparison_result,
)


def _expression(source: str) -> ast.expr:
    return ast.parse(source, mode="eval").body


def _call(source: str) -> ast.Call:
    node = _expression(source)
    assert isinstance(node, ast.Call)
    return node


def _analyzed_call(source: str, node: ast.Call):
    index = analyze_python_source_bindings(source)
    fact = index.call_fact(node)
    assert fact is not None
    return index, fact


def _statement_call(source: str, statement_index: int = -1) -> ast.Call:
    statement = ast.parse(source).body[statement_index]
    value = statement.value
    assert isinstance(value, ast.Call)
    return value


def _shared_expansion_dag(
    depth: int, leaf: StaticExpressionResult | None = None
) -> StaticExpressionResult:
    item = StaticExpressionResult.scalar("leaf") if leaf is None else leaf
    result = StaticExpressionResult(
        truth=True,
        kind="tuple",
        items=(ExpressionSequenceItem(item),),
        release_may_call=item.release_may_call,
        length=1,
    )
    for _ in range(depth):
        result = StaticExpressionResult(
            truth=True,
            kind="tuple",
            items=(
                ExpressionSequenceItem(result, expanded=True),
                ExpressionSequenceItem(result, expanded=True),
            ),
            release_may_call=result.release_may_call,
        )
    return result


def _deep_element_chain(
    depth: int, leaf: StaticExpressionResult
) -> StaticExpressionResult:
    result = leaf
    for _ in range(depth):
        result = StaticExpressionResult(kind="tuple", element_result=result)
    return result


@pytest.mark.parametrize(
    ("name", "expression", "kind"),
    [
        ("bool", "bool()", "bool"),
        ("int", "int()", "int"),
        ("float", "float()", "float"),
        ("complex", "complex()", "complex"),
        ("str", "str()", "str"),
        ("bytes", "bytes()", "bytes"),
        ("bytearray", "bytearray()", "bytearray"),
        ("tuple", "tuple()", "tuple"),
        ("list", "list()", "list"),
        ("set", "set()", "set"),
        ("frozenset", "frozenset()", "frozenset"),
        ("dict", "dict()", "dict"),
        ("range", "range(3)", "range"),
        ("len", "len(())", "int"),
    ],
)
def test_exact_builtin_identities_publish_the_cataloged_normal_kind(
    name: str, expression: str, kind: str
) -> None:
    source = f"result = {expression}\n"
    call = _statement_call(source)
    index, fact = _analyzed_call(source, call)

    assert BUILTIN_SHAPE_NAMES == frozenset(BUILTIN_SHAPE_IDENTITIES)
    assert fact.callee_is(BUILTIN_SHAPE_IDENTITIES[name])
    assert fact.exact_builtin_name() == name
    assert index.expression_result(call).kind == kind


@pytest.mark.parametrize(
    ("name", "expression"),
    [
        ("bool", "bool()"),
        ("int", "int()"),
        ("float", "float()"),
        ("complex", "complex()"),
        ("str", "str()"),
        ("bytes", "bytes()"),
        ("bytearray", "bytearray()"),
        ("tuple", "tuple()"),
        ("list", "list()"),
        ("set", "set()"),
        ("frozenset", "frozenset()"),
        ("dict", "dict()"),
        ("range", "range(1)"),
        ("len", "len(())"),
    ],
)
def test_deferred_activation_keeps_builtin_identity_only_as_possible(
    name: str, expression: str
) -> None:
    source = f"def value():\n    return {expression}\n"
    tree = ast.parse(source)
    call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    assert isinstance(call.func, ast.Name)
    index, fact = _analyzed_call(source, call)
    callee = index.expression_fact(call.func)
    assert callee is not None

    assert fact.callee_may_be(BUILTIN_SHAPE_IDENTITIES[name])
    assert fact.callee_identities & OTHER_IDENTITY
    assert callee.binding_invalidated
    assert callee.effects & EXECUTES_ARBITRARY_PYTHON
    assert index.expression_result(call) == UNKNOWN_EXPRESSION_RESULT


def test_deferred_activation_keeps_lexical_result_transport_precise() -> None:
    source = "def value():\n    literal = ()\n    alias = literal\n    return alias\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    returned = next(node for node in ast.walk(tree) if isinstance(node, ast.Return))
    assert isinstance(returned.value, ast.Name)
    fact = index.expression_fact(returned.value)
    assert fact is not None

    result = index.expression_result(returned.value)
    assert result.kind == "tuple"
    assert result.length == 0
    assert not fact.binding_invalidated
    assert not fact.effects & EXECUTES_ARBITRARY_PYTHON


def test_deferred_activation_invalidates_global_read_after_explicit_write() -> None:
    source = (
        "value = None\n"
        "def replace():\n"
        "    global value\n"
        "    value = ()\n"
        "    return value\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    returned = next(node for node in ast.walk(tree) if isinstance(node, ast.Return))
    assert isinstance(returned.value, ast.Name)
    fact = index.expression_fact(returned.value)
    assert fact is not None

    assert index.expression_result(returned.value) == UNKNOWN_EXPRESSION_RESULT
    assert fact.binding_invalidated
    assert fact.effects & EXECUTES_ARBITRARY_PYTHON


def test_scope_facts_publish_activation_namespace_stability() -> None:
    source = (
        "module_list = [item for item in ()]\n"
        "module_set = {item for item in ()}\n"
        "module_dict = {item: item for item in ()}\n"
        "module_generator = (item for item in ())\n"
        "module_lambda = lambda: None\n"
        "class ModuleClass:\n"
        "    pass\n"
        "annotated: int\n"
        "def deferred():\n"
        "    local_list = [item for item in ()]\n"
        "    local_set = {item for item in ()}\n"
        "    local_dict = {item: item for item in ()}\n"
        "    local_generator = (item for item in ())\n"
        "    class LocalClass:\n"
        "        pass\n"
    )
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=(3, 14))
    )

    module = next(scope for scope in index.scopes if scope.kind == "module")
    deferred = next(scope for scope in index.scopes if scope.name == "deferred")
    module_class = next(scope for scope in index.scopes if scope.name == "ModuleClass")
    local_class = next(scope for scope in index.scopes if scope.name == "LocalClass")
    lambda_scope = next(scope for scope in index.scopes if scope.kind == "lambda")
    annotation_scope = next(
        scope for scope in index.scopes if scope.kind == "annotation"
    )

    assert module.activation_namespace_stable
    assert not deferred.activation_namespace_stable
    assert not lambda_scope.activation_namespace_stable
    assert not annotation_scope.activation_namespace_stable
    assert module_class.parent_scope_id == module.scope_id
    assert module_class.activation_namespace_stable
    assert local_class.parent_scope_id == deferred.scope_id
    assert not local_class.activation_namespace_stable

    module_comprehensions = [
        scope
        for scope in index.scopes
        if scope.kind == "comprehension" and scope.parent_scope_id == module.scope_id
    ]
    deferred_comprehensions = [
        scope
        for scope in index.scopes
        if scope.kind == "comprehension" and scope.parent_scope_id == deferred.scope_id
    ]
    assert (
        sum(scope.activation_namespace_stable for scope in module_comprehensions) == 3
    )
    assert (
        sum(not scope.activation_namespace_stable for scope in module_comprehensions)
        == 1
    )
    assert len(deferred_comprehensions) == 4
    assert all(
        not scope.activation_namespace_stable for scope in deferred_comprehensions
    )


@pytest.mark.parametrize(
    ("source", "name"),
    [
        ("alias = len\nresult = alias([])\n", "len"),
        ("from builtins import tuple as make\nresult = make([])\n", "tuple"),
        ("import builtins as core\nresult = core.range(2)\n", "range"),
    ],
)
def test_aliases_and_builtin_imports_retain_exact_captured_identity(
    source: str, name: str
) -> None:
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)
    assert fact.exact_builtin_name() == name


@pytest.mark.parametrize(
    "source",
    [
        "alias = int\ndef value():\n    return alias()\n",
        "from builtins import int as alias\ndef value():\n    return alias()\n",
        "import builtins as core\ndef value():\n    return core.int()\n",
    ],
)
def test_deferred_activation_widens_module_builtin_aliases(source: str) -> None:
    tree = ast.parse(source)
    call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    index, fact = _analyzed_call(source, call)

    assert fact.callee_may_be(PythonIdentity.BUILTIN_INT)
    assert fact.callee_identities & OTHER_IDENTITY
    assert index.expression_result(call) == UNKNOWN_EXPRESSION_RESULT


@pytest.mark.parametrize("name", sorted(BUILTIN_SHAPE_NAMES))
def test_each_builtin_member_guard_rejects_direct_mutation(name: str) -> None:
    source = (
        f"import builtins\nbuiltins.{name} = replacement\nresult = builtins.{name}()\n"
    )
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert fact.exact_builtin_name() is None
    assert fact.callee_identities & OTHER_IDENTITY


@pytest.mark.parametrize(
    "mutation",
    [
        "builtins.len = replacement",
        "del builtins.len",
        "builtins.len = len",
        "setattr(builtins, 'len', replacement)",
    ],
)
def test_builtin_member_guard_rejects_mutation_delete_and_rebind_forms(
    mutation: str,
) -> None:
    source = f"import builtins\n{mutation}\nresult = builtins.len([])\n"
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert fact.exact_builtin_name() is None
    assert fact.callee_identities & OTHER_IDENTITY


def test_builtin_import_after_invalidation_stays_conservative() -> None:
    definite_source = (
        "import builtins\nbuiltins.len = replacement\n"
        "from builtins import len as captured\nresult = captured([])\n"
    )
    possible_source = (
        "import builtins\nif flag:\n    builtins.len = replacement\n"
        "from builtins import len as captured\nresult = captured([])\n"
    )
    definite_call = _statement_call(definite_source)
    possible_call = _statement_call(possible_source)
    _definite_index, definite = _analyzed_call(definite_source, definite_call)
    _possible_index, possible = _analyzed_call(possible_source, possible_call)

    assert not definite.callee_may_be(PythonIdentity.BUILTIN_LEN)
    assert possible.callee_may_be(PythonIdentity.BUILTIN_LEN)
    assert possible.callee_identities & OTHER_IDENTITY


def test_callee_capture_precedes_argument_rebinding() -> None:
    source = "alias = len\nresult = alias(((alias := 0),))\n"
    call = _statement_call(source)
    index, fact = _analyzed_call(source, call)

    assert fact.exact_builtin_name() == "len"
    assert index.expression_result(call).value == 1


@pytest.mark.parametrize(
    "expression",
    [
        "len()",
        "len(value=items)",
        "len(*items)",
        "range()",
        "tuple(items, other)",
        "dict(value=1)",
    ],
)
def test_invalid_arity_keywords_and_splats_remain_generic(expression: str) -> None:
    node = _call(expression)
    argument_count = len(node.args) + len(node.keywords)
    shape = builtin_call_shape(
        node.func.id,
        node,
        (UNKNOWN_EXPRESSION_RESULT,) * argument_count,
    )

    assert not shape.specialization_valid
    assert shape.result.kind == ("int" if node.func.id == "len" else node.func.id)


def test_call_facts_separate_evaluation_invocation_and_cleanup_effects() -> None:
    source = "result = len([])\n"
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert fact.evaluation_effects & ALLOCATES
    assert fact.invocation_effects & RAISES
    assert not fact.invocation_effects & EXECUTES_ARBITRARY_PYTHON
    assert fact.cleanup_effects & RELEASES_REFERENCE
    assert not fact.cleanup_effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)
    assert fact.callee_elision_safe
    assert fact.callee_retention_safe


@pytest.mark.parametrize(
    ("source", "safe"),
    [
        ("result = len([])\n", True),
        ("x = []\nresult = len(x)\n", True),
        ("result = len([Probe()])\n", False),
    ],
)
def test_cleanup_lifetime_distinguishes_owned_names_from_temporaries(
    source: str, safe: bool
) -> None:
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert fact.callee_elision_safe is safe
    assert fact.callee_retention_safe is safe
    assert bool(fact.cleanup_effects & RUNS_FINALIZER) is (not safe)


def test_exceptional_later_argument_releases_prior_temporaries() -> None:
    source = "result = consume(Probe(), fail())\n"
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert fact.evaluation_effects & RELEASES_REFERENCE
    assert fact.evaluation_effects & RUNS_FINALIZER


@pytest.mark.parametrize(
    ("middle", "release_may_call"),
    [
        ("True", False),
        ("globals() is globals()", True),
    ],
)
def test_exceptional_name_cleanup_requires_unbroken_owner_continuity(
    middle: str, release_may_call: bool
) -> None:
    source = f"value = [bytearray()]\nconsume(value, {middle}, int('bad'))\n"
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)
    assert bool(fact.evaluation_effects & RUNS_FINALIZER) is release_may_call


def test_invocation_does_not_replay_earlier_argument_effects() -> None:
    source = "len(((x := []),))\nresult = len(x)\n"
    call = _statement_call(source)
    index, fact = _analyzed_call(source, call)
    argument = call.args[0]
    assert isinstance(argument, ast.Name)

    assert fact.exact_builtin_name() == "len"
    assert fact.callee_elision_safe
    assert index.expression_result(argument).kind == "list"


def test_result_only_changes_participate_in_diff_equality_and_join() -> None:
    pool = python_binding_flow._StatePool()
    one = StaticExpressionResult.scalar(1)
    two = StaticExpressionResult.scalar(2)
    left = pool.set_binding(0, 0, int(PythonIdentity.INERT_VALUE), result=one)
    right = pool.set_binding(0, 0, int(PythonIdentity.INERT_VALUE), result=two)

    assert pool.changed_slots_between(left, right) == (0,)
    assert not pool.equivalent(left, right)
    joined = pool.join(left, right)
    assert pool.result(joined, 0).kind == "int"
    assert not pool.result(joined, 0).value_known


def test_release_callbacks_require_both_identity_and_result_uncertainty() -> None:
    safe_shape = StaticExpressionResult(kind="tuple", release_may_call=False)

    assert not python_binding_flow._release_may_call(OTHER_IDENTITY, safe_shape)
    assert not python_binding_flow._release_may_call(
        int(PythonIdentity.CURRENT_GLOBALS), UNKNOWN_EXPRESSION_RESULT
    )
    assert python_binding_flow._release_may_call(
        OTHER_IDENTITY, UNKNOWN_EXPRESSION_RESULT
    )
    assert python_binding_flow._release_may_call(
        int(PythonIdentity.INERT_VALUE), UNKNOWN_EXPRESSION_RESULT
    )


def test_retained_callback_boundary_publishes_release_custody() -> None:
    fresh_list = StaticExpressionResult(
        kind="list", release_may_call=False, fresh_container=True
    )
    bytearray_result = StaticExpressionResult(
        kind="bytearray", release_may_call=False, fresh_container=True
    )

    assert not python_binding_flow._result_after_retained_boundary(
        fresh_list, RAISES
    ).release_may_call
    assert python_binding_flow._result_after_retained_boundary(
        fresh_list, EXECUTES_ARBITRARY_PYTHON
    ).release_may_call
    assert not python_binding_flow._result_after_retained_boundary(
        bytearray_result, EXECUTES_ARBITRARY_PYTHON
    ).release_may_call


def test_safe_replacement_does_not_widen_later_builtin_shape() -> None:
    source = "x = ()\nx = ()\nresult = tuple()\n"
    call = _statement_call(source)
    index, fact = _analyzed_call(source, call)

    assert fact.exact_builtin_name() == "tuple"
    assert index.expression_result(call).kind == "tuple"


@pytest.mark.parametrize(
    "body",
    [
        "if flag is None:\n    x = ()\nelse:\n    x = tuple()",
        "x = ()\nwhile flag is None:\n    x = tuple()",
    ],
)
def test_stable_module_branch_and_loop_fixpoints_retain_common_result_shape(
    body: str,
) -> None:
    source = f"{body}\nresult = x\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    assignment = tree.body[-1]
    assert isinstance(assignment, ast.Assign)
    assert isinstance(assignment.value, ast.Name)

    result = index.expression_result(assignment.value)
    assert result.kind == "tuple"
    assert result.length == 0


def test_deferred_loop_does_not_claim_constructor_shape_from_foreign_globals() -> None:
    source = (
        "def run(flag):\n"
        "    x = ()\n"
        "    while flag is None:\n"
        "        x = tuple()\n"
        "    return x\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    returned = next(node for node in ast.walk(tree) if isinstance(node, ast.Return))
    assert isinstance(returned.value, ast.Name)

    assert index.expression_result(call) == UNKNOWN_EXPRESSION_RESULT
    assert index.expression_result(returned.value) == UNKNOWN_EXPRESSION_RESULT


def test_callbackful_condition_can_invalidate_builtin_shape_before_join() -> None:
    source = (
        "def run(flag):\n"
        "    if flag:\n"
        "        x = ()\n"
        "    else:\n"
        "        x = tuple()\n"
        "    return x\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    returned = next(node for node in ast.walk(tree) if isinstance(node, ast.Return))
    assert isinstance(returned.value, ast.Name)

    # flag.__bool__ may replace builtins.tuple before the else call executes.
    assert index.expression_result(returned.value) == UNKNOWN_EXPRESSION_RESULT


@pytest.mark.parametrize(
    ("expression", "kind"),
    [
        ("[callback(item) for item in source]", "list"),
        ("{callback(item) for item in source}", "set"),
        ("{callback(item): callback(item) for item in source}", "dict"),
    ],
)
def test_comprehension_normal_result_keeps_exact_outer_kind(
    expression: str, kind: str
) -> None:
    source = f"result = {expression}\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    assignment = tree.body[0]
    assert isinstance(assignment, ast.Assign)

    result = index.expression_result(assignment.value)
    assert result.kind == kind
    assert result.truth is None
    assert result.items is None
    assert result.length is None
    assert result.release_may_call


def test_deferred_future_widening_clears_stale_result_shape() -> None:
    source = "x = ()\ndef read():\n    return x\nx = []\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    returned = next(node for node in ast.walk(tree) if isinstance(node, ast.Return))
    assert isinstance(returned.value, ast.Name)
    assert index.expression_result(returned.value) == UNKNOWN_EXPRESSION_RESULT


def test_mutable_publication_strips_alias_sensitive_facts() -> None:
    item = StaticExpressionResult.scalar(1)
    result = StaticExpressionResult(
        truth=True,
        kind="list",
        items=(ExpressionSequenceItem(item),),
        release_may_call=False,
        fresh_container=True,
        length=1,
    )
    published = expression_result_for_publication(result)

    assert published.kind == "list"
    assert published.truth is None
    assert published.items is None
    assert published.length is None
    assert not published.fresh_container
    assert published.release_may_call


def test_clean_name_load_transports_published_mutable_kind() -> None:
    source = "value = [1]\nresult = value\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    assignment = tree.body[-1]
    assert isinstance(assignment, ast.Assign)
    assert isinstance(assignment.value, ast.Name)

    result = index.expression_result(assignment.value)
    assert result.kind == "list"
    assert result.truth is None
    assert result.items is None
    assert result.length is None
    assert not result.fresh_container
    assert result.release_may_call


@pytest.mark.parametrize("value", ["[]", "{}", "{0}", "([],)"])
def test_published_reference_container_release_can_run_callbacks(value: str) -> None:
    source = (
        "def run(callback):\n"
        f"    value = {value}\n"
        "    callback(value)\n"
        "    value = None\n"
    )
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    replacement = function.body[-1]
    assert isinstance(replacement, ast.Assign)
    fact = analyze_python_source_bindings(source).statement_fact(replacement)
    assert fact is not None
    assert fact.effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK) == (
        RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
    )


def test_published_reference_free_tuple_release_stays_callback_free() -> None:
    source = (
        "def run(callback):\n    value = ()\n    callback(value)\n    value = None\n"
    )
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    replacement = function.body[-1]
    assert isinstance(replacement, ast.Assign)
    fact = analyze_python_source_bindings(source).statement_fact(replacement)
    assert fact is not None
    assert not fact.effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)


def test_exact_bytearray_release_stays_callback_free_after_publication() -> None:
    source = "value = bytearray(b'payload')\nvalue = None\n"
    tree = ast.parse(source)
    replacement = tree.body[-1]
    assert isinstance(replacement, ast.Assign)
    index = analyze_python_source_bindings(source)
    fact = index.statement_fact(replacement)
    assert fact is not None
    assert not fact.effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)
    initial = tree.body[0]
    assert isinstance(initial, ast.Assign)
    assert index.expression_result(initial.value).kind == "bytearray"
    assert not index.expression_result(initial.value).release_may_call


def test_bytearray_publication_retains_recursive_release_safety() -> None:
    bytearray_result = StaticExpressionResult(
        kind="bytearray", release_may_call=False, fresh_container=True
    )
    published = expression_result_for_publication(bytearray_result)
    assert published.kind == "bytearray"
    assert not published.fresh_container
    assert not published.release_may_call

    containing_tuple = StaticExpressionResult(
        kind="tuple",
        items=(ExpressionSequenceItem(bytearray_result),),
        release_may_call=False,
        length=1,
    )
    published_tuple = expression_result_for_publication(containing_tuple)
    assert published_tuple.kind == "tuple"
    assert published_tuple.items is None
    assert not published_tuple.release_may_call
    assert expression_result_for_publication(published_tuple) is published_tuple


def test_publication_safe_tuple_alias_chain_retains_release_proof() -> None:
    source = (
        "value = (bytearray(),)\n"
        "alias = value\n"
        "last = alias\n"
        "value = None\n"
        "alias = None\n"
        "last = None\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)

    for replacement in tree.body[-3:]:
        assert isinstance(replacement, ast.Assign)
        fact = index.statement_fact(replacement)
        assert fact is not None
        assert not fact.effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)


def test_value_selectors_transport_publication_release_proof() -> None:
    source = (
        "value = (bytearray(),)\n"
        "named = (alias := value)\n"
        "selected = value if True else missing\n"
        "chosen = False or value\n"
        "value = alias = named = selected = chosen = None\n"
    )
    tree = ast.parse(source)
    replacement = tree.body[-1]
    assert isinstance(replacement, ast.Assign)
    fact = analyze_python_source_bindings(source).statement_fact(replacement)
    assert fact is not None
    assert not fact.effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)


def test_publication_proof_is_semantic_across_shape_erasure_and_join() -> None:
    bytearray_result = StaticExpressionResult(
        kind="bytearray", release_may_call=False, fresh_container=True
    )
    safe_results = tuple(
        expression_result_for_publication(
            StaticExpressionResult(
                truth=True,
                kind="tuple",
                items=(ExpressionSequenceItem(bytearray_result),) * length,
                release_may_call=False,
                length=length,
            )
        )
        for length in (1, 2)
    )
    joined_safe = join_static_expression_results(safe_results)
    assert not joined_safe.release_may_call
    assert expression_result_for_publication(joined_safe) is joined_safe

    empty_list = StaticExpressionResult(
        truth=False,
        kind="list",
        items=(),
        release_may_call=False,
        fresh_container=True,
        length=0,
    )
    unsafe_results = tuple(
        StaticExpressionResult(
            truth=True,
            kind="tuple",
            items=(ExpressionSequenceItem(empty_list),) * length,
            release_may_call=False,
            length=length,
        )
        for length in (1, 2)
    )
    joined_unsafe = join_static_expression_results(unsafe_results)
    assert expression_result_for_publication(joined_unsafe).release_may_call
    assert joined_safe != joined_unsafe
    mixed = join_static_expression_results((joined_safe, joined_unsafe))
    assert expression_result_for_publication(mixed).release_may_call


def test_tuple_constructor_transports_erased_publication_proof_only_for_tuple() -> None:
    bytearray_result = StaticExpressionResult(
        kind="bytearray", release_may_call=False, fresh_container=True
    )
    source = StaticExpressionResult(
        truth=True,
        kind="tuple",
        items=(ExpressionSequenceItem(bytearray_result),),
        release_may_call=False,
        length=1,
    )
    published = expression_result_for_publication(source)

    tuple_result = builtin_call_shape(
        "tuple", _call("tuple(value)"), (published,)
    ).result
    list_result = builtin_call_shape("list", _call("list(value)"), (published,)).result
    assert not expression_result_for_publication(tuple_result).release_may_call
    assert expression_result_for_publication(list_result).release_may_call


@pytest.mark.parametrize(
    ("source", "release_may_call"),
    [
        (
            "for item in (box := [bytearray()]):\n    globals()\n",
            True,
        ),
        ("for item in [bytearray()]:\n    pass\n", False),
        ("for item in []:\n    globals()\n", False),
    ],
)
def test_loop_terminal_release_uses_only_reachable_body_boundaries(
    source: str, release_may_call: bool
) -> None:
    tree = ast.parse(source)
    loop = tree.body[-1]
    assert isinstance(loop, ast.For)
    fact = analyze_python_source_bindings(source).statement_fact(loop)
    assert fact is not None and fact.iteration is not None
    assert bool(fact.iteration.release_effects & RUNS_FINALIZER) is release_may_call


def test_loop_target_transports_element_release_fact_across_backedge() -> None:
    source = "for item in ['left', 'right']:\n    pass\n"
    tree = ast.parse(source)
    loop = tree.body[0]
    assert isinstance(loop, ast.For)
    fact = analyze_python_source_bindings(source).statement_fact(loop)
    assert fact is not None and fact.iteration is not None
    assert fact.iteration.element_result.kind == "str"
    assert not fact.iteration.element_result.release_may_call
    assert not fact.effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)


@pytest.mark.parametrize(
    ("source", "release_may_call"),
    [
        (
            "result = [globals() for item in (box := [bytearray()])]\n",
            True,
        ),
        ("result = [None for item in [bytearray()]]\n", False),
        ("result = [globals() for item in []]\n", False),
    ],
)
def test_comprehension_terminal_release_uses_only_reachable_body_boundaries(
    source: str, release_may_call: bool
) -> None:
    tree = ast.parse(source)
    assignment = tree.body[0]
    assert isinstance(assignment, ast.Assign)
    fact = analyze_python_source_bindings(source).expression_fact(assignment.value)
    assert fact is not None
    assert bool(fact.effects & RUNS_FINALIZER) is release_may_call


def test_globals_member_store_transports_rhs_result() -> None:
    source = "globals()['saved'] = ()\nresult = saved\n"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    assignment = tree.body[-1]
    assert isinstance(assignment, ast.Assign)
    assert isinstance(assignment.value, ast.Name)

    result = index.expression_result(assignment.value)
    assert result.kind == "tuple"
    assert result.items == ()
    assert result.length == 0


@pytest.mark.parametrize(
    "source",
    [
        "result = globals()\n",
        "namespace = globals()\nresult = namespace\n",
        "def owner(): pass\nresult = owner.__globals__\n",
        "read = globals\nresult = read()\n",
    ],
)
def test_module_namespace_projects_exact_kind_without_contents(source: str) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    result = index.expression_result(tree.body[-1].value)
    assert result.kind == "dict"
    assert result.evaluation_required
    assert result.truth is None
    assert result.items is None
    assert result.length is None
    assert not result.fresh_container


@pytest.mark.parametrize(
    "source",
    [
        "globals = replacement\nresult = globals()\n",
        "def owner():\n    return globals()\n",
        "def owner():\n    namespace = globals()\n    return namespace\n",
    ],
)
def test_namespace_identity_without_bootstrap_custody_is_not_exact_dict(
    source: str,
) -> None:
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    statement = tree.body[-1]
    node = (
        statement.body[-1].value
        if isinstance(statement, ast.FunctionDef)
        else statement.value
    )
    assert index.expression_result(node).kind == "unknown"


def test_reflective_read_retains_rooted_callee_without_frontend_elision() -> None:
    source = "result = globals()\n"
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert not fact.callee_elision_safe
    assert fact.callee_retention_safe
    assert fact.cleanup_effects == RELEASES_REFERENCE
    assert not fact.cleanup_effects & (RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK)


def test_callbackful_invocation_does_not_claim_rooted_callee_retention() -> None:
    source = "result = callback()\n"
    call = _statement_call(source)
    _index, fact = _analyzed_call(source, call)

    assert not fact.callee_retention_safe
    assert fact.cleanup_effects & RUNS_FINALIZER
    assert fact.cleanup_effects & RUNS_WEAKREF_CALLBACK


def test_recursively_stable_tuple_publication_retains_contents() -> None:
    result = StaticExpressionResult(
        truth=True,
        kind="tuple",
        items=(
            ExpressionSequenceItem(StaticExpressionResult.scalar(1)),
            ExpressionSequenceItem(StaticExpressionResult.scalar("two")),
        ),
        release_may_call=False,
        length=2,
    )
    assert expression_result_for_publication(result) == result


def test_unstable_nested_immutable_publication_widens_release_custody() -> None:
    nested = StaticExpressionResult(
        truth=True,
        kind="list",
        items=(ExpressionSequenceItem(UNKNOWN_EXPRESSION_RESULT),),
        release_may_call=False,
        fresh_container=True,
        length=1,
    )
    result = StaticExpressionResult(
        truth=True,
        kind="tuple",
        items=(ExpressionSequenceItem(nested),),
        release_may_call=False,
        length=1,
    )
    published = expression_result_for_publication(result)

    assert published.kind == "tuple"
    assert published.truth is True
    assert published.items is None
    assert published.length == 1
    assert published.release_may_call


def test_result_joins_preserve_common_kind_without_cross_kind_invention() -> None:
    integer = join_static_expression_results(
        (StaticExpressionResult.scalar(1), StaticExpressionResult.scalar(2))
    )
    mixed = join_static_expression_results(
        (StaticExpressionResult.scalar(1), StaticExpressionResult.scalar("1"))
    )

    assert integer.kind == "int"
    assert not integer.value_known
    assert mixed.kind == "unknown"


def test_literal_identity_distinguishes_signed_zero_nan_sign_and_bool_int() -> None:
    positive_zero = StaticExpressionResult.scalar(0.0)
    negative_zero = StaticExpressionResult.scalar(-0.0)
    positive_nan = StaticExpressionResult.scalar(float("nan"))
    negative_nan = StaticExpressionResult.scalar(-float("nan"))

    assert positive_zero != negative_zero
    assert positive_nan != negative_nan
    assert StaticExpressionResult.scalar(True) != StaticExpressionResult.scalar(1)
    joined_zero = join_static_expression_results((positive_zero, negative_zero))
    assert not joined_zero.value_known


@pytest.mark.parametrize("name", ["bytes", "bytearray"])
def test_huge_integer_shapes_never_materialize_host_containers(name: str) -> None:
    huge = StaticExpressionResult.scalar(1 << 1_000_000)
    node = _call(f"{name}(value)")
    shape = builtin_call_shape(name, node, (huge,))

    assert shape.result.kind == name
    assert not shape.result.value_known


def test_ordered_constructor_preserves_compact_expansion_provenance() -> None:
    one = StaticExpressionResult.scalar(1)
    two = StaticExpressionResult.scalar(2)
    nested = StaticExpressionResult(
        truth=True,
        kind="tuple",
        items=(ExpressionSequenceItem(two),),
        release_may_call=False,
        length=1,
    )
    ordered = StaticExpressionResult(
        truth=True,
        kind="list",
        items=(
            ExpressionSequenceItem(one),
            ExpressionSequenceItem(nested, expanded=True),
        ),
        release_may_call=False,
        fresh_container=True,
        length=2,
    )
    tuple_shape = builtin_call_shape("tuple", _call("tuple(value)"), (ordered,))
    assert tuple_shape.result.items is ordered.items
    assert tuple_shape.result.length == 2
    assert tuple_shape.result.items is not None
    assert tuple_shape.result.items[1].expanded

    for name in ("set", "frozenset", "dict"):
        shape = builtin_call_shape(name, _call(f"{name}(value)"), (ordered,))
        assert shape.result.items is None


def test_deep_shared_expansion_hash_and_equality_are_compact_and_iterative() -> None:
    left = _shared_expansion_dag(1_500)
    right = _shared_expansion_dag(1_500)

    assert hash(left) == hash(right)
    assert left == right
    assert expression_result_for_publication(left) is left


def test_deep_shared_mutable_leaf_is_cached_as_publication_unstable() -> None:
    mutable = StaticExpressionResult(
        kind="list", release_may_call=False, fresh_container=True
    )
    result = _shared_expansion_dag(1_500, mutable)
    published = expression_result_for_publication(result)

    assert published.kind == "tuple"
    assert published.items is None
    assert published.release_may_call


def test_result_equality_checks_children_after_cached_hash_collisions() -> None:
    one = StaticExpressionResult.scalar(1)
    two = StaticExpressionResult.scalar(2)
    object.__setattr__(one, "_semantic_hash", 7)
    object.__setattr__(two, "_semantic_hash", 7)
    left = StaticExpressionResult(
        kind="tuple",
        items=(ExpressionSequenceItem(one),),
        release_may_call=False,
    )
    right = StaticExpressionResult(
        kind="tuple",
        items=(ExpressionSequenceItem(two),),
        release_may_call=False,
    )

    assert hash(left) == hash(right)
    assert left != right


def test_tuple_and_list_constructors_reuse_deep_provenance_dag() -> None:
    source = _shared_expansion_dag(1_500)
    for name in ("tuple", "list"):
        shape = builtin_call_shape(name, _call(f"{name}(value)"), (source,))
        assert shape.result.items is source.items


def test_iteration_and_membership_visit_shared_expansions_once() -> None:
    result = _shared_expansion_dag(1_500)
    iteration = PythonIterationFact.from_result(result, 0)
    membership = static_comparison_result(
        StaticExpressionResult.scalar("leaf"), ast.In(), result
    )

    assert iteration.element_strings is not None
    assert iteration.element_strings.values == frozenset({"leaf"})
    assert iteration.element_result.kind == "str"
    assert not iteration.element_result.release_may_call
    assert membership.truth is True


@pytest.mark.parametrize("unknown_first", [False, True])
def test_unknown_expansion_invalidates_all_finite_iteration_facts(
    unknown_first: bool,
) -> None:
    items = (
        ExpressionSequenceItem(StaticExpressionResult.scalar("known")),
        ExpressionSequenceItem(UNKNOWN_EXPRESSION_RESULT, expanded=True),
    )
    result = StaticExpressionResult(
        kind="tuple", items=items[::-1] if unknown_first else items
    )

    iteration = PythonIterationFact.from_result(result, 0)

    assert iteration.element_strings is None
    assert iteration.element_result is UNKNOWN_EXPRESSION_RESULT
    assert not iteration.empty


@pytest.mark.parametrize(
    ("name", "expression", "arguments"),
    [
        ("range", "range(value)", (StaticExpressionResult.scalar(True),)),
        ("bytearray", "bytearray(value)", (StaticExpressionResult.scalar(3),)),
        ("bytes", "bytes(value)", (StaticExpressionResult.scalar(False),)),
        (
            "int",
            "int(text, base)",
            (StaticExpressionResult.scalar("10"), StaticExpressionResult.scalar(2)),
        ),
        (
            "int",
            "int(text, base=base)",
            (StaticExpressionResult.scalar(b"10"), StaticExpressionResult.scalar(2)),
        ),
        (
            "complex",
            "complex(real, imag)",
            (StaticExpressionResult.scalar(1), StaticExpressionResult.scalar(2.0)),
        ),
        (
            "dict",
            "dict(value)",
            (StaticExpressionResult(kind="dict", release_may_call=True),),
        ),
    ],
)
def test_primitive_signature_siblings_are_callback_free(
    name: str,
    expression: str,
    arguments: tuple[StaticExpressionResult, ...],
) -> None:
    shape = builtin_call_shape(name, _call(expression), arguments)
    assert not shape.invocation_effects & (
        EXECUTES_ARBITRARY_PYTHON | INVOKES_ITERATION_CALLBACK
    )


def test_two_complex_operands_remain_generic_for_cross_version_warning_hooks() -> None:
    shape = builtin_call_shape(
        "complex",
        _call("complex(real, imag)"),
        (
            StaticExpressionResult(kind="complex", release_may_call=False),
            StaticExpressionResult.scalar(1),
        ),
    )
    assert shape.invocation_effects & EXECUTES_ARBITRARY_PYTHON


def test_safe_list_mutation_preserves_kind_and_homogeneous_element_result() -> None:
    source = "items = [1]\nitems.append(2)\nfor item in items:\n    pass\n"
    tree = ast.parse(source)
    loop = tree.body[-1]
    assert isinstance(loop, ast.For)
    index = analyze_python_source_bindings(source)
    iteration = index.iteration_fact(loop)
    assert iteration is not None

    assert index.expression_result(loop.iter).kind == "list"
    assert iteration.element_result.kind == "int"


def test_arbitrary_callback_expires_element_result_but_keeps_live_local_kind() -> None:
    source = (
        "def run(callback):\n"
        "    items = [1]\n"
        "    callback()\n"
        "    for item in items:\n"
        "        pass\n"
    )
    tree = ast.parse(source)
    loop = next(node for node in ast.walk(tree) if isinstance(node, ast.For))
    index = analyze_python_source_bindings(source)
    iteration = index.iteration_fact(loop)
    assert iteration is not None

    assert index.expression_result(loop.iter).kind == "list"
    assert iteration.element_result is UNKNOWN_EXPRESSION_RESULT


def test_argument_rebinding_does_not_restore_bound_method_receiver_fact() -> None:
    source = (
        "items = [1]\n"
        "items.append((items := ['replacement']))\n"
        "for item in items:\n"
        "    pass\n"
    )
    tree = ast.parse(source)
    loop = tree.body[-1]
    assert isinstance(loop, ast.For)
    iteration = analyze_python_source_bindings(source).iteration_fact(loop)
    assert iteration is not None

    assert iteration.element_result.kind == "str"


@pytest.mark.parametrize(
    "source",
    [
        (
            "items = [1]\n"
            "def replace():\n"
            "    global items\n"
            "    items = ['replacement']\n"
            "replace()\n"
            "for item in items:\n"
            "    pass\n"
        ),
        (
            "def run():\n"
            "    items = [1]\n"
            "    def replace():\n"
            "        nonlocal items\n"
            "        items = ['replacement']\n"
            "    replace()\n"
            "    for item in items:\n"
            "        pass\n"
        ),
    ],
)
def test_callback_visible_global_and_nonlocal_rebinding_never_keeps_stale_element(
    source: str,
) -> None:
    tree = ast.parse(source)
    loop = next(node for node in ast.walk(tree) if isinstance(node, ast.For))
    iteration = analyze_python_source_bindings(source).iteration_fact(loop)
    assert iteration is not None

    assert iteration.element_result.kind != "int"


def test_callbackful_index_protocol_cannot_restore_or_derive_list_contents() -> None:
    receiver = StaticExpressionResult(
        kind="list",
        element_result=StaticExpressionResult.scalar(1),
    )
    shape = builtin_method_call_shape(
        receiver,
        "pop",
        _call("items.pop(index)"),
        (UNKNOWN_EXPRESSION_RESULT,),
    )
    assert shape is not None

    assert shape.result is UNKNOWN_EXPRESSION_RESULT
    assert shape.receiver_after is None
    assert shape.invocation_effects & EXECUTES_ARBITRARY_PYTHON


def test_setdefault_does_not_claim_default_for_unknown_existing_value() -> None:
    receiver = StaticExpressionResult(
        truth=True,
        kind="dict",
        element_result=StaticExpressionResult.scalar("key"),
    )
    shape = builtin_method_call_shape(
        receiver,
        "setdefault",
        _call("mapping.setdefault('key', 7)"),
        (StaticExpressionResult.scalar("key"), StaticExpressionResult.scalar(7)),
    )
    assert shape is not None

    assert shape.result is UNKNOWN_EXPRESSION_RESULT


@pytest.mark.parametrize(
    ("receiver_kind", "method", "expression", "arguments"),
    [
        (
            "bytes",
            "split",
            "value.split(separator)",
            (UNKNOWN_EXPRESSION_RESULT,),
        ),
        (
            "bytearray",
            "replace",
            "value.replace(old, new)",
            (UNKNOWN_EXPRESSION_RESULT, StaticExpressionResult.scalar(b"new")),
        ),
        (
            "bytes",
            "find",
            "value.find(needle)",
            (UNKNOWN_EXPRESSION_RESULT,),
        ),
        (
            "bytes",
            "startswith",
            "value.startswith(prefixes)",
            (
                StaticExpressionResult(
                    kind="tuple", element_result=UNKNOWN_EXPRESSION_RESULT
                ),
            ),
        ),
        (
            "bytearray",
            "join",
            "value.join(parts)",
            (
                StaticExpressionResult(
                    kind="list", element_result=UNKNOWN_EXPRESSION_RESULT
                ),
            ),
        ),
    ],
)
def test_buffer_method_arguments_retain_pep688_callback_boundary(
    receiver_kind: ExpressionKind,
    method: str,
    expression: str,
    arguments: tuple[StaticExpressionResult, ...],
) -> None:
    shape = builtin_method_call_shape(
        StaticExpressionResult(kind=receiver_kind),
        method,
        _call(expression),
        arguments,
    )
    assert shape is not None

    assert shape.invocation_effects & EXECUTES_ARBITRARY_PYTHON


@pytest.mark.parametrize(
    ("receiver_kind", "method", "expression", "arguments"),
    [
        (
            "bytes",
            "split",
            "value.split(separator)",
            (StaticExpressionResult.scalar(b","),),
        ),
        (
            "bytearray",
            "replace",
            "value.replace(old, new)",
            (
                StaticExpressionResult.scalar(b"old"),
                StaticExpressionResult(kind="bytearray", release_may_call=False),
            ),
        ),
        (
            "bytes",
            "startswith",
            "value.startswith(prefixes)",
            (
                StaticExpressionResult(
                    kind="tuple",
                    element_result=StaticExpressionResult.scalar(b"prefix"),
                ),
            ),
        ),
        (
            "bytearray",
            "join",
            "value.join(parts)",
            (
                StaticExpressionResult(
                    kind="list",
                    element_result=StaticExpressionResult.scalar(b"part"),
                ),
            ),
        ),
    ],
)
def test_exact_builtin_buffer_arguments_are_callback_free(
    receiver_kind: ExpressionKind,
    method: str,
    expression: str,
    arguments: tuple[StaticExpressionResult, ...],
) -> None:
    shape = builtin_method_call_shape(
        StaticExpressionResult(kind=receiver_kind),
        method,
        _call(expression),
        arguments,
    )
    assert shape is not None

    assert not shape.invocation_effects & EXECUTES_ARBITRARY_PYTHON


def test_explicit_open_encoding_retains_codec_registry_callback_boundary() -> None:
    node = _call("open('data.txt', encoding='user-codec')")
    result, effects = builtin_open_result(
        node,
        (
            StaticExpressionResult.scalar("data.txt"),
            StaticExpressionResult.scalar("user-codec"),
        ),
    )

    assert result.kind == "file_text"
    assert effects & EXECUTES_ARBITRARY_PYTHON


def test_callback_expiry_sanitizes_mutable_element_nested_in_immutable_owner() -> None:
    mutable_element = StaticExpressionResult(
        kind="list",
        element_result=StaticExpressionResult.scalar(1),
    )
    owner = StaticExpressionResult(kind="tuple", element_result=mutable_element)
    expired = expression_result_without_mutable_contents(owner)
    projected = expired.element_result
    assert projected is not None

    assert expired.kind == "tuple"
    assert projected.kind == "list"
    assert projected.element_result is None


def test_deep_element_graph_join_and_expiry_are_iterative() -> None:
    depth = 1_500
    left = _deep_element_chain(depth, StaticExpressionResult.scalar(1))
    right = _deep_element_chain(depth, StaticExpressionResult.scalar(2))
    joined = join_static_expression_results((left, right))
    mutable = _deep_element_chain(
        depth,
        StaticExpressionResult(
            kind="list", element_result=StaticExpressionResult.scalar(1)
        ),
    )
    expired = expression_result_without_mutable_contents(mutable)

    for _ in range(depth):
        assert joined.kind == "tuple"
        assert expired.kind == "tuple"
        assert joined.element_result is not None
        assert expired.element_result is not None
        joined = joined.element_result
        expired = expired.element_result
    assert joined.kind == "int"
    assert not joined.value_known
    assert expired.kind == "list"
    assert expired.element_result is None


@pytest.mark.parametrize(
    ("expression", "element_kind"),
    [("open('data.txt')", "str"), ("open('data.bin', 'rb')", "bytes")],
)
def test_exact_file_context_and_loop_publish_iteration_element(
    expression: str, element_kind: str
) -> None:
    source = f"with {expression} as stream:\n    for item in stream:\n        pass\n"
    tree = ast.parse(source)
    loop = next(node for node in ast.walk(tree) if isinstance(node, ast.For))
    iteration = analyze_python_source_bindings(source).iteration_fact(loop)
    assert iteration is not None

    assert iteration.element_result.kind == element_kind


@pytest.mark.parametrize(
    "source",
    [
        "import builtins\nstream = builtins.open('data.txt')\n",
        "from builtins import open as acquire\nstream = acquire('data.txt')\n",
    ],
)
def test_builtin_open_import_paths_share_exact_file_result_authority(
    source: str,
) -> None:
    call = _statement_call(source)
    index, fact = _analyzed_call(source, call)

    assert fact.callee_is(PythonIdentity.BUILTIN_OPEN)
    assert index.expression_result(call).kind == "file_text"


def test_comprehension_transports_iteration_target_and_result_element() -> None:
    source = "values = [item for item in [1, 2]]\nfor value in values:\n    pass\n"
    tree = ast.parse(source)
    assignment = tree.body[0]
    loop = tree.body[1]
    assert isinstance(assignment, ast.Assign)
    assert isinstance(assignment.value, ast.ListComp)
    assert isinstance(loop, ast.For)
    index = analyze_python_source_bindings(source)
    clause = assignment.value.generators[0]
    clause_iteration = index.iteration_fact(clause)
    result_iteration = index.iteration_fact(loop)
    assert clause_iteration is not None
    assert result_iteration is not None

    assert clause_iteration.element_result.kind == "int"
    assert index.expression_result(assignment.value.elt).kind == "int"
    assert index.expression_result(assignment.value).element_result is not None
    assert index.expression_result(assignment.value).element_result.kind == "int"
    assert result_iteration.element_result.kind == "int"


@pytest.mark.parametrize(
    ("name", "method"),
    [("str", "__str__"), ("bytes", "__bytes__")],
)
def test_hook_returned_strict_subclasses_do_not_publish_exact_builtin_shape(
    name: str, method: str
) -> None:
    subclass = "Text" if name == "str" else "Binary"
    base_literal = "'value'" if name == "str" else "b'value'"
    source = (
        f"class {subclass}({name}):\n    pass\n"
        f"class Value:\n    def {method}(self):\n"
        f"        return {subclass}({base_literal})\n"
        f"value = {name}(Value())\n"
        f"size = len({name}(Value()))\n"
    )
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    value_assignment = tree.body[2]
    size_assignment = tree.body[3]
    assert isinstance(value_assignment, ast.Assign)
    assert isinstance(value_assignment.value, ast.Call)
    assert isinstance(size_assignment, ast.Assign)
    assert isinstance(size_assignment.value, ast.Call)
    conversion = value_assignment.value
    chained_conversion = size_assignment.value.args[0]
    assert isinstance(chained_conversion, ast.Call)

    conversion_fact = index.call_fact(conversion)
    chained_fact = index.call_fact(size_assignment.value)
    assert conversion_fact is not None
    assert chained_fact is not None
    assert conversion_fact.invocation_effects & EXECUTES_ARBITRARY_PYTHON
    assert index.expression_result(conversion) == UNKNOWN_EXPRESSION_RESULT
    assert index.expression_result(chained_conversion) == UNKNOWN_EXPRESSION_RESULT
    assert chained_fact.invocation_effects & EXECUTES_ARBITRARY_PYTHON
    assert not chained_fact.callee_elision_safe


@pytest.mark.parametrize("name", ["str", "bytes"])
def test_codec_forms_do_not_claim_exact_result_shape(name: str) -> None:
    node = _call(f"{name}(value, encoding)")
    shape = builtin_call_shape(
        name,
        node,
        (
            StaticExpressionResult.scalar(b"1"),
            StaticExpressionResult.scalar("utf-8"),
        )
        if name == "str"
        else (
            StaticExpressionResult.scalar("1"),
            StaticExpressionResult.scalar("utf-8"),
        ),
    )

    assert shape.invocation_effects & EXECUTES_ARBITRARY_PYTHON
    assert shape.result == UNKNOWN_EXPRESSION_RESULT
