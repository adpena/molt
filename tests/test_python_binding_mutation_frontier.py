from __future__ import annotations

import ast
from itertools import product

import pytest

from molt.compiler_analysis import python_binding_flow as flow
from molt.compiler_analysis.python_value_identity import (
    OTHER_IDENTITY,
    UNBOUND_IDENTITY,
    PythonIdentity,
)
from molt.compiler_analysis.static_truth import (
    ExpressionSequenceItem,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    expression_result_for_owned_binding,
    expression_result_without_mutable_contents,
)

INERT = int(PythonIdentity.INERT_VALUE)


def _has_unknown_shape(result: StaticExpressionResult) -> bool:
    # Provenance is orthogonal to shape; preserve all prior shape/lifetime checks.
    from dataclasses import replace

    return (
        replace(
            result,
            identities=UNKNOWN_EXPRESSION_RESULT.identities,
            exposes_module_globals=False,
        )
        == UNKNOWN_EXPRESSION_RESULT
    )


def _shapes() -> tuple[StaticExpressionResult, ...]:
    scalar = StaticExpressionResult.scalar(1)
    mutable = StaticExpressionResult(
        kind="list",
        truth=True,
        length=1,
        fresh_container=True,
        release_may_call=False,
        items=(ExpressionSequenceItem(scalar),),
        element_result=scalar,
    )
    nested = StaticExpressionResult(
        kind="tuple",
        truth=True,
        length=1,
        release_may_call=False,
        items=(ExpressionSequenceItem(mutable),),
        element_result=mutable,
    )
    return (
        UNKNOWN_EXPRESSION_RESULT,
        scalar,
        StaticExpressionResult.scalar("text"),
        mutable,
        expression_result_for_owned_binding(mutable),
        StaticExpressionResult(kind="list", element_result=scalar),
        StaticExpressionResult(kind="dict", element_result=mutable),
        StaticExpressionResult(
            kind="bytearray",
            truth=False,
            length=0,
            fresh_container=True,
            release_may_call=False,
        ),
        nested,
        StaticExpressionResult(
            kind="frozenset",
            truth=True,
            length=1,
            items=(ExpressionSequenceItem(nested),),
        ),
        StaticExpressionResult(kind="tuple", element_result=nested),
        StaticExpressionResult(
            kind="list",
            items=(ExpressionSequenceItem(nested, expanded=True),),
            element_result=nested,
        ),
    )


def _reference_expiry(pool: flow._StatePool, state: int, exempt: frozenset[int]) -> int:
    """Deliberately exhaustive old traversal, with canonical expiry semantics."""
    environment = pool._binding_environments[state]
    updates = []
    for chunk_index in range(environment.chunk_count):
        chunk = pool._chunk_at(environment, chunk_index)
        remaining = chunk.active_mask
        while remaining:
            bit = remaining & -remaining
            remaining ^= bit
            slot = (chunk_index << flow._BINDING_CHUNK_SHIFT) | (bit.bit_length() - 1)
            raw = pool._binding_resolution(state, slot)
            if not raw.clean:
                continue
            widened = expression_result_without_mutable_contents(
                raw.result, preserve_owner=slot in exempt
            )
            if widened != raw.result:
                updates.append(
                    (
                        slot,
                        raw.identities,
                        raw.static_value,
                        widened,
                        pool.owner_token(state, slot),
                    )
                )
    return pool.set_bindings(state, updates) if updates else state


def _assert_frontier(pool: flow._StatePool, state: int) -> None:
    def visit(branch: flow._BindingBranch, level: int) -> None:
        ordinary = preserved = 0
        for offset, item in enumerate(branch.children):
            if level:
                assert isinstance(item, flow._BindingBranch)
                visit(item, level - 1)
            else:
                assert isinstance(item, flow._BindingChunk)
                ordinary_slots = preserved_slots = 0
                for position, result in enumerate(item.results):
                    bit = 1 << position
                    if not item.active_mask & item.clean_mask & bit:
                        continue
                    if expression_result_without_mutable_contents(result) != result:
                        ordinary_slots |= bit
                    if (
                        expression_result_without_mutable_contents(
                            result, preserve_owner=True
                        )
                        != result
                    ):
                        preserved_slots |= bit
                assert item.mutation_mask == ordinary_slots
                assert item.owner_mutation_mask == preserved_slots
            if item.mutation_mask:
                ordinary |= 1 << offset
            if item.owner_mutation_mask:
                preserved |= 1 << offset
        assert branch.mutation_mask == ordinary
        assert branch.owner_mutation_mask == preserved
        expected_extent = 0
        if branch.children:
            last = branch.children[-1]
            expected_extent = (len(branch.children) - 1) << (
                level * flow._BINDING_TREE_SHIFT
            )
            if level:
                assert isinstance(last, flow._BindingBranch)
                expected_extent += last.chunk_count
            else:
                expected_extent += 1
        assert branch.chunk_count == expected_extent

    environment = pool._binding_environments[state]
    visit(environment.root, environment.depth - 1)


def test_mutation_frontier_matches_exhaustive_expiry_across_storage_transitions() -> (
    None
):
    far = flow._BINDING_CHUNK_SIZE * flow._BINDING_TREE_SIZE**2
    modes = (
        "clean",
        "local-dirty",
        "namespace",
        "rebound",
        "join-absent",
        "join-dirty",
        "join-owner",
        "domain-growth",
        "recorded",
    )
    exemptions = (frozenset(), frozenset({0}), frozenset({far}), frozenset({0, far}))
    for shape, owner, mode in product(_shapes(), (0, 17), modes):
        pool = flow._StatePool()
        domain = (1 << 0) | (1 << far)
        if mode != "domain-growth":
            pool.set_taint_domain((1 << far) if mode == "local-dirty" else domain)
        base = pool.set_bindings(
            0,
            (
                (0, OTHER_IDENTITY, None, shape, owner),
                (far, OTHER_IDENTITY, None, shape, owner),
                (1, INERT, 1, StaticExpressionResult.scalar(1), 0),
            ),
        )
        state = base
        if mode == "local-dirty":
            state = pool.taint_slots(base, 1 << 0)
            state = pool.taint_slots(state, 1 << (far + 1))
        elif mode in {"namespace", "rebound", "join-dirty", "domain-growth"}:
            state = pool.taint_exposed_bindings(base)
            if mode == "rebound":
                state = pool.set_binding(
                    state, 0, OTHER_IDENTITY, result=shape, owner_token=owner
                )
            elif mode == "join-dirty":
                state = pool.join(base, state)
            elif mode == "domain-growth":
                assert pool._binding_resolution(state, 0).clean
                pool.set_taint_domain(domain)
                assert not pool._binding_resolution(state, 0).clean
        elif mode == "join-absent":
            state = pool.join(base, 0)
        elif mode == "join-owner":
            alternate = pool.set_binding(
                base, 0, OTHER_IDENTITY, result=shape, owner_token=owner + 1
            )
            state = pool.join(base, alternate)
        elif mode == "recorded":
            state = pool.set_bindings(
                base, ((0, OTHER_IDENTITY, None, shape, owner),), record_writes=True
            )
            assert pool.slot_updated_between(base, state, 0)
        _assert_frontier(pool, state)
        before = tuple(
            pool._binding_details(state, slot) for slot in (0, 1, far, far + 1)
        )
        for exempt in exemptions:
            expected = _reference_expiry(pool, state, exempt)
            actual = pool.invalidate_mutable_contents(state, except_slots=exempt)
            # Identical ordered transition payloads intern to the exact same state.
            assert actual == expected, (shape.kind, owner, mode, exempt)
            _assert_frontier(pool, actual)
            assert (
                tuple(
                    pool._binding_details(state, slot) for slot in (0, 1, far, far + 1)
                )
                == before
            )


def test_batch_publication_matches_point_writes_and_copies_each_path_once() -> None:
    slots = (0, 1, 31, 32, 33, 63, 127, 128, 129, 2047, 2048, 4095)
    pool = flow._StatePool()
    scalar = StaticExpressionResult.scalar(0)
    base = pool.set_bindings(0, tuple((slot, INERT, 0, scalar, 0) for slot in slots))
    updates = tuple(
        (slot, INERT, slot + 1, StaticExpressionResult.scalar(slot + 1), slot + 2)
        for slot in reversed(slots)
    )
    before_chunks, before_branches = (
        pool.binding_chunk_copies,
        pool.binding_branch_copies,
    )
    batch = pool.set_bindings(base, updates, record_writes=True)
    chunks = {slot >> flow._BINDING_CHUNK_SHIFT for slot in slots}
    depth = pool._binding_environments[batch].depth
    ancestors = {
        (level, chunk >> ((level + 1) * flow._BINDING_TREE_SHIFT))
        for chunk in chunks
        for level in range(depth)
    }
    assert pool.binding_chunk_copies - before_chunks == len(chunks)
    assert pool.binding_branch_copies - before_branches == len(ancestors)
    sequential = base
    for slot, identity, static, result, owner in updates:
        sequential = pool.set_binding(sequential, slot, identity, static, result, owner)
    assert pool.equivalent(batch, sequential)
    assert pool.owner_tokens_equal(batch, sequential)
    assert pool.changed_slots_between(base, batch) == slots
    assert pool.get(batch).parents == (base,)
    assert tuple(slot for slot, *_ in pool.get(batch).updated_bindings) == tuple(
        reversed(slots)
    )
    assert all(pool.slot_updated_between(base, batch, slot) for slot in slots)
    _assert_frontier(pool, batch)


def test_sparse_frontier_never_scans_inert_slots_or_re_resolves_candidates() -> None:
    pool = flow._StatePool()
    count = 4096
    stable = StaticExpressionResult.scalar(1)
    base = pool.set_bindings(
        0, tuple((slot, INERT, 1, stable, 0) for slot in range(count))
    )
    assert pool.invalidate_mutable_contents(base) == base
    assert pool.mutation_frontier_slot_visits == 0
    assert pool.mutation_frontier_chunk_visits == 0
    assert pool.mutation_frontier_node_visits == 1
    mutable = StaticExpressionResult(kind="list", element_result=stable)
    candidates = (17, count - 1)
    base = pool.set_bindings(
        base,
        tuple((slot, OTHER_IDENTITY, None, mutable, slot + 1) for slot in candidates),
    )
    before = (
        pool.mutation_frontier_node_visits,
        pool.mutation_frontier_chunk_visits,
        pool.mutation_frontier_slot_visits,
        pool.binding_resolution_calls,
        pool.binding_chunk_copies,
    )
    expired = pool.invalidate_mutable_contents(base)
    after = (
        pool.mutation_frontier_node_visits,
        pool.mutation_frontier_chunk_visits,
        pool.mutation_frontier_slot_visits,
        pool.binding_resolution_calls,
        pool.binding_chunk_copies,
    )
    delta = tuple(end - start for start, end in zip(before, after, strict=True))
    assert delta[0] <= 2 * pool._binding_environments[base].depth
    assert delta[1:] == (2, 2, 2, 2)
    assert pool.changed_slots_between(base, expired) == candidates
    before_slots = pool.mutation_frontier_slot_visits
    assert pool.invalidate_mutable_contents(expired) == expired
    assert pool.mutation_frontier_slot_visits == before_slots
    root = pool._binding_environments[expired].root
    assert root.mutation_mask == root.owner_mutation_mask == 0


def test_preserve_owner_frontier_exempts_only_outer_storage() -> None:
    pool = flow._StatePool()
    scalar = StaticExpressionResult.scalar(1)
    mutable = StaticExpressionResult(
        kind="list",
        length=1,
        truth=True,
        items=(ExpressionSequenceItem(scalar),),
        element_result=scalar,
    )
    nested = StaticExpressionResult(
        kind="tuple",
        length=1,
        truth=True,
        items=(ExpressionSequenceItem(mutable),),
        element_result=mutable,
    )
    state = pool.set_bindings(
        0,
        (
            (0, OTHER_IDENTITY, None, mutable, 1),
            (1, OTHER_IDENTITY, None, nested, 2),
        ),
    )
    chunk = pool._chunk_at(pool._binding_environments[state], 0)
    assert chunk.mutation_mask == 3
    assert chunk.owner_mutation_mask == 2
    before = pool.mutation_frontier_slot_visits
    expired = pool.invalidate_mutable_contents(state, except_slots=frozenset({0, 1}))
    assert pool.mutation_frontier_slot_visits - before == 1
    assert pool.result(expired, 0) == mutable
    assert pool.result(expired, 1) == expression_result_without_mutable_contents(
        nested, preserve_owner=True
    )
    assert pool.owner_token(expired, 0) == 1 and pool.owner_token(expired, 1) == 2


def test_recorded_same_shape_and_owner_only_writes_keep_history_without_copies() -> (
    None
):
    pool = flow._StatePool()
    result = StaticExpressionResult(
        kind="list", element_result=StaticExpressionResult.scalar(1)
    )
    bindings = (
        (0, OTHER_IDENTITY, None, result, 7),
        (32, OTHER_IDENTITY, None, result, 9),
    )
    base = pool.set_bindings(0, bindings)
    before = pool.binding_chunk_copies, pool.binding_branch_copies
    recorded = pool.set_bindings(base, bindings, record_writes=True)
    assert recorded != base
    assert pool._binding_environments[recorded] is pool._binding_environments[base]
    assert (pool.binding_chunk_copies, pool.binding_branch_copies) == before
    assert pool.changed_slots_between(base, recorded) == ()
    assert pool.transition_binding_events(base, recorded) == (
        (0, OTHER_IDENTITY),
        (32, OTHER_IDENTITY),
    )
    assert pool.slot_updated_between(base, recorded, 0)
    owner_only = pool.set_binding(
        recorded, 0, OTHER_IDENTITY, result=result, owner_token=8
    )
    assert pool.equivalent(base, owner_only)
    assert not pool.owner_tokens_equal(base, owner_only)
    joined = pool.join(recorded, owner_only)
    assert pool.owner_token(joined, 0) == 0
    assert pool.slot_updated_between(base, joined, 0)
    repeated = pool.set_bindings(
        base,
        (
            (0, OTHER_IDENTITY, None, result, 8),
            (0, OTHER_IDENTITY, None, result, 7),
        ),
        record_writes=True,
    )
    assert pool.owner_token(repeated, 0) == 7
    assert pool.slot_updated_between(base, repeated, 0)
    assert pool.get(repeated).updated_bindings[0][-2] == 8


def test_owner_comparison_and_resolution_follow_late_domain_growth() -> None:
    pool = flow._StatePool()
    result = StaticExpressionResult(
        kind="list", element_result=StaticExpressionResult.scalar(1)
    )
    base = pool.set_bindings(
        0,
        ((0, OTHER_IDENTITY, None, result, 7), (4095, OTHER_IDENTITY, None, result, 9)),
    )
    exposed = pool.taint_exposed_bindings(base)
    assert pool._binding_environments[exposed] is pool._binding_environments[base]
    assert pool.owner_tokens_equal(base, exposed)
    assert pool._binding_resolution(exposed, 0).clean
    pool.set_taint_domain(1 | (1 << 4095) | (1 << 4096))
    assert not pool._binding_resolution(exposed, 0).clean
    assert pool._binding_resolution(exposed, 0).owner_token == 7
    assert pool.owner_token(exposed, 0) == 0
    assert _has_unknown_shape(pool.result(exposed, 0))
    assert pool.binding(exposed, 4096) == UNBOUND_IDENTITY | OTHER_IDENTITY
    assert not pool.owner_tokens_equal(base, exposed)
    rebound = pool.set_binding(exposed, 0, OTHER_IDENTITY, result=result, owner_token=7)
    assert pool.owner_token(rebound, 0) == 7
    states = (0, base, exposed, rebound, pool.join(base, rebound))
    for left, right in product(states, repeat=2):
        reference = all(
            pool.owner_token(left, slot) == pool.owner_token(right, slot)
            for slot in (0, 4095, 4096)
        )
        assert pool.owner_tokens_equal(left, right) == reference
        _assert_frontier(pool, left)


def test_taint_batch_preserves_deleted_slot_custody() -> None:
    pool = flow._StatePool()
    result = StaticExpressionResult(
        kind="list", element_result=StaticExpressionResult.scalar(1)
    )
    slots = (31, 32, 127, 128, 2048)
    base = pool.set_bindings(
        0, tuple((slot, OTHER_IDENTITY, None, result, slot + 1) for slot in slots)
    )
    dirty = pool.taint_slots(base, sum(1 << slot for slot in reversed(slots)))
    assert pool.get(dirty).parents == (base,)
    assert tuple(slot for slot, *_ in pool.get(dirty).updated_bindings) == slots
    assert all(not pool._binding_resolution(dirty, slot).clean for slot in slots)
    assert pool._binding_environments[dirty].root.mutation_mask == 0
    assert pool._binding_environments[dirty].root.owner_mutation_mask == 0
    assert pool.invalidate_mutable_contents(dirty) == dirty
    # A deletion is present storage, not pristine absence. Taint must retain
    # the tombstone even when its payload has no value or mutable contents.
    deleted = pool.set_binding(dirty, slots[-1], UNBOUND_IDENTITY)
    assert pool._binding_resolution(deleted, slots[-1]).present
    assert pool._binding_resolution(deleted, slots[-1]).clean
    tainted = pool.taint_slots(deleted, 1 << slots[-1])
    assert (
        pool._binding_environments[tainted].chunk_count
        == (slots[-1] >> flow._BINDING_CHUNK_SHIFT) + 1
    )
    assert pool.binding(tainted, slots[-1]) == UNBOUND_IDENTITY | OTHER_IDENTITY
    raw = pool._binding_resolution(tainted, slots[-1])
    assert raw.present and not raw.clean and raw.identities == UNBOUND_IDENTITY
    _assert_frontier(pool, tainted)
    pool.set_taint_domain(sum(1 << slot for slot in (*slots, 2049)))
    assert pool.binding(tainted, slots[-1]) == UNBOUND_IDENTITY | OTHER_IDENTITY
    assert pool._binding_resolution(tainted, slots[-1]).present
    assert not pool._binding_resolution(tainted, 2049).present
    rebound = pool.set_binding(tainted, slots[-1], INERT, 7)
    assert pool.binding(rebound, slots[-1]) == INERT
    assert pool._binding_resolution(rebound, slots[-1]).clean
    assert pool.static_value(rebound, slots[-1]) == 7
    _assert_frontier(pool, rebound)


def test_radix_removal_trims_only_genuinely_absent_children() -> None:
    pool = flow._StatePool()
    state = pool.set_binding(0, 0, INERT, 7)
    retained = pool._binding_environments[state].root.children[0]
    extent = 1
    for level in range(3):
        filler = flow._EMPTY_BINDING_CHUNK if level == 0 else flow._EMPTY_BINDING_BRANCH
        children = (filler, retained, filler, filler)
        trimmed = flow._binding_branch(children, level)
        assert trimmed.children == (filler, retained)
        extent = (1 << (level * flow._BINDING_TREE_SHIFT)) + extent
        assert trimmed.chunk_count == extent
        assert flow._binding_branch(children, level, trimmed) is trimmed
        assert (
            flow._binding_branch((filler, filler), level) is flow._EMPTY_BINDING_BRANCH
        )
        retained = trimmed


def test_duplicate_slot_batches_preserve_ordered_effective_writes() -> None:
    a = StaticExpressionResult.scalar(1)
    b = StaticExpressionResult(kind="list", element_result=a)
    for record in (False, True):
        pool = flow._StatePool()
        base = pool.set_binding(0, 0, INERT, 1, a, 7)
        before = pool.binding_chunk_copies, pool.binding_branch_copies
        returned = pool.set_bindings(
            base,
            (
                (0, OTHER_IDENTITY, None, b, 8),
                (0, INERT, 1, a, 7),
            ),
            record_writes=record,
        )
        assert returned != base
        assert pool.result(returned, 0) == a
        assert pool.owner_token(returned, 0) == 7
        assert pool._binding_environments[returned] is pool._binding_environments[base]
        assert (pool.binding_chunk_copies, pool.binding_branch_copies) == before
        assert tuple(row[3] for row in pool.get(returned).updated_bindings) == (b, a)
        assert pool.slot_updated_between(base, returned, 0)
        assert pool.transition_binding_events(base, returned) == ((0, INERT),)
        _assert_frontier(pool, returned)
    pool = flow._StatePool()
    base = pool.set_bindings(0, ((0, INERT, 1, a, 7), (32, INERT, 1, a, 9)))
    before = pool.binding_resolution_calls
    interleaved = pool.set_bindings(
        base,
        (
            (0, OTHER_IDENTITY, None, b, 8),
            (32, OTHER_IDENTITY, None, b, 10),
            (0, INERT, 1, a, 7),
            (32, OTHER_IDENTITY, None, b, 10),
        ),
    )
    assert pool.binding_resolution_calls - before == 2
    assert tuple(row[0] for row in pool.get(interleaved).updated_bindings) == (0, 32, 0)
    assert pool.result(interleaved, 0) == a and pool.result(interleaved, 32) == b
    assert pool.slot_updated_between(base, interleaved, 0)
    before = pool.binding_resolution_calls
    recorded = pool.set_bindings(
        base, ((0, INERT, 1, a, 7), (0, INERT, 1, a, 7)), record_writes=True
    )
    assert pool.binding_resolution_calls == before
    assert len(pool.get(recorded).updated_bindings) == 2
    _assert_frontier(pool, interleaved)


def test_rejected_public_or_raw_batch_does_not_partially_intern_state() -> None:
    import pytest

    scalar = StaticExpressionResult.scalar(1)
    for mode in ("default", "recorded", "single-recorded", "raw"):
        pool = flow._StatePool()
        base = pool.set_binding(0, 0, INERT, 1, scalar)
        before = len(pool._states), len(pool._binding_environments), dict(pool._ids)
        with pytest.raises(ValueError, match="nonnegative"):
            if mode == "raw":
                pool.intern(
                    flow._BindingState(
                        parents=(base,),
                        updated_bindings=(
                            (32, INERT, 1, scalar, 0, True),
                            (-1, INERT, 1, scalar, 0, True),
                        ),
                    )
                )
            else:
                bindings = (
                    ((-1, INERT, 1, scalar, 0),)
                    if mode == "single-recorded"
                    else (
                        (32, INERT, 1, scalar, 0),
                        (-1, INERT, 1, scalar, 0),
                    )
                )
                pool.set_bindings(base, bindings, record_writes=mode != "default")
        assert (len(pool._states), len(pool._binding_environments), pool._ids) == before
        valid = pool.set_binding(base, 32, INERT, 1, scalar)
        assert valid == before[0]
        assert pool.get(valid).parents == (base,)
        assert pool.static_value(valid, 32) == 1
        assert pool.binding(base, 32) == UNBOUND_IDENTITY
        _assert_frontier(pool, valid)


def test_storage_diff_does_not_replace_epoch_or_absent_domain_comparison() -> None:
    pool = flow._StatePool()
    pool.set_taint_domain((1 << 0) | (1 << 4096))
    base = pool.set_binding(0, 0, INERT, 1, StaticExpressionResult.scalar(1), 9)
    exposed = pool.taint_exposed_bindings(base)
    assert pool.changed_slots_between(base, exposed) == ()
    assert not pool.equivalent(base, exposed)
    assert not pool.owner_tokens_equal(base, exposed)
    assert pool.binding(base, 4096) == UNBOUND_IDENTITY
    assert pool.binding(exposed, 4096) == UNBOUND_IDENTITY | OTHER_IDENTITY
    summary = flow._HistorySummary.build(pool, (base, exposed))
    assert summary.binding(pool, 0, 4096) == UNBOUND_IDENTITY | OTHER_IDENTITY


def test_augmented_scalar_operation_does_not_become_target_callback() -> None:
    tree = ast.parse("counter = 0\ncounter += 1\n")
    analysis = flow._Analyzer(
        flow.PythonBindingFlowPolicy(), "augmented-scalar-effects"
    ).analyze(tree)
    statement = tree.body[-1]
    assert isinstance(statement, ast.AugAssign)
    fact = analysis.facts.statement_fact(statement)
    assert fact is not None
    # Exact builtin scalar operands have no user dispatch. Their real effect
    # fact must not taint storage and invent a displaced callback-capable value.
    assert not fact.effects & flow.NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    completion = analysis.assignment_effects[
        flow.PythonNodeKey.from_node(statement.target)
    ]
    assert not completion & (
        flow.NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS | flow.WRITES_MODULE_METADATA
    )


@pytest.mark.parametrize(
    ("source", "required_effects"),
    [
        ("counter = unknown\ncounter += 1\n", flow.RUNS_FINALIZER),
        (
            "receiver.attribute += 1\n",
            flow.INVOKES_DESCRIPTOR | flow.RUNS_FINALIZER,
        ),
        (
            "receiver[index] += 1\n",
            flow.EXECUTES_ARBITRARY_PYTHON | flow.RUNS_FINALIZER,
        ),
        (
            "exposed = globals()\nexposed |= {'__package__': 'other'}\n",
            flow.WRITES_GLOBAL_NAMESPACE | flow.WRITES_MODULE_METADATA,
        ),
    ],
)
def test_augmented_assignment_preserves_publication_effects(
    source: str, required_effects: int
) -> None:
    tree = ast.parse(source)
    analysis = flow._Analyzer(
        flow.PythonBindingFlowPolicy(), "augmented-publication-effects"
    ).analyze(tree)
    statement = tree.body[-1]
    assert isinstance(statement, ast.AugAssign)
    completion = analysis.assignment_effects[
        flow.PythonNodeKey.from_node(statement.target)
    ]
    assert completion & required_effects == required_effects


@pytest.mark.parametrize("inplace", [False, True])
@pytest.mark.parametrize(
    ("left", "operator", "right", "expected"),
    [
        (12, "+", 2, 14),
        (12, "-", 2, 10),
        (12, "*", 2, 24),
        (12, "/", 2, 6.0),
        (12, "//", 2, 6),
        (12, "%", 2, 0),
        (12, "**", 2, 144),
        (12, "<<", 2, 48),
        (12, ">>", 2, 3),
        (12, "&", 2, 0),
        (12, "|", 2, 14),
        (12, "^", 2, 14),
        ("a", "+", "b", "ab"),
        ("%d", "%", 2, "2"),
        (b"a", "*", 2, b"aa"),
        (True, "&", False, False),
        (1.5, "+", 2.5, 4.0),
        (1j, "+", 2j, 3j),
    ],
)
def test_binary_operator_capability_uses_retained_scalar_facts(
    left: object, operator: str, right: object, expected: object, inplace: bool
) -> None:
    from molt.compiler_analysis.python_effects import expression_effect_mask

    update = (
        f"value {operator}= {right!r}"
        if inplace
        else f"value = value {operator} {right!r}"
    )
    source = f"value = {left!r}\n{update}\n"
    namespace: dict[str, object] = {}
    exec(compile(source, "<binary-scalar-capability-oracle>", "exec"), namespace)
    assert namespace["value"] == expected
    assert type(namespace["value"]) is type(expected)
    tree = ast.parse(source)
    analysis = flow._Analyzer(
        flow.PythonBindingFlowPolicy(), "binary-scalar-capability"
    ).analyze(tree)
    statement = analysis.facts.statement_fact(tree.body[-1])
    assert statement is not None
    assert not statement.effects & flow.NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    literal = ast.parse(f"{left!r} {operator} {right!r}", mode="eval").body
    assert (
        not expression_effect_mask(literal) & flow.NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    )


@pytest.mark.parametrize("inplace", [False, True])
def test_binary_operator_unknown_values_keep_dispatch_callbacks(inplace: bool) -> None:
    from molt.compiler_analysis.python_effects import expression_effect_mask

    source = "value = unknown\n" + (
        "value += 1\n" if inplace else "value = value + 1\n"
    )
    tree = ast.parse(source)
    analysis = flow._Analyzer(
        flow.PythonBindingFlowPolicy(), "binary-unknown-capability"
    ).analyze(tree)
    statement = analysis.facts.statement_fact(tree.body[-1])
    assert statement is not None
    assert statement.effects & flow.EXECUTES_ARBITRARY_PYTHON
    if not inplace:
        assert (
            expression_effect_mask(tree.body[-1].value) & flow.EXECUTES_ARBITRARY_PYTHON
        )
