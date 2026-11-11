"""Guard motion respects callbacks, failure order and CFG edge modes."""

from __future__ import annotations


from molt.frontend import MoltOp, MoltValue
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
