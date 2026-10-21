"""Guard motion respects callbacks, failure order and CFG edge modes."""

from __future__ import annotations

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend.cfg_analysis import CFGEdgeKind, build_cfg


def op(kind: str, *args: object, result: str = "none") -> MoltOp:
    return MoltOp(kind=kind, args=list(args), result=MoltValue(result))


def guard(value: str = "value") -> MoltOp:
    return op("GUARD_TAG", MoltValue(value), MoltValue("expected"))


def test_exception_edge_coalesced_with_fallthrough_records_both_modes() -> None:
    ops = [guard(), op("CHECK_EXCEPTION", "handler"), op("LABEL", "handler"), guard()]
    cfg = build_cfg(ops)
    edge = (cfg.op_blocks[1], cfg.op_blocks[2])
    assert cfg.edge_kinds[edge] == CFGEdgeKind.NORMAL | CFGEdgeKind.EXCEPTION


def test_cfg_records_complete_edge_modes_at_the_edge_authority() -> None:
    ops = [
        op("TRY_START", "handler"),
        op("CALL", MoltValue("callback")),
        op("TRY_END"),
        op("ret_void"),
        op("LABEL", "handler"),
        op("STATE_SWITCH"),
        op("STATE_LABEL", "resume"),
        op("ret_void"),
    ]
    cfg = build_cfg(ops)
    assert set(cfg.edge_kinds) == {
        (block, successor)
        for block, successors in cfg.successors.items()
        for successor in successors
    }
    entry = cfg.op_blocks[0]
    assert cfg.edge_kinds[entry, cfg.op_blocks[1]] == CFGEdgeKind.NORMAL
    # Region custody routes to the handler, never past a textual TRY_END.
    assert (entry, cfg.op_blocks[3]) not in cfg.edge_kinds
    assert cfg.edge_kinds[entry, cfg.op_blocks[4]] == CFGEdgeKind.EXCEPTION
    assert cfg.edge_kinds[cfg.op_blocks[5], cfg.op_blocks[6]] == CFGEdgeKind.RESUME


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
    assert result[0].kind == "GUARD_TAG"
    assert sum(item.kind == "GUARD_TAG" for item in result) == 3


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


def test_callback_control_preserves_control_class_but_advances_heap_epoch() -> None:
    gen = SimpleTIRGenerator()
    args = [MoltValue("task"), MoltValue("slot"), MoltValue("pending"), 1]
    control = op("STATE_TRANSITION", *args)
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
    assert sum(item.kind == "GUARD_TAG" for item in result) == 4


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
