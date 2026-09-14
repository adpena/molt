"""Guard motion/elision consumes whole-CFG must-facts, including callbacks."""

from __future__ import annotations

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend.cfg_analysis import CFGEdgeKind, build_cfg


def op(kind: str, *args: object, result: str = "none") -> MoltOp:
    return MoltOp(kind=kind, args=list(args), result=MoltValue(result))


def guard(value: str = "value") -> MoltOp:
    return op("GUARD_TYPE", MoltValue(value), MoltValue("expected"))


def elide(ops: list[MoltOp]) -> list[MoltOp]:
    rewritten, attempted, accepted, rejected = (
        SimpleTIRGenerator()._eliminate_redundant_guards_cfg(ops)
    )
    assert attempted == accepted + rejected
    return rewritten


@pytest.mark.parametrize("backedge", ["LOOP_END", "LOOP_CONTINUE", "JUMP"])
def test_guard_backedge_cannot_borrow_preloop_fact(backedge: str) -> None:
    first, body = guard(), guard()
    ops = [first, op("LOOP_START"), op("LABEL", "header"), body]
    ops.append(op("CALL", MoltValue("mutate"), result="callback"))
    if backedge == "LOOP_CONTINUE":
        ops.append(op("LOOP_CONTINUE"))
    elif backedge == "JUMP":
        ops.append(op("JUMP", "header"))
    ops.append(op("LOOP_END"))
    result = elide(ops)
    assert sum(item.kind == "GUARD_TYPE" for item in result) == 2


def test_preserved_loop_fact_still_elides_redundant_guard() -> None:
    ops = [guard(), op("LOOP_START"), guard(), op("LOOP_END"), guard()]
    assert sum(item.kind == "GUARD_TYPE" for item in elide(ops)) == 1


def test_loop_break_uses_exit_path_not_textual_suffix() -> None:
    ops = [
        guard(),
        op("LOOP_START"),
        op("CALL", MoltValue("mutate"), result="callback"),
        op("LOOP_BREAK"),
        guard(),
        op("LOOP_END"),
        guard(),
    ]
    result = elide(ops)
    assert result[-1] is ops[-1]


def test_jump_join_uses_all_predecessors() -> None:
    ops = [
        op("IF", MoltValue("cond")),
        guard(),
        op("JUMP", "join"),
        op("ELSE"),
        op("CALL", MoltValue("mutate"), result="callback"),
        op("END_IF"),
        op("LABEL", "join"),
        guard(),
    ]
    assert elide(ops)[-1] is ops[-1]


@pytest.mark.parametrize("route", ["TRY_START", "CHECK_EXCEPTION", "STATE_SWITCH"])
def test_guard_success_does_not_flow_to_exception_or_resume_entry(route: str) -> None:
    if route == "STATE_SWITCH":
        ops = [guard(), op(route), op("RETURN"), op("STATE_LABEL", "resume"), guard()]
    else:
        ops = [guard(), op(route, "handler")]
        if route == "TRY_START":
            ops.extend([op("CALL", MoltValue("mutate")), op("TRY_END")])
        ops.extend([op("RETURN"), op("LABEL", "handler"), guard()])
    assert elide(ops)[-1] is ops[-1]


@pytest.mark.parametrize("name", ["value", "expected"])
def test_result_rebinding_invalidates_guard_operands(name: str) -> None:
    ops = [guard(), op("COPY", MoltValue("replacement"), result=name), guard()]
    assert sum(item.kind == "GUARD_TYPE" for item in elide(ops)) == 2


def test_exception_edge_coalesced_with_fallthrough_has_no_success_fact() -> None:
    ops = [guard(), op("CHECK_EXCEPTION", "handler"), op("LABEL", "handler"), guard()]
    cfg = build_cfg(ops)
    edge = (cfg.index_to_block[1], cfg.index_to_block[2])
    assert cfg.edge_kinds[edge] == CFGEdgeKind.NORMAL | CFGEdgeKind.EXCEPTION
    assert sum(item.kind == "GUARD_TYPE" for item in elide(ops)) == 2


def test_cfg_records_complete_edge_modes_at_the_edge_authority() -> None:
    ops = [
        op("TRY_START", "handler"),
        op("CALL", MoltValue("callback")),
        op("TRY_END"),
        op("RETURN"),
        op("LABEL", "handler"),
        op("STATE_SWITCH"),
        op("STATE_LABEL", "resume"),
        op("RETURN"),
    ]
    cfg = build_cfg(ops)
    assert set(cfg.edge_kinds) == {
        (block, successor)
        for block, successors in cfg.successors.items()
        for successor in successors
    }
    entry = cfg.index_to_block[0]
    assert cfg.edge_kinds[entry, cfg.index_to_block[1]] == CFGEdgeKind.NORMAL
    assert cfg.edge_kinds[entry, cfg.index_to_block[3]] == CFGEdgeKind.EXCEPTION
    assert cfg.edge_kinds[entry, cfg.index_to_block[4]] == CFGEdgeKind.EXCEPTION
    assert (
        cfg.edge_kinds[cfg.index_to_block[5], cfg.index_to_block[6]]
        == CFGEdgeKind.RESUME
    )


def test_guard_callback_admission_does_not_borrow_emitter_constant() -> None:
    gen = SimpleTIRGenerator()
    gen._op_by_result = {"x": op("CONST", 1, result="x")}
    ops = [
        op("MISSING", result="x"),
        guard(),
        op("BOOL", MoltValue("x"), result="predicate"),
        guard(),
    ]
    rewritten, _, _, _ = gen._eliminate_redundant_guards_cfg(ops)
    assert sum(item.kind == "GUARD_TYPE" for item in rewritten) == 2


def rewrite_branches(
    then: list[MoltOp], otherwise: list[MoltOp], *, exact_condition: bool = True
) -> list[MoltOp]:
    condition = (
        op("IS", MoltValue("left"), MoltValue("right"), result="cond")
        if exact_condition
        else op("MISSING", result="cond")
    )
    ops = [
        condition,
        op("IF", MoltValue("cond")),
        *then,
        op("ELSE"),
        *otherwise,
        op("END_IF"),
    ]
    result, _ = SimpleTIRGenerator()._rewrite_structured_if_regions(
        ops, control=build_cfg(ops).control, branch_choice_by_if_index={}
    )
    assert result[0] is condition
    return result[1:]


def test_common_entry_guard_hoists_without_erasing_later_callback_guards() -> None:
    result = rewrite_branches(
        [guard(), op("CALL", MoltValue("left")), guard(), op("CONST", 1, result="a")],
        [guard(), op("CALL", MoltValue("right")), guard(), op("CONST", 2, result="b")],
    )
    assert result[0].kind == "GUARD_TYPE"
    assert sum(item.kind == "GUARD_TYPE" for item in result) == 3


def test_guard_after_callback_is_not_hoisted() -> None:
    result = rewrite_branches(
        [op("CALL", MoltValue("left")), guard(), op("CONST", 1, result="a")],
        [op("CALL", MoltValue("right")), guard(), op("CONST", 2, result="b")],
    )
    assert result[0].kind == "IF"


def test_guard_cannot_hoist_across_dynamic_branch_truth_callback() -> None:
    result = rewrite_branches(
        [guard(), op("CONST", 1, result="a")],
        [guard(), op("CONST", 2, result="b")],
        exact_condition=False,
    )
    assert result[0].kind == "IF"


@pytest.mark.parametrize(
    ("kind", "args"),
    [
        (
            "STATE_TRANSITION",
            [MoltValue("task"), MoltValue("slot"), MoltValue("pending"), 1],
        ),
        (
            "CHAN_SEND_YIELD",
            [MoltValue("channel"), MoltValue("item"), MoltValue("pending"), 1],
        ),
        ("CHAN_RECV_YIELD", [MoltValue("channel"), MoltValue("pending"), 1]),
        ("IF", [MoltValue("condition")]),
    ],
)
def test_callback_controls_invalidate_guard_availability(
    kind: str, args: list[object]
) -> None:
    gen = SimpleTIRGenerator()
    signature = gen._guard_signature(guard())
    assert signature is not None
    available = {signature}
    gen._clear_invalidated_guard_signatures(available, op(kind, *args))
    assert not available


@pytest.mark.parametrize(
    "kind", ["STATE_TRANSITION", "CHAN_SEND_YIELD", "CHAN_RECV_YIELD"]
)
def test_callback_control_preserves_control_class_but_advances_heap_epoch(
    kind: str,
) -> None:
    gen = SimpleTIRGenerator()
    args = [MoltValue("task"), MoltValue("slot"), MoltValue("pending"), 1]
    if kind == "CHAN_RECV_YIELD":
        args = [MoltValue("task"), MoltValue("pending"), 1]
    control = op(kind, *args)
    assert gen._op_effect_class(control) == "control"
    assert gen._op_may_access_arbitrary_heap(control)
    state = gen._empty_canonicalization_state()
    _, state = gen._canonicalize_block_with_state([control], state, induction_steps={})
    assert state["memory_epoch"] == 1


def test_guard_failure_order_is_not_sorted_across_branches() -> None:
    result = rewrite_branches(
        [guard("a"), guard("b"), op("CONST", 1, result="left")],
        [guard("b"), guard("a"), op("CONST", 2, result="right")],
    )
    assert result[0].kind == "IF"
    assert sum(item.kind == "GUARD_TYPE" for item in result) == 4


@pytest.mark.parametrize("body", [[], [op("LINE", 1)]])
def test_identical_branch_rewrites_preserve_dynamic_condition(
    body: list[MoltOp],
) -> None:
    ops = [
        op("MISSING", result="cond"),
        op("IF", MoltValue("cond")),
        *body,
        op("ELSE"),
        *body,
        op("END_IF"),
    ]
    gen = SimpleTIRGenerator()
    direct, _ = gen._canonicalize_structured_regions_pre_sccp(ops)
    assert any(item.kind == "IF" for item in direct)
    rewritten = rewrite_branches(body, body, exact_condition=False)
    assert any(item.kind == "IF" for item in rewritten)


def test_empty_loop_retains_its_backedge() -> None:
    ops = [op("LOOP_START"), op("LOOP_END")]
    assert SimpleTIRGenerator()._canonicalize_structured_regions_pre_sccp(ops) == (
        ops,
        0,
    )
