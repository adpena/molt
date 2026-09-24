"""MidendCFGMixin: frontend IR CFG-region normalization, guard pruning, edge threading, and labels."""

from __future__ import annotations

from collections import deque
from typing import Any, NoReturn

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend.cfg_analysis import CFGEdgeKind

from molt.frontend._types import (
    CFGGraph,
    ControlMaps,
    MoltOp,
    MoltValue,
    build_cfg,
)
from molt.frontend.lowering.try_regions import try_region_id
from molt.frontend.lowering.midend_dataflow import current_unique_result_definitions


class MidendCFGMixin(GeneratorMixinBase):
    def _guard_signature(self, op: MoltOp) -> tuple[Any, ...] | None:
        if op.kind not in {"GUARD_TAG", "GUARD_TYPE", "GUARD_DICT_SHAPE"}:
            return None
        if op.result.name != "none":
            return None
        normalized_args: list[Any] = []
        for arg in op.args:
            if isinstance(arg, MoltValue):
                normalized_args.append(("v", arg.name))
            else:
                normalized_args.append(("c", arg))
        return (op.kind, tuple(normalized_args))

    def _clear_invalidated_guard_signatures(
        self, available: set[tuple[Any, ...]], op: MoltOp
    ) -> None:
        if not available:
            return
        # SimpleIR can reuse result names before SSA construction. A new
        # binding invalidates every guard mentioning the old value.
        if op.result.name != "none":
            rebound = ("v", op.result.name)
            available.difference_update([sig for sig in available if rebound in sig[1]])
        effect_class = self._op_effect_class(op)
        if self._op_may_access_arbitrary_heap(op):
            available.clear()
            return
        if effect_class == "writes_heap":
            stale = [
                sig
                for sig in available
                if sig and isinstance(sig, tuple) and sig[0] == "GUARD_DICT_SHAPE"
            ]
            for sig in stale:
                available.discard(sig)

    def _eliminate_redundant_fused_dict_increment_guards(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        use_counts: dict[str, int] = {}
        users_by_value: dict[str, set[int]] = {}
        removable_guard_producer_kinds = {
            "BUILTIN_TYPE",
            "CLASS_LAYOUT_VERSION",
            "CLASS_VERSION",
            "CONST",
            "CONST_BOOL",
            "CONST_STR",
            "MISSING",
        }
        guard_consumer_skip_kinds = {"CHECK_EXCEPTION", "LINE"}
        for op_index, op in enumerate(ops):
            for arg in op.args:
                if isinstance(arg, MoltValue):
                    use_counts[arg.name] = use_counts.get(arg.name, 0) + 1
                    users_by_value.setdefault(arg.name, set()).add(op_index)

        fused_dict_operand_index = {
            "DICT_STR_INT_INC": 0,
            "STRING_SPLIT_WS_DICT_INC": 1,
            "STRING_SPLIT_SEP_DICT_INC": 2,
        }

        remove_indices: set[int] = set()
        removed_guards = 0
        for idx, op in enumerate(ops):
            op = ops[idx]
            if (
                op.kind == "GUARD_DICT_SHAPE"
                and len(op.args) == 3
                and op.result.name != "none"
                and use_counts.get(op.result.name, 0) == 0
                and idx + 1 < len(ops)
            ):
                next_idx = idx + 1
                while (
                    next_idx < len(ops)
                    and ops[next_idx].kind in guard_consumer_skip_kinds
                ):
                    next_idx += 1
                if next_idx >= len(ops):
                    continue
                next_op = ops[next_idx]
                dict_operand_index = fused_dict_operand_index.get(next_op.kind)
                guarded = op.args[0]
                if (
                    dict_operand_index is not None
                    and len(next_op.args) > dict_operand_index
                    and isinstance(guarded, MoltValue)
                    and isinstance(next_op.args[dict_operand_index], MoltValue)
                    and guarded.name == next_op.args[dict_operand_index].name
                ):
                    remove_indices.add(idx)
                    removed_guards += 1

        if remove_indices:
            changed = True
            while changed:
                changed = False
                for idx, op in enumerate(ops):
                    if (
                        idx in remove_indices
                        or op.kind not in removable_guard_producer_kinds
                    ):
                        continue
                    if op.result.name == "none":
                        continue
                    users = users_by_value.get(op.result.name, set())
                    if users and users.issubset(remove_indices):
                        remove_indices.add(idx)
                        changed = True

        if not remove_indices:
            return ops, 0

        out = [op for idx, op in enumerate(ops) if idx not in remove_indices]
        return out, removed_guards

    def _eliminate_redundant_guards_cfg(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int, int, int]:
        with current_unique_result_definitions(self, ops):
            if not ops:
                return ops, 0, 0, 0
            signatures = [self._guard_signature(op) for op in ops]
            universe = {sig for sig in signatures if sig is not None}
            if not universe:
                return ops, 0, 0, 0
            cfg = build_cfg(ops)

            # Summarize transfer once. Fixed-point iterations use sets, never repeat
            # producer/effect classification or mutate the instruction stream.
            generated: dict[int, set[tuple[Any, ...]]] = {}
            preserved: dict[int, set[tuple[Any, ...]]] = {}
            for block in cfg.blocks:
                gen: set[tuple[Any, ...]] = set()
                keep = set(universe)
                for idx in range(block.start, block.end):
                    sig = signatures[idx]
                    if sig is not None:
                        gen.add(sig)
                    else:
                        self._clear_invalidated_guard_signatures(gen, ops[idx])
                        self._clear_invalidated_guard_signatures(keep, ops[idx])
                generated[block.id] = gen
                preserved[block.id] = keep

            # Guard success is a must-fact. Start at top away from the entry and
            # intersect every predecessor, including backedges, before rewriting.
            incoming = {block.id: set(universe) for block in cfg.blocks}
            outgoing = {block.id: set(universe) for block in cfg.blocks}
            pending = deque(sorted(cfg.reachable))
            queued = set(pending)
            while pending:
                block_id = pending.popleft()
                queued.remove(block_id)
                preds = [
                    pred for pred in cfg.predecessors[block_id] if pred in cfg.reachable
                ]
                available: set[tuple[Any, ...]] = set()
                if block_id != 0 and preds:
                    available = set(universe)
                    for pred in preds:
                        # Success facts cannot cross failure or externally
                        # resumed execution, including coalesced normal edges.
                        edge_kind = cfg.edge_kinds[pred, block_id]
                        if edge_kind & (CFGEdgeKind.EXCEPTION | CFGEdgeKind.RESUME):
                            available.clear()
                            break
                        available.intersection_update(outgoing[pred])
                incoming[block_id] = available
                result = generated[block_id] | (available & preserved[block_id])
                if result != outgoing[block_id]:
                    outgoing[block_id] = result
                    for successor in cfg.successors[block_id]:
                        if successor in cfg.reachable and successor not in queued:
                            pending.append(successor)
                            queued.add(successor)

            removed: set[int] = set()
            attempted = 0
            for block in cfg.blocks:
                available = (
                    set(incoming[block.id]) if block.id in cfg.reachable else set()
                )
                for idx in range(block.start, block.end):
                    sig = signatures[idx]
                    if sig is not None:
                        attempted += 1
                        if sig in available and block.id in cfg.reachable:
                            removed.add(idx)
                        available.add(sig)
                    else:
                        self._clear_invalidated_guard_signatures(available, ops[idx])
            rewritten = [op for idx, op in enumerate(ops) if idx not in removed]
            return rewritten, attempted, len(removed), attempted - len(removed)

    def _op_equal_for_tail_merge(self, left: MoltOp, right: MoltOp) -> bool:
        return (
            left.kind == right.kind
            and left.result.name == right.result.name
            and left.args == right.args
            and left.metadata == right.metadata
        )

    def _can_tail_merge_op(self, op: MoltOp) -> bool:
        if op.result.name != "none":
            return False
        if op.kind in {
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
            "RETURN",
            "RAISE",
            "RAISE_CAUSE",
            "RERAISE",
            "LABEL",
            "STATE_LABEL",
        }:
            return False
        return True

    def _rewrite_structured_if_regions(
        self,
        ops: list[MoltOp],
        *,
        control: ControlMaps,
        branch_choice_by_if_index: dict[int, bool],
    ) -> tuple[list[MoltOp], int]:
        if_to_else = control.if_to_else
        if_to_end = control.if_to_end

        branch_prunes = 0

        def rewrite_range(start: int, end: int) -> list[MoltOp]:
            nonlocal branch_prunes
            out: list[MoltOp] = []
            i = start
            while i < end:
                op = ops[i]
                if op.kind != "IF" or i not in if_to_end:
                    out.append(op)
                    i += 1
                    continue

                else_idx = if_to_else.get(i)
                end_if_idx = if_to_end[i]
                then_start = i + 1
                then_end = else_idx if else_idx is not None else end_if_idx
                then_ops = rewrite_range(then_start, then_end)
                else_ops = (
                    rewrite_range(else_idx + 1, end_if_idx)
                    if else_idx is not None
                    else []
                )
                can_elide_condition = not self._op_may_access_arbitrary_heap(
                    op
                ) and self._op_instance_cannot_raise(op, {})

                branch_choice = branch_choice_by_if_index.get(i)
                if branch_choice is True:
                    out.extend(then_ops)
                    branch_prunes += 1
                    i = end_if_idx + 1
                    continue
                if branch_choice is False:
                    out.extend(else_ops)
                    branch_prunes += 1
                    i = end_if_idx + 1
                    continue

                if (
                    else_idx is not None
                    and then_ops
                    and else_ops
                    and can_elide_condition
                ):
                    # Only the ordered common entry prefix dominates both
                    # branches. A matching guard later in a branch may follow
                    # a callback, a different failure, or conditional execution.
                    hoisted_count = 0
                    for left, right in zip(then_ops, else_ops):
                        if self._guard_signature(left) is None:
                            break
                        if not self._op_equal_for_tail_merge(left, right):
                            break
                        hoisted_count += 1
                    self.midend_stats["guard_hoist_attempts"] += max(1, hoisted_count)
                    if hoisted_count:
                        self.midend_stats["guard_hoist_accepted"] += hoisted_count
                        out.extend(then_ops[:hoisted_count])
                        then_ops = then_ops[hoisted_count:]
                        else_ops = else_ops[hoisted_count:]
                    else:
                        self.midend_stats["guard_hoist_rejected"] += 1

                shared_tail: list[MoltOp] = []
                while then_ops and else_ops:
                    tail_then = then_ops[-1]
                    tail_else = else_ops[-1]
                    if not self._op_equal_for_tail_merge(tail_then, tail_else):
                        break
                    if not self._can_tail_merge_op(tail_then):
                        break
                    shared_tail.append(tail_then)
                    then_ops = then_ops[:-1]
                    else_ops = else_ops[:-1]
                shared_tail.reverse()

                if not then_ops and not else_ops and can_elide_condition:
                    out.extend(shared_tail)
                    i = end_if_idx + 1
                    continue

                out.append(op)
                out.extend(then_ops)
                if else_idx is not None and else_ops:
                    out.append(ops[else_idx])
                    out.extend(else_ops)
                out.append(ops[end_if_idx])
                out.extend(shared_tail)
                i = end_if_idx + 1
            return out

        with current_unique_result_definitions(self, ops):
            rewritten = rewrite_range(0, len(ops))
        return rewritten, branch_prunes

    def _canonicalize_structured_regions_pre_sccp(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0
        cfg = build_cfg(ops)
        control = cfg.control
        if_to_else = control.if_to_else
        if_to_end = control.if_to_end
        loop_start_to_end = control.loop_start_to_end

        structural_prunes = 0

        def rewrite_range(start: int, end: int) -> list[MoltOp]:
            nonlocal structural_prunes
            out: list[MoltOp] = []
            i = start
            while i < end:
                op = ops[i]
                if op.kind == "IF" and i in if_to_end:
                    else_idx = if_to_else.get(i)
                    end_if_idx = if_to_end[i]
                    then_start = i + 1
                    then_end = else_idx if else_idx is not None else end_if_idx
                    then_ops = rewrite_range(then_start, then_end)
                    else_ops = (
                        rewrite_range(else_idx + 1, end_if_idx)
                        if else_idx is not None
                        else []
                    )
                    can_elide_condition = not self._op_may_access_arbitrary_heap(
                        op
                    ) and self._op_instance_cannot_raise(op, {})
                    if not then_ops and not else_ops and can_elide_condition:
                        structural_prunes += 1
                        i = end_if_idx + 1
                        continue
                    if (
                        else_idx is not None
                        and then_ops == else_ops
                        and can_elide_condition
                    ):
                        structural_prunes += 1
                        out.extend(then_ops)
                        i = end_if_idx + 1
                        continue
                    out.append(op)
                    out.extend(then_ops)
                    if else_idx is not None and else_ops:
                        out.append(ops[else_idx])
                        out.extend(else_ops)
                    out.append(ops[end_if_idx])
                    i = end_if_idx + 1
                    continue
                if op.kind == "LOOP_START" and i in loop_start_to_end:
                    loop_end = loop_start_to_end[i]
                    body = rewrite_range(i + 1, loop_end)
                    # An empty body does not prove termination. The backedge
                    # remains observable even when no value is produced.
                    out.append(op)
                    out.extend(body)
                    out.append(ops[loop_end])
                    i = loop_end + 1
                    continue
                out.append(op)
                i += 1
            return out

        with current_unique_result_definitions(self, ops):
            rewritten = rewrite_range(0, len(ops))
        return rewritten, structural_prunes

    def _rewrite_loop_edge_threading(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
        control: ControlMaps,
        executable_edges: set[tuple[int, int]],
        loop_break_choice_by_index: dict[int, bool],
    ) -> tuple[list[MoltOp], int, int, int]:
        single_exec_succ_by_block: dict[int, int] = {}
        executable_blocks: set[int] = {0} if cfg.blocks else set()
        for block in cfg.blocks:
            succs = cfg.successors.get(block.id, [])
            chosen = [succ for succ in succs if (block.id, succ) in executable_edges]
            for succ in chosen:
                executable_blocks.add(block.id)
                executable_blocks.add(succ)
            if len(chosen) == 1:
                single_exec_succ_by_block[block.id] = chosen[0]

        label_alias: dict[str, str] = {}
        region_labels = {
            str(region_id)
            for op in ops
            if op.kind in {"TRY_START", "TRY_END"}
            if (region_id := try_region_id(op)) is not None
        }

        def collect_label_aliases() -> None:
            def alias_target_from_body(body_ops: list[MoltOp]) -> str | None:
                if (
                    len(body_ops) == 1
                    and body_ops[0].kind == "JUMP"
                    and body_ops[0].args
                ):
                    return self._control_label_key(body_ops[0].args[0])
                if (
                    len(body_ops) == 2
                    and body_ops[0].kind == "CHECK_EXCEPTION"
                    and body_ops[0].args
                    and body_ops[1].kind == "JUMP"
                    and body_ops[1].args
                ):
                    check_key = self._control_label_key(body_ops[0].args[0])
                    jump_key = self._control_label_key(body_ops[1].args[0])
                    if check_key is not None and check_key == jump_key:
                        return jump_key
                return None

            for block in cfg.blocks:
                if block.start >= block.end:
                    continue
                head = ops[block.start]
                if head.kind not in {"LABEL", "STATE_LABEL"} or not head.args:
                    continue
                head_key = self._control_label_key(head.args[0])
                if head_key is None or head_key in region_labels:
                    continue
                body_ops = [
                    ops[idx]
                    for idx in range(block.start + 1, block.end)
                    if ops[idx].kind != "LINE"
                ]
                target_key = alias_target_from_body(body_ops)
                if target_key is None and not body_ops:
                    succs = cfg.successors.get(block.id, [])
                    if len(succs) == 1:
                        succ_block = cfg.blocks[succs[0]]
                        succ_body = [
                            ops[idx]
                            for idx in range(succ_block.start, succ_block.end)
                            if ops[idx].kind != "LINE"
                        ]
                        target_key = alias_target_from_body(succ_body)
                if target_key is None or target_key == head_key:
                    continue
                if cfg.label_to_block.get(target_key) is None:
                    continue
                label_alias[head_key] = target_key

        def resolve_label_alias(label_key: str) -> str:
            resolved = label_key
            seen: set[str] = set()
            while resolved in label_alias and resolved not in seen:
                seen.add(resolved)
                resolved = label_alias[resolved]
            return resolved

        collect_label_aliases()

        loop_remove_markers: set[int] = set()
        for loop_start, loop_end in control.loop_start_to_end.items():
            end_block = cfg.index_to_block.get(loop_end)
            start_block = cfg.index_to_block.get(loop_start)
            if end_block is None or start_block is None:
                continue
            if (end_block, start_block) in executable_edges:
                continue
            # Keep loop markers whenever dynamic loop-control ops are present
            # anywhere in the loop body. Restricting this to only currently
            # executable blocks can invalidate structure after later rewrites.
            body_has_dynamic_loop_control = any(
                ops[idx].kind
                in {
                    "LOOP_BREAK",
                    "LOOP_BREAK_IF_TRUE",
                    "LOOP_BREAK_IF_FALSE",
                    "LOOP_BREAK_IF_EXCEPTION",
                    "LOOP_CONTINUE",
                }
                for idx in range(loop_start + 1, loop_end)
            )
            if body_has_dynamic_loop_control:
                continue
            loop_remove_markers.add(loop_start)
            loop_remove_markers.add(loop_end)

        out: list[MoltOp] = []
        loop_rewrites = 0
        loop_marker_prunes = 0
        check_exception_threads = 0
        block_jump_label_arg: dict[int, Any] = {}
        for block_id, label in cfg.block_entry_label.items():
            label_key = self._control_label_key(label)
            if label_key is None:
                block_jump_label_arg[block_id] = label
                continue
            resolved_label = resolve_label_alias(label_key)
            block_jump_label_arg[block_id] = self._coerce_control_label_like(
                label, resolved_label
            )

        for idx, op in enumerate(ops):
            if op.kind == "CHECK_EXCEPTION":
                if op.args:
                    original_key = self._control_label_key(op.args[0])
                    if original_key is not None:
                        resolved_key = resolve_label_alias(original_key)
                        if resolved_key != original_key:
                            out.append(
                                MoltOp(
                                    kind=op.kind,
                                    args=[
                                        self._coerce_control_label_like(
                                            op.args[0], resolved_key
                                        ),
                                        *op.args[1:],
                                    ],
                                    result=op.result,
                                    metadata=op.metadata,
                                )
                            )
                            check_exception_threads += 1
                            continue
            if idx in loop_remove_markers and op.kind in {"LOOP_START", "LOOP_END"}:
                loop_marker_prunes += 1
                continue
            if op.kind == "LOOP_END":
                block_id = cfg.index_to_block.get(idx)
                if block_id is not None:
                    chosen = single_exec_succ_by_block.get(block_id)
                    succs = cfg.successors.get(block_id, [])
                    loop_start_idx = control.loop_end_to_start.get(idx)
                    back_succ = (
                        None
                        if loop_start_idx is None
                        else cfg.index_to_block.get(loop_start_idx)
                    )
                    if chosen is not None and len(succs) >= 2 and back_succ is not None:
                        exit_succ = next(
                            (succ for succ in succs if succ != back_succ), None
                        )
                        if chosen == back_succ and exit_succ is not None:
                            loop_rewrites += 1
                            back_label = block_jump_label_arg.get(back_succ)
                            if back_label is not None:
                                out.append(
                                    MoltOp(
                                        kind="JUMP",
                                        args=[back_label],
                                        result=MoltValue("none"),
                                        metadata=op.metadata,
                                    )
                                )
                                continue
                            out.append(
                                MoltOp(
                                    kind="LOOP_CONTINUE",
                                    args=[],
                                    result=MoltValue("none"),
                                    metadata=op.metadata,
                                )
                            )
                            continue
                        if chosen == exit_succ:
                            loop_rewrites += 1
                            exit_label = (
                                None
                                if exit_succ is None
                                else block_jump_label_arg.get(exit_succ)
                            )
                            if exit_label is not None:
                                out.append(
                                    MoltOp(
                                        kind="JUMP",
                                        args=[exit_label],
                                        result=MoltValue("none"),
                                        metadata=op.metadata,
                                    )
                                )
                                continue
                            out.append(
                                MoltOp(
                                    kind="LOOP_BREAK",
                                    args=[],
                                    result=MoltValue("none"),
                                    metadata=op.metadata,
                                )
                            )
                            continue
            if op.kind in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE"}:
                break_taken = loop_break_choice_by_index.get(idx)
                if break_taken is None:
                    block_id = cfg.index_to_block.get(idx)
                    if block_id is not None:
                        chosen = single_exec_succ_by_block.get(block_id)
                        succs = cfg.successors.get(block_id, [])
                        if chosen is not None and len(succs) >= 2:
                            break_taken = chosen == succs[1]
                if break_taken is True:
                    loop_rewrites += 1
                    block_id = cfg.index_to_block.get(idx)
                    succs = [] if block_id is None else cfg.successors.get(block_id, [])
                    break_succ = succs[1] if len(succs) >= 2 else None
                    break_label = (
                        None
                        if break_succ is None
                        else block_jump_label_arg.get(break_succ)
                    )
                    if break_label is not None:
                        out.append(
                            MoltOp(
                                kind="JUMP",
                                args=[break_label],
                                result=MoltValue("none"),
                                metadata=op.metadata,
                            )
                        )
                        continue
                    out.append(
                        MoltOp(
                            kind="LOOP_BREAK",
                            args=[],
                            result=MoltValue("none"),
                            metadata=op.metadata,
                        )
                    )
                    continue
                if break_taken is False:
                    loop_rewrites += 1
                    continue
            out.append(op)

        return out, loop_rewrites, loop_marker_prunes, check_exception_threads

    def _range_overlaps_executable_blocks(
        self,
        cfg: CFGGraph,
        *,
        start: int,
        end_inclusive: int,
        executable_blocks: set[int],
    ) -> bool:
        for block in cfg.blocks:
            if block.id not in executable_blocks:
                continue
            if block.start <= end_inclusive and block.end > start:
                return True
        return False

    def _prune_unreachable_cfg_regions(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
        executable_blocks: set[int],
    ) -> tuple[list[MoltOp], int, int]:
        if not cfg.blocks:
            return ops, 0, 0

        keep = [True] * len(ops)
        region_ranges: list[tuple[int, int]] = []

        control = cfg.control
        region_maps = [
            control.if_to_end,
            control.loop_start_to_end,
        ]
        for mapping in region_maps:
            for start, end in mapping.items():
                if start < 0 or end < start or end >= len(ops):
                    continue
                if not self._range_overlaps_executable_blocks(
                    cfg,
                    start=start,
                    end_inclusive=end,
                    executable_blocks=executable_blocks,
                ):
                    region_ranges.append((start, end))

        region_ranges.sort()
        merged_ranges: list[tuple[int, int]] = []
        for start, end in region_ranges:
            if not merged_ranges:
                merged_ranges.append((start, end))
                continue
            prev_start, prev_end = merged_ranges[-1]
            if start <= prev_end + 1:
                merged_ranges[-1] = (prev_start, max(prev_end, end))
            else:
                merged_ranges.append((start, end))

        for start, end in merged_ranges:
            for idx in range(start, end + 1):
                keep[idx] = False

        structural_keep = {
            "IF",
            "ELSE",
            "END_IF",
            "LOOP_START",
            "LOOP_END",
            "TRY_START",
            "TRY_END",
            "LABEL",
            "STATE_LABEL",
        }
        removed_blocks = 0
        for block in cfg.blocks:
            if block.id in executable_blocks:
                continue
            removed_any = False
            for idx in range(block.start, block.end):
                if not keep[idx]:
                    removed_any = True
                    continue
                op = ops[idx]
                if op.kind in structural_keep:
                    continue
                keep[idx] = False
                removed_any = True
            if removed_any:
                removed_blocks += 1

        out = [op for idx, op in enumerate(ops) if keep[idx]]
        if out == ops:
            return ops, 0, 0
        return out, len(merged_ranges), removed_blocks

    def _control_label_key(self, value: Any) -> str | None:
        if isinstance(value, bool):
            return None
        if isinstance(value, int):
            return str(value)
        if isinstance(value, str):
            text = value.strip()
            if not text:
                return None
            return text
        return None

    def _coerce_control_label_like(self, exemplar: Any, key: str) -> Any:
        if isinstance(exemplar, bool):
            return exemplar
        if isinstance(exemplar, int):
            if key.startswith(("+", "-")):
                sign = key[0]
                digits = key[1:]
                if digits.isdigit():
                    return int(f"{sign}{digits}")
            elif key.isdigit():
                return int(key)
            return exemplar
        if isinstance(exemplar, str):
            return key
        return key

    def _ensure_structural_cfg_validity(
        self, ops: list[MoltOp], *, stage: str
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        close_for_open = {
            "IF": "END_IF",
            "LOOP_START": "LOOP_END",
        }
        open_for_close = {close: open_ for open_, close in close_for_open.items()}
        # Only IF/LOOP are textual brackets. Exception regions are labeled
        # path-local custody transitions: several alternative TRY_END markers
        # may occur inside still-live IF/LOOP bodies. Preserve all of them for
        # the shared CFG exception authority, without synthesizing other closes.
        control_stack: list[tuple[str, Any]] = []
        rewritten: list[MoltOp] = []
        rewrites = 0

        def fail(message: str) -> NoReturn:
            self.midend_stats["cfg_structural_failures"] += 1
            raise RuntimeError(
                f"Malformed control flow after {stage} in "
                f"{self._active_midend_function_name}: {message}"
            )

        def synthetic_close(open_kind: str) -> MoltOp:
            nonlocal rewrites
            close_kind = close_for_open[open_kind]
            rewrites += 1
            return MoltOp(
                kind=close_kind,
                args=[],
                result=MoltValue("none"),
                metadata={
                    "synthetic": "cfg_structural_canonicalizer",
                    "stage": stage,
                },
            )

        def append_synthetic_close(open_kind: str) -> None:
            rewritten.append(synthetic_close(open_kind))

        for idx, op in enumerate(ops):
            kind = op.kind
            if kind in {"IF", "LOOP_START"}:
                if kind == "IF":
                    aux: Any = False  # seen_else
                else:
                    aux = None
                control_stack.append((kind, aux))
                rewritten.append(op)
                continue

            if kind == "ELSE":
                if_indices = [
                    i
                    for i, (open_kind, _seen_else) in enumerate(control_stack)
                    if open_kind == "IF"
                ]
                if not if_indices:
                    rewrites += 1
                    continue
                while control_stack and control_stack[-1][0] != "IF":
                    dangling_kind, _ = control_stack.pop()
                    append_synthetic_close(dangling_kind)
                if not control_stack:
                    rewrites += 1
                    continue
                open_kind, seen_else = control_stack[-1]
                if open_kind != "IF":
                    rewrites += 1
                    continue
                if seen_else:
                    rewrites += 1
                    continue
                control_stack[-1] = ("IF", True)
                rewritten.append(op)
                continue

            if kind in open_for_close:
                required_open = open_for_close[kind]
                open_indices = [
                    i
                    for i, (open_kind, _seen_else) in enumerate(control_stack)
                    if open_kind == required_open
                ]
                if not open_indices:
                    rewrites += 1
                    continue
                while control_stack and control_stack[-1][0] != required_open:
                    dangling_kind, _ = control_stack.pop()
                    append_synthetic_close(dangling_kind)
                if control_stack:
                    control_stack.pop()
                rewritten.append(op)
                continue

            if kind in {
                "LOOP_BREAK",
                "LOOP_BREAK_IF_TRUE",
                "LOOP_BREAK_IF_FALSE",
                "LOOP_BREAK_IF_EXCEPTION",
                "LOOP_CONTINUE",
            }:
                if not any(open_kind == "LOOP_START" for open_kind, _ in control_stack):
                    # Structural repairs should be fail-closed for malformed
                    # labels/targets, but loop-control ops outside loop scope
                    # can be safely elided as no-ops to keep IR canonical.
                    rewrites += 1
                    continue
                rewritten.append(op)
                continue

            rewritten.append(op)

        trailing_closes: list[MoltOp] = []
        while control_stack:
            dangling_kind, _ = control_stack.pop()
            trailing_closes.append(synthetic_close(dangling_kind))
        if trailing_closes:
            # A synthetic structured-region close is never a function
            # terminator. Keep the canonical execution-frame exit immediately
            # adjacent to the return and insert repairs before that terminal
            # tail. Appending closes after a return made the serialized function
            # appear unterminated and, more importantly, placed executable
            # control markers on no reachable function-exit path.
            terminal_start = len(rewritten)
            if rewritten and rewritten[-1].kind in {"ret", "ret_void", "RETURN"}:
                terminal_start -= 1
                if (
                    terminal_start > 0
                    and rewritten[terminal_start - 1].kind == "TRACE_EXIT"
                ):
                    terminal_start -= 1
            rewritten[terminal_start:terminal_start] = trailing_closes

        labels: dict[str, int] = {}
        for idx, op in enumerate(rewritten):
            if op.kind not in {"LABEL", "STATE_LABEL"}:
                continue
            if not op.args:
                fail(f"{op.kind} at op index {idx} is missing label argument")
            label_key = self._control_label_key(op.args[0])
            if label_key is None:
                fail(f"{op.kind} at op index {idx} has invalid label {op.args[0]!r}")
            assert label_key is not None
            if label_key in labels:
                prior = labels[label_key]
                fail(
                    f"duplicate label {label_key!r} at op index {idx}; "
                    f"already defined at {prior}"
                )
            labels[label_key] = idx

        for idx, op in enumerate(rewritten):
            if op.kind in {"TRY_START", "TRY_END"} and not op.args:
                # Anonymous IR regions have no named target. All source-level
                # scopes carry explicit labels, including every path-local end.
                continue
            if op.kind not in {"JUMP", "CHECK_EXCEPTION", "TRY_START", "TRY_END"}:
                continue
            if not op.args:
                fail(f"{op.kind} at op index {idx} is missing target label")
            label_key = self._control_label_key(op.args[0])
            if label_key is None:
                fail(f"{op.kind} at op index {idx} has invalid target {op.args[0]!r}")
            assert label_key is not None
            if label_key not in labels:
                fail(f"{op.kind} at op index {idx} targets unknown label {label_key!r}")

        return rewritten, rewrites

    def _normalize_try_except_join_labels(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        def collect_alias_labels(
            local_ops: list[MoltOp], local_cfg: CFGGraph
        ) -> dict[str, str]:
            alias_label: dict[str, str] = {}
            region_labels = {
                str(region_id)
                for op in local_ops
                if op.kind in {"TRY_START", "TRY_END"}
                if (region_id := try_region_id(op)) is not None
            }

            def extract_alias_target(body_ops: list[MoltOp]) -> str | None:
                if (
                    len(body_ops) == 1
                    and body_ops[0].kind == "JUMP"
                    and body_ops[0].args
                ):
                    return self._control_label_key(body_ops[0].args[0])
                if (
                    len(body_ops) == 2
                    and body_ops[0].kind == "CHECK_EXCEPTION"
                    and body_ops[0].args
                    and body_ops[1].kind == "JUMP"
                    and body_ops[1].args
                ):
                    exc_target = self._control_label_key(body_ops[0].args[0])
                    normal_target = self._control_label_key(body_ops[1].args[0])
                    if exc_target is not None and exc_target == normal_target:
                        return exc_target
                return None

            for block in local_cfg.blocks:
                if block.start >= block.end:
                    continue
                head = local_ops[block.start]
                if head.kind not in {"LABEL", "STATE_LABEL"} or not head.args:
                    continue
                label_key = self._control_label_key(head.args[0])
                if label_key is None or label_key in region_labels:
                    continue

                body_ops = [
                    local_ops[idx]
                    for idx in range(block.start + 1, block.end)
                    if local_ops[idx].kind != "LINE"
                ]
                target_key = extract_alias_target(body_ops)
                if target_key is None and not body_ops:
                    succs = local_cfg.successors.get(block.id, [])
                    if len(succs) == 1:
                        succ_block = local_cfg.blocks[succs[0]]
                        succ_body = [
                            local_ops[idx]
                            for idx in range(succ_block.start, succ_block.end)
                            if local_ops[idx].kind != "LINE"
                        ]
                        target_key = extract_alias_target(succ_body)
                if target_key is None or target_key == label_key:
                    continue
                if local_cfg.label_to_block.get(target_key) is None:
                    continue
                alias_label[label_key] = target_key
            return alias_label

        total_rewrites = 0
        current = ops
        for _ in range(6):
            local_cfg = build_cfg(current)
            if not local_cfg.blocks:
                break
            alias_label = collect_alias_labels(current, local_cfg)

            def resolve_alias(label: str) -> str:
                resolved = label
                seen: set[str] = set()
                while resolved in alias_label and resolved not in seen:
                    seen.add(resolved)
                    resolved = alias_label[resolved]
                return resolved

            round_rewrites = 0
            skip_indices: set[int] = set()
            out: list[MoltOp] = []
            i = 0
            while i < len(current):
                if i in skip_indices:
                    i += 1
                    continue
                op = current[i]
                rewritten = op
                if op.kind in {"JUMP", "CHECK_EXCEPTION"} and op.args:
                    first = op.args[0]
                    label_key = self._control_label_key(first)
                    if label_key is not None:
                        resolved = resolve_alias(label_key)
                        if resolved != label_key:
                            new_first = self._coerce_control_label_like(first, resolved)
                            rewritten = MoltOp(
                                kind=op.kind,
                                args=[new_first, *op.args[1:]],
                                result=op.result,
                                metadata=op.metadata,
                            )
                            round_rewrites += 1

                if rewritten.kind == "CHECK_EXCEPTION" and rewritten.args:
                    check_target_key = self._control_label_key(rewritten.args[0])
                    if check_target_key is not None:
                        j = i + 1
                        while j < len(current) and current[j].kind == "LINE":
                            j += 1
                        if (
                            j < len(current)
                            and current[j].kind == "JUMP"
                            and current[j].args
                        ):
                            jump_target_key = self._control_label_key(
                                current[j].args[0]
                            )
                            if jump_target_key is not None:
                                resolved_check = resolve_alias(check_target_key)
                                resolved_jump = resolve_alias(jump_target_key)
                                if resolved_check == resolved_jump:
                                    out.append(
                                        MoltOp(
                                            kind="JUMP",
                                            args=[
                                                self._coerce_control_label_like(
                                                    rewritten.args[0], resolved_check
                                                )
                                            ],
                                            result=MoltValue("none"),
                                            metadata=rewritten.metadata,
                                        )
                                    )
                                    skip_indices.add(j)
                                    round_rewrites += 1
                                    i += 1
                                    continue

                out.append(rewritten)
                i += 1

            total_rewrites += round_rewrites
            if out == current:
                break
            current = out

        return current, total_rewrites

    def _prune_dead_labels_and_noop_jumps(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int, int]:
        if not ops:
            return ops, 0, 0

        current = ops
        total_label_prunes = 0
        total_jump_elisions = 0

        for _ in range(6):
            jump_elisions = 0
            no_noop_jumps: list[MoltOp] = []
            i = 0
            while i < len(current):
                op = current[i]
                if op.kind == "JUMP" and op.args:
                    target = str(op.args[0])
                    j = i + 1
                    while j < len(current) and current[j].kind == "LINE":
                        j += 1
                    if (
                        j < len(current)
                        and current[j].kind == "LABEL"
                        and current[j].args
                        and str(current[j].args[0]) == target
                    ):
                        jump_elisions += 1
                        i += 1
                        continue
                no_noop_jumps.append(op)
                i += 1

            referenced_labels: set[str] = set()
            for op in no_noop_jumps:
                if op.kind in {"JUMP", "CHECK_EXCEPTION"} and op.args:
                    referenced_labels.add(str(op.args[0]))
                elif op.kind in {"TRY_START", "TRY_END"}:
                    region = try_region_id(op)
                    if region is not None:
                        referenced_labels.add(str(region))

            label_prunes = 0
            cleaned: list[MoltOp] = []
            for idx, op in enumerate(no_noop_jumps):
                if op.kind == "LABEL" and op.args:
                    name = str(op.args[0])
                    if name not in referenced_labels:
                        label_prunes += 1
                        continue
                cleaned.append(op)

            total_label_prunes += label_prunes
            total_jump_elisions += jump_elisions
            if cleaned == current:
                break
            current = cleaned

        return current, total_label_prunes, total_jump_elisions
