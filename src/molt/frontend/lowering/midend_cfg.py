"""MidendCFGMixin: frontend IR CFG-region normalization, guard pruning, edge threading, and labels."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, NoReturn

from molt.frontend._types import (
    CFGGraph,
    ControlMaps,
    MoltOp,
    MoltValue,
    build_cfg,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class MidendCFGMixin(_MixinBase):
    def _can_hoist_guard_pair(self, first: MoltOp, second: MoltOp) -> bool:
        if first.kind != second.kind:
            return False
        if first.kind not in {"GUARD_TAG", "GUARD_TYPE", "GUARD_DICT_SHAPE"}:
            return False
        if first.result.name != "none" or second.result.name != "none":
            return False
        if len(first.args) != len(second.args):
            return False
        for left, right in zip(first.args, second.args):
            if isinstance(left, MoltValue) and isinstance(right, MoltValue):
                if left.name != right.name:
                    return False
                continue
            if left != right:
                return False
        return True

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
        effect_class = self._op_effect_class(op.kind)
        if self._is_uncertain_heap_boundary(op.kind):
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
        if not ops:
            return ops, 0, 0, 0
        cfg = build_cfg(ops)
        control = cfg.control
        if_to_else = control.if_to_else
        if_to_end = control.if_to_end
        loop_start_to_end = control.loop_start_to_end
        try_start_to_end = control.try_start_to_end

        def process_range(
            start: int,
            end: int,
            in_guards: set[tuple[Any, ...]],
        ) -> tuple[list[MoltOp], set[tuple[Any, ...]], int, int]:
            out: list[MoltOp] = []
            available = set(in_guards)
            attempted = 0
            accepted = 0
            i = start
            while i < end:
                op = ops[i]
                if op.kind == "IF" and i in if_to_end:
                    else_idx = if_to_else.get(i)
                    end_if_idx = if_to_end[i]
                    then_start = i + 1
                    then_end = else_idx if else_idx is not None else end_if_idx
                    then_ops, then_out, then_attempts, then_accepted = process_range(
                        then_start,
                        then_end,
                        set(available),
                    )
                    if else_idx is not None:
                        else_ops, else_out, else_attempts, else_accepted = (
                            process_range(
                                else_idx + 1,
                                end_if_idx,
                                set(available),
                            )
                        )
                    else:
                        else_ops, else_out, else_attempts, else_accepted = (
                            [],
                            set(available),
                            0,
                            0,
                        )
                    attempted += then_attempts + else_attempts
                    accepted += then_accepted + else_accepted
                    out.append(op)
                    out.extend(then_ops)
                    if else_idx is not None:
                        out.append(ops[else_idx])
                        out.extend(else_ops)
                    out.append(ops[end_if_idx])
                    available = then_out.intersection(else_out)
                    i = end_if_idx + 1
                    continue

                if op.kind == "LOOP_START" and i in loop_start_to_end:
                    loop_end = loop_start_to_end[i]
                    body_ops, body_out, body_attempts, body_accepted = process_range(
                        i + 1,
                        loop_end,
                        set(available),
                    )
                    attempted += body_attempts
                    accepted += body_accepted
                    out.append(op)
                    out.extend(body_ops)
                    out.append(ops[loop_end])
                    # Loop may execute zero times, so only guards guaranteed on both
                    # paths remain available after the loop region.
                    available = available.intersection(body_out)
                    i = loop_end + 1
                    continue

                if op.kind == "TRY_START" and i in try_start_to_end:
                    try_end = try_start_to_end[i]
                    body_ops, body_out, body_attempts, body_accepted = process_range(
                        i + 1,
                        try_end,
                        set(available),
                    )
                    attempted += body_attempts
                    accepted += body_accepted
                    out.append(op)
                    out.extend(body_ops)
                    out.append(ops[try_end])
                    # Try body may exit via exceptional edge, so preserve only
                    # guards guaranteed on both normal and exceptional paths.
                    available = available.intersection(body_out)
                    i = try_end + 1
                    continue

                sig = self._guard_signature(op)
                if sig is not None:
                    attempted += 1
                    if sig in available:
                        accepted += 1
                        i += 1
                        continue
                    available.add(sig)
                    out.append(op)
                    i += 1
                    continue

                self._clear_invalidated_guard_signatures(available, op)
                out.append(op)
                i += 1

            return out, available, attempted, accepted

        rewritten, _out_guards, attempted, accepted = process_range(0, len(ops), set())
        rejected = max(0, attempted - accepted)
        return rewritten, attempted, accepted, rejected

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

                if else_idx is not None and then_ops and else_ops:
                    hoisted_guards = self._collect_movable_common_guards(
                        then_ops, else_ops
                    )
                    self.midend_stats["guard_hoist_attempts"] += max(
                        1, len(hoisted_guards)
                    )
                    if hoisted_guards:
                        self.midend_stats["guard_hoist_accepted"] += len(hoisted_guards)
                        for hoisted in hoisted_guards:
                            sig = self._guard_signature(hoisted)
                            if sig is None:
                                continue
                            then_ops = [
                                op
                                for op in then_ops
                                if self._guard_signature(op) != sig
                            ]
                            else_ops = [
                                op
                                for op in else_ops
                                if self._guard_signature(op) != sig
                            ]
                        out.extend(hoisted_guards)
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

                if not then_ops and not else_ops:
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
        try_start_to_end = control.try_start_to_end

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
                    if not then_ops and not else_ops:
                        structural_prunes += 1
                        i = end_if_idx + 1
                        continue
                    if else_idx is not None and then_ops == else_ops:
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
                    if not body:
                        structural_prunes += 1
                        i = loop_end + 1
                        continue
                    out.append(op)
                    out.extend(body)
                    out.append(ops[loop_end])
                    i = loop_end + 1
                    continue
                if op.kind == "TRY_START" and i in try_start_to_end:
                    try_end = try_start_to_end[i]
                    body = rewrite_range(i + 1, try_end)
                    if not body:
                        structural_prunes += 1
                        i = try_end + 1
                        continue
                    out.append(op)
                    out.extend(body)
                    out.append(ops[try_end])
                    i = try_end + 1
                    continue
                out.append(op)
                i += 1
            return out

        rewritten = rewrite_range(0, len(ops))
        return rewritten, structural_prunes

    def _compute_postdominators_for_cfg(self, cfg: CFGGraph) -> dict[int, set[int]]:
        block_count = len(cfg.blocks)
        if block_count == 0:
            return {}
        reachable = set(cfg.reachable)
        postdom: dict[int, set[int]] = {}
        for block_id in range(block_count):
            if block_id in reachable:
                postdom[block_id] = set(reachable)
            else:
                postdom[block_id] = {block_id}

        exits = [
            block_id
            for block_id in reachable
            if not any(succ in reachable for succ in cfg.successors.get(block_id, []))
        ]
        if not exits and reachable:
            exits = [max(reachable)]
        for exit_block in exits:
            postdom[exit_block] = {exit_block}

        changed = True
        while changed:
            changed = False
            for block_id in reversed(range(block_count)):
                if block_id not in reachable or block_id in exits:
                    continue
                succs = [s for s in cfg.successors.get(block_id, []) if s in reachable]
                if not succs:
                    new_set = {block_id}
                else:
                    new_set = set.intersection(*(postdom[s] for s in succs))
                    new_set.add(block_id)
                if new_set != postdom[block_id]:
                    postdom[block_id] = new_set
                    changed = True
        return postdom

    def _rewrite_loop_try_edge_threading(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
        control: ControlMaps,
        executable_edges: set[tuple[int, int]],
        loop_break_choice_by_index: dict[int, bool],
        try_exception_possible_by_start: dict[int, bool],
        try_normal_possible_by_start: dict[int, bool],
        guard_fail_indices: set[int],
    ) -> tuple[list[MoltOp], int, int, int, int, int, int]:
        single_exec_succ_by_block: dict[int, int] = {}
        executable_blocks: set[int] = {0} if cfg.blocks else set()
        postdominators = self._compute_postdominators_for_cfg(cfg)
        for block in cfg.blocks:
            succs = cfg.successors.get(block.id, [])
            chosen = [succ for succ in succs if (block.id, succ) in executable_edges]
            for succ in chosen:
                executable_blocks.add(block.id)
                executable_blocks.add(succ)
            if len(chosen) == 1:
                single_exec_succ_by_block[block.id] = chosen[0]

        label_alias: dict[str, str] = {}

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
                if head_key is None:
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

        try_remove_starts = {
            start
            for start, can_raise in try_exception_possible_by_start.items()
            if not can_raise
        }
        for start in control.try_start_to_end:
            block_id = cfg.index_to_block.get(start)
            if block_id is None:
                continue
            chosen = single_exec_succ_by_block.get(block_id)
            succs = cfg.successors.get(block_id, [])
            if chosen is not None and succs and chosen == succs[0]:
                try_remove_starts.add(start)
        try_remove_ends = {
            control.try_start_to_end[start]
            for start in try_remove_starts
            if start in control.try_start_to_end
        }

        try_unreachable_body_indices: set[int] = set()
        threaded_check_exception_jumps: dict[int, Any] = {}
        check_exception_elisions: set[int] = set()
        check_try_owner: dict[int, int] = {}
        for start, end in control.try_start_to_end.items():
            for idx in range(start + 1, end):
                if idx >= len(ops) or ops[idx].kind != "CHECK_EXCEPTION":
                    continue
                owner = check_try_owner.get(idx)
                if owner is None or start > owner:
                    check_try_owner[idx] = start
        for idx, start in check_try_owner.items():
            if not try_exception_possible_by_start.get(start, True):
                check_exception_elisions.add(idx)

        for start, end in control.try_start_to_end.items():
            if try_normal_possible_by_start.get(start, True):
                continue
            stop_idx: int | None = None
            for idx in range(start + 1, end):
                if idx in guard_fail_indices:
                    stop_idx = idx
                    break
                if ops[idx].kind in {"RAISE", "RAISE_CAUSE", "RERAISE"}:
                    stop_idx = idx
                    break
            if stop_idx is None:
                continue
            start_block = cfg.index_to_block.get(start)
            stop_block = cfg.index_to_block.get(stop_idx)
            end_block = cfg.index_to_block.get(end)
            if start_block is None or stop_block is None or end_block is None:
                continue
            if stop_block not in cfg.dominators.get(end_block, {end_block}):
                continue
            stop_postdominates_start = stop_block in postdominators.get(
                start_block, {start_block}
            )

            threaded_check_idx: int | None = None
            for check_idx in range(stop_idx + 1, end):
                check_op = ops[check_idx]
                if check_op.kind != "CHECK_EXCEPTION" or not check_op.args:
                    continue
                if any(
                    ops[mid].kind not in {"LINE", "LABEL", "STATE_LABEL"}
                    for mid in range(stop_idx + 1, check_idx)
                ):
                    continue
                check_block = cfg.index_to_block.get(check_idx)
                if check_block is None:
                    continue
                if stop_block not in cfg.dominators.get(check_block, {check_block}):
                    continue
                target_label = str(check_op.args[0])
                target_block = cfg.label_to_block.get(target_label)
                if target_block is None:
                    continue
                if target_block not in cfg.successors.get(check_block, []):
                    continue
                threaded_check_idx = check_idx
                target_key = self._control_label_key(check_op.args[0])
                if target_key is None:
                    threaded_check_exception_jumps[check_idx] = check_op.args[0]
                else:
                    resolved_key = resolve_label_alias(target_key)
                    threaded_check_exception_jumps[check_idx] = (
                        self._coerce_control_label_like(check_op.args[0], resolved_key)
                    )
                break

            if threaded_check_idx is not None:
                for idx in range(stop_idx + 1, threaded_check_idx):
                    try_unreachable_body_indices.add(idx)
                for idx in range(threaded_check_idx + 1, end):
                    try_unreachable_body_indices.add(idx)
            else:
                if not stop_postdominates_start:
                    continue
                for idx in range(stop_idx + 1, end):
                    try_unreachable_body_indices.add(idx)
            # Only remove try markers for exceptional-only lanes when we can
            # prove no in-region CHECK_EXCEPTION dispatch depends on marker
            # structure before the guaranteed trap point.
            has_pretrap_check_exception = any(
                ops[idx].kind == "CHECK_EXCEPTION"
                for idx in range(start + 1, stop_idx + 1)
            )
            if not has_pretrap_check_exception and (
                stop_postdominates_start or threaded_check_idx is not None
            ):
                try_remove_starts.add(start)
                try_remove_ends.add(end)

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
        try_marker_prunes = 0
        loop_marker_prunes = 0
        try_body_prunes = 0
        check_exception_threads = 0
        check_exception_elisions_count = 0
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
                target = threaded_check_exception_jumps.get(idx)
                if target is not None:
                    out.append(
                        MoltOp(
                            kind="JUMP",
                            args=[target],
                            result=MoltValue("none"),
                            metadata=op.metadata,
                        )
                    )
                    check_exception_threads += 1
                    continue
                if idx in check_exception_elisions:
                    check_exception_elisions_count += 1
                    continue
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
            if idx in try_unreachable_body_indices:
                try_body_prunes += 1
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
            if idx in try_remove_starts and op.kind == "TRY_START":
                try_marker_prunes += 1
                continue
            if idx in try_remove_ends and op.kind == "TRY_END":
                try_marker_prunes += 1
                continue
            out.append(op)

        return (
            out,
            loop_rewrites,
            try_marker_prunes,
            loop_marker_prunes,
            try_body_prunes,
            check_exception_threads,
            check_exception_elisions_count,
        )

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
            control.try_start_to_end,
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
            "TRY_START": "TRY_END",
        }
        open_for_close = {close: open_ for open_, close in close_for_open.items()}
        # Stack entries are (kind, aux). For "IF", `aux` is the bool `seen_else`.
        # For "TRY_START", `aux` carries the region id (handler label) so that the
        # DIVERGENT `TRY_END`s a `with`/`try` legitimately emits — one on the
        # protected-body exit path and one on the exception-handler path, sharing a
        # `try_region_id` — pair to the SAME open frame instead of being treated as
        # a single bracket. For "LOOP_START", `aux` is unused (None).
        control_stack: list[tuple[str, Any]] = []
        rewritten: list[MoltOp] = []
        rewrites = 0

        def try_region_id(op: MoltOp) -> Any:
            # The region id is the try's handler label. `visit_Try`/finally carry
            # it in `args[0]`; `with`/`async with` carry it in
            # `metadata["try_region_id"]` (their TRY_START/TRY_END have empty args).
            if op.metadata is not None and "try_region_id" in op.metadata:
                return op.metadata["try_region_id"]
            if op.args:
                return op.args[0]
            return None

        def fail(message: str) -> NoReturn:
            self.midend_stats["cfg_structural_failures"] += 1
            raise RuntimeError(
                f"Malformed control flow after {stage} in "
                f"{self._active_midend_function_name}: {message}"
            )

        def append_synthetic_close(open_kind: str) -> None:
            nonlocal rewrites
            close_kind = close_for_open[open_kind]
            rewritten.append(
                MoltOp(
                    kind=close_kind,
                    args=[],
                    result=MoltValue("none"),
                    metadata={
                        "synthetic": "cfg_structural_canonicalizer",
                        "stage": stage,
                    },
                )
            )
            rewrites += 1

        for idx, op in enumerate(ops):
            kind = op.kind
            if kind in {"IF", "LOOP_START", "TRY_START"}:
                if kind == "IF":
                    aux: Any = False  # seen_else
                elif kind == "TRY_START":
                    aux = try_region_id(op)  # handler-label region id
                else:
                    aux = None
                control_stack.append((kind, aux))
                rewritten.append(op)
                continue

            if kind == "TRY_END":
                # `TRY_END` is a DIVERGENT-PATH close, not a strict bracket: a
                # `with`/`try` emits ONE `TRY_START` but a `TRY_END` on the normal
                # protected-body exit AND on the exception-handler entry (after
                # `LABEL try_exc`). When the body cannot fall through (returns /
                # raises) only the handler-path `TRY_END` is emitted, so a region
                # has ONE or TWO textual closes. Pairing by region id makes this
                # exact: the FIRST `TRY_END` for a region closes its frame; any
                # LATER `TRY_END` with the same id is a redundant divergent close
                # and is elided WITHOUT disturbing other open frames.
                #
                # This is what fixes the P45 `for`-in-`with` miscompile: the inner
                # `with`'s second (handler) `TRY_END` arrives while the enclosing
                # `LOOP_START` is still open. The generic close logic below would
                # synth-close that `LOOP_START` to reach the outer `TRY_START`,
                # then elide the loop's real `LOOP_CONTINUE`/`LOOP_END` — orphaning
                # the back-edge so the loop runs once. Region-id pairing leaves the
                # loop untouched.
                region_id = try_region_id(op)
                frame_idx = None
                for i in range(len(control_stack) - 1, -1, -1):
                    open_kind, open_aux = control_stack[i]
                    if open_kind == "TRY_START" and (
                        region_id is None or open_aux == region_id
                    ):
                        frame_idx = i
                        break
                if frame_idx is None:
                    # No open try frame for this region: a redundant divergent
                    # close (its frame was already closed on the body path) or a
                    # stray close. Elide it; never tear down other open frames.
                    rewrites += 1
                    continue
                # Close this try frame. Any frames ABOVE it are genuinely dangling
                # (their own close never appeared inside the try body) — repair
                # them with synthetic closes, mirroring the END_IF/LOOP_END path.
                while len(control_stack) - 1 > frame_idx:
                    dangling_kind, _ = control_stack.pop()
                    append_synthetic_close(dangling_kind)
                control_stack.pop()
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

        while control_stack:
            dangling_kind, _ = control_stack.pop()
            append_synthetic_close(dangling_kind)

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
            if op.kind not in {"JUMP", "CHECK_EXCEPTION"}:
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
                if label_key is None:
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
                if op.kind == "JUMP" and op.args:
                    referenced_labels.add(str(op.args[0]))
                elif op.kind == "CHECK_EXCEPTION" and op.args:
                    referenced_labels.add(str(op.args[0]))

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
