"""MidendPipelineMixin: frontend IR midend orchestration rounds, LICM, fixed-point CSE, and entry/exit cleanup."""

from __future__ import annotations

import os
import sys
import time
from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    CFGGraph,
    CanonicalizationState,
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


class MidendPipelineMixin(_MixinBase):
    def _run_ir_midend_passes(self, ops: list[MoltOp]) -> list[MoltOp]:
        self._refresh_midend_env_config_if_needed()
        # Dev-profile mid-end gate removed: MISSING-value miscompile fixes
        # (SCCP non-propagation, DCE protection, definite-assignment hardening)
        # are now in place (ROADMAP TL2).  MOLT_MIDEND_DEV_ENABLE is no longer
        # required — mid-end runs for both dev and release profiles.
        # Correctness gate: keep stdlib modules out of mid-end canonicalization
        # until canonicalized stdlib lowering is proven stable (ROADMAP TL2).
        if self._source_is_stdlib_module:
            self.midend_stats["midend_module_skips"] += 1
            return ops
        ops = self._coalesce_check_exception_ops(ops)
        ops, structural_rewrites = self._ensure_structural_cfg_validity(
            ops, stage="midend_entry"
        )
        self.midend_stats["cfg_structural_canonicalizations"] += structural_rewrites
        oversized_skip_threshold = self.midend_env.skip_op_threshold
        _module_function_count, _module_total_ops, monolith_pressure_level = (
            self._current_module_pressure_snapshot()
        )
        effective_skip_threshold = oversized_skip_threshold
        if monolith_pressure_level >= 1:
            effective_skip_threshold = min(effective_skip_threshold, 650)
        if monolith_pressure_level >= 2:
            effective_skip_threshold = min(effective_skip_threshold, 500)
        if len(ops) >= effective_skip_threshold:
            cfg = build_cfg(ops)
            policy = self._resolve_midend_function_policy(
                ops,
                function_name=self._active_midend_function_name,
                block_count=max(1, len(cfg.blocks)),
            )
            if len(ops) >= oversized_skip_threshold or (
                not policy.promoted and policy.tier != "A"
            ):
                self.midend_stats["midend_oversized_function_skips"] += 1
                self._record_midend_policy_outcome(
                    policy=policy,
                    spent_ms=0.0,
                    work_units_spent=0.0,
                    degraded=True,
                    degrade_events=[
                        {
                            "reason": "oversized_function_skip",
                            "stage": "midend_entry",
                            "action": "emit_unoptimized_ir",
                            "spent_ms": 0.0,
                            "value": {
                                "op_count": len(ops),
                                "threshold": effective_skip_threshold,
                            },
                        }
                    ],
                    round_snapshots=[],
                )
                return ops
        return self._canonicalize_control_aware_ops(ops)

    def _hoist_loop_invariant_pure_ops(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        cfg = build_cfg(ops)
        if not cfg.blocks:
            return ops, 0

        control = cfg.control
        target_start_by_index: dict[int, int] = {}
        loop_ranges = sorted(
            (
                (start, end)
                for start, end in control.loop_start_to_end.items()
                if end > start
            ),
            key=lambda item: (item[1] - item[0], item[0]),
        )

        for loop_start, loop_end in loop_ranges:
            if loop_end is None or loop_end <= loop_start:
                continue
            # In generators, loops containing state_yield create resume points
            # inside the loop body.  Hoisting definitions before the loop would
            # leave them undefined when the generator is resumed at that point.
            has_yield = any(
                ops[i].kind == "STATE_YIELD" for i in range(loop_start + 1, loop_end)
            )
            if has_yield:
                continue
            pre_defs = self._collect_defined_value_names(ops[:loop_start])
            hoisted_defs: set[str] = set()
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.result.name == "none":
                    continue
                if op.kind == "PHI":
                    continue
                if self._op_effect_class(op.kind) != "pure":
                    continue
                uses: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, uses)
                if uses.issubset(pre_defs.union(hoisted_defs)):
                    target_start_by_index.setdefault(idx, loop_start)
                    hoisted_defs.add(op.result.name)

        if not target_start_by_index:
            return ops, 0

        out: list[MoltOp] = []
        hoisted_count = 0
        for idx, op in enumerate(ops):
            if op.kind == "LOOP_START":
                hoisted_here = [
                    ops[candidate_idx]
                    for candidate_idx, target_start in sorted(
                        target_start_by_index.items()
                    )
                    if target_start == idx
                ]
                out.extend(hoisted_here)
                hoisted_count += len(hoisted_here)
            if idx in target_start_by_index:
                continue
            out.append(op)

        return out, hoisted_count

    def _run_cse_canonicalization_round(
        self,
        ops: list[MoltOp],
        *,
        allow_cross_block_const_dedupe: bool,
        max_cse_iterations_override: int | None = None,
        sccp_iter_cap_override: int | None = None,
    ) -> tuple[list[MoltOp], int]:
        round_cfg = build_cfg(ops)
        if not round_cfg.blocks:
            return ops, 0
        sccp = self._compute_sccp(
            ops, round_cfg, max_iters_override=sccp_iter_cap_override
        )
        working_ops, phi_trims = self._trim_phi_args_by_executable_edges(
            ops, round_cfg, sccp.executable_edges
        )
        if phi_trims > 0:
            round_cfg = build_cfg(working_ops)
            if not round_cfg.blocks:
                return working_ops, phi_trims
            sccp = self._compute_sccp(
                working_ops,
                round_cfg,
                max_iters_override=sccp_iter_cap_override,
            )
        sccp_in_consts = self._sccp_in_const_int_values(sccp)
        induction_steps = self._analyze_loop_induction_steps(working_ops, round_cfg)

        # Build value-name -> defining-block-id map so we can filter cross-block
        # aliases to only those whose targets are defined in dominating blocks.
        _value_def_block: dict[str, int] = {}
        for _blk in round_cfg.blocks:
            for _op_idx in range(_blk.start, _blk.end):
                _def_name = working_ops[_op_idx].result.name
                if _def_name != "none" and _def_name not in _value_def_block:
                    _value_def_block[_def_name] = _blk.id

        block_inputs: dict[int, CanonicalizationState] = {
            block.id: self._empty_canonicalization_state() for block in round_cfg.blocks
        }
        block_outputs: dict[int, CanonicalizationState] = {
            block.id: self._empty_canonicalization_state() for block in round_cfg.blocks
        }
        block_canonical_ops: dict[int, list[MoltOp]] = {
            block.id: [] for block in round_cfg.blocks
        }

        changed = True
        iterations = 0
        if max_cse_iterations_override is not None and max_cse_iterations_override > 0:
            max_cse_iterations = max_cse_iterations_override
        else:
            max_cse_iterations = (
                self.midend_env.cse_iter_cap_override
                if self.midend_env.cse_iter_cap_override is not None
                else 20
            )
        while changed and iterations < max_cse_iterations:
            iterations += 1
            changed = False
            for block in round_cfg.blocks:
                block_id = block.id
                if block_id == 0 or block_id not in round_cfg.reachable:
                    in_state = self._empty_canonicalization_state()
                else:
                    pred_states = [
                        block_outputs[pred]
                        for pred in round_cfg.predecessors.get(block_id, [])
                        if pred in round_cfg.reachable
                    ]
                    in_state = self._intersect_canonicalization_states(pred_states)
                    if not allow_cross_block_const_dedupe:
                        # Keep cross-block propagation limited to must-facts only.
                        # Alias/value-reuse state remains block-local to avoid
                        # rewriting gaps at control joins.
                        in_state["aliases"] = {}
                        in_state["available_values"] = {}
                    else:
                        # Filter aliases and available_values to only those
                        # whose target values are defined in blocks that
                        # dominate the current block.  Without this guard,
                        # CSE can rewrite an operand (e.g. a STORE_INDEX
                        # value arg) to reference a variable from a non-
                        # dominating block, producing invalid IR — the
                        # "return-buffer" bug.
                        block_doms = round_cfg.dominators.get(block_id, {block_id})
                        filtered_aliases: dict[str, MoltValue] = {}
                        for _ak, _av in in_state["aliases"].items():
                            _target_block = _value_def_block.get(_av.name)
                            if _target_block is None or _target_block in block_doms:
                                filtered_aliases[_ak] = _av
                        in_state["aliases"] = filtered_aliases
                        filtered_avail: dict[tuple[Any, ...], MoltValue] = {}
                        for _vk, _vv in in_state["available_values"].items():
                            _target_block = _value_def_block.get(_vv.name)
                            if _target_block is None or _target_block in block_doms:
                                filtered_avail[_vk] = _vv
                        in_state["available_values"] = filtered_avail
                        self._invalidate_canonicalization_state_signature(in_state)

                for name, value in sccp_in_consts.get(block_id, {}).items():
                    in_state["const_int_values"][name] = value
                    in_state["value_type_tags"][name] = BUILTIN_TYPE_TAGS["int"]

                if self._canonicalization_state_signature(
                    in_state
                ) != self._canonicalization_state_signature(block_inputs[block_id]):
                    block_inputs[block_id] = self._clone_canonicalization_state(
                        in_state
                    )
                    changed = True

                canonical_ops, out_state = self._canonicalize_block_with_state(
                    working_ops[block.start : block.end],
                    in_state,
                    induction_steps=induction_steps,
                )
                if self._canonicalization_state_signature(
                    out_state
                ) != self._canonicalization_state_signature(block_outputs[block_id]):
                    block_outputs[block_id] = self._clone_canonicalization_state(
                        out_state
                    )
                    changed = True
                if canonical_ops != block_canonical_ops[block_id]:
                    block_canonical_ops[block_id] = canonical_ops
                    changed = True

        if changed:
            self.midend_stats["cse_iteration_cap_hits"] = (
                self.midend_stats.get("cse_iteration_cap_hits", 0) + 1
            )
            return working_ops, phi_trims

        canonicalized_ops: list[MoltOp] = []
        for block_id in range(len(round_cfg.blocks)):
            canonicalized_ops.extend(block_canonical_ops[block_id])

        # ── Global alias resolution ─────────────────────────────────────
        # CSE creates aliases when merging duplicate ops within a block.
        # When allow_cross_block_const_dedupe is False, these aliases are
        # NOT propagated to successor blocks — a variable eliminated in
        # block A can still be referenced in block B.  Collect the union
        # of all aliases from every block's output state and apply them
        # to the reassembled ops so that no dangling references remain.
        global_aliases: dict[str, MoltValue] = {}
        for block_id in range(len(round_cfg.blocks)):
            for alias_name, alias_target in block_outputs[block_id]["aliases"].items():
                if alias_name not in global_aliases:
                    global_aliases[alias_name] = alias_target
        if global_aliases:
            resolved_ops: list[MoltOp] = []
            for op in canonicalized_ops:
                new_args = [
                    self._rewrite_aliases_in_arg(arg, global_aliases) for arg in op.args
                ]
                if new_args != op.args:
                    resolved_ops.append(
                        MoltOp(
                            kind=op.kind,
                            args=new_args,
                            result=op.result,
                            metadata=op.metadata,
                        )
                    )
                else:
                    resolved_ops.append(op)
            canonicalized_ops = resolved_ops

        return canonicalized_ops, phi_trims

    def _canonicalize_control_aware_ops_impl(
        self,
        ops: list[MoltOp],
        *,
        allow_cross_block_const_dedupe: bool,
    ) -> list[MoltOp]:
        self._refresh_midend_env_config_if_needed()
        # Current contract: sparse SCCP covers arithmetic/boolean/comparison/type
        # families plus bounded loop facts used by today’s fixed-point passes;
        # heap/call-specialization widening and stronger cross-iteration solvers
        # remain roadmap work and are intentionally not inferred here.
        validated_ops, preflight_rewrites = self._ensure_structural_cfg_validity(
            ops, stage="midend_fixed_point_entry"
        )
        self.midend_stats["cfg_structural_canonicalizations"] += preflight_rewrites
        cfg: CFGGraph = build_cfg(validated_ops)
        if not cfg.blocks:
            return validated_ops

        func_stats = self._midend_function_stats()
        func_stats["sccp_attempted"] += 1
        func_stats["edge_thread_attempted"] += 1
        func_stats["gvn_attempted"] += 1
        func_stats["cse_attempted"] += 1
        func_stats["licm_attempted"] += 1
        func_stats["dce_attempted"] += 1

        policy = self._resolve_midend_function_policy(
            validated_ops,
            function_name=self._active_midend_function_name,
            block_count=len(cfg.blocks),
        )
        pass_start = time.perf_counter()
        rewritten_ops, pre_cfg_rewrites = self._canonicalize_cfg_before_optimization(
            validated_ops
        )
        self._record_midend_pass_sample(
            "cfg_precanonicalize",
            elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
            accepted=pre_cfg_rewrites > 0,
            degraded=False,
        )

        # `midend_start` measures wall-clock for TELEMETRY ONLY (logged in
        # degrade/pass events).  It MUST NOT feed any pass-selection decision —
        # the degrade ladder gates on the deterministic `work_units_spent`
        # accumulator below so the emitted IR is a pure function of the input
        # (#73; the old wall-clock gate made IR depend on machine speed).
        midend_start = time.perf_counter()
        # Deterministic degrade-ladder accumulator: charged the live op count at
        # each inter-pass checkpoint via `charge_work(...)`.  Compared against
        # the deterministic `policy.work_budget` to decide degradation.
        work_units_spent = 0.0
        degrade_events: list[dict[str, Any]] = []
        degraded = False
        enable_deep_edge_thread = policy.enable_deep_edge_thread
        enable_cse = policy.enable_cse
        enable_licm = policy.enable_licm
        enable_guard_hoist = policy.enable_guard_hoist
        max_rounds = max(2, policy.max_rounds)
        sccp_iter_cap = max(1, policy.sccp_iter_cap)
        cse_iter_cap = max(1, policy.cse_iter_cap)

        # --- Per-function DETERMINISTIC work budget, 3-level degrade ladder ---
        # (formerly a wall-time budget, MOL-27; made deterministic for #73).
        per_func_work_budget = max(0.0, float(policy.work_budget))
        degrade_level: int = 0
        degrade_level_reasons: list[str] = []

        if self.midend_env.max_rounds_override is not None:
            max_rounds = max(2, self.midend_env.max_rounds_override)
        if self.midend_env.sccp_iter_cap_override is not None:
            sccp_iter_cap = self.midend_env.sccp_iter_cap_override
        if self.midend_env.cse_iter_cap_override is not None:
            cse_iter_cap = self.midend_env.cse_iter_cap_override

        def spent_midend_ms() -> float:
            # Telemetry only — never feeds a pass-selection decision (#73).
            return (time.perf_counter() - midend_start) * 1000.0

        def charge_work(units: float) -> None:
            # Deterministic work accounting for the degrade ladder.  `units` is
            # an op-count-derived (hence input-determined) cost.
            nonlocal work_units_spent
            if units > 0.0:
                work_units_spent += float(units)

        def add_degrade_event(
            reason: str,
            stage: str,
            action: str,
            *,
            value: Any | None = None,
        ) -> None:
            event: dict[str, Any] = {
                "reason": reason,
                "stage": stage,
                "action": action,
                "spent_ms": round(max(0.0, spent_midend_ms()), 3),
            }
            if value is not None:
                event["value"] = value
            degrade_events.append(event)

        if not enable_deep_edge_thread:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_deep_edge_thread",
            )
        if not enable_cse:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_cse",
            )
        if not enable_guard_hoist:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_guard_hoist",
            )
        if not enable_licm:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_licm",
            )

        def maybe_apply_budget_degrade(
            stage: str,
            round_index: int,
            *,
            ops_now: int,
            upcoming_pass: str | None = None,
        ) -> None:
            """Deterministically degrade the optimisation pipeline when the
            accumulated DETERMINISTIC work exceeds the per-function work budget.

            `ops_now` is the live op count at this checkpoint; it is charged to
            the work accumulator before the budget is evaluated.  Charging the
            op count makes the total work scale with how much IR each pass had
            to process — a deterministic proxy for compile cost — so the
            resulting pass selection (and thus the emitted IR) depends only on
            the input, never on wall-clock timing (#73).
            """
            nonlocal degraded
            nonlocal degrade_level
            nonlocal enable_deep_edge_thread
            nonlocal enable_cse
            nonlocal enable_guard_hoist
            nonlocal enable_licm
            nonlocal max_rounds
            nonlocal sccp_iter_cap
            nonlocal cse_iter_cap
            if per_func_work_budget < 0:
                return
            charge_work(max(1, ops_now))
            while work_units_spent > per_func_work_budget:
                action: str | None = None
                proof_floor = round_index + 2
                if max_rounds > proof_floor:
                    max_rounds = proof_floor
                    action = f"cap_rounds_to_{max_rounds}"
                elif sccp_iter_cap > 8:
                    sccp_iter_cap = max(8, sccp_iter_cap // 2)
                    action = f"shrink_sccp_iter_cap_to_{sccp_iter_cap}"
                elif cse_iter_cap > 4:
                    cse_iter_cap = max(4, cse_iter_cap // 2)
                    action = f"shrink_cse_iter_cap_to_{cse_iter_cap}"
                elif enable_cse:
                    enable_cse = False
                    action = "disable_cse"
                elif enable_deep_edge_thread:
                    enable_deep_edge_thread = False
                    action = "disable_deep_edge_thread"
                elif enable_guard_hoist:
                    enable_guard_hoist = False
                    action = "disable_guard_hoist"
                elif enable_licm:
                    enable_licm = False
                    action = "disable_licm"
                if action is None:
                    break
                degraded = True
                degrade_level = min(3, degrade_level + 1)
                degrade_level_reasons.append(
                    f"work_budget_exceeded at {stage}: "
                    f"work={work_units_spent:.0f} > budget="
                    f"{per_func_work_budget:.0f}; action={action}"
                )
                extra_value: dict[str, Any] = {
                    "work_units": round(work_units_spent, 1),
                    "work_budget": round(per_func_work_budget, 1),
                }
                if upcoming_pass is not None:
                    extra_value["upcoming_pass"] = upcoming_pass
                add_degrade_event(
                    "work_budget_exceeded", stage, action, value=extra_value
                )

        total_branch_prunes = 0
        total_loop_edge_prunes = 0
        total_try_edge_prunes = 0
        total_loop_marker_prunes = 0
        total_unreachable_blocks = 0
        total_region_prunes = pre_cfg_rewrites
        total_label_prunes = 0
        total_jump_noops = 0
        total_try_join_threads = 0
        total_licm_hoists = 0
        total_phi_edge_trims = 0
        total_loop_rewrite_attempts = 0
        total_loop_rewrite_accepted = 0

        gvn_hits_before = self.midend_stats.get("gvn_hits", 0)
        dce_removed_before = self.midend_stats.get("dce_removed_total", 0)
        guard_hoist_attempts_before = self.midend_stats.get("guard_hoist_attempts", 0)
        guard_hoist_accepted_before = self.midend_stats.get("guard_hoist_accepted", 0)
        guard_hoist_rejected_before = self.midend_stats.get("guard_hoist_rejected", 0)

        converged = False
        round_index = 0
        round_snapshots: list[dict[str, Any]] = []
        cse_dce_closure_failed = False
        while round_index < max_rounds:
            round_index += 1
            maybe_apply_budget_degrade(
                f"round_{round_index}_start",
                round_index - 1,
                ops_now=len(rewritten_ops),
                upcoming_pass="simplify",
            )
            step_before = rewritten_ops
            step_ops = rewritten_ops
            post_cse_dce_ran = False

            # 1) simplify
            pass_start = time.perf_counter()
            step_ops, structural_prunes = (
                self._canonicalize_structured_regions_pre_sccp(step_ops)
            )
            self._record_midend_pass_sample(
                "simplify",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=structural_prunes > 0,
                degraded=degraded,
            )
            total_region_prunes += structural_prunes
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_simplify",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="sccp_edge_thread",
            )

            # 2) SCCP/edge-thread
            iter_cfg = build_cfg(step_ops)
            if iter_cfg.blocks:
                pass_start = time.perf_counter()
                iter_sccp = self._compute_sccp(
                    step_ops,
                    iter_cfg,
                    max_iters_override=sccp_iter_cap,
                )
                step_ops, phi_trims = self._trim_phi_args_by_executable_edges(
                    step_ops, iter_cfg, iter_sccp.executable_edges
                )
                total_phi_edge_trims += phi_trims
                if phi_trims > 0:
                    iter_cfg = build_cfg(step_ops)
                    if iter_cfg.blocks:
                        iter_sccp = self._compute_sccp(
                            step_ops,
                            iter_cfg,
                            max_iters_override=sccp_iter_cap,
                        )
                if iter_cfg.blocks:
                    step_ops, branch_prunes = self._rewrite_structured_if_regions(
                        step_ops,
                        control=iter_cfg.control,
                        branch_choice_by_if_index=iter_sccp.branch_choice_by_if_index,
                    )
                    total_branch_prunes += branch_prunes
                else:
                    branch_prunes = 0

                threaded_cfg = build_cfg(step_ops)
                loop_rewrite_attempts = sum(
                    1
                    for op in step_ops
                    if op.kind
                    in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE", "LOOP_END"}
                )
                total_loop_rewrite_attempts += loop_rewrite_attempts
                if threaded_cfg.blocks and enable_deep_edge_thread:
                    threaded_sccp = self._compute_sccp(
                        step_ops,
                        threaded_cfg,
                        max_iters_override=sccp_iter_cap,
                    )
                    (
                        step_ops,
                        loop_rewrites,
                        try_marker_prunes,
                        loop_marker_prunes,
                        try_body_prunes,
                        check_exception_threads,
                        check_exception_elisions,
                    ) = self._rewrite_loop_try_edge_threading(
                        step_ops,
                        cfg=threaded_cfg,
                        control=threaded_cfg.control,
                        executable_edges=threaded_sccp.executable_edges,
                        loop_break_choice_by_index=threaded_sccp.loop_break_choice_by_index,
                        try_exception_possible_by_start=threaded_sccp.try_exception_possible_by_start,
                        try_normal_possible_by_start=threaded_sccp.try_normal_possible_by_start,
                        guard_fail_indices=threaded_sccp.guard_fail_indices,
                    )
                else:
                    (
                        loop_rewrites,
                        try_marker_prunes,
                        loop_marker_prunes,
                        try_body_prunes,
                        check_exception_threads,
                        check_exception_elisions,
                    ) = (0, 0, 0, 0, 0, 0)

                total_loop_edge_prunes += loop_rewrites
                total_try_edge_prunes += (
                    try_marker_prunes
                    + try_body_prunes
                    + check_exception_threads
                    + check_exception_elisions
                )
                total_loop_marker_prunes += loop_marker_prunes
                total_loop_rewrite_accepted += (
                    loop_rewrites + loop_marker_prunes + try_body_prunes
                )
                self._record_midend_pass_sample(
                    "sccp_edge_thread",
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=(
                        branch_prunes
                        + loop_rewrites
                        + loop_marker_prunes
                        + try_marker_prunes
                        + try_body_prunes
                        + check_exception_threads
                        + check_exception_elisions
                        + phi_trims
                    )
                    > 0,
                    degraded=degraded
                    or (not enable_deep_edge_thread and loop_rewrite_attempts > 0),
                )
            else:
                self._record_midend_pass_sample(
                    "sccp_edge_thread",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=degraded,
                )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_sccp",
                round_index - 1,
                ops_now=len(step_ops),
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_pre_join",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="join_canonicalize",
            )

            # 3) join canonicalize
            pass_start = time.perf_counter()
            join_cfg = build_cfg(step_ops)
            if join_cfg.blocks:
                step_ops, try_join_threads = self._normalize_try_except_join_labels(
                    step_ops, cfg=join_cfg
                )
            else:
                try_join_threads = 0
            total_try_join_threads += try_join_threads
            total_try_edge_prunes += try_join_threads
            self._record_midend_pass_sample(
                "join_canonicalize",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=try_join_threads > 0,
                degraded=degraded,
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_join",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="guard_hoist",
            )

            pass_start = time.perf_counter()
            guard_prune_input = step_ops
            step_ops, fused_dict_guard_prunes = (
                self._eliminate_redundant_fused_dict_increment_guards(step_ops)
            )
            if fused_dict_guard_prunes:
                self.midend_stats["fused_dict_guard_prunes"] = (
                    self.midend_stats.get("fused_dict_guard_prunes", 0)
                    + fused_dict_guard_prunes
                )
                func_stats["fused_dict_guard_prunes"] += fused_dict_guard_prunes
            self._record_midend_pass_sample(
                "fused_dict_guard_prune",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=step_ops != guard_prune_input,
                degraded=degraded,
            )

            if enable_guard_hoist:
                pass_start = time.perf_counter()
                step_ops, guard_attempts, guard_accepts, guard_rejects = (
                    self._eliminate_redundant_guards_cfg(step_ops)
                )
                self.midend_stats["guard_hoist_attempts"] += guard_attempts
                self.midend_stats["guard_hoist_accepted"] += guard_accepts
                self.midend_stats["guard_hoist_rejected"] += guard_rejects
                self._record_midend_pass_sample(
                    "guard_hoist",
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=guard_accepts > 0,
                    degraded=degraded,
                )
            else:
                self._record_midend_pass_sample(
                    "guard_hoist",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_guard_hoist",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="licm",
            )

            # Auxiliary: LICM/loop hoists in same deterministic round.
            if enable_licm:
                pass_start = time.perf_counter()
                step_ops, licm_hoists = self._hoist_loop_invariant_pure_ops(step_ops)
                self._record_midend_pass_sample(
                    "licm",
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=licm_hoists > 0,
                    degraded=degraded,
                )
            else:
                licm_hoists = 0
                self._record_midend_pass_sample(
                    "licm",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
            total_licm_hoists += licm_hoists
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_hoists",
                round_index - 1,
                ops_now=len(step_ops),
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_pre_prune",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="prune",
            )

            # 4) prune
            pass_start = time.perf_counter()
            prune_cfg = build_cfg(step_ops)
            if prune_cfg.blocks:
                prune_sccp = self._compute_sccp(
                    step_ops,
                    prune_cfg,
                    max_iters_override=sccp_iter_cap,
                )
                step_ops, region_prunes, unreachable_blocks = (
                    self._prune_unreachable_cfg_regions(
                        step_ops,
                        cfg=prune_cfg,
                        executable_blocks=prune_sccp.executable_blocks,
                    )
                )
            else:
                region_prunes, unreachable_blocks = 0, 0
            total_region_prunes += region_prunes
            total_unreachable_blocks += unreachable_blocks

            step_ops, label_prunes, jump_noops = self._prune_dead_labels_and_noop_jumps(
                step_ops
            )
            total_label_prunes += label_prunes
            total_jump_noops += jump_noops
            step_ops, round_structural_rewrites = self._ensure_structural_cfg_validity(
                step_ops,
                stage=f"midend_fixed_point_round_{round_index}",
            )
            total_region_prunes += round_structural_rewrites
            self.midend_stats["cfg_structural_canonicalizations"] += (
                round_structural_rewrites
            )
            self._record_midend_pass_sample(
                "prune",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=(
                    region_prunes
                    + unreachable_blocks
                    + label_prunes
                    + jump_noops
                    + round_structural_rewrites
                )
                > 0,
                degraded=degraded,
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_prune",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="verifier",
            )

            # 5) verifier
            pass_start = time.perf_counter()
            # Compute predefined from the ORIGINAL ops at round start, not the
            # current ops.  If LICM+CSE eliminated a variable's definition,
            # that variable is NOT predefined — it's a dangling reference that
            # must be caught.  Using step_ops here masks the bug because
            # _infer_predefined_value_names treats "used but not defined" as
            # predefined (assumed to be a function parameter).
            round_predefined = self._infer_predefined_value_names(step_before)
            round_failures = self._verify_definite_assignment_in_ops(
                step_ops, predefined_value_names=round_predefined
            )
            self._record_midend_pass_sample(
                "verifier",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=not round_failures,
                degraded=degraded,
            )

            def run_verified_dce(
                dce_input: list[MoltOp],
                *,
                pass_name: str,
            ) -> tuple[list[MoltOp], bool]:
                pass_start = time.perf_counter()
                dce_candidate = self._eliminate_dead_trivial_consts(dce_input)
                dce_failures = self._verify_definite_assignment_in_ops(
                    dce_candidate, predefined_value_names=round_predefined
                )
                accepted = (not dce_failures) and dce_candidate != dce_input
                self._record_midend_pass_sample(
                    pass_name,
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=accepted,
                    degraded=degraded,
                )
                if dce_failures:
                    return dce_input, False
                return dce_candidate, accepted

            if round_failures:
                step_ops = step_before
                self._record_midend_pass_sample(
                    "dce",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
                self._record_midend_pass_sample(
                    "cse",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
            else:
                # 6) DCE
                step_ops, _dce_accepted = run_verified_dce(step_ops, pass_name="dce")
                maybe_apply_budget_degrade(
                    f"round_{round_index}_post_dce",
                    round_index - 1,
                    ops_now=len(step_ops),
                    upcoming_pass="cse",
                )

                # 7) CSE
                if enable_cse:
                    cse_dce_closure_converged = False
                    cse_dce_fp_max_iters = max(1, self.midend_env.cse_fp_max_iters)
                    for cse_dce_fp_iter in range(1, cse_dce_fp_max_iters + 1):
                        pass_start = time.perf_counter()
                        cse_input = step_ops
                        cse_candidate, cse_phi_trims = (
                            self._run_cse_canonicalization_round(
                                step_ops,
                                allow_cross_block_const_dedupe=(
                                    allow_cross_block_const_dedupe
                                ),
                                max_cse_iterations_override=cse_iter_cap,
                                sccp_iter_cap_override=sccp_iter_cap,
                            )
                        )
                        total_phi_edge_trims += cse_phi_trims
                        cse_failures = self._verify_definite_assignment_in_ops(
                            cse_candidate, predefined_value_names=round_predefined
                        )
                        cse_accepted = (not cse_failures) and (
                            cse_candidate != cse_input or cse_phi_trims > 0
                        )
                        self._record_midend_pass_sample(
                            "cse",
                            elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                            accepted=cse_accepted,
                            degraded=degraded,
                        )
                        if cse_failures:
                            cse_dce_closure_converged = True
                            break
                        step_ops = cse_candidate
                        step_ops, post_cse_dce_accepted = run_verified_dce(
                            step_ops, pass_name="post_cse_dce"
                        )
                        post_cse_dce_ran = True
                        if cse_dce_fp_iter > 1:
                            charge_work(max(1, len(step_ops)))
                        if not cse_accepted and not post_cse_dce_accepted:
                            cse_dce_closure_converged = True
                            break
                    if not cse_dce_closure_converged:
                        cse_dce_closure_failed = True
                        self.midend_stats["cse_dce_fp_cap_hits"] = (
                            self.midend_stats.get("cse_dce_fp_cap_hits", 0) + 1
                        )
                        add_degrade_event(
                            "cse_dce_fixed_point_cap",
                            "cse_dce_closure",
                            "fail_closed_non_convergence",
                            value=cse_dce_fp_max_iters,
                        )
                        degraded = True
                        self._record_midend_pass_sample(
                            "cse_dce_closure",
                            elapsed_ms=0.0,
                            accepted=False,
                            degraded=True,
                        )
                else:
                    self._record_midend_pass_sample(
                        "cse",
                        elapsed_ms=0.0,
                        accepted=False,
                        degraded=True,
                    )
                maybe_apply_budget_degrade(
                    f"round_{round_index}_post_cse",
                    round_index - 1,
                    ops_now=len(step_ops),
                )

            rewritten_ops = step_ops
            round_passes_run: list[str] = [
                "simplify",
                "sccp_edge_thread",
                "join_canonicalize",
            ]
            if enable_guard_hoist:
                round_passes_run.append("guard_hoist")
            if enable_licm:
                round_passes_run.append("licm")
            round_passes_run.append("prune")
            round_passes_run.append("verifier")
            if not round_failures:
                round_passes_run.append("dce")
                if enable_cse:
                    round_passes_run.append("cse")
                if post_cse_dce_ran:
                    round_passes_run.append("post_cse_dce")
            round_changed = rewritten_ops != step_before
            round_snapshots.append(
                {
                    "round": round_index,
                    "spent_ms": round(max(0.0, spent_midend_ms()), 3),
                    "passes_run": round_passes_run,
                    "changed": round_changed,
                }
            )
            if cse_dce_closure_failed:
                break
            if not round_changed:
                converged = True
                break

        if not converged:
            self.midend_stats["fixed_point_fail_fast"] += 1
            add_degrade_event(
                "fixed_point_round_cap",
                "fixed_point_exit",
                "fail_closed_non_convergence",
                value=max_rounds,
            )
            degraded = True
            self._record_midend_policy_outcome(
                policy=policy,
                spent_ms=spent_midend_ms(),
                work_units_spent=work_units_spent,
                degraded=degraded,
                degrade_events=degrade_events,
                round_snapshots=round_snapshots,
            )
            raise RuntimeError(
                "midend deterministic fixed-point failed to converge within "
                f"{max_rounds} rounds for {self._active_midend_function_name}"
            )

        if converged:
            probe_ops = rewritten_ops
            probe_ops, _probe_cfg_rewrites = self._canonicalize_cfg_before_optimization(
                probe_ops
            )
            probe_ops, _probe_region_prunes = (
                self._canonicalize_structured_regions_pre_sccp(probe_ops)
            )
            probe_ops, _probe_label_prunes, _probe_jump_noops = (
                self._prune_dead_labels_and_noop_jumps(probe_ops)
            )
            probe_ops, _probe_validity_rewrites = self._ensure_structural_cfg_validity(
                probe_ops, stage="midend_idempotence_probe"
            )
            if probe_ops != rewritten_ops:
                self.midend_stats["fixed_point_fail_fast"] += 1
                add_degrade_event(
                    "idempotence_probe_mismatch",
                    "idempotence_probe",
                    "fail_closed_idempotence_probe",
                )
                degraded = True
                self._record_midend_policy_outcome(
                    policy=policy,
                    spent_ms=spent_midend_ms(),
                    work_units_spent=work_units_spent,
                    degraded=degraded,
                    degrade_events=degrade_events,
                    round_snapshots=round_snapshots,
                )
                raise RuntimeError(
                    "midend idempotence check failed after convergence for "
                    f"{self._active_midend_function_name}"
                )

        final_guard_prune_input = rewritten_ops
        rewritten_ops, final_fused_dict_guard_prunes = (
            self._eliminate_redundant_fused_dict_increment_guards(rewritten_ops)
        )
        if final_fused_dict_guard_prunes:
            self.midend_stats["fused_dict_guard_prunes"] = (
                self.midend_stats.get("fused_dict_guard_prunes", 0)
                + final_fused_dict_guard_prunes
            )
            func_stats["fused_dict_guard_prunes"] += final_fused_dict_guard_prunes
            final_predefined = self._infer_predefined_value_names(
                final_guard_prune_input
            )
            final_failures = self._verify_definite_assignment_in_ops(
                rewritten_ops, predefined_value_names=final_predefined
            )
            if final_failures:
                rewritten_ops = final_guard_prune_input
                self.midend_stats["fused_dict_guard_prunes"] -= (
                    final_fused_dict_guard_prunes
                )
                func_stats["fused_dict_guard_prunes"] -= final_fused_dict_guard_prunes

        self.midend_stats["sccp_branch_prunes"] += total_branch_prunes
        self.midend_stats["loop_edge_thread_prunes"] += (
            total_loop_edge_prunes + total_loop_marker_prunes
        )
        self.midend_stats["try_edge_thread_prunes"] += total_try_edge_prunes
        self.midend_stats["licm_hoists"] += total_licm_hoists
        self.midend_stats["unreachable_blocks_removed"] += total_unreachable_blocks
        self.midend_stats["cfg_region_prunes"] += total_region_prunes
        self.midend_stats["label_prunes"] += total_label_prunes
        self.midend_stats["jump_noop_elisions"] += total_jump_noops
        self.midend_stats["phi_edge_trims"] += total_phi_edge_trims

        sccp_applied = (
            total_branch_prunes
            + total_loop_edge_prunes
            + total_try_edge_prunes
            + total_loop_marker_prunes
            + total_region_prunes
            + total_unreachable_blocks
            + total_label_prunes
            + total_jump_noops
            + total_try_join_threads
            + total_phi_edge_trims
            + total_licm_hoists
        )
        if sccp_applied > 0:
            func_stats["sccp_accepted"] += 1

        edge_thread_applied = (
            total_branch_prunes
            + total_loop_edge_prunes
            + total_try_edge_prunes
            + total_loop_marker_prunes
            + total_region_prunes
            + total_unreachable_blocks
            + total_label_prunes
            + total_jump_noops
            + total_try_join_threads
        )
        if edge_thread_applied > 0:
            func_stats["edge_thread_accepted"] += 1
        else:
            func_stats["edge_thread_rejected"] += 1

        func_stats["loop_rewrite_attempted"] += total_loop_rewrite_attempts
        func_stats["loop_rewrite_accepted"] += total_loop_rewrite_accepted
        func_stats["loop_rewrite_rejected"] += max(
            0, total_loop_rewrite_attempts - total_loop_rewrite_accepted
        )

        guard_hoist_attempt_delta = (
            self.midend_stats.get("guard_hoist_attempts", 0)
            - guard_hoist_attempts_before
        )
        guard_hoist_accept_delta = (
            self.midend_stats.get("guard_hoist_accepted", 0)
            - guard_hoist_accepted_before
        )
        guard_hoist_reject_delta = (
            self.midend_stats.get("guard_hoist_rejected", 0)
            - guard_hoist_rejected_before
        )
        func_stats["guard_hoist_attempted"] += max(0, guard_hoist_attempt_delta)
        func_stats["guard_hoist_accepted"] += max(0, guard_hoist_accept_delta)
        func_stats["guard_hoist_rejected"] += max(0, guard_hoist_reject_delta)

        if self.midend_stats.get("gvn_hits", 0) > gvn_hits_before:
            func_stats["gvn_accepted"] += 1
            func_stats["cse_accepted"] += 1
        if total_licm_hoists > 0:
            func_stats["licm_accepted"] += 1
        else:
            func_stats["licm_rejected"] += 1
        if self.midend_stats.get("dce_removed_total", 0) > dce_removed_before:
            func_stats["dce_accepted"] += 1

        self._record_midend_policy_outcome(
            policy=policy,
            spent_ms=spent_midend_ms(),
            work_units_spent=work_units_spent,
            degraded=degraded,
            degrade_events=degrade_events,
            round_snapshots=round_snapshots,
        )
        return rewritten_ops

    def _canonicalize_control_aware_ops(self, ops: list[MoltOp]) -> list[MoltOp]:
        predefined = self._infer_predefined_value_names(ops)
        self.midend_stats["expanded_attempts"] += 1

        expanded_ops = self._canonicalize_control_aware_ops_impl(
            ops, allow_cross_block_const_dedupe=True
        )
        expanded_failures = self._verify_definite_assignment_in_ops(
            expanded_ops, predefined_value_names=predefined
        )
        if not expanded_failures:
            self.midend_stats["expanded_accepted"] += 1
            return expanded_ops

        self.midend_stats["expanded_fallbacks"] += 1
        # Diagnostic: log which variables caused the verification failure so
        # that the cross-block CSE issue can be traced.  Each failure is a
        # tuple (op_index, op_kind, value_name).
        if os.getenv("MOLT_MIDEND_STATS"):
            failed_vars = sorted({name for _, _, name in expanded_failures})
            failed_ops = sorted({kind for _, kind, _ in expanded_failures})
            print(
                f"molt midend cross-block CSE fallback:"
                f" func={self._active_midend_function_name!r}"
                f" failed_vars={failed_vars}"
                f" failed_ops={failed_ops}"
                f" failure_count={len(expanded_failures)}",
                file=sys.stderr,
            )
        safe_ops = self._canonicalize_control_aware_ops_impl(
            ops, allow_cross_block_const_dedupe=False
        )
        return safe_ops

    def _coalesce_check_exception_ops(self, ops: list[MoltOp]) -> list[MoltOp]:
        # Keep coalescing conservative: moving checks across value-producing ops can
        # expose uninitialized/missing operands when an exception is already pending.
        # `LINE` is metadata-only and safe to commute with `CHECK_EXCEPTION`.
        safe_after_check = {"LINE"}
        out: list[MoltOp] = []
        pending_check: MoltOp | None = None
        for op in ops:
            if op.kind == "CHECK_EXCEPTION":
                pending_check = op
                continue
            if pending_check is not None and op.kind not in safe_after_check:
                out.append(pending_check)
                pending_check = None
            out.append(op)
        if pending_check is not None:
            out.append(pending_check)
        return out
