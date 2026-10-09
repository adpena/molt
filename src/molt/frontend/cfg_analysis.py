from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import IntFlag
from functools import cached_property, lru_cache
from types import MappingProxyType
from typing import Any, Protocol, Sequence

from molt.frontend.lowering.try_regions import try_region_id


class OpLike(Protocol):
    kind: str
    args: list[Any]


@dataclass(frozen=True)
class BasicBlock:
    id: int
    start: int
    end: int


@dataclass(frozen=True)
class ControlMaps:
    if_to_else: Mapping[int, int]
    if_to_end: Mapping[int, int]
    else_to_end: Mapping[int, int]
    loop_start_to_end: Mapping[int, int]
    loop_end_to_start: Mapping[int, int]
    loop_owner: Mapping[int, int]


class CFGEdgeKind(IntFlag):
    """Coalesced edges retain every execution mode they represent."""

    NORMAL = 1
    EXCEPTION = 2
    RESUME = 4


# Successor construction ORs plain bits per edge and converts each edge once:
# IntFlag arithmetic goes through the enum machinery on every operation.
_NORMAL_EDGE = int(CFGEdgeKind.NORMAL)
_EXCEPTION_EDGE = int(CFGEdgeKind.EXCEPTION)
_RESUME_EDGE = int(CFGEdgeKind.RESUME)
_EDGE_KIND_BY_BITS = tuple(CFGEdgeKind(bits) for bits in range(1 << len(CFGEdgeKind)))


@dataclass(frozen=True)
class CFGGraph:
    """Control flow of one op list. Immutable: ``build_cfg`` gives every op list
    with the same control projection the same graph."""

    blocks: Sequence[BasicBlock]
    index_to_block: Mapping[int, int]
    label_to_block: Mapping[str, int]
    block_entry_label: Mapping[int, str]
    control: ControlMaps
    successors: Mapping[int, Sequence[int]]
    edge_kinds: Mapping[tuple[int, int], CFGEdgeKind]
    predecessors: Mapping[int, Sequence[int]]
    reachable: frozenset[int] | set[int]

    # Dominance is derived on first use: most graphs a pass round builds are
    # never asked a dominance query.
    @cached_property
    def idom(self) -> dict[int, int]:
        """Immediate dominator of each reachable block; the entry maps to itself."""
        return _compute_immediate_dominators(
            successors=self.successors, predecessors=self.predecessors
        )

    @cached_property
    def _dominance_span(self) -> dict[int, tuple[int, int]]:
        return _dominator_tree_spans(self.idom)

    def dominates(self, dominator: int, block: int) -> bool:
        """Whether every entry path to ``block`` passes through ``dominator``.

        An unreachable block is dominated only by itself.
        """
        inner = self._dominance_span.get(block)
        if inner is None:
            return dominator == block
        outer = self._dominance_span.get(dominator)
        return outer is not None and outer[0] <= inner[0] and inner[1] <= outer[1]


def _collect_control_maps(kinds: Sequence[str]) -> ControlMaps:
    if_stack: list[int] = []
    if_to_else: dict[int, int] = {}
    if_to_end: dict[int, int] = {}
    else_to_end: dict[int, int] = {}

    loop_stack: list[int] = []
    loop_start_to_end: dict[int, int] = {}
    loop_end_to_start: dict[int, int] = {}
    loop_owner: dict[int, int] = {}

    for idx, kind in enumerate(kinds):
        if loop_stack:
            loop_owner[idx] = loop_stack[-1]
        if kind == "IF":
            if_stack.append(idx)
        elif kind == "ELSE":
            if if_stack:
                if_to_else[if_stack[-1]] = idx
        elif kind == "END_IF":
            if if_stack:
                if_idx = if_stack.pop()
                if_to_end[if_idx] = idx
                else_idx = if_to_else.get(if_idx)
                if else_idx is not None:
                    else_to_end[else_idx] = idx
        elif kind == "LOOP_START":
            loop_stack.append(idx)
            loop_owner[idx] = idx
        elif kind == "LOOP_END":
            if loop_stack:
                start_idx = loop_stack.pop()
                loop_start_to_end[start_idx] = idx
                loop_end_to_start[idx] = start_idx

    return ControlMaps(
        if_to_else=MappingProxyType(if_to_else),
        if_to_end=MappingProxyType(if_to_end),
        else_to_end=MappingProxyType(else_to_end),
        loop_start_to_end=MappingProxyType(loop_start_to_end),
        loop_end_to_start=MappingProxyType(loop_end_to_start),
        loop_owner=MappingProxyType(loop_owner),
    )


def _build_basic_blocks(
    kinds: Sequence[str], operands: Mapping[int, str | None]
) -> tuple[list[BasicBlock], dict[int, int], dict[str, int], dict[int, str]]:
    if not kinds:
        return [], {}, {}, {}

    leader_kinds = {
        "IF",
        "ELSE",
        "END_IF",
        "LOOP_START",
        "LOOP_END",
        "LOOP_BREAK",
        "LOOP_BREAK_IF_TRUE",
        "LOOP_BREAK_IF_FALSE",
        "LOOP_BREAK_IF_EXCEPTION",
        "LOOP_CONTINUE",
        "TRY_START",
        "TRY_END",
        "JUMP",
        "ret",
        "ret_void",
        "RAISE",
        "RAISE_CAUSE",
        "RERAISE",
        "LABEL",
        "STATE_LABEL",
        "STATE_SWITCH",
        "STATE_YIELD",
        "STATE_TRANSITION",
        "CHECK_EXCEPTION",
    }
    split_after_kinds = leader_kinds

    leaders: set[int] = {0}
    for idx, kind in enumerate(kinds):
        if kind in leader_kinds:
            leaders.add(idx)
        if kind in split_after_kinds and idx + 1 < len(kinds):
            leaders.add(idx + 1)

    leader_list = sorted(leaders)
    blocks: list[BasicBlock] = []
    for block_idx, start in enumerate(leader_list):
        end = (
            leader_list[block_idx + 1]
            if block_idx + 1 < len(leader_list)
            else len(kinds)
        )
        blocks.append(BasicBlock(id=block_idx, start=start, end=end))

    index_to_block: dict[int, int] = {}
    for block in blocks:
        for idx in range(block.start, block.end):
            index_to_block[idx] = block.id

    label_to_block: dict[str, int] = {}
    block_entry_label: dict[int, str] = {}
    for block in blocks:
        if kinds[block.start] in _LABEL_KINDS:
            label = operands[block.start]
            if label is not None:
                label_to_block[label] = block.id
                block_entry_label[block.id] = label

    return blocks, index_to_block, label_to_block, block_entry_label


def _compute_successors(
    *,
    kinds: Sequence[str],
    operands: Mapping[int, str | None],
    blocks: list[BasicBlock],
    index_to_block: dict[int, int],
    label_to_block: dict[str, int],
    control: ControlMaps,
) -> tuple[dict[int, list[int]], dict[tuple[int, int], CFGEdgeKind]]:
    successors: dict[int, list[int]] = {block.id: [] for block in blocks}
    edge_bits: dict[tuple[int, int], int] = {}
    if not blocks:
        return successors, {}

    # Collect resume-target blocks: blocks immediately following a STATE_YIELD.
    # These are only reachable via STATE_SWITCH dispatch, not via normal
    # fall-through from the STATE_YIELD block.
    state_yield_resume_blocks: list[int] = []
    for block in blocks:
        for idx in range(block.start, block.end):
            if kinds[idx] == "STATE_YIELD" and idx + 1 < len(kinds):
                resume_block = index_to_block.get(idx + 1)
                if resume_block is not None:
                    state_yield_resume_blocks.append(resume_block)
    # Also collect STATE_LABEL blocks as resume targets (for async state machines).
    for block in blocks:
        if kinds[block.start] == "STATE_LABEL":
            state_yield_resume_blocks.append(block.id)

    def add_succ(block_id: int, succ: int | None, kind: int = _NORMAL_EDGE) -> None:
        if succ is None:
            return
        if succ < 0 or succ >= len(blocks):
            return
        if succ not in successors[block_id]:
            successors[block_id].append(succ)
        edge = (block_id, succ)
        edge_bits[edge] = edge_bits.get(edge, 0) | kind

    def block_for_index(idx: int | None) -> int | None:
        if idx is None:
            return None
        return index_to_block.get(idx)

    for block in blocks:
        block_id = block.id
        if block.start >= block.end:
            continue
        op_idx = block.end - 1
        kind = kinds[op_idx]
        next_block = block_id + 1 if block_id + 1 < len(blocks) else None

        if kind == "JUMP":
            add_succ(block_id, label_to_block.get(operands[op_idx]))
            continue
        if kind == "IF":
            add_succ(block_id, next_block)
            false_idx = control.if_to_else.get(op_idx)
            if false_idx is not None:
                # The IF's false branch should target the first op AFTER the
                # ELSE marker (the actual else-content), not the ELSE marker
                # block itself.  The ELSE marker terminates the then-path and
                # jumps to END_IF; it must not be the entry point for the
                # else-path, otherwise the else-content block has no predecessor
                # and becomes unreachable.
                add_succ(block_id, block_for_index(false_idx + 1))
            else:
                false_idx = control.if_to_end.get(op_idx)
                add_succ(block_id, block_for_index(false_idx))
            continue
        if kind == "ELSE":
            end_if_idx = control.else_to_end.get(op_idx)
            # Target the END_IF block itself (not end_if_idx + 1) — END_IF
            # is a block leader so it starts its own block which falls through
            # to the merge point.  Using end_if_idx + 1 produces an out-of-
            # bounds index when END_IF is the last op, silently giving the
            # ELSE block zero successors and making the else-content block
            # unreachable.  That breaks liveness and allows the CSE/DCE to
            # incorrectly eliminate ops inside else branches.
            add_succ(block_id, block_for_index(end_if_idx))
            continue
        if kind == "LOOP_BREAK":
            owner = control.loop_owner.get(op_idx)
            end_idx = (
                control.loop_start_to_end.get(owner) if owner is not None else None
            )
            exit_idx = None if end_idx is None else end_idx + 1
            add_succ(block_id, block_for_index(exit_idx))
            continue
        if kind in {
            "LOOP_BREAK_IF_TRUE",
            "LOOP_BREAK_IF_FALSE",
            "LOOP_BREAK_IF_EXCEPTION",
        }:
            add_succ(block_id, next_block)
            owner = control.loop_owner.get(op_idx)
            end_idx = (
                control.loop_start_to_end.get(owner) if owner is not None else None
            )
            exit_idx = None if end_idx is None else end_idx + 1
            add_succ(block_id, block_for_index(exit_idx))
            continue
        if kind == "LOOP_CONTINUE":
            owner = control.loop_owner.get(op_idx)
            add_succ(block_id, block_for_index(owner))
            continue
        if kind == "LOOP_END":
            add_succ(block_id, next_block)
            add_succ(block_id, block_for_index(control.loop_end_to_start.get(op_idx)))
            continue
        if kind == "TRY_START":
            add_succ(block_id, next_block)
            # Region markers describe path-local custody, not textual brackets.
            # Keep handler reachability conservative until shared TIR exception
            # analysis can prove the actual exceptional edges.
            add_succ(block_id, label_to_block.get(operands[op_idx]), _EXCEPTION_EDGE)
            continue
        if kind == "CHECK_EXCEPTION":
            add_succ(block_id, next_block)
            add_succ(block_id, label_to_block.get(operands[op_idx]), _EXCEPTION_EDGE)
            continue
        if kind in {"ret", "ret_void"}:
            continue
        if kind in {"RAISE", "RAISE_CAUSE", "RERAISE"}:
            # molt's exception model lowers `raise` to "set the pending
            # exception flag and CONTINUE" (the native/WASM/LLVM `raise` op calls
            # `molt_raise` and falls through — it does NOT branch). The frontend
            # always emits the routing immediately after a raise
            # (`CHECK_EXCEPTION <handler>; JUMP <handler>` via `_emit_raise_exit`)
            # and the backends transfer control to the handler THROUGH those ops,
            # not through any implicit edge. Modelling `raise` as a hard CFG
            # terminator (no successors) makes that routing unreachable, so SCCP/
            # DCE prune it — orphaning a handler that the body can only reach
            # after raising (e.g. a try body whose sole statement is `raise`) and
            # silently dropping the `except` clause. The fall-through edge mirrors
            # the real lowering and keeps the explicit routing live.
            add_succ(block_id, next_block)
            continue
        if kind == "STATE_YIELD":
            # STATE_YIELD suspends the generator (returns to caller).
            # The block after it is only reachable via STATE_SWITCH on
            # the next call to next()/send()/throw(), NOT via fall-through.
            # Treat it like a return (no successors).
            continue
        if kind == "STATE_SWITCH":
            # STATE_SWITCH dispatches to any resume block in the function:
            # blocks after STATE_YIELD ops and STATE_LABEL blocks.
            add_succ(block_id, next_block, _RESUME_EDGE)
            for resume_block in state_yield_resume_blocks:
                add_succ(block_id, resume_block, _RESUME_EDGE)
            continue

        add_succ(block_id, next_block)

    return successors, {
        edge: _EDGE_KIND_BY_BITS[bits] for edge, bits in edge_bits.items()
    }


def _compute_predecessors(successors: dict[int, list[int]]) -> dict[int, list[int]]:
    predecessors: dict[int, list[int]] = {block_id: [] for block_id in successors}
    for block_id, succs in successors.items():
        for succ in succs:
            if succ not in predecessors:
                predecessors[succ] = []
            if block_id not in predecessors[succ]:
                predecessors[succ].append(block_id)
    return predecessors


def _reachable_blocks(successors: dict[int, list[int]]) -> set[int]:
    if not successors:
        return set()
    seen: set[int] = set()
    stack = [0]
    while stack:
        block_id = stack.pop()
        if block_id in seen:
            continue
        seen.add(block_id)
        for succ in successors.get(block_id, []):
            if succ not in seen:
                stack.append(succ)
    return seen


def _compute_immediate_dominators(
    *,
    successors: dict[int, list[int]],
    predecessors: dict[int, list[int]],
) -> dict[int, int]:
    """Immediate dominators of the blocks reachable from the entry block 0.

    Cooper, Harvey and Kennedy, "A Simple, Fast Dominance Algorithm" (2001):
    iterate over reverse postorder, intersecting predecessor paths in the
    dominator tree. It keeps one entry per block; a full dominator set per
    block grows quadratically and took 5 GB for a 12,000-op module body.
    """
    if not successors:
        return {}
    postorder: list[int] = []
    seen = {0}
    stack = [(0, iter(successors.get(0, ())))]
    while stack:
        block, pending = stack[-1]
        for successor in pending:
            if successor not in seen:
                seen.add(successor)
                stack.append((successor, iter(successors.get(successor, ()))))
                break
        else:
            stack.pop()
            postorder.append(block)
    # Work on postorder numbers: the entry has the highest, every dominator a
    # higher number than the blocks it dominates, and lists replace dicts.
    number = {block: index for index, block in enumerate(postorder)}
    preds = [
        [number[pred] for pred in predecessors.get(block, ()) if pred in number]
        for block in postorder
    ]
    undefined = -1
    entry = len(postorder) - 1
    doms = [undefined] * len(postorder)
    doms[entry] = entry
    changed = True
    while changed:
        changed = False
        for node in range(entry - 1, -1, -1):
            chosen = undefined
            for pred in preds[node]:
                if doms[pred] == undefined:
                    continue
                if chosen == undefined:
                    chosen = pred
                    continue
                left, right = pred, chosen
                while left != right:
                    while left < right:
                        left = doms[left]
                    while right < left:
                        right = doms[right]
                chosen = left
            if chosen != undefined and doms[node] != chosen:
                doms[node] = chosen
                changed = True
    return {
        postorder[node]: postorder[dom]
        for node, dom in enumerate(doms)
        if dom != undefined
    }


def _dominator_tree_spans(idom: dict[int, int]) -> dict[int, tuple[int, int]]:
    """Pre/post visit numbers in the dominator tree, for O(1) dominance tests."""
    children: dict[int, list[int]] = {}
    roots: list[int] = []
    for block, parent in idom.items():
        if block == parent:
            roots.append(block)
        else:
            children.setdefault(parent, []).append(block)
    spans: dict[int, tuple[int, int]] = {}
    counter = 0
    for root in sorted(roots):
        entry: dict[int, int] = {root: counter}
        counter += 1
        stack = [(root, iter(sorted(children.get(root, ()))))]
        while stack:
            block, pending = stack[-1]
            child = next(pending, None)
            if child is None:
                stack.pop()
                spans[block] = (entry[block], counter)
                counter += 1
            else:
                entry[child] = counter
                counter += 1
                stack.append((child, iter(sorted(children.get(child, ())))))
    return spans


# The CFG reads every op's kind and the first operand of these kinds only. The
# projection carries exactly that, so it is both the builder's sole input and
# the key under which equal op lists share one graph.
_LABEL_KINDS = frozenset({"LABEL", "STATE_LABEL"})
_OPERAND_KINDS = _LABEL_KINDS | {"JUMP", "CHECK_EXCEPTION", "TRY_START"}

_ControlProjection = tuple[tuple[str, ...], tuple[tuple[int, str | None], ...]]


def _control_operand(op: OpLike) -> str | None:
    """The label an operand-reading op contributes, spelled as the CFG uses it."""
    if op.kind in _LABEL_KINDS:
        # An operand-less label names no block.
        return str(op.args[0]) if op.args else None
    if op.kind == "TRY_START":
        handler = try_region_id(op)
        return str(handler) if handler is not None else ""
    return str(op.args[0]) if op.args else ""


def _control_projection(ops: Sequence[OpLike]) -> _ControlProjection:
    kinds = tuple([op.kind for op in ops])
    operands = tuple(
        (index, _control_operand(op))
        for index, op in enumerate(ops)
        if op.kind in _OPERAND_KINDS
    )
    return kinds, operands


def build_cfg(ops: Sequence[OpLike]) -> CFGGraph:
    """The CFG of ``ops``.

    Midend pass rounds rebuild the CFG of an op list most of whose rounds
    change no control flow, so graphs are shared by control projection.
    """
    return _cfg_for_projection(_control_projection(ops))


@lru_cache(maxsize=128)
def _cfg_for_projection(projection: _ControlProjection) -> CFGGraph:
    kinds, operand_items = projection
    operands = dict(operand_items)
    control = _collect_control_maps(kinds)
    blocks, index_to_block, label_to_block, block_entry_label = _build_basic_blocks(
        kinds, operands
    )
    successors, edge_kinds = _compute_successors(
        kinds=kinds,
        operands=operands,
        blocks=blocks,
        index_to_block=index_to_block,
        label_to_block=label_to_block,
        control=control,
    )
    predecessors = _compute_predecessors(successors)
    reachable = _reachable_blocks(successors)
    return CFGGraph(
        blocks=tuple(blocks),
        index_to_block=MappingProxyType(index_to_block),
        label_to_block=MappingProxyType(label_to_block),
        block_entry_label=MappingProxyType(block_entry_label),
        control=control,
        successors=MappingProxyType(
            {block: tuple(targets) for block, targets in successors.items()}
        ),
        edge_kinds=MappingProxyType(edge_kinds),
        predecessors=MappingProxyType(
            {block: tuple(sources) for block, sources in predecessors.items()}
        ),
        reachable=frozenset(reachable),
    )
