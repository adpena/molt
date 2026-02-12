from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Protocol, Sequence


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
    if_to_else: dict[int, int]
    if_to_end: dict[int, int]
    else_to_end: dict[int, int]
    loop_start_to_end: dict[int, int]
    loop_end_to_start: dict[int, int]
    loop_owner: dict[int, int]
    try_start_to_end: dict[int, int]
    try_end_to_start: dict[int, int]


@dataclass(frozen=True)
class CFGGraph:
    blocks: list[BasicBlock]
    index_to_block: dict[int, int]
    label_to_block: dict[str, int]
    block_entry_label: dict[int, str]
    control: ControlMaps
    successors: dict[int, list[int]]
    predecessors: dict[int, list[int]]
    reachable: set[int]
    dominators: dict[int, set[int]]


def _collect_control_maps(ops: Sequence[OpLike]) -> ControlMaps:
    if_stack: list[int] = []
    if_to_else: dict[int, int] = {}
    if_to_end: dict[int, int] = {}
    else_to_end: dict[int, int] = {}

    loop_stack: list[int] = []
    loop_start_to_end: dict[int, int] = {}
    loop_end_to_start: dict[int, int] = {}
    loop_owner: dict[int, int] = {}

    try_stack: list[int] = []
    try_start_to_end: dict[int, int] = {}
    try_end_to_start: dict[int, int] = {}

    for idx, op in enumerate(ops):
        if loop_stack:
            loop_owner[idx] = loop_stack[-1]
        if op.kind == "IF":
            if_stack.append(idx)
        elif op.kind == "ELSE":
            if if_stack:
                if_to_else[if_stack[-1]] = idx
        elif op.kind == "END_IF":
            if if_stack:
                if_idx = if_stack.pop()
                if_to_end[if_idx] = idx
                else_idx = if_to_else.get(if_idx)
                if else_idx is not None:
                    else_to_end[else_idx] = idx
        elif op.kind == "LOOP_START":
            loop_stack.append(idx)
            loop_owner[idx] = idx
        elif op.kind == "LOOP_END":
            if loop_stack:
                start_idx = loop_stack.pop()
                loop_start_to_end[start_idx] = idx
                loop_end_to_start[idx] = start_idx
        elif op.kind == "TRY_START":
            try_stack.append(idx)
        elif op.kind == "TRY_END":
            if try_stack:
                start_idx = try_stack.pop()
                try_start_to_end.setdefault(start_idx, idx)
                try_end_to_start.setdefault(idx, start_idx)

    return ControlMaps(
        if_to_else=if_to_else,
        if_to_end=if_to_end,
        else_to_end=else_to_end,
        loop_start_to_end=loop_start_to_end,
        loop_end_to_start=loop_end_to_start,
        loop_owner=loop_owner,
        try_start_to_end=try_start_to_end,
        try_end_to_start=try_end_to_start,
    )


def _build_basic_blocks(
    ops: Sequence[OpLike], control: ControlMaps
) -> tuple[list[BasicBlock], dict[int, int], dict[str, int], dict[int, str]]:
    if not ops:
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
        "LOOP_CONTINUE",
        "TRY_START",
        "TRY_END",
        "JUMP",
        "RETURN",
        "RAISE",
        "RAISE_CAUSE",
        "RERAISE",
        "LABEL",
        "STATE_LABEL",
        "CHECK_EXCEPTION",
    }
    split_after_kinds = leader_kinds

    leaders: set[int] = {0}
    for idx, op in enumerate(ops):
        if op.kind in leader_kinds:
            leaders.add(idx)
        if op.kind in split_after_kinds and idx + 1 < len(ops):
            leaders.add(idx + 1)

    leader_list = sorted(leaders)
    blocks: list[BasicBlock] = []
    for block_idx, start in enumerate(leader_list):
        end = (
            leader_list[block_idx + 1] if block_idx + 1 < len(leader_list) else len(ops)
        )
        blocks.append(BasicBlock(id=block_idx, start=start, end=end))

    index_to_block: dict[int, int] = {}
    for block in blocks:
        for idx in range(block.start, block.end):
            index_to_block[idx] = block.id

    label_to_block: dict[str, int] = {}
    block_entry_label: dict[int, str] = {}
    for block in blocks:
        op = ops[block.start]
        if op.kind in {"LABEL", "STATE_LABEL"} and op.args:
            label = str(op.args[0])
            label_to_block[label] = block.id
            block_entry_label[block.id] = label

    return blocks, index_to_block, label_to_block, block_entry_label


def _compute_successors(
    *,
    ops: Sequence[OpLike],
    blocks: list[BasicBlock],
    index_to_block: dict[int, int],
    label_to_block: dict[str, int],
    control: ControlMaps,
) -> dict[int, list[int]]:
    successors: dict[int, list[int]] = {block.id: [] for block in blocks}
    if not blocks:
        return successors

    def add_succ(block_id: int, succ: int | None) -> None:
        if succ is None:
            return
        if succ < 0 or succ >= len(blocks):
            return
        if succ not in successors[block_id]:
            successors[block_id].append(succ)

    def block_for_index(idx: int | None) -> int | None:
        if idx is None:
            return None
        return index_to_block.get(idx)

    for block in blocks:
        block_id = block.id
        if block.start >= block.end:
            continue
        op_idx = block.end - 1
        op = ops[op_idx]
        next_block = block_id + 1 if block_id + 1 < len(blocks) else None

        if op.kind == "JUMP":
            target = str(op.args[0]) if op.args else ""
            add_succ(block_id, label_to_block.get(target))
            continue
        if op.kind == "IF":
            add_succ(block_id, next_block)
            false_idx = control.if_to_else.get(op_idx)
            if false_idx is None:
                false_idx = control.if_to_end.get(op_idx)
            add_succ(block_id, block_for_index(false_idx))
            continue
        if op.kind == "ELSE":
            end_if_idx = control.else_to_end.get(op_idx)
            after_end_if = None if end_if_idx is None else end_if_idx + 1
            add_succ(block_id, block_for_index(after_end_if))
            continue
        if op.kind == "LOOP_BREAK":
            owner = control.loop_owner.get(op_idx)
            end_idx = (
                control.loop_start_to_end.get(owner) if owner is not None else None
            )
            exit_idx = None if end_idx is None else end_idx + 1
            add_succ(block_id, block_for_index(exit_idx))
            continue
        if op.kind in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE"}:
            add_succ(block_id, next_block)
            owner = control.loop_owner.get(op_idx)
            end_idx = (
                control.loop_start_to_end.get(owner) if owner is not None else None
            )
            exit_idx = None if end_idx is None else end_idx + 1
            add_succ(block_id, block_for_index(exit_idx))
            continue
        if op.kind == "LOOP_CONTINUE":
            owner = control.loop_owner.get(op_idx)
            add_succ(block_id, block_for_index(owner))
            continue
        if op.kind == "LOOP_END":
            add_succ(block_id, next_block)
            add_succ(block_id, block_for_index(control.loop_end_to_start.get(op_idx)))
            continue
        if op.kind == "TRY_START":
            add_succ(block_id, next_block)
            try_end_idx = control.try_start_to_end.get(op_idx)
            add_succ(
                block_id,
                block_for_index(None if try_end_idx is None else try_end_idx + 1),
            )
            continue
        if op.kind == "CHECK_EXCEPTION":
            add_succ(block_id, next_block)
            target = str(op.args[0]) if op.args else ""
            add_succ(block_id, label_to_block.get(target))
            continue
        if op.kind in {"RETURN", "RAISE", "RAISE_CAUSE", "RERAISE"}:
            continue

        add_succ(block_id, next_block)

    return successors


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


def _compute_dominators(
    *,
    block_count: int,
    predecessors: dict[int, list[int]],
    reachable: set[int],
) -> dict[int, set[int]]:
    dominators: dict[int, set[int]] = {}
    all_blocks = set(range(block_count))
    for block_id in range(block_count):
        if block_id == 0:
            dominators[block_id] = {0}
        elif block_id in reachable:
            dominators[block_id] = all_blocks.copy()
        else:
            dominators[block_id] = {block_id}

    changed = True
    while changed:
        changed = False
        for block_id in range(1, block_count):
            if block_id not in reachable:
                continue
            preds = [p for p in predecessors.get(block_id, []) if p in reachable]
            if not preds:
                new_dom = {block_id}
            else:
                pred_sets = [dominators[p] for p in preds]
                new_dom = set.intersection(*pred_sets)
                new_dom.add(block_id)
            if new_dom != dominators[block_id]:
                dominators[block_id] = new_dom
                changed = True
    return dominators


def build_cfg(ops: Sequence[OpLike]) -> CFGGraph:
    control = _collect_control_maps(ops)
    blocks, index_to_block, label_to_block, block_entry_label = _build_basic_blocks(
        ops, control
    )
    successors = _compute_successors(
        ops=ops,
        blocks=blocks,
        index_to_block=index_to_block,
        label_to_block=label_to_block,
        control=control,
    )
    predecessors = _compute_predecessors(successors)
    reachable = _reachable_blocks(successors)
    dominators = _compute_dominators(
        block_count=len(blocks),
        predecessors=predecessors,
        reachable=reachable,
    )
    return CFGGraph(
        blocks=blocks,
        index_to_block=index_to_block,
        label_to_block=label_to_block,
        block_entry_label=block_entry_label,
        control=control,
        successors=successors,
        predecessors=predecessors,
        reachable=reachable,
        dominators=dominators,
    )
