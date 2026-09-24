from __future__ import annotations

import ast
from dataclasses import fields, replace
from itertools import product
import struct

import pytest

from molt.compiler_analysis import python_binding_flow as flow
from molt.compiler_analysis.literal_identity import literal_identity_key
from molt.compiler_analysis.python_binding_facts import (
    OTHER_IDENTITY,
    UNBOUND_IDENTITY,
    PythonBindingTelemetry,
    PythonExpressionFact,
    PythonIdentity,
    PythonParameterRef,
    python_static_value_key,
)
from molt.compiler_analysis.static_truth import (
    ExpressionKind,
    ExpressionSequenceItem,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    _same_scalar_value,
    join_static_expression_results,
)

INERT = int(PythonIdentity.INERT_VALUE)


def _reference_result_join(
    results: tuple[StaticExpressionResult, ...],
) -> StaticExpressionResult:
    """Join control-flow alternatives in the canonical result algebra."""

    if not results:
        return UNKNOWN_EXPRESSION_RESULT
    completed: dict[tuple[int, ...], StaticExpressionResult] = {}
    pending = [(results, False)]
    while pending:
        current, expanded = pending.pop()
        key = tuple(id(result) for result in current)
        if key in completed:
            continue
        first = current[0]
        if all(result == first for result in current[1:]):
            completed[key] = first
            continue
        child_results = (
            tuple(
                result.element_result
                for result in current
                if result.element_result is not None
            )
            if all(result.element_result is not None for result in current)
            else None
        )
        child_key = (
            None
            if child_results is None
            else tuple(id(result) for result in child_results)
        )
        if child_results is not None and child_key not in completed and not expanded:
            pending.append((current, True))
            pending.append((child_results, False))
            continue
        kind: ExpressionKind = (
            first.kind
            if all(result.kind == first.kind for result in current)
            else "unknown"
        )
        exact = first.value_known and all(
            result.value_known
            and result.kind == first.kind
            and _same_scalar_value(result.value, first.value)
            for result in current[1:]
        )
        completed[key] = StaticExpressionResult(
            truth=(
                first.truth
                if all(result.truth is first.truth for result in current[1:])
                else None
            ),
            value=first.value if exact else None,
            value_known=exact,
            kind=kind,
            evaluation_required=any(result.evaluation_required for result in current),
            items=(
                first.items
                if all(result.items == first.items for result in current[1:])
                else None
            ),
            release_may_call=any(result.release_may_call for result in current),
            fresh_container=all(result.fresh_container for result in current),
            length=(
                first.length
                if all(result.length == first.length for result in current[1:])
                else None
            ),
            element_result=(None if child_key is None else completed[child_key]),
            _publication_release_stable=all(
                bool(result._publication_release_stable) for result in current
            ),
        )
    return completed[tuple(id(result) for result in results)]


def _result_cases() -> tuple[StaticExpressionResult, ...]:
    nan = struct.unpack("!d", bytes.fromhex("7ff8000000000001"))[0]
    other_nan = struct.unpack("!d", bytes.fromhex("7ff8000000000002"))[0]
    scalar = StaticExpressionResult.scalar(1)
    cases = [
        UNKNOWN_EXPRESSION_RESULT,
        *(
            StaticExpressionResult.scalar(x)
            for x in (
                None,
                True,
                1,
                False,
                0,
                0.0,
                -0.0,
                nan,
                other_nan,
                complex(-0.0, 0.0),
                "a",
            )
        ),
        replace(scalar, evaluation_required=True),
        StaticExpressionResult(kind="int", truth=True, release_may_call=False),
        StaticExpressionResult(
            kind="list", element_result=scalar, fresh_container=True
        ),
        StaticExpressionResult(
            kind="tuple",
            items=(ExpressionSequenceItem(scalar),),
            element_result=scalar,
            length=1,
            release_may_call=False,
        ),
    ]
    cases.append(replace(cases[-1], _publication_release_stable=False))
    return tuple(cases)


def test_result_join_matches_preoptimization_algebra_and_absorbs_in_order() -> None:
    cases = _result_cases()
    for operands in (*product(cases, repeat=2), *product(cases[::3], repeat=3)):
        result = join_static_expression_results(operands)
        reference = _reference_result_join(operands)
        assert result == reference
        assert result._semantic_key == reference._semantic_key
        assert (
            result._publication_release_stable is reference._publication_release_stable
        )
        if result.value_known:
            assert literal_identity_key(result.value) == literal_identity_key(
                reference.value
            )
        equal_inputs = [candidate for candidate in operands if candidate == reference]
        if equal_inputs:
            assert result is equal_inputs[0]
    assert join_static_expression_results(()) is UNKNOWN_EXPRESSION_RESULT


def test_result_join_remains_stack_safe_and_preserves_shared_graphs() -> None:
    left = StaticExpressionResult.scalar(1)
    right = replace(left, evaluation_required=True)
    for _ in range(1500):
        left = StaticExpressionResult(kind="tuple", element_result=left)
        right = StaticExpressionResult(kind="tuple", element_result=right)
    assert join_static_expression_results((left, right)) == _reference_result_join(
        (left, right)
    )
    scalar = StaticExpressionResult.scalar(1)
    shared = StaticExpressionResult(
        kind="tuple", items=(ExpressionSequenceItem(scalar),) * 4, element_result=scalar
    )
    assert join_static_expression_results((shared, replace(shared))) is shared


@pytest.mark.parametrize("recorded", [False, True])
def test_static_bool_int_identity_reaches_writes_interning_join_and_facts(
    recorded: bool,
) -> None:
    pool = flow._StatePool()
    root = pool.set_bindings(
        0, ((0, INERT, True, UNKNOWN_EXPRESSION_RESULT, 0),), record_writes=recorded
    )
    integer = pool.set_bindings(
        root, ((0, INERT, 1, UNKNOWN_EXPRESSION_RESULT, 0),), record_writes=recorded
    )
    assert integer != root
    assert type(pool.static_value(root, 0)) is bool
    assert type(pool.static_value(integer, 0)) is int
    assert pool.slot_updated_between(root, integer, 0)
    assert pool.changed_slots_between(root, integer) == (0,)
    boolean_sibling = pool.set_binding(0, 0, INERT, True)
    integer_sibling = pool.set_binding(0, 0, INERT, 1)
    assert boolean_sibling != integer_sibling
    assert pool.static_value(pool.join(boolean_sibling, integer_sibling), 0) is None
    raw_bool = flow._BindingState(
        parents=(0,),
        updated_slot=0,
        updated_value=INERT,
        updated_static_value=True,
        updated_clean=True,
    )
    raw_int = replace(raw_bool, updated_static_value=1)
    assert raw_bool != raw_int
    assert pool.intern(raw_bool) != pool.intern(raw_int)
    # Public fact equality uses the same exact static identity, not dataclass ==.
    node = flow._Analyzer(flow.PythonBindingPolicy(), "static-identity")._node_key(
        ast.parse("x").body[0]
    )
    fact = PythonExpressionFact(node, 0, INERT, 0, True)
    assert fact != replace(fact, static_value=1)
    assert python_static_value_key(True) != python_static_value_key(1)


def _projection(pool: flow._StatePool, state: int, slot: int) -> tuple[object, ...]:
    value = pool._binding_resolution(state, slot).public()
    return (
        value.identities,
        python_static_value_key(value.static_value),
        value.result,
        value.clean,
        value.owner_token,
    )


def _assert_fold(pool: flow._StatePool, frame: flow._ObservedStateFrame) -> None:
    folded = frame.exceptional_state(pool)
    flat = pool.join(*frame.states)
    assert pool.equivalent(folded, flat)
    assert pool.owner_tokens_equal(folded, flat)
    for slot in (0, 1, 31, 32, 129):
        assert _projection(pool, folded, slot) == _projection(pool, flat, slot)
    for base in set(frame.states):
        assert pool.transition_binding_events(
            base, folded
        ) == pool.transition_binding_events(base, flat)
        for slot in (0, 1, 32):
            assert pool.slot_updated_between(
                base, folded, slot
            ) == pool.slot_updated_between(base, flat, slot)


def test_observation_fold_preserves_full_history_and_new_parent_work() -> None:
    pool = flow._StatePool()
    history = [0]
    frame = flow._ObservedStateFrame(history)
    state = 0
    for index in range(24):
        state = pool.set_binding(state, index % 3, INERT, index)
        history.extend((state, state))
        frame.exceptional_state(pool)
    assert frame.states is history and len(history) == 49
    assert pool.observation_fold_new_parents == 25
    assert pool.join_max_parents <= 2
    assert frame.exceptional_state(pool) == frame.accumulator
    _assert_fold(pool, frame)


def test_observation_fold_rebuilds_for_late_earlier_id_and_keeps_representative() -> (
    None
):
    pool = flow._StatePool()
    refs = [PythonParameterRef("same") for _ in range(3)]
    states = [
        pool.set_binding(0, 0, INERT, ref, owner_token=index + 1)
        for index, ref in enumerate(refs)
    ]
    frame = flow._ObservedStateFrame([states[0], states[2]])
    frame.exceptional_state(pool)
    frame.states.append(states[1])
    folded = frame.exceptional_state(pool)
    assert pool.observation_fold_rebuilds == 1
    assert pool.static_value(folded, 0) is refs[0]
    assert pool.owner_token(folded, 0) == 0
    _assert_fold(pool, frame)


def test_late_domain_admission_is_independent_of_sharing_and_nested_transfers() -> None:
    pool = flow._StatePool()
    first = pool.set_binding(0, 0, INERT, 7, owner_token=41)
    tainted = pool.taint_module_bindings(first)
    detached = pool.set_binding(tainted, 1, INERT, 9)
    shared_join = pool.join(first, tainted)
    detached_join = pool.join(first, detached)
    nested = pool.join(detached_join, pool.set_binding(detached_join, 32, INERT, 8))
    transferred = pool.set_binding(nested, 31, INERT, 11)
    frame = flow._ObservedStateFrame([first, detached])
    folded = frame.exceptional_state(pool)
    candidates = (shared_join, detached_join, nested, transferred, folded)
    assert all(pool._binding_resolution(state, 0).clean for state in candidates)
    assert all(pool.owner_token(state, 0) == 41 for state in candidates)
    old_generation = pool.taint_domain_generation
    pool.set_taint_domain((1 << 0) | (1 << 129))
    assert pool.taint_domain_generation == old_generation + 1
    for state in candidates:
        assert not pool._binding_resolution(state, 0).clean
        assert pool.binding(state, 0) == INERT | OTHER_IDENTITY
        assert pool.owner_token(state, 0) == 0
        assert pool.static_value(state, 0) is None
    assert frame.exceptional_state(pool) == folded
    assert pool.join(first, detached) == detached_join
    assert pool.intern(pool.get(detached_join)) == detached_join
    assert pool.observation_fold_rebuilds == 0
    _assert_fold(pool, frame)


def test_default_write_refreshes_stale_custody_without_duplicate_batch_writes() -> None:
    pool = flow._StatePool()
    first = pool.set_binding(0, 0, INERT, 7, owner_token=41)
    tainted = pool.taint_module_bindings(first)
    update = (0, INERT, 7, UNKNOWN_EXPRESSION_RESULT, 41)
    rebound = pool.set_bindings(tainted, (update, update))
    assert rebound != tainted
    assert pool.get(rebound).updated_slot == 0
    assert pool.get(rebound).updated_bindings == ()
    assert pool.set_bindings(rebound, (update, update)) == rebound
    assert pool.slot_updated_between(tainted, rebound, 0)
    pool.set_taint_domain(1)
    assert not pool._binding_resolution(tainted, 0).clean
    assert pool._binding_resolution(rebound, 0).clean
    assert pool.owner_token(rebound, 0) == 41
    assert pool.static_value(rebound, 0) == 7


def test_binding_namespace_epochs_are_nonnegative_and_monotone() -> None:
    pool = flow._StatePool()
    with pytest.raises(ValueError, match="epochs cannot regress"):
        pool.intern(flow._BindingState(taint_epoch=-1))
    tainted = pool.taint_module_bindings(0)
    with pytest.raises(ValueError, match="epochs cannot regress"):
        pool.intern(flow._BindingState(parents=(tainted,), taint_epoch=0))
    with pytest.raises(ValueError, match="epochs cannot regress"):
        pool.intern(flow._BindingState(parents=(0, tainted), taint_epoch=0))
    base = pool.set_binding(0, 0, INERT, 7)
    sibling = pool.set_binding(base, 1, INERT, 9)
    advanced = pool.intern(flow._BindingState(parents=(base, sibling), taint_epoch=1))
    pool.set_taint_domain(1)
    assert not pool._binding_resolution(advanced, 0).clean


def test_fold_matches_flat_with_absence_taint_owner_writes_and_domain_growth() -> None:
    for grow_at in (0, 2, 5):
        pool = flow._StatePool()
        history = [0]
        frame = flow._ObservedStateFrame(history)
        base = 0
        for step in range(7):
            if step == grow_at:
                pool.set_taint_domain((1 << 0) | (1 << 32) | (1 << 129))
            if step % 3 == 0:
                base = pool.taint_module_bindings(base)
            result = StaticExpressionResult.scalar(step % 2)
            base = pool.set_bindings(
                base,
                ((step % 2 * 32, INERT, step % 2, result, step + 1),),
                record_writes=True,
            )
            history.append(base)
            _assert_fold(pool, frame)


def test_equal_storage_join_does_not_erase_ancestor_writes() -> None:
    pool = flow._StatePool()
    a = pool.set_binding(0, 0, INERT, 1)
    b = pool.set_bindings(
        a, ((0, INERT, 1, UNKNOWN_EXPRESSION_RESULT, 0),), record_writes=True
    )
    joined = pool.join(a, b)
    assert joined not in (a, b)
    assert pool.slot_updated_between(a, joined, 0)
    assert pool.get(joined).parents == (a, b)


def _detached(
    node: flow._BindingChunk | flow._BindingBranch,
) -> flow._BindingChunk | flow._BindingBranch:
    if isinstance(node, flow._BindingChunk):
        return replace(node)
    return replace(node, children=tuple(_detached(child) for child in node.children))


def test_semantic_history_ignores_sharing_and_includes_absent_domain_slots() -> None:
    pool = flow._StatePool()
    pool.set_taint_domain((1 << 0) | (1 << 129))
    base = pool.set_binding(0, 0, INERT, 1)
    tainted = pool.taint_module_bindings(base)
    other = pool.invalidate_members(tainted, 1)
    joined = pool.join(tainted, other)
    assert pool.changed_slots_between(base, joined) == ()
    events = pool.transition_binding_events(base, joined)
    assert dict(events)[0] & OTHER_IDENTITY
    assert dict(events)[129] == UNBOUND_IDENTITY | OTHER_IDENTITY
    assert not pool.equivalent(base, joined)
    environment = pool._binding_environments[joined]
    root = _detached(environment.root)
    assert isinstance(root, flow._BindingBranch)
    pool._binding_environments[joined] = replace(environment, root=root)
    assert pool.changed_slots_between(base, joined) == (0,)
    assert pool.transition_binding_events(base, joined) == events
    assert not pool.equivalent(base, joined)


def test_two_way_join_reuses_exact_payloads_without_skipping_custody() -> None:
    pool = flow._StatePool()
    result = StaticExpressionResult.scalar(8)
    base = pool.set_binding(0, 0, INERT, 8, result, 13)
    left = pool.set_binding(base, 1, INERT, 1)
    right = pool.set_binding(base, 1, INERT, 2)
    joined = pool.join(left, right)
    assert pool.result(joined, 0) is result
    assert pool.owner_token(joined, 0) == 13
    assert pool.static_value(joined, 1) is None
    assert pool.join_custody_identity_skips
    assert pool.join_payload_algebra_calls
    # A changed epoch prevents copying stale raw custody, including outside the
    # current domain: those epochs become observable after later domain growth.
    later = pool.taint_module_bindings(right)
    epoch_join = pool.join(left, later)
    assert pool._binding_resolution(epoch_join, 0).clean
    pool.set_taint_domain(1)
    assert pool.join(left, later) == epoch_join
    assert not pool._binding_resolution(epoch_join, 0).clean


def test_empty_module_needs_no_module_exit_join_and_telemetry_has_one_schema() -> None:
    analyzer = flow._Analyzer(flow.PythonBindingPolicy(), "empty")
    index = analyzer.analyze(ast.parse(""))
    assert index.telemetry is not None
    assert index.telemetry.join_calls == 0
    from tools.profile_python_binding_flow import _analysis_telemetry

    assert set(_analysis_telemetry(index) or ()) == {
        item.name for item in fields(PythonBindingTelemetry)
    }


def test_nested_exception_consumers_use_fold_without_coalescing_lexical_history() -> (
    None
):
    source = """
def outer(flag):
    value = 0
    def inner():
        return value
    while flag:
        try:
            value = callback(value)
            if flag:
                raise RuntimeError(value)
        except Exception:
            value = alternate()
        finally:
            flag = stop()
    return inner
"""
    analyzer = flow._Analyzer(flow.PythonBindingPolicy(), "exception-fold")
    index = analyzer.analyze(ast.parse(source))
    assert index.telemetry is not None
    assert index.telemetry.observation_fold_calls > 0
    assert index.telemetry.observation_fold_new_parents > 0
    assert not analyzer._observed_stack


def test_history_summary_refreshes_domain_dependent_events_and_initial_projection() -> (
    None
):
    pool = flow._StatePool()
    stored = pool.set_binding(0, 0, INERT, 1)
    older = pool.taint_module_bindings(stored)
    other = pool.invalidate_members(older, 1)
    joined = pool.join(older, other)
    history = [stored, joined]
    summary = flow._HistorySummary.build(pool, history)
    assert summary.binding(pool, 0, 0) == INERT
    pool.set_taint_domain((1 << 0) | (1 << 129))
    reference = flow._HistorySummary.build(pool, history)
    for slot in (0, 129):
        assert summary.binding(pool, 0, slot) == reference.binding(pool, 0, slot)
    assert summary.domain_generation == pool.taint_domain_generation
    assert summary.binding(pool, 0, 129) & OTHER_IDENTITY
    history.append(pool.set_binding(joined, 1, INERT, 2))
    summary.refresh(pool)
    assert summary.state_count == len(history)
    assert summary.properties(2) == flow._HistorySummary.build(
        pool, history
    ).properties(2)
