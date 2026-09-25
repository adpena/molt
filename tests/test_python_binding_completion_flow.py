from __future__ import annotations

import ast

import pytest

from molt.compiler_analysis.python_binding_facts import (
    PythonCompletion as Completion,
    PythonCompletionFlow as Flow,
    PythonIdentity,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_source_bindings,
)
from molt.compiler_analysis.python_effects_generated import (
    INVOKES_COMPARISON_CALLBACK,
    RAISES,
)
from molt.compiler_analysis.python_source_keys import python_ast_digest


def _join_events(left: frozenset[str], right: frozenset[str]) -> frozenset[str]:
    return left | right


@pytest.mark.parametrize(
    "left,right",
    [
        (False, 0),
        (1, 1.0),
        (0.0, -0.0),
        (0j, complex(-0.0, 0.0)),
        ("value", b"value"),
        (None, Ellipsis),
        ([1], (1,)),
        ((1, 2), (2, 1)),
    ],
)
def test_ast_identity_frames_scalar_types_and_container_order(
    left: object, right: object
) -> None:
    assert python_ast_digest(ast.Constant(value=left)) != python_ast_digest(
        ast.Constant(value=right)
    )


def test_ast_identity_owns_spans_fields_and_unordered_constant_members() -> None:
    left = ast.parse("value = 1\n", filename="first.py")
    right = ast.parse("value = 1\n", filename="second.py")
    assert python_ast_digest(left) == python_ast_digest(right)
    ast.increment_lineno(right)
    assert python_ast_digest(left) != python_ast_digest(right)
    assert python_ast_digest(ast.Constant(value=frozenset((1, "a", (2, 3))))) == (
        python_ast_digest(ast.Constant(value=frozenset(((2, 3), "a", 1))))
    )
    missing = ast.Constant()
    present = ast.Constant(value=None)
    assert python_ast_digest(missing) != python_ast_digest(present)
    with pytest.raises(TypeError, match="unsupported Python AST identity value"):
        python_ast_digest(ast.Constant(value=object()))


@pytest.mark.parametrize("cycle", ["ast", "list", "tuple"])
def test_ast_identity_rejects_cycles_in_synthetic_inputs(cycle: str) -> None:
    tree = ast.UnaryOp(op=ast.UAdd(), operand=ast.Constant(value=1))
    if cycle == "ast":
        tree.operand = tree
    else:
        values: list[object] = []
        values.append(values if cycle == "list" else (values,))
        tree.operand = ast.Constant(value=values)
    with pytest.raises(ValueError, match="cyclic Python AST identity value"):
        python_ast_digest(tree)


def test_ast_identity_shared_subtrees_have_value_identity() -> None:
    child = ast.Constant(value=1)
    shared = ast.Tuple(elts=[child, child], ctx=ast.Load())
    distinct = ast.Tuple(
        elts=[ast.Constant(value=1), ast.Constant(value=1)], ctx=ast.Load()
    )
    assert python_ast_digest(shared) == python_ast_digest(distinct)


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_deep_elif_chain_preserves_every_fact_and_terminal_tail(
    target: tuple[int, int],
) -> None:
    source = (
        "def select(value):\n"
        + "".join(
            f"    {'if' if value == 0 else 'elif'} value == {value}:\n"
            f"        return {value}\n"
            for value in range(384)
        )
        + "    else:\n        return -1\n    unreachable()\n"
    )
    # This is ordinary parser/compiler-accepted Python, not a synthetic AST
    # beyond the host language's supported source nesting.
    compile(source, "<deep-elif>", "exec")
    tree = ast.parse(source)
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    conditionals = [node for node in ast.walk(tree) if isinstance(node, ast.If)]
    assert len(conditionals) == 384
    for node in conditionals:
        fact = index.statement_fact(node)
        assert fact is not None
        assert fact.completions & Completion.RETURN
        assert not fact.completions & Completion.NORMAL
        assert index.expression_fact(node.test) is not None
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    assert index.statement_fact(function.body[-1]) is None


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_deep_body_conditionals_share_the_stack_safe_scheduler(
    target: tuple[int, int],
) -> None:
    depth = 96
    source = (
        "def nested(flag):\n"
        + "".join(
            "    " * level + "if flag is None:\n" for level in range(1, depth + 1)
        )
        + "    " * (depth + 1)
        + "return 1\n    return 2\n    unreachable()\n"
    )
    compile(source, "<deep-body>", "exec")
    tree = ast.parse(source)
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    for node in ast.walk(tree):
        if isinstance(node, ast.If):
            fact = index.statement_fact(node)
            assert fact is not None
            assert fact.completions & (Completion.NORMAL | Completion.RETURN) == (
                Completion.NORMAL | Completion.RETURN
            )
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    assert index.statement_fact(function.body[-2]) is not None
    assert index.statement_fact(function.body[-1]) is None


def test_conditional_alternatives_do_not_share_successor_bindings() -> None:
    source = (
        "def choose(flag):\n"
        "    value = 'before'\n"
        "    if flag is None:\n"
        "        value = 'body'\n"
        "    else:\n"
        "        return value\n"
        "    return value\n"
    )
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    conditional = function.body[1]
    assert isinstance(conditional, ast.If)
    alternative = conditional.orelse[0]
    tail = function.body[-1]
    assert isinstance(alternative, ast.Return) and alternative.value is not None
    assert isinstance(tail, ast.Return) and tail.value is not None
    index = analyze_python_source_bindings(source)
    assert index.static_value(alternative.value) == "before"
    assert index.static_value(tail.value) == "body"


def test_conditional_test_failure_precedes_branch_observations() -> None:
    source = (
        "def choose(flag):\n"
        "    value = 'before'\n"
        "    try:\n"
        "        if flag:\n"
        "            value = 'body'\n"
        "    except:\n"
        "        return value\n"
    )
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    attempt = function.body[1]
    assert isinstance(attempt, ast.Try)
    returned = attempt.handlers[0].body[0]
    assert isinstance(returned, ast.Return) and returned.value is not None
    index = analyze_python_source_bindings(source)
    assert index.static_value(returned.value) == "before"


@pytest.mark.parametrize("incoming", list(Completion))
@pytest.mark.parametrize("final", [None, *list(Completion)])
def test_finally_preserves_or_overrides_each_completion(
    incoming: Completion,
    final: Completion | None,
) -> None:
    before = Flow.single(incoming, 1, effects=RAISES)
    calls: list[int] = []

    def execute(state: int) -> Flow[int]:
        calls.append(state)
        return Flow() if final is None else Flow.single(final, 2)

    result = before.apply_finally(execute, join_states=max)
    assert calls == [1]
    expected = (
        Completion.NONE
        if final is None
        else incoming
        if final == Completion.NORMAL
        else final
    )
    assert result.completions == expected
    assert result.effects & RAISES
    assert all(state == 2 for _kind, state in result.successors())


def test_sequence_visits_only_normal_and_preserves_pending_terminals() -> None:
    incoming = Flow(normal=1, returned=8, raised=9)
    calls: list[int] = []

    def execute(state: int) -> Flow[int]:
        calls.append(state)
        return Flow(broken=state + 1)

    result = incoming.sequence(execute, join_states=max).sequence(
        execute, join_states=max
    )
    assert calls == [1]
    assert result == Flow(returned=8, raised=9, broken=2)
    assert (
        Flow[int]().apply_finally(execute, join_states=max).completions
        == Completion.NONE
    )
    assert calls == [1]


@pytest.mark.parametrize("incoming", list(Completion))
def test_context_exit_suppresses_only_pending_raise(incoming: Completion) -> None:
    result = Flow.single(incoming, 1).unwind_context(
        lambda _state: Flow(normal=2, raised=3),
        join_states=max,
    )
    expected = incoming | Completion.RAISE
    if incoming == Completion.RAISE:
        expected |= Completion.NORMAL
    assert result.completions == expected
    assert result.raised == 3
    assert (result.normal is not None) == (
        incoming in {Completion.NORMAL, Completion.RAISE}
    )


def test_loop_break_skips_else_and_continue_reaches_exhaustion() -> None:
    else_states: list[int] = []

    def execute_else(state: int) -> Flow[int]:
        else_states.append(state)
        return Flow(normal=state + 10)

    broken = Flow.loop(
        0,
        lambda _header: (None, Flow(broken=4)),
        execute_else,
        join_states=max,
        equivalent_states=lambda left, right: left == right,
        widen_state=lambda state: state,
    )
    assert broken == Flow(normal=4)
    assert else_states == []

    def advance(header: int) -> tuple[int | None, Flow[int]]:
        return (None, Flow(continued=1)) if header == 0 else (header, Flow())

    continued = Flow.loop(
        0,
        advance,
        execute_else,
        join_states=max,
        equivalent_states=lambda left, right: left == right,
        widen_state=lambda state: state,
    )
    assert else_states == [1]
    assert continued == Flow(normal=11)


def test_loop_executes_widened_header_before_publishing_exhaustion() -> None:
    visits: list[int] = []
    widened: list[int] = []

    def advance(header: int) -> tuple[int | None, Flow[int]]:
        visits.append(header)
        return header, Flow(continued=header + 1)

    def widen(state: int) -> int:
        widened.append(state)
        return 100

    result = Flow.loop(
        0,
        advance,
        lambda exhausted: Flow(normal=exhausted),
        join_states=max,
        equivalent_states=lambda left, right: left == right,
        widen_state=widen,
    )
    assert len(widened) == 1
    assert visits[-1] == 100
    assert result.normal == 100


def test_exception_group_deferred_raise_does_not_skip_later_handlers() -> None:
    observed: list[tuple[str, frozenset[str]]] = []

    def execute(handler: str, state: frozenset[str]) -> Flow[frozenset[str]]:
        observed.append((handler, state))
        updated = state | {handler}
        return Flow(raised=updated) if handler == "first" else Flow(normal=updated)

    result = Flow.exception_group_handlers(
        frozenset(),
        ("first", "second"),
        evaluate_type=lambda _handler, state: Flow(normal=state),
        split_group=lambda state: Flow(normal=state),
        execute_handler=execute,
        merge_group=lambda state: Flow(normal=state),
        join_states=_join_events,
    )
    assert ("second", frozenset({"first"})) in observed
    assert result.normal == frozenset({"second"})
    assert result.raised == frozenset({"first", "second"})


def test_exception_group_terminal_handlers_cannot_fabricate_normal_exit() -> None:
    result = Flow.exception_group_handlers(
        0,
        (1, 2),
        evaluate_type=lambda _handler, state: Flow(normal=state),
        split_group=lambda state: Flow(normal=state),
        execute_handler=lambda handler, state: Flow(raised=state + handler),
        merge_group=lambda state: Flow(normal=state),
        join_states=max,
    )
    assert result.normal is None
    assert result.raised is not None


def test_exception_group_type_failure_does_not_execute_handlers() -> None:
    calls: list[int] = []

    def execute(handler: int, state: int) -> Flow[int]:
        calls.append(handler)
        return Flow(normal=state)

    result = Flow.exception_group_handlers(
        0,
        (1, 2),
        evaluate_type=lambda _handler, _state: Flow(raised=3),
        split_group=lambda state: Flow(normal=state),
        execute_handler=execute,
        merge_group=lambda state: Flow(normal=state),
        join_states=max,
    )
    assert calls == []
    assert result == Flow(raised=3)


def test_exception_group_protocol_callbacks_own_split_and_merge_order() -> None:
    observed: list[tuple[str, int]] = []

    def evaluate(_handler: str, state: int) -> Flow[int]:
        observed.append(("type", state))
        return Flow(normal=state + 1)

    def split(state: int) -> Flow[int]:
        observed.append(("split", state))
        return Flow(normal=state + 1)

    def handle(_handler: str, state: int) -> Flow[int]:
        observed.append(("handler", state))
        return Flow(normal=state + 1)

    def merge(state: int) -> Flow[int]:
        observed.append(("merge", state))
        return Flow(normal=state + 10)

    result = Flow.exception_group_handlers(
        0,
        ("handler",),
        evaluate_type=evaluate,
        split_group=split,
        execute_handler=handle,
        merge_group=merge,
        join_states=max,
    )
    assert observed[:3] == [("type", 0), ("split", 1), ("handler", 2)]
    assert observed[3:] == [("merge", 2), ("merge", 3)]
    assert result.normal == 3
    assert result.raised == 13


def test_exception_group_split_failure_aborts_remaining_handlers_and_merge() -> None:
    invoked: list[str] = []

    def handle(_handler: int, state: int) -> Flow[int]:
        invoked.append("handler")
        return Flow(normal=state)

    def merge(state: int) -> Flow[int]:
        invoked.append("merge")
        return Flow(normal=state)

    result = Flow.exception_group_handlers(
        0,
        (1, 2),
        evaluate_type=lambda _handler, state: Flow(normal=state),
        split_group=lambda _state: Flow(raised=7),
        execute_handler=handle,
        merge_group=merge,
        join_states=max,
    )
    assert invoked == []
    assert result == Flow(raised=7)


def test_exception_group_merge_failure_overrides_pending_exception() -> None:
    result = Flow.exception_group_handlers(
        0,
        (1,),
        evaluate_type=lambda _handler, state: Flow(normal=state),
        split_group=lambda state: Flow(normal=state),
        execute_handler=lambda _handler, _state: Flow(raised=2),
        merge_group=lambda _state: Flow(raised=9),
        join_states=max,
    )
    assert result == Flow(raised=9)


def test_cpython_exception_group_subclass_rebinds_between_handlers() -> None:
    source = (
        "marker = 'initial'\n"
        "class Group(ExceptionGroup):\n"
        "    def split(self, condition):\n"
        "        global marker\n"
        "        marker = 'split'\n"
        "        return super().split(condition)\n"
        "    def derive(self, exceptions):\n"
        "        global marker\n"
        "        marker = 'derive'\n"
        "        return Group(self.message, exceptions)\n"
        "try:\n"
        "    raise Group('group', [ValueError(), TypeError()])\n"
        "except* ValueError:\n"
        "    marker = 'clean'\n"
        "except* TypeError:\n"
        "    observed = marker\n"
    )
    namespace: dict[str, object] = {}
    exec(source, namespace)
    assert namespace["observed"] == "derive"
    tree = ast.parse(source)
    statement = tree.body[-1]
    assert isinstance(statement, ast.TryStar)
    assignment = statement.handlers[-1].body[0]
    assert isinstance(assignment, ast.Assign)
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(assignment.value)
    assert fact is not None and fact.static_value is None


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("terminal", ["return 1", "raise"])
def test_terminal_function_tail_has_no_binding_facts(
    target: tuple[int, int],
    terminal: str,
) -> None:
    source = f"def function():\n    {terminal}\n    unreachable()\n"
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    assert index.statement_fact(function.body[0]) is not None
    assert index.statement_fact(function.body[1]) is None
    assert index.statement_completions(function.body[1]) is None
    call = function.body[1]
    assert isinstance(call, ast.Expr) and isinstance(call.value, ast.Call)
    assert index.call_fact(call.value) is None


def test_terminal_handler_cannot_taint_surviving_normal_import_identity() -> None:
    source = (
        "import importlib\n"
        "try:\n    import sys\n"
        "except:\n    importlib = replacement\n    raise\n"
        "importlib.import_module('live')\n"
    )
    index = analyze_python_source_bindings(source)
    assert index.calls[-1].callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    ("body", "final", "completion"),
    [
        ("return 1", "pass", Completion.RETURN),
        ("return 1", "raise", Completion.RAISE),
        ("raise", "return 2", Completion.RETURN),
    ],
)
def test_binding_finally_routes_terminal_override(
    body: str,
    final: str,
    completion: Completion,
) -> None:
    source = f"def function():\n    try:\n        {body}\n    finally:\n        {final}\n    unreachable()\n"
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    index = analyze_python_source_bindings(source)
    assert index.statement_completions(function.body[0]) == completion
    assert index.statement_fact(function.body[1]) is None


@pytest.mark.parametrize(
    ("body", "else_reached", "after_reached"),
    [
        ("break", False, True),
        ("continue", False, False),
        (
            "try:\n            break\n        finally:\n            continue",
            False,
            False,
        ),
    ],
)
def test_binding_while_true_distinguishes_break_continue_and_finally(
    body: str,
    else_reached: bool,
    after_reached: bool,
) -> None:
    source = f"def function():\n    while True:\n        {body}\n    else:\n        exhausted()\n    after()\n"
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    loop = function.body[0]
    assert isinstance(loop, ast.While)
    index = analyze_python_source_bindings(source)
    assert (index.statement_fact(loop.orelse[0]) is not None) is else_reached
    assert (index.statement_fact(function.body[1]) is not None) is after_reached


@pytest.mark.parametrize(
    ("terminal", "after_reached"), [("return 1", False), ("raise", True)]
)
def test_binding_context_exit_does_not_suppress_return(
    terminal: str,
    after_reached: bool,
) -> None:
    source = (
        f"def function(manager):\n    with manager:\n        {terminal}\n    after()\n"
    )
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    index = analyze_python_source_bindings(source)
    assert (index.statement_fact(function.body[1]) is not None) is after_reached


@pytest.mark.parametrize("operation", ["missing", "del missing"])
def test_possible_name_failure_keeps_exception_handler_reachable(
    operation: str,
) -> None:
    source = f"try:\n    {operation}\nexcept NameError:\n    recovered()\n"
    tree = ast.parse(source)
    statement = tree.body[0]
    assert isinstance(statement, ast.Try)
    index = analyze_python_source_bindings(source)
    assert index.statement_fact(statement.handlers[0].body[0]) is not None
    body_fact = index.statement_fact(statement.body[0])
    assert body_fact is not None and body_fact.completions & Completion.RAISE


def test_except_star_deferred_raise_preserves_later_handler_analysis() -> None:
    source = (
        "try:\n    raise\n"
        "except* ValueError:\n    first()\n    raise\n    unreachable()\n"
        "except* TypeError:\n    second()\n"
    )
    tree = ast.parse(source)
    statement = tree.body[0]
    assert isinstance(statement, ast.TryStar)
    index = analyze_python_source_bindings(source)
    assert index.statement_fact(statement.handlers[0].body[-1]) is None
    assert index.statement_fact(statement.handlers[1].body[0]) is not None


def test_truth_protocol_effects_are_distinct_from_expression_evaluation() -> None:
    source = "def function(value):\n    if value:\n        pass\n"
    tree = ast.parse(source)
    conditional = next(node for node in ast.walk(tree) if isinstance(node, ast.If))
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(conditional.test)
    assert fact is not None
    assert not fact.effects & INVOKES_COMPARISON_CALLBACK
    assert fact.truth_effects & INVOKES_COMPARISON_CALLBACK
    assert fact.truth_effects & RAISES


def test_finally_call_facts_join_all_incoming_completions() -> None:
    source = (
        "def function(flag):\n"
        "    import importlib\n"
        "    try:\n"
        "        if flag is None:\n"
        "            alias = importlib.import_module\n"
        "            return\n"
        "        alias = replacement\n"
        "    finally:\n"
        "        alias('package')\n"
    )
    tree = ast.parse(source)
    call = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "alias"
    )
    index = analyze_python_source_bindings(source)
    fact = index.call_fact(call)
    assert fact is not None
    assert fact.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert not fact.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


def test_deferred_definition_is_queued_once_across_finally_completions() -> None:
    source = (
        "def outer(flag):\n"
        "    target = 'before'\n"
        "    try:\n"
        "        if flag is None:\n"
        "            return\n"
        "    finally:\n"
        "        def inner():\n"
        "            return target\n"
        "    target = 'after'\n"
    )
    tree = ast.parse(source)
    inner = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.FunctionDef) and node.name == "inner"
    )
    index = analyze_python_source_bindings(source)
    assert sum(scope.name == "inner" for scope in index.scopes) == 1
    returned = inner.body[0]
    assert isinstance(returned, ast.Return) and returned.value is not None
    fact = index.expression_fact(returned.value)
    assert fact is not None and fact.static_value is None


def test_repeated_class_body_uses_one_source_scope_and_one_method_job() -> None:
    source = (
        "def outer(flag):\n"
        "    while flag is None:\n"
        "        class Container:\n"
        "            def child(self):\n"
        "                return 0\n"
    )
    index = analyze_python_source_bindings(source)
    assert (
        sum(
            scope.kind == "class" and scope.name == "Container"
            for scope in index.scopes
        )
        == 1
    )
    assert (
        sum(
            scope.kind == "function" and scope.name == "child" for scope in index.scopes
        )
        == 1
    )


@pytest.mark.parametrize("pattern", ["_", "1 | _", "(1 | _) as captured"])
def test_irrefutable_match_terminal_body_has_no_fallthrough(pattern: str) -> None:
    source = f"def function(value):\n    match value:\n        case {pattern}:\n            return 1\n    unreachable()\n"
    tree = ast.parse(source)
    function = tree.body[0]
    assert isinstance(function, ast.FunctionDef)
    index = analyze_python_source_bindings(source)
    assert index.statement_fact(function.body[1]) is None
