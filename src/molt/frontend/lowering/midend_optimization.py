"""MidendOptimizationMixin: frontend IR midend policy and CFG rewrites.

Move-only extraction from frontend/__init__.py. Owns the optimization policy,
SCCP lattice, CFG canonicalization, guard elimination, CSE/LICM rounds, and
structural CFG cleanup used by SimpleTIRGenerator before serialization.
"""

from __future__ import annotations

import os
import sys
import time
from collections import deque
from typing import TYPE_CHECKING, Any, NoReturn, Sequence, cast

from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    CFGGraph,
    CanonicalizationState,
    ControlMaps,
    LoopBoundFact,
    MidendEnvConfig,
    MidendFunctionPolicy,
    MidendProfile,
    MidendTier,
    MidendTierClassification,
    MoltOp,
    MoltValue,
    SCCPResult,
    _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY,
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
    _MIDEND_DEGRADE_CHECKPOINTS,
    _MIDEND_ENV_KEYS,
    _MIDEND_WORK_BASE_UNITS_PER_MS,
    _MIDEND_WORK_GROWTH_HEADROOM,
    _MOLT_MODULE_CHUNK_PREFIX,
    _SCCP_MISSING,
    _SCCP_OVERDEFINED,
    _SCCP_UNKNOWN,
    _TrackedOpsList,
    build_cfg,
)
from molt.frontend.lowering.op_kinds_generated import FRONTEND_EFFECT_CLASS

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class MidendOptimizationMixin(_MixinBase):
    def _init_midend_state(
        self,
        optimization_profile: MidendProfile,
        pgo_hot_functions: set[str] | None,
    ) -> None:
        if optimization_profile not in {"dev", "release"}:
            optimization_profile = "release"
        self.optimization_profile: MidendProfile = optimization_profile
        self.midend_hot_functions: set[str] = {
            symbol.strip()
            for symbol in (pgo_hot_functions or set())
            if isinstance(symbol, str) and symbol.strip()
        }
        self.midend_env = self._resolve_midend_env_config()
        self._midend_env_snapshot = self._capture_midend_env_snapshot()
        self.midend_stats: dict[str, int] = {
            "expanded_attempts": 0,
            "expanded_accepted": 0,
            "expanded_fallbacks": 0,
            "midend_module_skips": 0,
            "midend_oversized_function_skips": 0,
            "invalid_unbound_rollback": 0,
            "invalid_unbound_uses": 0,
            "fixed_point_fail_fast": 0,
            "cfg_structural_failures": 0,
            "cfg_structural_canonicalizations": 0,
            "sccp_iteration_cap_hits": 0,
            "cse_dce_fp_cap_hits": 0,
            "sccp_branch_prunes": 0,
            "loop_edge_thread_prunes": 0,
            "try_edge_thread_prunes": 0,
            "unreachable_blocks_removed": 0,
            "cfg_region_prunes": 0,
            "label_prunes": 0,
            "jump_noop_elisions": 0,
            "licm_hoists": 0,
            "guard_hoist_attempts": 0,
            "guard_hoist_accepted": 0,
            "guard_hoist_rejected": 0,
            "fused_dict_guard_prunes": 0,
            "phi_edge_trims": 0,
            "gvn_hits": 0,
            "dce_removed_total": 0,
        }
        self.midend_stats_by_function: dict[str, dict[str, int]] = {}
        self.midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]] = {}
        self.midend_policy_outcomes_by_function: dict[str, dict[str, Any]] = {}
        self._active_midend_function_name = "<direct>"
        self._midend_stats_reported = False

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

    def _midend_function_stats(self) -> dict[str, int]:
        name = self._active_midend_function_name
        stats = self.midend_stats_by_function.get(name)
        if stats is None:
            stats = {
                "sccp_attempted": 0,
                "sccp_accepted": 0,
                "sccp_iteration_cap_hits": 0,
                "edge_thread_attempted": 0,
                "edge_thread_accepted": 0,
                "edge_thread_rejected": 0,
                "loop_rewrite_attempted": 0,
                "loop_rewrite_accepted": 0,
                "loop_rewrite_rejected": 0,
                "guard_hoist_attempted": 0,
                "guard_hoist_accepted": 0,
                "guard_hoist_rejected": 0,
                "fused_dict_guard_prunes": 0,
                "cse_attempted": 0,
                "cse_accepted": 0,
                "cse_readheap_attempted": 0,
                "cse_readheap_accepted": 0,
                "cse_readheap_rejected": 0,
                "gvn_attempted": 0,
                "gvn_accepted": 0,
                "licm_attempted": 0,
                "licm_accepted": 0,
                "licm_rejected": 0,
                "dce_attempted": 0,
                "dce_accepted": 0,
                "dce_pure_op_attempted": 0,
                "dce_pure_op_accepted": 0,
                "dce_pure_op_rejected": 0,
            }
            self.midend_stats_by_function[name] = stats
        return stats

    def _midend_pass_stats(self, pass_name: str) -> dict[str, Any]:
        func_name = self._active_midend_function_name
        per_func = self.midend_pass_stats_by_function.setdefault(func_name, {})
        stats = per_func.get(pass_name)
        if stats is None:
            stats = {
                "attempted": 0,
                "accepted": 0,
                "rejected": 0,
                "degraded": 0,
                "ms_total": 0.0,
                "ms_max": 0.0,
                "samples_ms": [],
            }
            per_func[pass_name] = stats
        return stats

    @staticmethod
    def _midend_csv_tokens(value: str) -> set[str]:
        return {token.strip() for token in value.split(",") if token.strip()}

    @staticmethod
    def _midend_float_env(name: str, default: float) -> float:
        raw = os.getenv(name, "").strip()
        if not raw:
            return default
        try:
            return float(raw)
        except ValueError:
            return default

    @staticmethod
    def _midend_positive_int_env(name: str, default: int, *, minimum: int = 1) -> int:
        floor = max(1, minimum)
        fallback = max(floor, int(default))
        raw = os.getenv(name, "").strip()
        if not raw:
            return fallback
        try:
            parsed = int(raw)
        except ValueError:
            return fallback
        if parsed < floor:
            return fallback
        return parsed

    def _resolve_midend_env_config(self) -> MidendEnvConfig:
        work_budget_override_raw = os.getenv("MOLT_MIDEND_WORK_BUDGET", "").strip()
        work_budget_override: float | None = None
        if work_budget_override_raw:
            try:
                work_budget_override = max(0.0, float(work_budget_override_raw))
            except ValueError:
                work_budget_override = None
        max_rounds_override = os.getenv("MOLT_MIDEND_MAX_ROUNDS", "").strip()
        sccp_iter_cap_override = os.getenv("MOLT_SCCP_MAX_ITERS", "").strip()
        cse_iter_cap_override = os.getenv("MOLT_CSE_MAX_ITERS", "").strip()
        return MidendEnvConfig(
            skip_op_threshold=self._midend_positive_int_env(
                "MOLT_MIDEND_SKIP_OP_THRESHOLD", 800, minimum=1
            ),
            monolith_function_threshold=max(
                8,
                self._midend_positive_int_env(
                    "MOLT_MIDEND_MONOLITH_FUNCTION_THRESHOLD", 48
                ),
            ),
            monolith_total_ops_threshold=max(
                256,
                self._midend_positive_int_env(
                    "MOLT_MIDEND_MONOLITH_TOTAL_OPS_THRESHOLD", 4000
                ),
            ),
            hot_tier_promotion_enabled=os.getenv("MOLT_MIDEND_HOT_TIER_PROMOTION", "1")
            .strip()
            .lower()
            not in {"0", "false", "no", "off"},
            work_budget_override=work_budget_override,
            budget_alpha=self._midend_float_env("MOLT_MIDEND_BUDGET_ALPHA", 0.03),
            budget_beta=self._midend_float_env("MOLT_MIDEND_BUDGET_BETA", 0.75),
            budget_scale=max(
                0.0, self._midend_float_env("MOLT_MIDEND_BUDGET_SCALE", 1.0)
            ),
            max_rounds_override=(
                self._midend_positive_int_env("MOLT_MIDEND_MAX_ROUNDS", 2, minimum=2)
                if max_rounds_override
                else None
            ),
            sccp_iter_cap_override=(
                self._midend_positive_int_env("MOLT_SCCP_MAX_ITERS", 1, minimum=1)
                if sccp_iter_cap_override
                else None
            ),
            cse_iter_cap_override=(
                self._midend_positive_int_env("MOLT_CSE_MAX_ITERS", 1, minimum=1)
                if cse_iter_cap_override
                else None
            ),
            cse_fp_max_iters=self._midend_positive_int_env(
                "MOLT_CSE_FP_MAX_ITERS", 3, minimum=1
            ),
        )

    @staticmethod
    def _capture_midend_env_snapshot() -> tuple[str | None, ...]:
        return tuple(os.environ.get(name) for name in _MIDEND_ENV_KEYS)

    def _adjust_module_pressure_counts(
        self,
        *,
        function_delta: int = 0,
        ops_delta: int = 0,
    ) -> None:
        self._module_pressure_function_count += function_delta
        self._module_pressure_total_ops += ops_delta

    def _sync_module_pressure_counts_from_funcs_map(self) -> None:
        function_count = 0
        total_ops = 0
        for name, info in self.funcs_map.items():
            if not isinstance(info, dict):
                continue
            if name != "molt_main" and "ops" in info:
                function_count += 1
            total_ops += len(info.get("ops", []))
        self._module_pressure_function_count = function_count
        self._module_pressure_total_ops = total_ops
        self._module_pressure_funcs_map_ref = self.funcs_map

    def _current_module_pressure_snapshot(self) -> tuple[int, int, int]:
        if self._module_pressure_funcs_map_ref is not self.funcs_map:
            self._sync_module_pressure_counts_from_funcs_map()
        function_count = self._module_pressure_function_count
        total_ops = self._module_pressure_total_ops
        func_threshold = self.midend_env.monolith_function_threshold
        ops_threshold = self.midend_env.monolith_total_ops_threshold
        hard_func_threshold = max(func_threshold + 1, func_threshold * 2)
        hard_ops_threshold = max(ops_threshold + 1, ops_threshold * 2)
        level = 0
        if function_count >= func_threshold or total_ops >= ops_threshold:
            level = 1
        if function_count >= hard_func_threshold or total_ops >= hard_ops_threshold:
            level = 2
        return function_count, total_ops, level

    def _new_tracked_ops(
        self,
        initial: list[MoltOp] | None = None,
        *,
        count_function: bool = False,
    ) -> _TrackedOpsList:
        if count_function:
            self._module_pressure_function_count += 1
        tracked = _TrackedOpsList(self, initial)
        self._module_pressure_total_ops += len(tracked)
        return tracked

    def _refresh_midend_env_config_if_needed(self) -> None:
        snapshot = self._capture_midend_env_snapshot()
        if snapshot == self._midend_env_snapshot:
            return
        self.midend_env = self._resolve_midend_env_config()
        self._midend_env_snapshot = snapshot

    def _midend_hot_function_match(self, function_name: str) -> str | None:
        if not self.midend_hot_functions:
            return None
        module_name = self.module_name or ""
        aliases: set[str] = {function_name}
        if module_name:
            aliases.add(f"{module_name}::{function_name}")
            aliases.add(f"{module_name}.{function_name}")
        if function_name == "molt_main":
            init_symbol = self.module_init_symbol(module_name or "__main__")
            aliases.add(init_symbol)
            if module_name:
                aliases.add(f"{module_name}::{init_symbol}")
                aliases.add(f"{module_name}.{init_symbol}")
        for alias in sorted(aliases):
            if alias in self.midend_hot_functions:
                return alias
        return None

    @staticmethod
    def _promote_midend_tier_one_step(tier: MidendTier) -> MidendTier:
        if tier == "C":
            return "B"
        if tier == "B":
            return "A"
        return tier

    def _classify_midend_tier(
        self, function_name: str, ops: list[MoltOp]
    ) -> MidendTierClassification:
        forced_tier = os.getenv("MOLT_MIDEND_TIER_FORCE", "").strip().upper()
        if forced_tier in {"A", "B", "C"}:
            return MidendTierClassification(
                tier=cast(MidendTier, forced_tier),
                source="forced_env",
                allow_hot_promotion=False,
            )

        tier_a_functions = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_A_FUNCTIONS", "")
        )
        tier_b_functions = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_B_FUNCTIONS", "")
        )
        tier_c_functions = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_C_FUNCTIONS", "")
        )
        if function_name in tier_a_functions:
            return MidendTierClassification(
                tier="A",
                source="function_override",
                allow_hot_promotion=False,
            )
        if function_name in tier_c_functions:
            return MidendTierClassification(
                tier="C",
                source="function_override",
                allow_hot_promotion=False,
            )
        if function_name in tier_b_functions:
            return MidendTierClassification(
                tier="B",
                source="function_override",
                allow_hot_promotion=False,
            )

        module_name = self.module_name or ""
        tier_a_prefixes = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_A_MODULE_PREFIXES", "")
        )
        tier_b_prefixes = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_B_MODULE_PREFIXES", "")
        )
        tier_c_prefixes = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_C_MODULE_PREFIXES", "")
        )
        for prefix in sorted(tier_a_prefixes):
            if module_name == prefix or module_name.startswith(f"{prefix}."):
                return MidendTierClassification(
                    tier="A",
                    source="module_prefix_override",
                    allow_hot_promotion=False,
                )
        for prefix in sorted(tier_c_prefixes):
            if module_name == prefix or module_name.startswith(f"{prefix}."):
                return MidendTierClassification(
                    tier="C",
                    source="module_prefix_override",
                    allow_hot_promotion=False,
                )
        for prefix in sorted(tier_b_prefixes):
            if module_name == prefix or module_name.startswith(f"{prefix}."):
                return MidendTierClassification(
                    tier="B",
                    source="module_prefix_override",
                    allow_hot_promotion=False,
                )

        op_count = len(ops)
        if function_name == "molt_main":
            if op_count >= 1800:
                return MidendTierClassification(
                    tier="C",
                    source="module_entry_oversized",
                    allow_hot_promotion=True,
                )
            return MidendTierClassification(
                tier="A",
                source="module_entry_default",
                allow_hot_promotion=True,
            )

        chunk_prefix = f"{self.module_prefix}{_MOLT_MODULE_CHUNK_PREFIX}_"
        if self._source_is_stdlib_module:
            # Stdlib defaults to the lightest tier unless explicitly elevated
            # via A/B overrides above.
            if function_name.startswith(chunk_prefix):
                return MidendTierClassification(
                    tier="C",
                    source="stdlib_chunk_default",
                    allow_hot_promotion=True,
                )
            return MidendTierClassification(
                tier="C",
                source="stdlib_default",
                allow_hot_promotion=True,
            )
        if op_count >= 1800:
            return MidendTierClassification(
                tier="C",
                source="op_count_threshold",
                allow_hot_promotion=True,
            )
        return MidendTierClassification(
            tier="B",
            source="default",
            allow_hot_promotion=True,
        )

    def _resolve_midend_function_policy(
        self,
        ops: list[MoltOp],
        *,
        function_name: str | None = None,
        block_count: int = 1,
    ) -> MidendFunctionPolicy:
        self._refresh_midend_env_config_if_needed()
        profile = self.optimization_profile
        profile_override = os.getenv("MOLT_MIDEND_PROFILE", "").strip().lower()
        if profile_override in {"dev", "release"}:
            profile = cast(MidendProfile, profile_override)

        resolved_function = function_name or self._active_midend_function_name
        tier_classification = self._classify_midend_tier(resolved_function, ops)
        tier_base = tier_classification.tier
        tier = tier_base
        promoted = False
        promotion_source = ""
        promotion_signal = ""
        module_function_count, module_total_ops, monolith_pressure_level = (
            self._current_module_pressure_snapshot()
        )
        hot_promotion_enabled = self.midend_env.hot_tier_promotion_enabled
        if hot_promotion_enabled and tier_classification.allow_hot_promotion:
            hot_signal = self._midend_hot_function_match(resolved_function)
            if hot_signal and tier_base in {"B", "C"}:
                promoted_tier = self._promote_midend_tier_one_step(tier_base)
                if promoted_tier != tier_base:
                    tier = promoted_tier
                    promoted = True
                    promotion_source = "pgo_hot_functions"
                    promotion_signal = hot_signal
        defaults: dict[tuple[MidendProfile, MidendTier], dict[str, Any]] = {
            ("dev", "A"): {
                "max_rounds": 2,
                "sccp_iter_cap": 48,
                "cse_iter_cap": 16,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 60.0,
            },
            ("dev", "B"): {
                "max_rounds": 1,
                "sccp_iter_cap": 24,
                "cse_iter_cap": 8,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 35.0,
            },
            ("dev", "C"): {
                "max_rounds": 1,
                "sccp_iter_cap": 12,
                "cse_iter_cap": 4,
                "enable_deep_edge_thread": False,
                "enable_cse": False,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 20.0,
            },
            ("release", "A"): {
                "max_rounds": 4,
                "sccp_iter_cap": 128,
                "cse_iter_cap": 48,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": True,
                "enable_guard_hoist": True,
                "budget_base_ms": 180.0,
            },
            ("release", "B"): {
                "max_rounds": 3,
                "sccp_iter_cap": 96,
                "cse_iter_cap": 32,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": True,
                "enable_guard_hoist": True,
                "budget_base_ms": 110.0,
            },
            ("release", "C"): {
                "max_rounds": 2,
                "sccp_iter_cap": 48,
                "cse_iter_cap": 16,
                "enable_deep_edge_thread": False,
                "enable_cse": True,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 70.0,
            },
        }
        selected = dict(defaults[(profile, tier)])
        monolith_pressure_exempt = (
            resolved_function == "molt_main" or promoted or tier == "A"
        )
        if not monolith_pressure_exempt and monolith_pressure_level >= 1:
            selected["max_rounds"] = max(1, int(selected["max_rounds"]) - 1)
            selected["sccp_iter_cap"] = max(
                8, int(int(selected["sccp_iter_cap"]) * 0.75)
            )
            selected["cse_iter_cap"] = max(4, int(int(selected["cse_iter_cap"]) * 0.75))
            selected["budget_base_ms"] = float(selected["budget_base_ms"]) * 0.85
        if not monolith_pressure_exempt and monolith_pressure_level >= 2:
            selected["max_rounds"] = max(1, int(selected["max_rounds"]) - 1)
            selected["sccp_iter_cap"] = max(
                8, int(int(selected["sccp_iter_cap"]) * 0.75)
            )
            selected["cse_iter_cap"] = max(4, int(int(selected["cse_iter_cap"]) * 0.75))
            selected["enable_guard_hoist"] = False
            selected["budget_base_ms"] = float(selected["budget_base_ms"]) * 0.8
        alpha = self.midend_env.budget_alpha
        beta = self.midend_env.budget_beta
        scale = max(0.0, self.midend_env.budget_scale)
        budget_ms = (
            selected["budget_base_ms"]
            + alpha * max(1, len(ops))
            + beta * max(1, block_count)
        ) * scale
        budget_ms_override_raw = os.getenv("MOLT_MIDEND_BUDGET_MS", "").strip()
        if budget_ms_override_raw:
            try:
                budget_ms = max(0.0, float(budget_ms_override_raw))
            except ValueError:
                pass
        # Deterministic work-unit budget for the degrade ladder (#73).  The
        # ladder accumulates a deterministic cost — the op count processed —
        # at each inter-pass checkpoint, and degrades when the running total
        # exceeds this budget.  Because the work-units depend only on the IR
        # (op/block counts) and the deterministic per-tier round/iteration
        # caps, the resulting pass selection — and the emitted IR — is a pure
        # function of the input, independent of wall-clock timing.
        #
        # Calibration: a non-pathological function executes the full
        # `max_rounds` round loop, hitting ~`_MIDEND_DEGRADE_CHECKPOINTS`
        # work-charges per round, each ≈ the live op count.  The ceiling below
        # admits that nominal cost (plus generous per-round growth headroom and
        # the per-tier base) so normal functions never degrade, while a pass
        # that pathologically balloons the op count still trips the ladder and
        # bounds compile time — matching the original safety intent without its
        # nondeterminism.
        work_budget_override = self.midend_env.work_budget_override
        if work_budget_override is not None:
            work_budget = work_budget_override
        else:
            base_ops = max(1, len(ops))
            rounds = max(1, int(selected["max_rounds"]))
            nominal_round_work = _MIDEND_DEGRADE_CHECKPOINTS * base_ops
            work_budget = (
                float(selected["budget_base_ms"]) * _MIDEND_WORK_BASE_UNITS_PER_MS
                + _MIDEND_WORK_GROWTH_HEADROOM * rounds * nominal_round_work
                + beta * max(1, block_count)
            ) * scale
        return MidendFunctionPolicy(
            profile=profile,
            tier=tier,
            tier_base=tier_base,
            tier_source=tier_classification.source,
            promoted=promoted,
            promotion_source=promotion_source,
            promotion_signal=promotion_signal,
            max_rounds=max(2, int(selected["max_rounds"])),
            sccp_iter_cap=int(selected["sccp_iter_cap"]),
            cse_iter_cap=int(selected["cse_iter_cap"]),
            enable_deep_edge_thread=bool(selected["enable_deep_edge_thread"]),
            enable_cse=bool(selected["enable_cse"]),
            enable_licm=bool(selected["enable_licm"]),
            enable_guard_hoist=bool(selected["enable_guard_hoist"]),
            budget_ms=float(budget_ms),
            work_budget=float(work_budget),
            allow_hot_promotion=bool(
                tier_classification.allow_hot_promotion and hot_promotion_enabled
            ),
            module_function_count=module_function_count,
            module_total_ops=module_total_ops,
            monolith_pressure_level=monolith_pressure_level,
        )

    def _record_midend_pass_sample(
        self,
        pass_name: str,
        *,
        elapsed_ms: float,
        accepted: bool,
        degraded: bool = False,
    ) -> None:
        stats = self._midend_pass_stats(pass_name)
        stats["attempted"] = int(stats.get("attempted", 0)) + 1
        if accepted:
            stats["accepted"] = int(stats.get("accepted", 0)) + 1
        else:
            stats["rejected"] = int(stats.get("rejected", 0)) + 1
        if degraded:
            stats["degraded"] = int(stats.get("degraded", 0)) + 1
        stats["ms_total"] = float(stats.get("ms_total", 0.0)) + max(0.0, elapsed_ms)
        stats["ms_max"] = max(float(stats.get("ms_max", 0.0)), max(0.0, elapsed_ms))
        samples = stats.get("samples_ms")
        if not isinstance(samples, list):
            samples = []
            stats["samples_ms"] = samples
        samples.append(max(0.0, elapsed_ms))
        if len(samples) > 256:
            del samples[: len(samples) - 256]

    @staticmethod
    def _pass_stat_p95(samples: list[float]) -> float:
        if not samples:
            return 0.0
        ordered = sorted(samples)
        idx = max(0, min(len(ordered) - 1, int((len(ordered) - 1) * 0.95)))
        return float(ordered[idx])

    def _record_midend_policy_outcome(
        self,
        *,
        policy: MidendFunctionPolicy,
        spent_ms: float,
        work_units_spent: float,
        degraded: bool,
        degrade_events: list[dict[str, Any]],
        round_snapshots: list[dict[str, Any]] | None = None,
    ) -> None:
        self.midend_policy_outcomes_by_function[self._active_midend_function_name] = {
            "profile": policy.profile,
            "tier": policy.tier,
            "tier_base": policy.tier_base,
            "tier_source": policy.tier_source,
            "tier_effective": policy.tier,
            "promoted": policy.promoted,
            "promotion_source": policy.promotion_source,
            "promotion_signal": policy.promotion_signal,
            "allow_hot_promotion": policy.allow_hot_promotion,
            "module_function_count": policy.module_function_count,
            "module_total_ops": policy.module_total_ops,
            "monolith_pressure_level": policy.monolith_pressure_level,
            "budget_ms": round(policy.budget_ms, 3),
            "spent_ms": round(max(0.0, spent_ms), 3),
            "work_budget": round(max(0.0, policy.work_budget), 3),
            "work_units_spent": round(max(0.0, work_units_spent), 3),
            "degraded": degraded,
            "degrade_events": list(degrade_events),
            "round_snapshots": list(round_snapshots) if round_snapshots else [],
        }

    def _log_degrade_levels(
        self,
        degrade_level: int,
        reasons: list[str],
        budget_ms: float,
    ) -> None:
        """Log which functions hit which degrade level and why (MOL-27)."""
        func_name = self._active_midend_function_name
        outcome = self.midend_policy_outcomes_by_function.get(func_name)
        if outcome is not None:
            outcome["degrade_level"] = degrade_level
            outcome["degrade_level_reasons"] = list(reasons)
            outcome["per_func_budget_ms"] = round(budget_ms, 3)
        if os.getenv("MOLT_MIDEND_STATS") is not None:
            level_desc = {
                1: "skip LICM + guard hoist",
                2: "skip SCCP multi-pass",
                3: "skip all optimisation",
            }.get(degrade_level, f"unknown({degrade_level})")
            print(
                f"molt midend degrade: {func_name} level={degrade_level}"
                f" ({level_desc}) budget_ms={budget_ms:.1f}"
                f" reasons={reasons}",
                file=sys.stderr,
            )

    def _maybe_report_midend_stats(self) -> None:
        if self._midend_stats_reported:
            return
        if os.getenv("MOLT_MIDEND_STATS") is None:
            return
        self._midend_stats_reported = True
        ordered_keys = [
            "expanded_attempts",
            "expanded_accepted",
            "expanded_fallbacks",
            "midend_module_skips",
            "midend_oversized_function_skips",
            "invalid_unbound_rollback",
            "invalid_unbound_uses",
            "fixed_point_fail_fast",
            "cfg_structural_failures",
            "cfg_structural_canonicalizations",
            "sccp_iteration_cap_hits",
            "cse_dce_fp_cap_hits",
            "sccp_branch_prunes",
            "loop_edge_thread_prunes",
            "try_edge_thread_prunes",
            "unreachable_blocks_removed",
            "cfg_region_prunes",
            "label_prunes",
            "jump_noop_elisions",
            "licm_hoists",
            "guard_hoist_attempts",
            "guard_hoist_accepted",
            "guard_hoist_rejected",
            "phi_edge_trims",
            "gvn_hits",
            "dce_removed_total",
        ]
        rendered = " ".join(
            f"{key}={self.midend_stats.get(key, 0)}" for key in ordered_keys
        )
        print(
            f"molt midend stats: {rendered}",
            file=sys.stderr,
        )
        per_func = []
        for func_name in sorted(self.midend_stats_by_function):
            stats = self.midend_stats_by_function[func_name]
            per_func.append(
                f"{func_name}:"
                f"sccp={stats.get('sccp_accepted', 0)}/{stats.get('sccp_attempted', 0)},"
                f"sccp_cap={stats.get('sccp_iteration_cap_hits', 0)},"
                f"edge_thread={stats.get('edge_thread_accepted', 0)}/{stats.get('edge_thread_attempted', 0)}"
                f"(rej={stats.get('edge_thread_rejected', 0)}),"
                f"loop_rewrite={stats.get('loop_rewrite_accepted', 0)}/{stats.get('loop_rewrite_attempted', 0)}"
                f"(rej={stats.get('loop_rewrite_rejected', 0)}),"
                f"guard_hoist={stats.get('guard_hoist_accepted', 0)}/{stats.get('guard_hoist_attempted', 0)}"
                f"(rej={stats.get('guard_hoist_rejected', 0)}),"
                f"cse={stats.get('cse_accepted', 0)}/{stats.get('cse_attempted', 0)},"
                f"cse_readheap={stats.get('cse_readheap_accepted', 0)}/{stats.get('cse_readheap_attempted', 0)}"
                f"(rej={stats.get('cse_readheap_rejected', 0)}),"
                f"gvn={stats.get('gvn_accepted', 0)}/{stats.get('gvn_attempted', 0)},"
                f"licm={stats.get('licm_accepted', 0)}/{stats.get('licm_attempted', 0)}"
                f"(rej={stats.get('licm_rejected', 0)}),"
                f"dce={stats.get('dce_accepted', 0)}/{stats.get('dce_attempted', 0)},"
                f"dce_pure={stats.get('dce_pure_op_accepted', 0)}/{stats.get('dce_pure_op_attempted', 0)}"
                f"(rej={stats.get('dce_pure_op_rejected', 0)})"
            )
        if per_func:
            print(
                "molt midend function stats: " + " | ".join(per_func),
                file=sys.stderr,
            )
            hotspot_candidates: list[tuple[int, str, str, int, int]] = []
            tracked = [
                ("sccp_iteration_cap_hits", "sccp_cap"),
                ("edge_thread_rejected", "edge_thread"),
                ("loop_rewrite_rejected", "loop_rewrite"),
                ("cse_readheap_rejected", "cse_readheap"),
                ("dce_pure_op_rejected", "dce_pure_op"),
                ("guard_hoist_rejected", "guard_hoist"),
                ("licm_rejected", "licm"),
            ]
            for func_name, stats in self.midend_stats_by_function.items():
                for key, family in tracked:
                    rejected = int(stats.get(key, 0))
                    attempted = int(
                        stats.get(
                            {
                                "sccp_iteration_cap_hits": "sccp_attempted",
                                "edge_thread_rejected": "edge_thread_attempted",
                                "loop_rewrite_rejected": "loop_rewrite_attempted",
                                "cse_readheap_rejected": "cse_readheap_attempted",
                                "dce_pure_op_rejected": "dce_pure_op_attempted",
                                "guard_hoist_rejected": "guard_hoist_attempted",
                                "licm_rejected": "licm_attempted",
                            }[key],
                            0,
                        )
                    )
                    if rejected > 0:
                        hotspot_candidates.append(
                            (rejected, func_name, family, rejected, attempted)
                        )
            if hotspot_candidates:
                hotspot_candidates.sort(reverse=True)
                _score, func_name, family, rejected, attempted = hotspot_candidates[0]
                print(
                    "molt midend hotspot: "
                    f"{func_name} family={family} rejected={rejected} attempted={attempted}",
                    file=sys.stderr,
                )
        if self.midend_policy_outcomes_by_function:
            rendered_policy = []
            for func_name in sorted(self.midend_policy_outcomes_by_function):
                outcome = self.midend_policy_outcomes_by_function[func_name]
                rendered_policy.append(
                    f"{func_name}:profile={outcome.get('profile')},"
                    f"tier={outcome.get('tier')},"
                    f"spent_ms={outcome.get('spent_ms')},"
                    f"budget_ms={outcome.get('budget_ms')},"
                    f"degraded={outcome.get('degraded')}"
                )
            print(
                "molt midend policy outcomes: " + " | ".join(rendered_policy),
                file=sys.stderr,
            )
        pass_hotspots: list[tuple[float, str, str, float, float, int, int, int]] = []
        for func_name, per_pass in self.midend_pass_stats_by_function.items():
            for pass_name, stats in per_pass.items():
                samples = [
                    float(sample)
                    for sample in stats.get("samples_ms", [])
                    if isinstance(sample, (int, float))
                ]
                p95 = self._pass_stat_p95(samples)
                total_ms = float(stats.get("ms_total", 0.0))
                pass_hotspots.append(
                    (
                        total_ms,
                        func_name,
                        pass_name,
                        total_ms,
                        p95,
                        int(stats.get("attempted", 0)),
                        int(stats.get("accepted", 0)),
                        int(stats.get("degraded", 0)),
                    )
                )
        if pass_hotspots:
            pass_hotspots.sort(reverse=True)
            top_passes = []
            for (
                _score,
                func_name,
                pass_name,
                total_ms,
                p95,
                attempted,
                accepted,
                degraded,
            ) in pass_hotspots[:10]:
                top_passes.append(
                    f"{func_name}:{pass_name} total_ms={total_ms:.3f} "
                    f"p95_ms={p95:.3f} attempted={attempted} "
                    f"accepted={accepted} degraded={degraded}"
                )
            print(
                "molt midend pass hotspots: " + " | ".join(top_passes),
                file=sys.stderr,
            )

    def _resolve_alias_value(
        self, value: MoltValue, aliases: dict[str, MoltValue]
    ) -> MoltValue:
        current = value
        visited: set[str] = set()
        while current.name in aliases and current.name not in visited:
            visited.add(current.name)
            current = aliases[current.name]
        return current

    def _rewrite_aliases_in_arg(self, value: Any, aliases: dict[str, MoltValue]) -> Any:
        if isinstance(value, MoltValue):
            return self._resolve_alias_value(value, aliases)
        if isinstance(value, list):
            return [self._rewrite_aliases_in_arg(item, aliases) for item in value]
        if isinstance(value, tuple):
            return tuple(self._rewrite_aliases_in_arg(item, aliases) for item in value)
        if isinstance(value, dict):
            return {
                self._rewrite_aliases_in_arg(k, aliases): self._rewrite_aliases_in_arg(
                    v, aliases
                )
                for k, v in value.items()
            }
        return value

    def _is_canonicalization_barrier_op(self, op_kind: str) -> bool:
        if op_kind in {"RETURN", "RAISE", "RAISE_CAUSE", "RERAISE"}:
            return True
        if op_kind.startswith("EXCEPTION_"):
            return True
        if op_kind.startswith("STATE_"):
            return True
        return False

    def _const_type_tag(self, op: MoltOp) -> int | None:
        if op.kind == "CONST_BOOL":
            return BUILTIN_TYPE_TAGS["bool"]
        if op.kind == "CONST":
            value = op.args[0]
            if isinstance(value, int) and not isinstance(value, bool):
                return BUILTIN_TYPE_TAGS["int"]
        if op.kind == "CONST_BIGINT":
            return BUILTIN_TYPE_TAGS["int"]
        if op.kind == "CONST_FLOAT":
            return BUILTIN_TYPE_TAGS["float"]
        if op.kind == "CONST_STR":
            return BUILTIN_TYPE_TAGS["str"]
        if op.kind == "CONST_BYTES":
            return BUILTIN_TYPE_TAGS["bytes"]
        return None

    def _empty_canonicalization_state(self) -> CanonicalizationState:
        return {
            "aliases": {},
            "const_int_values": {},
            "value_type_tags": {},
            "available_values": {},
            "guard_dict_shapes": {},
            "alias_epochs": {},
            "object_epochs": {},
            "memory_epoch": 0,
        }

    def _clone_canonicalization_state(
        self, state: CanonicalizationState
    ) -> CanonicalizationState:
        cloned: CanonicalizationState = {
            "aliases": state["aliases"].copy(),
            "const_int_values": state["const_int_values"].copy(),
            "value_type_tags": state["value_type_tags"].copy(),
            "available_values": state["available_values"].copy(),
            "guard_dict_shapes": state["guard_dict_shapes"].copy(),
            "alias_epochs": state["alias_epochs"].copy(),
            "object_epochs": state["object_epochs"].copy(),
            "memory_epoch": state["memory_epoch"],
        }
        cached_signature = cast(Any, state).get(
            _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY
        )
        if cached_signature is not None:
            cast(Any, cloned)[_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY] = (
                cached_signature
            )
        return cloned

    def _invalidate_canonicalization_state_signature(
        self, state: CanonicalizationState
    ) -> None:
        cast(Any, state).pop(_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY, None)

    def _const_cache_key_for_op(self, op: MoltOp) -> tuple[Any, ...] | None:
        if op.kind in {"CONST_NONE", "CONST_NOT_IMPLEMENTED", "CONST_ELLIPSIS"}:
            return (op.kind,)
        if op.kind == "CONST_BYTES":
            return ("CONST_BYTES", bytes(op.args[0]))
        if op.kind in {"CONST_BOOL", "CONST_BIGINT", "CONST_FLOAT", "CONST_STR"}:
            value = op.args[0]
            try:
                hash(value)
                normalized = value
            except TypeError:
                normalized = repr(value)
            return (op.kind, normalized)
        if op.kind == "CONST":
            value = op.args[0]
            try:
                hash(value)
                normalized = value
            except TypeError:
                normalized = repr(value)
            return ("CONST", type(value).__name__, normalized)
        return None

    def _op_effect_class(self, op_kind: str) -> str:
        return FRONTEND_EFFECT_CLASS.get(op_kind, "unknown")

    def _is_pure_op_for_global_cse(self, op_kind: str) -> bool:
        return self._op_effect_class(op_kind) == "pure"

    def _is_cse_eligible_op(self, op_kind: str) -> bool:
        return self._op_effect_class(op_kind) in {"pure", "reads_heap"}

    def _normalize_value_operand_key(
        self, value: Any, const_int_values: dict[str, int]
    ) -> tuple[str, Any] | None:
        if not isinstance(value, MoltValue):
            return None
        const_value = const_int_values.get(value.name)
        if const_value is not None:
            return ("const_int", const_value)
        return ("ssa", value.name)

    def _normalize_operand_key_for_value_numbering(
        self, value: Any, const_int_values: dict[str, int]
    ) -> tuple[str, Any] | None:
        if isinstance(value, MoltValue):
            return self._normalize_value_operand_key(value, const_int_values)
        try:
            hash(value)
            return ("const", value)
        except TypeError:
            return ("const_repr", repr(value))

    def _const_type_tag_for_lattice_value(self, value: Any) -> int | None:
        if isinstance(value, bool):
            return BUILTIN_TYPE_TAGS["bool"]
        if isinstance(value, int):
            return BUILTIN_TYPE_TAGS["int"]
        if isinstance(value, float):
            return BUILTIN_TYPE_TAGS["float"]
        if isinstance(value, str):
            return BUILTIN_TYPE_TAGS["str"]
        if isinstance(value, bytes):
            return BUILTIN_TYPE_TAGS["bytes"]
        if isinstance(value, list):
            return BUILTIN_TYPE_TAGS["list"]
        if isinstance(value, tuple):
            return BUILTIN_TYPE_TAGS["tuple"]
        if isinstance(value, dict):
            return BUILTIN_TYPE_TAGS["dict"]
        if isinstance(value, set):
            return BUILTIN_TYPE_TAGS["set"]
        if isinstance(value, frozenset):
            return BUILTIN_TYPE_TAGS["frozenset"]
        if isinstance(value, range):
            return BUILTIN_TYPE_TAGS["range"]
        return None

    def _heap_alias_class_for_read_op(
        self, op: MoltOp, value_type_tags: dict[str, int]
    ) -> str | None:
        if not op.args:
            return None
        primary = op.args[0]
        if not isinstance(primary, MoltValue):
            return "indexable"
        type_tag = value_type_tags.get(primary.name)
        if op.kind == "LEN":
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["frozenset"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            return "indexable"
        if op.kind == "INDEX":
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            return "indexable"
        if op.kind == "CONTAINS":
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["frozenset"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            return "indexable"
        if op.kind in {
            "GET_ATTR",
            "GETATTR",
            "LOAD_ATTR",
            "GETATTR_NAME",
            "HASATTR_NAME",
            "GETATTR_SPECIAL_OBJ",
            "GETATTR_GENERIC_OBJ",
            "GETATTR_GENERIC_PTR",
            "GETATTR_NAME_DEFAULT",
            "GUARDED_GETATTR",
            "MODULE_GET_ATTR",
        }:
            return "attr"
        return "indexable"

    def _is_uncertain_heap_boundary(self, op_kind: str) -> bool:
        return op_kind in {
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "INVOKE_FFI",
        }

    def _heap_alias_classes_for_write_op(
        self, op: MoltOp, value_type_tags: dict[str, int]
    ) -> set[str]:
        if op.kind in {
            "DICT_SET",
            "DICT_STR_INT_INC",
            "DICT_SPLIT_COUNT_INT_INC",
            "DICT_SETDEFAULT",
            "DICT_POP",
            "DICT_POPITEM",
            "DICT_CLEAR",
            "DICT_UPDATE",
            "DICT_UPDATE_KWSTAR",
        }:
            return {"dict", "indexable"}
        if op.kind in {
            "LIST_APPEND",
            "LIST_EXTEND",
            "LIST_POP",
            "LIST_REMOVE",
            "LIST_INSERT",
            "LIST_CLEAR",
            "LIST_REVERSE",
        }:
            return {"list", "indexable"}
        if op.kind in {
            "STORE_ATTR",
            "SET_ATTR",
            "SETATTR",
            "SETATTR_INIT",
            "SETATTR_GENERIC_OBJ",
            "SETATTR_GENERIC_PTR",
            "GUARDED_SETATTR",
            "GUARDED_SETATTR_INIT",
            "DEL_ATTR",
            "DELATTR",
            "SETATTR_NAME",
            "DELATTR_NAME",
        }:
            return {"attr"}
        if op.kind in {"STORE_INDEX", "SET_INDEX", "DEL_INDEX"}:
            if not op.args or not isinstance(op.args[0], MoltValue):
                return {"dict", "list", "indexable"}
            type_tag = value_type_tags.get(op.args[0].name)
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return {"dict", "indexable"}
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return {"list", "indexable"}
            return {"dict", "list", "indexable"}
        return {"dict", "list", "indexable", "attr"}

    def _is_heap_read_key(self, key: tuple[Any, ...]) -> bool:
        return bool(key) and key[0] == "READ_HEAP_CLASS"

    def _heap_read_key_class(self, key: tuple[Any, ...]) -> str | None:
        if not self._is_heap_read_key(key):
            return None
        if len(key) < 2:
            return None
        read_class = key[1]
        if not isinstance(read_class, str):
            return None
        return read_class

    def _is_read_key_invalidated_by_alias_classes(
        self, key: tuple[Any, ...], alias_classes: set[str]
    ) -> bool:
        read_class = self._heap_read_key_class(key)
        if read_class is None:
            return False
        if read_class == "immutable_len":
            return False
        if read_class == "indexable":
            return bool(alias_classes.intersection({"indexable", "dict", "list"}))
        return read_class in alias_classes

    def _int_const_from_definition(
        self, name: str, definitions: dict[str, MoltOp]
    ) -> int | None:
        memo: dict[str, int | None] = {}
        visiting: set[str] = set()

        def resolve(value_name: str) -> int | None:
            if value_name in memo:
                return memo[value_name]
            if value_name in visiting:
                memo[value_name] = None
                return None
            visiting.add(value_name)
            op = definitions.get(value_name)
            resolved: int | None = None
            if op is not None:
                if op.kind in {"CONST", "CONST_BIGINT"} and op.args:
                    raw = op.args[0]
                    if isinstance(raw, int) and not isinstance(raw, bool):
                        resolved = raw
                elif op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
                    lhs = op.args[0]
                    rhs = op.args[1]
                    if isinstance(lhs, MoltValue) and isinstance(rhs, MoltValue):
                        lhs_const = resolve(lhs.name)
                        rhs_const = resolve(rhs.name)
                        if lhs_const is not None and rhs_const is not None:
                            if op.kind == "ADD":
                                resolved = lhs_const + rhs_const
                            elif op.kind == "SUB":
                                resolved = lhs_const - rhs_const
                            else:
                                resolved = lhs_const * rhs_const
                elif op.kind == "ABS" and len(op.args) == 1:
                    arg = op.args[0]
                    if isinstance(arg, MoltValue):
                        arg_const = resolve(arg.name)
                        if arg_const is not None:
                            resolved = abs(arg_const)
                elif op.kind == "PHI" and op.args:
                    phi_values: list[int] = []
                    for arg in op.args:
                        if not isinstance(arg, MoltValue):
                            phi_values = []
                            break
                        phi_const = resolve(arg.name)
                        if phi_const is None:
                            phi_values = []
                            break
                        phi_values.append(phi_const)
                    if phi_values and all(v == phi_values[0] for v in phi_values):
                        resolved = phi_values[0]
            visiting.discard(value_name)
            memo[value_name] = resolved
            return resolved

        return resolve(name)

    def _compare_int_truth(self, op_kind: str, lhs: int, rhs: int) -> bool | None:
        if op_kind == "EQ":
            return lhs == rhs
        if op_kind == "NE":
            return lhs != rhs
        if op_kind == "LT":
            return lhs < rhs
        if op_kind == "LE":
            return lhs <= rhs
        if op_kind == "GT":
            return lhs > rhs
        if op_kind == "GE":
            return lhs >= rhs
        return None

    def _detect_induction_step_from_recurrence(
        self, phi_name: str, recurrence: MoltOp, definitions: dict[str, MoltOp]
    ) -> int | None:
        if recurrence.kind not in {"ADD", "SUB"} or len(recurrence.args) != 2:
            return None
        lhs = recurrence.args[0]
        rhs = recurrence.args[1]
        if (
            isinstance(lhs, MoltValue)
            and lhs.name == phi_name
            and isinstance(rhs, MoltValue)
        ):
            rhs_const = self._int_const_from_definition(rhs.name, definitions)
            if rhs_const is None:
                return None
            if recurrence.kind == "ADD":
                return rhs_const
            return -rhs_const
        if (
            isinstance(rhs, MoltValue)
            and rhs.name == phi_name
            and isinstance(lhs, MoltValue)
            and recurrence.kind == "ADD"
        ):
            return self._int_const_from_definition(lhs.name, definitions)
        return None

    def _normalize_compare_for_induction(
        self, compare_op: str, lhs_is_iv: bool
    ) -> str | None:
        if lhs_is_iv:
            if compare_op in {"LT", "LE", "GT", "GE"}:
                return compare_op
            return None
        swapped = {
            "LT": "GT",
            "LE": "GE",
            "GT": "LT",
            "GE": "LE",
        }
        return swapped.get(compare_op)

    def _prove_monotonic_loop_compare(self, fact: LoopBoundFact) -> bool | None:
        start = fact.start
        step = fact.step
        bound = fact.bound
        compare_op = fact.compare_op

        if step == 0:
            return self._compare_int_truth(compare_op, start, bound)

        if step > 0:
            if compare_op == "LT" and start >= bound:
                return False
            if compare_op == "LE" and start > bound:
                return False
            if compare_op == "GT" and start > bound:
                return True
            if compare_op == "GE" and start >= bound:
                return True
            if compare_op == "EQ" and start > bound:
                return False
            if compare_op == "NE" and start > bound:
                return True
            return None

        if compare_op == "LT" and start < bound:
            return True
        if compare_op == "LE" and start <= bound:
            return True
        if compare_op == "GT" and start <= bound:
            return False
        if compare_op == "GE" and start < bound:
            return False
        if compare_op == "EQ" and start < bound:
            return False
        if compare_op == "NE" and start < bound:
            return True
        return None

    def _analyze_loop_bound_facts(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[int, LoopBoundFact]:
        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        loop_bound_facts: dict[int, LoopBoundFact] = {}

        def resolve_affine_iv_term(
            value: MoltValue,
            induction: dict[str, tuple[int, int]],
            *,
            seen: set[str] | None = None,
        ) -> tuple[str, int] | None:
            if value.name in induction:
                return value.name, 0
            if seen is None:
                seen = set()
            if value.name in seen:
                return None
            next_seen = set(seen)
            next_seen.add(value.name)
            def_op = definitions.get(value.name)
            if (
                def_op is None
                or def_op.kind not in {"ADD", "SUB"}
                or len(def_op.args) != 2
            ):
                return None
            lhs = def_op.args[0]
            rhs = def_op.args[1]
            if isinstance(lhs, MoltValue):
                lhs_term = resolve_affine_iv_term(lhs, induction, seen=next_seen)
            else:
                lhs_term = None
            if isinstance(rhs, MoltValue):
                rhs_term = resolve_affine_iv_term(rhs, induction, seen=next_seen)
            else:
                rhs_term = None
            if lhs_term is not None and isinstance(rhs, MoltValue):
                c = self._int_const_from_definition(rhs.name, definitions)
                if c is None:
                    return None
                if def_op.kind == "SUB":
                    c = -c
                return lhs_term[0], lhs_term[1] + c
            if (
                rhs_term is not None
                and isinstance(lhs, MoltValue)
                and def_op.kind == "ADD"
            ):
                c = self._int_const_from_definition(lhs.name, definitions)
                if c is None:
                    return None
                return rhs_term[0], rhs_term[1] + c
            return None

        for loop_start, loop_end in cfg.control.loop_start_to_end.items():
            if loop_end <= loop_start:
                continue

            induction_by_phi: dict[str, tuple[int, int]] = {}
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.kind != "PHI" or not op.args or op.result.name == "none":
                    continue
                phi_name = op.result.name
                start_value: int | None = None
                step_value: int | None = None
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        continue
                    recurrence = definitions.get(arg.name)
                    if recurrence is not None:
                        step = self._detect_induction_step_from_recurrence(
                            phi_name, recurrence, definitions
                        )
                        if step is not None:
                            step_value = step
                            continue
                    start_candidate = self._int_const_from_definition(
                        arg.name, definitions
                    )
                    if start_candidate is not None:
                        start_value = start_candidate
                if step_value is not None and start_value is not None:
                    induction_by_phi[phi_name] = (start_value, step_value)

            if not induction_by_phi:
                continue

            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if (
                    op.kind not in {"LT", "LE", "GT", "GE", "EQ", "NE"}
                    or len(op.args) != 2
                ):
                    continue
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    continue

                iv_name: str | None = None
                bound_value: int | None = None
                normalized_op: str | None = None
                lhs_term = resolve_affine_iv_term(lhs, induction_by_phi)
                rhs_term = resolve_affine_iv_term(rhs, induction_by_phi)
                if lhs_term is not None and rhs_term is None:
                    iv_name = lhs_term[0]
                    rhs_bound = self._int_const_from_definition(rhs.name, definitions)
                    if rhs_bound is None:
                        continue
                    bound_value = rhs_bound - lhs_term[1]
                    normalized_op = self._normalize_compare_for_induction(
                        op.kind, lhs_is_iv=True
                    )
                elif rhs_term is not None and lhs_term is None:
                    iv_name = rhs_term[0]
                    lhs_bound = self._int_const_from_definition(lhs.name, definitions)
                    if lhs_bound is None:
                        continue
                    bound_value = lhs_bound - rhs_term[1]
                    normalized_op = self._normalize_compare_for_induction(
                        op.kind, lhs_is_iv=False
                    )
                else:
                    continue

                if iv_name is None or bound_value is None or normalized_op is None:
                    continue
                start_value, step_value = induction_by_phi[iv_name]
                if op.result.name == "none":
                    continue
                loop_bound_facts[idx] = LoopBoundFact(
                    iv_name=iv_name,
                    start=start_value,
                    step=step_value,
                    bound=bound_value,
                    compare_op=normalized_op,
                    compare_index=idx,
                    compare_result=op.result.name,
                )

        return loop_bound_facts

    def _analyze_affine_loop_compare_truth(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[int, bool]:
        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        compare_truth: dict[int, bool] = {}

        def resolve_affine_iv_term(
            value: MoltValue,
            induction: dict[str, tuple[int, int]],
            *,
            seen: set[str] | None = None,
        ) -> tuple[str, int] | None:
            if value.name in induction:
                return value.name, 0
            if seen is None:
                seen = set()
            if value.name in seen:
                return None
            next_seen = set(seen)
            next_seen.add(value.name)
            def_op = definitions.get(value.name)
            if (
                def_op is None
                or def_op.kind not in {"ADD", "SUB"}
                or len(def_op.args) != 2
            ):
                return None
            lhs = def_op.args[0]
            rhs = def_op.args[1]
            lhs_term = (
                resolve_affine_iv_term(lhs, induction, seen=next_seen)
                if isinstance(lhs, MoltValue)
                else None
            )
            rhs_term = (
                resolve_affine_iv_term(rhs, induction, seen=next_seen)
                if isinstance(rhs, MoltValue)
                else None
            )
            if lhs_term is not None and isinstance(rhs, MoltValue):
                rhs_const = self._int_const_from_definition(rhs.name, definitions)
                if rhs_const is None:
                    return None
                if def_op.kind == "SUB":
                    rhs_const = -rhs_const
                return lhs_term[0], lhs_term[1] + rhs_const
            if (
                rhs_term is not None
                and isinstance(lhs, MoltValue)
                and def_op.kind == "ADD"
            ):
                lhs_const = self._int_const_from_definition(lhs.name, definitions)
                if lhs_const is None:
                    return None
                return rhs_term[0], rhs_term[1] + lhs_const
            return None

        for loop_start, loop_end in cfg.control.loop_start_to_end.items():
            if loop_end <= loop_start:
                continue

            induction_by_phi: dict[str, tuple[int, int]] = {}
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.kind != "PHI" or not op.args or op.result.name == "none":
                    continue
                phi_name = op.result.name
                start_value: int | None = None
                step_value: int | None = None
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        continue
                    recurrence = definitions.get(arg.name)
                    if recurrence is not None:
                        step = self._detect_induction_step_from_recurrence(
                            phi_name, recurrence, definitions
                        )
                        if step is not None:
                            step_value = step
                            continue
                    start_candidate = self._int_const_from_definition(
                        arg.name, definitions
                    )
                    if start_candidate is not None:
                        start_value = start_candidate
                if step_value is not None and start_value is not None:
                    induction_by_phi[phi_name] = (start_value, step_value)

            if not induction_by_phi:
                continue

            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if (
                    op.kind not in {"LT", "LE", "GT", "GE", "EQ", "NE"}
                    or len(op.args) != 2
                ):
                    continue
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    continue
                lhs_term = resolve_affine_iv_term(lhs, induction_by_phi)
                rhs_term = resolve_affine_iv_term(rhs, induction_by_phi)
                if lhs_term is None or rhs_term is None:
                    continue
                if lhs_term[0] != rhs_term[0]:
                    continue
                proven = self._compare_int_truth(op.kind, lhs_term[1], rhs_term[1])
                if isinstance(proven, bool):
                    compare_truth[idx] = proven

        return compare_truth

    def _analyze_loop_induction_steps(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[str, int]:
        induction_steps: dict[str, int] = {}
        for fact in self._analyze_loop_bound_facts(ops, cfg).values():
            induction_steps.setdefault(fact.iv_name, fact.step)
        if induction_steps:
            return induction_steps

        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        for op in ops:
            if op.kind != "PHI" or not op.args or op.result.name == "none":
                continue
            phi_name = op.result.name
            for arg in op.args:
                if not isinstance(arg, MoltValue):
                    continue
                recurrence = definitions.get(arg.name)
                if recurrence is None:
                    continue
                step = self._detect_induction_step_from_recurrence(
                    phi_name, recurrence, definitions
                )
                if step is not None:
                    induction_steps[phi_name] = step
                    break
        return induction_steps

    def _value_number_key_for_op(
        self,
        op: MoltOp,
        const_int_values: dict[str, int],
        value_type_tags: dict[str, int],
        induction_steps: dict[str, int],
        *,
        alias_epochs: dict[str, int],
        object_epochs: dict[str, int],
        memory_epoch: int,
    ) -> tuple[Any, ...] | None:
        if not self._is_cse_eligible_op(op.kind):
            return None
        effect_class = self._op_effect_class(op.kind)

        const_key = self._const_cache_key_for_op(op)
        if const_key is not None:
            return ("CONST",) + const_key

        if op.kind == "IS" and len(op.args) == 2:
            lhs = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs is not None and rhs is not None:
                return ("IS", lhs, rhs)

        if op.kind == "TYPE_OF" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                if effect_class == "reads_heap":
                    return ("READ_HEAP", memory_epoch, "TYPE_OF", arg)
                return ("TYPE_OF", arg)

        if op.kind == "NOT" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                return ("NOT", arg)

        if op.kind == "ABS" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                return ("ABS", arg)

        if op.kind in {"AND", "OR"} and len(op.args) == 2:
            lhs = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs is not None and rhs is not None:
                return ("BOOL_BINOP", op.kind, lhs, rhs)

        if (
            op.kind in {"EQ", "NE", "LT", "LE", "GT", "GE", "STRING_EQ"}
            and len(op.args) == 2
        ):
            lhs_key = self._normalize_operand_key_for_value_numbering(
                op.args[0], const_int_values
            )
            rhs_key = self._normalize_operand_key_for_value_numbering(
                op.args[1], const_int_values
            )
            if lhs_key is None or rhs_key is None:
                return None
            if op.kind in {"EQ", "NE", "STRING_EQ"} and rhs_key < lhs_key:
                lhs_key, rhs_key = rhs_key, lhs_key
            return ("CMP_PURE", op.kind, lhs_key, rhs_key)

        if op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
            lhs_key = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs_key = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs_key is None or rhs_key is None:
                return None

            if op.kind in {"ADD", "MUL"} and rhs_key < lhs_key:
                lhs_key, rhs_key = rhs_key, lhs_key

            lhs = op.args[0]
            rhs = op.args[1]
            if (
                op.kind in {"ADD", "SUB"}
                and isinstance(lhs, MoltValue)
                and isinstance(rhs, MoltValue)
                and lhs.name in induction_steps
                and rhs.name in const_int_values
            ):
                return (
                    "INDUCT_ARITH",
                    op.kind,
                    lhs.name,
                    induction_steps[lhs.name],
                    const_int_values[rhs.name],
                )

            return ("ARITH_PURE", op.kind, lhs_key, rhs_key)
        if effect_class == "reads_heap":
            normalized_args: list[tuple[str, Any]] = []
            for arg in op.args:
                key = self._normalize_operand_key_for_value_numbering(
                    arg, const_int_values
                )
                if key is None:
                    return None
                normalized_args.append(key)
            read_alias_class = self._heap_alias_class_for_read_op(op, value_type_tags)
            if read_alias_class is None:
                return None
            object_epoch = 0
            if op.args and isinstance(op.args[0], MoltValue):
                object_epoch = object_epochs.get(op.args[0].name, 0)
            if read_alias_class == "immutable_len":
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            class_epoch = alias_epochs.get(read_alias_class, 0)
            if read_alias_class in {"dict", "list"}:
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    class_epoch,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            if read_alias_class == "indexable":
                indexable_epoch = alias_epochs.get("indexable", 0)
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    indexable_epoch,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            return (
                "READ_HEAP_CLASS",
                read_alias_class,
                class_epoch,
                object_epoch,
                memory_epoch,
                op.kind,
                tuple(normalized_args),
            )
        return None

    def _kill_value_in_canonicalization_state(
        self, state: CanonicalizationState, name: str
    ) -> None:
        aliases: dict[str, MoltValue] = state["aliases"]
        aliases.pop(name, None)
        stale_aliases = [key for key, value in aliases.items() if value.name == name]
        for key in stale_aliases:
            aliases.pop(key, None)

        state["const_int_values"].pop(name, None)
        state["value_type_tags"].pop(name, None)

        available_values: dict[tuple[Any, ...], MoltValue] = state["available_values"]
        stale_values = [
            key for key, value in available_values.items() if value.name == name
        ]
        for key in stale_values:
            available_values.pop(key, None)

        guard_dict_shapes: dict[str, tuple[str, str]] = state["guard_dict_shapes"]
        guard_dict_shapes.pop(name, None)
        stale_dict_shapes = [
            key
            for key, (dict_type_name, version_name) in guard_dict_shapes.items()
            if dict_type_name == name or version_name == name
        ]
        for key in stale_dict_shapes:
            guard_dict_shapes.pop(key, None)
        object_epochs: dict[str, int] = state["object_epochs"]
        object_epochs.pop(name, None)
        self._invalidate_canonicalization_state_signature(state)

    def _intersect_canonicalization_state(
        self, left: CanonicalizationState, right: CanonicalizationState
    ) -> CanonicalizationState:
        aliases: dict[str, MoltValue] = {}
        for key, left_value in left["aliases"].items():
            right_value = right["aliases"].get(key)
            if (
                isinstance(right_value, MoltValue)
                and right_value.name == left_value.name
            ):
                aliases[key] = left_value

        const_int_values: dict[str, int] = {}
        for key, left_value in left["const_int_values"].items():
            right_value = right["const_int_values"].get(key)
            if isinstance(right_value, int) and right_value == left_value:
                const_int_values[key] = left_value

        value_type_tags: dict[str, int] = {}
        for key, left_value in left["value_type_tags"].items():
            right_value = right["value_type_tags"].get(key)
            if isinstance(right_value, int) and right_value == left_value:
                value_type_tags[key] = left_value

        available_values: dict[tuple[Any, ...], MoltValue] = {}
        for key, left_value in left["available_values"].items():
            right_value = right["available_values"].get(key)
            if (
                isinstance(right_value, MoltValue)
                and right_value.name == left_value.name
            ):
                available_values[key] = left_value

        guard_dict_shapes: dict[str, tuple[str, str]] = {}
        for key, left_value in left["guard_dict_shapes"].items():
            right_value = right["guard_dict_shapes"].get(key)
            if (
                isinstance(right_value, tuple)
                and len(right_value) == 2
                and right_value == left_value
            ):
                guard_dict_shapes[key] = left_value

        alias_epochs: dict[str, int] = {}
        left_alias_epochs = left["alias_epochs"]
        right_alias_epochs = right["alias_epochs"]
        for key in set(left_alias_epochs.keys()).union(right_alias_epochs.keys()):
            alias_epochs[key] = max(
                int(left_alias_epochs.get(key, 0)),
                int(right_alias_epochs.get(key, 0)),
            )

        object_epochs: dict[str, int] = {}
        left_object_epochs = left["object_epochs"]
        right_object_epochs = right["object_epochs"]
        for key in set(left_object_epochs.keys()).union(right_object_epochs.keys()):
            object_epochs[key] = max(
                int(left_object_epochs.get(key, 0)),
                int(right_object_epochs.get(key, 0)),
            )

        return {
            "aliases": aliases,
            "const_int_values": const_int_values,
            "value_type_tags": value_type_tags,
            "available_values": available_values,
            "guard_dict_shapes": guard_dict_shapes,
            "alias_epochs": alias_epochs,
            "object_epochs": object_epochs,
            "memory_epoch": max(left["memory_epoch"], right["memory_epoch"]),
        }

    def _intersect_canonicalization_states(
        self, states: list[CanonicalizationState]
    ) -> CanonicalizationState:
        if not states:
            return self._empty_canonicalization_state()
        merged = self._clone_canonicalization_state(states[0])
        for state in states[1:]:
            merged = self._intersect_canonicalization_state(merged, state)
        return merged

    def _canonicalization_state_signature(
        self, state: CanonicalizationState
    ) -> tuple[
        tuple[tuple[str, str], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[tuple[Any, ...], str], ...],
        tuple[tuple[str, tuple[str, str]], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[str, int], ...],
        int,
    ]:
        cached_signature = cast(Any, state).get(
            _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY
        )
        if cached_signature is not None:
            return cast(
                tuple[
                    tuple[tuple[str, str], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[tuple[Any, ...], str], ...],
                    tuple[tuple[str, tuple[str, str]], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[str, int], ...],
                    int,
                ],
                cached_signature,
            )
        alias_items = tuple(
            sorted((key, value.name) for key, value in state["aliases"].items())
        )
        const_items = tuple(sorted(state["const_int_values"].items()))
        tag_items = tuple(sorted(state["value_type_tags"].items()))
        available_items = tuple(
            sorted(
                (key, value.name) for key, value in state["available_values"].items()
            )
        )
        dict_shape_items = tuple(sorted(state["guard_dict_shapes"].items()))
        alias_epoch_items = tuple(sorted(state["alias_epochs"].items()))
        object_epoch_items = tuple(sorted(state["object_epochs"].items()))
        memory_epoch = state["memory_epoch"]
        signature = (
            alias_items,
            const_items,
            tag_items,
            available_items,
            dict_shape_items,
            alias_epoch_items,
            object_epoch_items,
            memory_epoch,
        )
        cast(Any, state)[_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY] = signature
        return signature

    def _canonicalize_block_with_state(
        self,
        ops: list[MoltOp],
        in_state: CanonicalizationState,
        *,
        induction_steps: dict[str, int],
    ) -> tuple[list[MoltOp], CanonicalizationState]:
        func_stats = self._midend_function_stats()
        state = self._clone_canonicalization_state(in_state)
        aliases: dict[str, MoltValue] = state["aliases"]
        const_int_values: dict[str, int] = state["const_int_values"]
        value_type_tags: dict[str, int] = state["value_type_tags"]
        available_values: dict[tuple[Any, ...], MoltValue] = state["available_values"]
        guard_dict_shapes: dict[str, tuple[str, str]] = state["guard_dict_shapes"]
        alias_epochs: dict[str, int] = state["alias_epochs"]
        object_epochs: dict[str, int] = state["object_epochs"]
        memory_epoch = state["memory_epoch"]
        state_dirty = False

        out: list[MoltOp] = []
        for op in ops:
            canonical_args = [
                self._rewrite_aliases_in_arg(arg, aliases) for arg in op.args
            ]
            canonical_op = MoltOp(
                kind=op.kind,
                args=canonical_args,
                result=op.result,
                metadata=op.metadata,
                source_line=op.source_line,
                col_offset=op.col_offset,
                end_col_offset=op.end_col_offset,
            )

            result_name = canonical_op.result.name
            if result_name != "none":
                self._kill_value_in_canonicalization_state(state, result_name)

            if canonical_op.kind == "PHI" and canonical_op.args:
                phi_args = canonical_op.args
                if all(
                    isinstance(arg, MoltValue) and arg.name == phi_args[0].name
                    for arg in phi_args
                ):
                    shared = self._resolve_alias_value(phi_args[0], aliases)
                    aliases[result_name] = shared
                    state_dirty = True
                    if shared.name in const_int_values:
                        const_int_values[result_name] = const_int_values[shared.name]
                    if shared.name in value_type_tags:
                        value_type_tags[result_name] = value_type_tags[shared.name]
                    continue

            if canonical_op.kind == "GUARD_DICT_SHAPE" and len(canonical_op.args) == 3:
                guarded_obj = canonical_op.args[0]
                dict_type = canonical_op.args[1]
                version = canonical_op.args[2]
                if (
                    isinstance(guarded_obj, MoltValue)
                    and isinstance(dict_type, MoltValue)
                    and isinstance(version, MoltValue)
                ):
                    expected = (dict_type.name, version.name)
                    if guard_dict_shapes.get(guarded_obj.name) == expected:
                        continue

            if canonical_op.kind == "GUARD_TAG" and len(canonical_op.args) == 2:
                guarded = canonical_op.args[0]
                expected = canonical_op.args[1]
                if isinstance(guarded, MoltValue) and isinstance(expected, MoltValue):
                    actual_tag = value_type_tags.get(guarded.name)
                    expected_tag = const_int_values.get(expected.name)
                    if actual_tag is not None and expected_tag == actual_tag:
                        continue

            value_key = self._value_number_key_for_op(
                canonical_op,
                const_int_values,
                value_type_tags,
                induction_steps,
                alias_epochs=alias_epochs,
                object_epochs=object_epochs,
                memory_epoch=memory_epoch,
            )
            effect_class = self._op_effect_class(canonical_op.kind)
            if (
                effect_class == "reads_heap"
                and value_key is not None
                and result_name != "none"
            ):
                func_stats["cse_readheap_attempted"] += 1
            if value_key is not None and result_name != "none":
                cached = available_values.get(value_key)
                if cached is not None:
                    shared = self._resolve_alias_value(cached, aliases)
                    aliases[result_name] = shared
                    self.midend_stats["gvn_hits"] += 1
                    if effect_class == "reads_heap":
                        func_stats["cse_readheap_accepted"] += 1
                    if shared.name in const_int_values:
                        const_int_values[result_name] = const_int_values[shared.name]
                    if shared.name in value_type_tags:
                        value_type_tags[result_name] = value_type_tags[shared.name]
                    continue
                if effect_class == "reads_heap":
                    func_stats["cse_readheap_rejected"] += 1

            # Constant-fold arithmetic: when both operands are known
            # constants, compute the result.  If it overflows the
            # 47-bit signed inline range, replace with CONST_BIGINT
            # to prevent Cranelift 0.130 constant-folding miscompilation.
            _folded_to_bigint = False
            if (
                canonical_op.kind in {"ADD", "SUB", "MUL", "POW"}
                and len(canonical_op.args) == 2
            ):
                lhs, rhs = canonical_op.args
                if isinstance(lhs, MoltValue) and isinstance(rhs, MoltValue):
                    lhs_const = const_int_values.get(lhs.name)
                    rhs_const = const_int_values.get(rhs.name)
                    if lhs_const is not None and rhs_const is not None:
                        if canonical_op.kind == "ADD":
                            folded = lhs_const + rhs_const
                        elif canonical_op.kind == "SUB":
                            folded = lhs_const - rhs_const
                        elif canonical_op.kind == "MUL":
                            folded = lhs_const * rhs_const
                        else:
                            # POW – only fold for small non-negative exponents
                            # to avoid float results and unbounded computation.
                            if 0 <= rhs_const <= 64:
                                folded = lhs_const**rhs_const
                            else:
                                folded = None
                        if folded is None:
                            # Skip folding (e.g. negative exponent)
                            out.append(canonical_op)
                            continue
                        if not (_INLINE_INT_MIN <= folded <= _INLINE_INT_MAX):
                            canonical_op = MoltOp(
                                kind="CONST_BIGINT",
                                args=[str(folded)],
                                result=canonical_op.result,
                                source_line=canonical_op.source_line,
                                col_offset=canonical_op.col_offset,
                                end_col_offset=canonical_op.end_col_offset,
                            )
                            _folded_to_bigint = True
                        elif canonical_op.kind == "POW":
                            canonical_op = MoltOp(
                                kind="CONST",
                                args=[folded],
                                result=canonical_op.result,
                                source_line=canonical_op.source_line,
                                col_offset=canonical_op.col_offset,
                                end_col_offset=canonical_op.end_col_offset,
                            )
                        const_int_values[result_name] = folded
                        state_dirty = True

            # Constant-fold bitwise operations: when both operands are
            # known integer constants, compute the result at compile time.
            if (
                not _folded_to_bigint
                and canonical_op.kind
                in {
                    "BIT_AND",
                    "BIT_OR",
                    "BIT_XOR",
                    "LSHIFT",
                    "RSHIFT",
                    "INVERT",
                }
                and len(canonical_op.args) >= 1
            ):
                args = canonical_op.args
                if canonical_op.kind == "INVERT" and len(args) == 1:
                    arg = args[0]
                    if isinstance(arg, MoltValue):
                        arg_const = const_int_values.get(arg.name)
                        if arg_const is not None:
                            folded_bw = ~arg_const
                            if _INLINE_INT_MIN <= folded_bw <= _INLINE_INT_MAX:
                                const_int_values[result_name] = folded_bw
                                state_dirty = True
                elif len(args) == 2:
                    lhs_bw, rhs_bw = args
                    if isinstance(lhs_bw, MoltValue) and isinstance(rhs_bw, MoltValue):
                        lc = const_int_values.get(lhs_bw.name)
                        rc = const_int_values.get(rhs_bw.name)
                        if lc is not None and rc is not None:
                            if canonical_op.kind == "BIT_AND":
                                folded_bw = lc & rc
                            elif canonical_op.kind == "BIT_OR":
                                folded_bw = lc | rc
                            elif canonical_op.kind == "BIT_XOR":
                                folded_bw = lc ^ rc
                            elif canonical_op.kind == "LSHIFT":
                                folded_bw = lc << rc if 0 <= rc <= 128 else None
                            elif canonical_op.kind == "RSHIFT":
                                folded_bw = lc >> rc if 0 <= rc <= 128 else None
                            else:
                                folded_bw = None
                            if folded_bw is not None:
                                if not (
                                    _INLINE_INT_MIN <= folded_bw <= _INLINE_INT_MAX
                                ):
                                    canonical_op = MoltOp(
                                        kind="CONST_BIGINT",
                                        args=[str(folded_bw)],
                                        result=canonical_op.result,
                                        source_line=canonical_op.source_line,
                                        col_offset=canonical_op.col_offset,
                                        end_col_offset=canonical_op.end_col_offset,
                                    )
                                    _folded_to_bigint = True
                                const_int_values[result_name] = folded_bw
                                state_dirty = True

            out.append(canonical_op)

            if not _folded_to_bigint and canonical_op.kind == "CONST":
                value = canonical_op.args[0]
                if isinstance(value, int) and not isinstance(value, bool):
                    const_int_values[result_name] = value
                    state_dirty = True
            elif canonical_op.kind == "ABS" and len(canonical_op.args) == 1:
                arg = canonical_op.args[0]
                if isinstance(arg, MoltValue):
                    arg_const = const_int_values.get(arg.name)
                    if arg_const is not None:
                        const_int_values[result_name] = abs(arg_const)
                        state_dirty = True
            elif canonical_op.kind == "GUARD_TAG" and len(canonical_op.args) == 2:
                guarded, expected = canonical_op.args
                if isinstance(guarded, MoltValue) and isinstance(expected, MoltValue):
                    expected_tag = const_int_values.get(expected.name)
                    if expected_tag is not None:
                        value_type_tags[guarded.name] = expected_tag
                        state_dirty = True
            elif (
                canonical_op.kind == "GUARD_DICT_SHAPE" and len(canonical_op.args) == 3
            ):
                guarded_obj, dict_type, version = canonical_op.args
                if (
                    isinstance(guarded_obj, MoltValue)
                    and isinstance(dict_type, MoltValue)
                    and isinstance(version, MoltValue)
                ):
                    guard_dict_shapes[guarded_obj.name] = (dict_type.name, version.name)
                    state_dirty = True
            type_tag = self._const_type_tag(canonical_op)
            if type_tag is None and result_name != "none":
                if canonical_op.kind in {
                    "NOT",
                    "IS",
                    "AND",
                    "OR",
                    "EQ",
                    "NE",
                    "LT",
                    "LE",
                    "GT",
                    "GE",
                    "STRING_EQ",
                    "ISINSTANCE",
                    "EXCEPTION_MATCH_BUILTIN",
                }:
                    type_tag = BUILTIN_TYPE_TAGS["bool"]
                elif canonical_op.kind in {"LEN", "TYPE_OF"}:
                    type_tag = BUILTIN_TYPE_TAGS["int"]
                elif canonical_op.kind == "ABS" and len(canonical_op.args) == 1:
                    abs_arg = canonical_op.args[0]
                    if isinstance(abs_arg, MoltValue):
                        abs_arg_tag = value_type_tags.get(abs_arg.name)
                        if abs_arg_tag in {
                            BUILTIN_TYPE_TAGS["int"],
                            BUILTIN_TYPE_TAGS["float"],
                        }:
                            type_tag = abs_arg_tag
                elif canonical_op.kind == "DICT_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["dict"]
                elif canonical_op.kind == "LIST_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["list"]
                elif canonical_op.kind == "TUPLE_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["tuple"]
                elif canonical_op.kind == "SET_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["set"]
                elif canonical_op.kind == "FROZENSET_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["frozenset"]
                elif canonical_op.kind == "RANGE_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["range"]
            if type_tag is not None and result_name != "none":
                value_type_tags[result_name] = type_tag
                state_dirty = True
            if canonical_op.kind == "IS" and result_name != "none":
                value_type_tags[result_name] = BUILTIN_TYPE_TAGS["bool"]
                state_dirty = True
            if value_key is not None and result_name != "none":
                available_values[value_key] = canonical_op.result
                state_dirty = True

            if self._is_canonicalization_barrier_op(canonical_op.kind):
                aliases.clear()
                const_int_values.clear()
                value_type_tags.clear()
                available_values.clear()
                guard_dict_shapes.clear()
                state_dirty = True

            if effect_class == "writes_heap":
                write_alias_classes = self._heap_alias_classes_for_write_op(
                    canonical_op, value_type_tags
                )
                if self._is_uncertain_heap_boundary(canonical_op.kind):
                    memory_epoch += 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_heap_read_key(key)
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                    for alias_class in sorted(alias_epochs):
                        alias_epochs[alias_class] = alias_epochs.get(alias_class, 0) + 1
                    guard_dict_shapes.clear()
                    state_dirty = True
                    continue
                if canonical_op.args and isinstance(canonical_op.args[0], MoltValue):
                    obj_name = canonical_op.args[0].name
                    object_epochs[obj_name] = object_epochs.get(obj_name, 0) + 1
                    state_dirty = True
                if write_alias_classes:
                    for alias_class in sorted(write_alias_classes):
                        alias_epochs[alias_class] = alias_epochs.get(alias_class, 0) + 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_read_key_invalidated_by_alias_classes(
                            key, write_alias_classes
                        )
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                else:
                    memory_epoch += 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_heap_read_key(key)
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                guard_dict_shapes.clear()
                state_dirty = True

        state["alias_epochs"] = alias_epochs
        state["object_epochs"] = object_epochs
        state["memory_epoch"] = memory_epoch
        if state_dirty:
            self._invalidate_canonicalization_state_signature(state)
        return out, state

    def _compute_block_use_def(self, ops: list[MoltOp]) -> tuple[set[str], set[str]]:
        use: set[str] = set()
        defs: set[str] = set()
        for op in ops:
            arg_names: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, arg_names)
            use.update(name for name in arg_names if name not in defs)
            out_name = op.result.name
            if out_name != "none":
                defs.add(out_name)
        return use, defs

    def _find_unbound_value_uses(
        self, ops: list[MoltOp], *, params: Sequence[str] = ()
    ) -> list[tuple[int, str, str]]:
        defined: set[str] = set(params)
        defined.update(self._collect_defined_value_names(ops))
        missing: list[tuple[int, str, str]] = []
        for idx, op in enumerate(ops):
            used_names: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, used_names)
            for name in sorted(used_names):
                if name != "none" and name not in defined:
                    missing.append((idx, op.kind, name))
        return missing

    def _infer_predefined_value_names(self, ops: list[MoltOp]) -> set[str]:
        used: set[str] = set()
        for op in ops:
            for arg in op.args:
                self._collect_arg_value_names(arg, used)
        defined = self._collect_defined_value_names(ops)
        return used - defined

    def _verify_definite_assignment_in_ops(
        self,
        ops: list[MoltOp],
        *,
        predefined_value_names: set[str] | None = None,
    ) -> list[tuple[int, str, str]]:
        if not ops:
            return []

        predefined = set(predefined_value_names or set())
        cfg: CFGGraph = build_cfg(ops)
        if not cfg.blocks:
            return []
        all_defs = self._collect_defined_value_names(ops).union(predefined)

        # Track which value names are produced by MISSING ops so we can
        # verify they haven't been eliminated by a prior pass.
        missing_value_defs: set[str] = set()
        for op in ops:
            if op.kind == "MISSING" and op.result.name != "none":
                missing_value_defs.add(op.result.name)

        # Propagate MISSING taint transitively through PHI nodes: if every
        # input to a PHI is MISSING-tainted, the PHI result is also tainted.
        # This catches cases where branch pruning collapses a PHI to a single
        # MISSING-carrying input that escapes into CALL arg positions.
        missing_tainted: set[str] = set(missing_value_defs)
        _phi_changed = True
        while _phi_changed:
            _phi_changed = False
            for op in ops:
                if op.kind != "PHI" or not op.args:
                    continue
                out_name = op.result.name
                if out_name == "none" or out_name in missing_tainted:
                    continue
                phi_value_args = [arg for arg in op.args if isinstance(arg, MoltValue)]
                if phi_value_args and all(
                    arg.name in missing_tainted for arg in phi_value_args
                ):
                    missing_tainted.add(out_name)
                    _phi_changed = True

        block_defs: dict[int, set[str]] = {}
        for block in cfg.blocks:
            defs: set[str] = set()
            for op in ops[block.start : block.end]:
                out_name = op.result.name
                if out_name != "none":
                    defs.add(out_name)
            block_defs[block.id] = defs

        in_defs: dict[int, set[str]] = {}
        out_defs: dict[int, set[str]] = {}
        for block_id in range(len(cfg.blocks)):
            if block_id == 0:
                initial = set(predefined)
            elif block_id in cfg.reachable:
                initial = set(all_defs)
            else:
                initial = set()
            in_defs[block_id] = initial
            out_defs[block_id] = initial.union(block_defs[block_id])

        changed = True
        while changed:
            changed = False
            for block_id in range(1, len(cfg.blocks)):
                if block_id not in cfg.reachable:
                    continue
                preds = [
                    pred
                    for pred in cfg.predecessors.get(block_id, [])
                    if pred in cfg.reachable
                ]
                if not preds:
                    new_in = set(predefined)
                else:
                    new_in = set.intersection(*(out_defs[pred] for pred in preds))
                new_out = new_in.union(block_defs[block_id])
                if new_in != in_defs[block_id] or new_out != out_defs[block_id]:
                    in_defs[block_id] = new_in
                    out_defs[block_id] = new_out
                    changed = True

        failures: list[tuple[int, str, str]] = []
        definition_index: dict[str, int] = {}
        definition_block: dict[str, int] = {}
        for op_idx, op in enumerate(ops):
            out_name = op.result.name
            if out_name == "none":
                continue
            if out_name in definition_index:
                failures.append((op_idx, op.kind, out_name))
                continue
            definition_index[out_name] = op_idx
            definition_block[out_name] = cfg.index_to_block[op_idx]

        # Collect which value names are consumed by GETATTR/CALL/LOOKUP ops
        # as default or sentinel arguments — these are the critical consumers
        # of MISSING sentinels.
        _missing_sentinel_consumer_ops = {
            "GETATTR_NAME_DEFAULT",
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "DICT_UPDATE_MISSING",
        }

        for block in cfg.blocks:
            block_id = block.id
            if block_id not in cfg.reachable:
                continue
            local_defs = set(in_defs[block_id])
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                used: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, used)
                missing = sorted(name for name in used if name not in local_defs)
                for name in missing:
                    failures.append((op_idx, op.kind, name))
                for name in sorted(used):
                    if name in predefined:
                        continue
                    def_idx = definition_index.get(name)
                    if def_idx is None:
                        # Value is used but has no definition at all — if it
                        # was originally a MISSING sentinel that got removed,
                        # flag this as a failure.
                        if name in missing_value_defs:
                            failures.append((op_idx, op.kind, name))
                        continue
                    def_block = definition_block[name]
                    if def_block not in cfg.dominators.get(block_id, set()):
                        failures.append((op_idx, op.kind, name))
                        continue
                    if def_block == block_id and def_idx >= op_idx:
                        failures.append((op_idx, op.kind, name))
                # Extra check: ops that consume MISSING-produced values
                # (sentinel consumers) must have those definitions still
                # present and dominating.
                if op.kind in _missing_sentinel_consumer_ops:
                    for arg in op.args:
                        if (
                            isinstance(arg, MoltValue)
                            and arg.name in missing_value_defs
                        ):
                            if arg.name not in local_defs:
                                failures.append((op_idx, op.kind, arg.name))
                # Transitive MISSING taint check: if a CALL/CALL_INDIRECT
                # arg is MISSING-tainted through a PHI collapse (not a direct
                # MISSING def), that means an uninitialized variable leaked
                # into a call site after branch pruning.
                if op.kind in {"CALL", "CALL_INDIRECT", "CALL_INTERNAL"}:
                    for arg in op.args:
                        if isinstance(arg, MoltValue) and (
                            arg.name in missing_tainted
                            and arg.name not in missing_value_defs
                        ):
                            failures.append((op_idx, op.kind, arg.name))
                out_name = op.result.name
                if out_name != "none":
                    local_defs.add(out_name)
        return failures

    def _dead_op_lattice_class(self, op_kind: str) -> str:
        effect = self._op_effect_class(op_kind)
        if effect == "control":
            return "protected"
        if effect == "pure":
            return "pure"
        if effect in {"reads_heap", "writes_heap"}:
            return effect
        return "unknown"

    def _eliminate_dead_trivial_consts(self, ops: list[MoltOp]) -> list[MoltOp]:
        if not ops:
            return []

        func_stats = self._midend_function_stats()
        cfg: CFGGraph = build_cfg(ops)
        if not cfg.blocks:
            return []

        def normalize_anchor_arg(value: Any) -> Any:
            if isinstance(value, MoltValue):
                return ("v", value.name)
            if isinstance(value, tuple):
                return ("t", tuple(normalize_anchor_arg(item) for item in value))
            if isinstance(value, list):
                return ("l", tuple(normalize_anchor_arg(item) for item in value))
            if isinstance(value, dict):
                return (
                    "d",
                    tuple(
                        sorted(
                            (
                                normalize_anchor_arg(key),
                                normalize_anchor_arg(item),
                            )
                            for key, item in value.items()
                        )
                    ),
                )
            try:
                hash(value)
                return ("c", value)
            except TypeError:
                return ("r", repr(value))

        def anchor_key(op: MoltOp) -> tuple[Any, ...] | None:
            out_name = op.result.name
            if out_name == "none":
                return None
            if self._dead_op_lattice_class(op.kind) != "pure":
                return None
            return (op.kind, tuple(normalize_anchor_arg(arg) for arg in op.args))

        anchor_first_result: dict[tuple[Any, ...], str] = {}
        anchor_counts: dict[tuple[Any, ...], int] = {}
        for op in ops:
            key = anchor_key(op)
            if key is None:
                continue
            anchor_counts[key] = anchor_counts.get(key, 0) + 1
            anchor_first_result.setdefault(key, op.result.name)
        preserve_anchor_results: set[str] = {
            anchor_first_result[key]
            for key, count in anchor_counts.items()
            if count > 1 and key in anchor_first_result
        }

        pure_attempted = 0
        uses_by_index: dict[int, set[str]] = {}
        defs_by_name: dict[str, list[int]] = {}
        removable_indices: set[int] = set()
        required_values: set[str] = set()
        worklist: list[str] = []

        def require_value(name: str) -> None:
            if name == "none" or name in required_values:
                return
            required_values.add(name)
            worklist.append(name)

        for idx, op in enumerate(ops):
            out_name = op.result.name
            uses: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, uses)
            uses_by_index[idx] = uses

            lattice_class = self._dead_op_lattice_class(op.kind)
            if out_name != "none":
                defs_by_name.setdefault(out_name, []).append(idx)
                if lattice_class == "pure":
                    pure_attempted += 1
                    # MISSING ops are runtime sentinels (uninitialized locals,
                    # optional defaults) that downstream GETATTR/CALL sites
                    # depend on — never eliminate them.
                    if out_name not in preserve_anchor_results and op.kind != "MISSING":
                        removable_indices.add(idx)

        for idx, op in enumerate(ops):
            if idx in removable_indices:
                continue
            for name in uses_by_index[idx]:
                require_value(name)

        required_removable_indices: set[int] = set()
        while worklist:
            value_name = worklist.pop()
            for producer_idx in defs_by_name.get(value_name, []):
                if producer_idx not in removable_indices:
                    continue
                if producer_idx in required_removable_indices:
                    continue
                required_removable_indices.add(producer_idx)
                for dependency_name in uses_by_index[producer_idx]:
                    require_value(dependency_name)

        remove_indices = removable_indices - required_removable_indices
        pure_removed = len(remove_indices)
        removed_count = pure_removed
        out = [op for idx, op in enumerate(ops) if idx not in remove_indices]
        self.midend_stats["dce_removed_total"] += removed_count
        func_stats["dce_pure_op_attempted"] += pure_attempted
        func_stats["dce_pure_op_accepted"] += pure_removed
        func_stats["dce_pure_op_rejected"] += max(0, pure_attempted - pure_removed)
        return out

    def _op_may_raise_for_sccp(self, op_kind: str) -> bool:
        non_raising = {
            "LINE",
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
            "LABEL",
            "STATE_LABEL",
            "PHI",
            "CONST",
            "CONST_BIGINT",
            "CONST_BOOL",
            "CONST_FLOAT",
            "CONST_STR",
            "CONST_BYTES",
            "CONST_NONE",
            "CONST_NOT_IMPLEMENTED",
            "CONST_ELLIPSIS",
            "MISSING",
            "ADD",
            "SUB",
            "MUL",
            "NOT",
            "IS",
            "TYPE_OF",
            "LEN",
            "EXCEPTION_NEW_BUILTIN",
            "EXCEPTION_NEW_BUILTIN_EMPTY",
            "EXCEPTION_NEW_BUILTIN_ONE",
            "EXCEPTION_MATCH_BUILTIN",
            "STORE_VAR",
            "DELETE_VAR",
            "LOAD_VAR",
        }
        if op_kind in non_raising:
            return False
        if op_kind.startswith("STATE_"):
            return False
        return True

    def _compute_sccp(
        self,
        ops: list[MoltOp],
        cfg: CFGGraph,
        *,
        max_iters_override: int | None = None,
    ) -> SCCPResult:
        # Current contract: SCCP tracks executable edges and supplies facts for
        # conservative loop/try marker rewrites only; broader LOOP_END and
        # exceptional-handler CFG rewrites remain roadmap work and must preserve
        # dominance/post-dominance invariants.
        in_values: dict[int, dict[str, Any]] = {block.id: {} for block in cfg.blocks}
        out_values: dict[int, dict[str, Any]] = {block.id: {} for block in cfg.blocks}
        executable_blocks: set[int] = {0} if cfg.blocks else set()
        executable_edges: set[tuple[int, int]] = set()
        branch_choice_by_if_index: dict[int, bool] = {}
        loop_break_choice_by_index: dict[int, bool] = {}
        try_exception_possible_by_start: dict[int, bool] = {}
        try_normal_possible_by_start: dict[int, bool] = {}
        guard_fail_indices: set[int] = set()
        loop_bound_facts = self._analyze_loop_bound_facts(ops, cfg)
        loop_compare_truth = self._analyze_affine_loop_compare_truth(ops, cfg)
        type_of_origin: dict[str, str] = {}
        for op in ops:
            if (
                op.kind == "TYPE_OF"
                and len(op.args) == 1
                and isinstance(op.args[0], MoltValue)
                and op.result.name != "none"
            ):
                type_of_origin[op.result.name] = op.args[0].name

        def type_fact_key(name: str) -> str:
            return f"__tag__:{name}"

        def dict_shape_fact_key(name: str) -> str:
            return f"__dict_shape__:{name}"

        def is_overdefined(value: Any) -> bool:
            return value is _SCCP_OVERDEFINED

        def is_missing_sentinel(value: Any) -> bool:
            return value is _SCCP_MISSING

        def merge_lattice(left: Any, right: Any) -> Any:
            # MISSING sentinels must never fold: if either side is MISSING,
            # the merge is overdefined so downstream operations cannot
            # constant-fold through a MISSING value.
            if is_missing_sentinel(left) or is_missing_sentinel(right):
                return _SCCP_OVERDEFINED
            if left is _SCCP_UNKNOWN:
                return right
            if right is _SCCP_UNKNOWN:
                return left
            if is_overdefined(left) or is_overdefined(right):
                return _SCCP_OVERDEFINED
            if left == right:
                return left
            return _SCCP_OVERDEFINED

        def merge_states(states: list[dict[str, Any]]) -> dict[str, Any]:
            if not states:
                return {}
            merged: dict[str, Any] = {}
            all_keys: set[str] = set()
            for state in states:
                all_keys.update(state.keys())
            for key in all_keys:
                current: Any = _SCCP_UNKNOWN
                for state in states:
                    current = merge_lattice(current, state.get(key, _SCCP_UNKNOWN))
                    if is_overdefined(current):
                        break
                if current is not _SCCP_UNKNOWN:
                    merged[key] = current
            return merged

        def value_lattice(name: str, known: dict[str, Any]) -> Any:
            return known.get(name, _SCCP_UNKNOWN)

        def value_type_tag(name: str, known: dict[str, Any]) -> int | None:
            fact = known.get(type_fact_key(name))
            if isinstance(fact, int):
                return fact
            value = value_lattice(name, known)
            if (
                value is _SCCP_UNKNOWN
                or is_overdefined(value)
                or is_missing_sentinel(value)
            ):
                return None
            return self._const_type_tag_for_lattice_value(value)

        def scalar_cmp_supported(value: Any) -> bool:
            if value is None:
                return True
            if isinstance(value, bool):
                return True
            if isinstance(value, int):
                return True
            if isinstance(value, float):
                return True
            if isinstance(value, str):
                return True
            if isinstance(value, bytes):
                return True
            return False

        def eval_lattice_value(op: MoltOp, known: dict[str, Any], op_index: int) -> Any:
            # MISSING ops produce runtime sentinel values that must never be
            # constant-folded or propagated.  Return _SCCP_MISSING so that
            # any downstream consumer goes to overdefined via merge_lattice.
            if op.kind == "MISSING":
                return _SCCP_MISSING
            if op.kind == "CONST":
                return op.args[0]
            if op.kind == "CONST_BOOL":
                return bool(op.args[0])
            if op.kind == "CONST_BIGINT":
                return int(op.args[0])
            if op.kind == "CONST_FLOAT":
                return float(op.args[0])
            if op.kind == "CONST_STR":
                return str(op.args[0])
            if op.kind == "CONST_BYTES":
                return bytes(op.args[0])
            if op.kind == "CONST_NONE":
                return None
            if op.kind == "CONST_NOT_IMPLEMENTED":
                return NotImplemented
            if op.kind == "CONST_ELLIPSIS":
                return Ellipsis
            if op.kind == "PHI" and op.args:
                block_id = cfg.index_to_block.get(op_index)
                if block_id is not None:
                    block_preds = cfg.predecessors.get(block_id, [])
                    if len(block_preds) == len(op.args):
                        merged: Any = _SCCP_UNKNOWN
                        seen_exec = False
                        for arg, pred in zip(op.args, block_preds):
                            if (pred, block_id) not in executable_edges:
                                continue
                            if not isinstance(arg, MoltValue):
                                return _SCCP_OVERDEFINED
                            seen_exec = True
                            merged = merge_lattice(
                                merged, value_lattice(arg.name, known)
                            )
                            if is_overdefined(merged):
                                return _SCCP_OVERDEFINED
                        if seen_exec:
                            return merged
                        return _SCCP_UNKNOWN
                merged = _SCCP_UNKNOWN
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        return _SCCP_OVERDEFINED
                    merged = merge_lattice(merged, value_lattice(arg.name, known))
                    if is_overdefined(merged):
                        return _SCCP_OVERDEFINED
                return merged
            if op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels must never fold through arithmetic.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if (
                    isinstance(lhs_value, int)
                    and not isinstance(lhs_value, bool)
                    and isinstance(rhs_value, int)
                    and not isinstance(rhs_value, bool)
                ):
                    if op.kind == "ADD":
                        return lhs_value + rhs_value
                    if op.kind == "SUB":
                        return lhs_value - rhs_value
                    return lhs_value * rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "NOT" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value) or is_missing_sentinel(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(arg_value, bool):
                    return not arg_value
                return _SCCP_OVERDEFINED
            if op.kind == "ABS" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value) or is_missing_sentinel(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(arg_value, (int, float)):
                    return abs(arg_value)
                return _SCCP_OVERDEFINED
            if op.kind in {"AND", "OR"} and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels must never fold through boolean ops.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if isinstance(lhs_value, bool) and isinstance(rhs_value, bool):
                    if op.kind == "AND":
                        return lhs_value and rhs_value
                    return lhs_value or rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "IS" and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels are singleton objects in the lattice but
                # represent distinct runtime values — never fold identity
                # comparisons through them.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                return lhs_value is rhs_value
            if op.kind in {"EQ", "NE", "LT", "LE", "GT", "GE"} and len(op.args) == 2:
                proven_static = loop_compare_truth.get(op_index)
                if isinstance(proven_static, bool):
                    return proven_static
                loop_fact = loop_bound_facts.get(op_index)
                if loop_fact is not None:
                    proven = self._prove_monotonic_loop_compare(loop_fact)
                    if isinstance(proven, bool):
                        return proven
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if not scalar_cmp_supported(lhs_value) or not scalar_cmp_supported(
                    rhs_value
                ):
                    return _SCCP_OVERDEFINED
                try:
                    if op.kind == "EQ":
                        return lhs_value == rhs_value
                    if op.kind == "NE":
                        return lhs_value != rhs_value
                    if op.kind == "LT":
                        return lhs_value < rhs_value
                    if op.kind == "LE":
                        return lhs_value <= rhs_value
                    if op.kind == "GT":
                        return lhs_value > rhs_value
                    return lhs_value >= rhs_value
                except Exception:
                    return _SCCP_OVERDEFINED
            if op.kind == "STRING_EQ" and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if isinstance(lhs_value, str) and isinstance(rhs_value, str):
                    return lhs_value == rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "TYPE_OF" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                tag = value_type_tag(arg.name, known)
                if tag is not None:
                    return tag
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value):
                    return _SCCP_OVERDEFINED
                return _SCCP_OVERDEFINED
            if op.kind == "ISINSTANCE" and len(op.args) == 2:
                obj = op.args[0]
                classinfo = op.args[1]
                if not isinstance(obj, MoltValue) or not isinstance(
                    classinfo, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                class_value = value_lattice(classinfo.name, known)
                if class_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(class_value):
                    return _SCCP_OVERDEFINED
                obj_tag = value_type_tag(obj.name, known)
                if obj_tag is None:
                    return _SCCP_UNKNOWN
                if isinstance(class_value, int):
                    return obj_tag == class_value
                if isinstance(class_value, tuple) and all(
                    isinstance(item, int) for item in class_value
                ):
                    return obj_tag in class_value
                return _SCCP_OVERDEFINED
            if op.kind == "LEN" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(
                    arg_value, (str, bytes, tuple, list, dict, set, frozenset, range)
                ):
                    return len(arg_value)
                return _SCCP_OVERDEFINED
            if op.kind == "CONTAINS" and len(op.args) == 2:
                container = op.args[0]
                item = op.args[1]
                if not isinstance(container, MoltValue) or not isinstance(
                    item, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                container_value = value_lattice(container.name, known)
                item_value = value_lattice(item.name, known)
                if container_value is _SCCP_UNKNOWN or item_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(container_value) or is_overdefined(item_value):
                    return _SCCP_OVERDEFINED
                if not isinstance(
                    container_value,
                    (str, bytes, tuple, list, dict, set, frozenset, range),
                ):
                    return _SCCP_OVERDEFINED
                try:
                    return item_value in container_value
                except Exception:
                    return _SCCP_OVERDEFINED
            if op.kind == "INDEX" and len(op.args) == 2:
                container = op.args[0]
                index = op.args[1]
                if not isinstance(container, MoltValue) or not isinstance(
                    index, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                container_value = value_lattice(container.name, known)
                index_value = value_lattice(index.name, known)
                if container_value is _SCCP_UNKNOWN or index_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(container_value) or is_overdefined(index_value):
                    return _SCCP_OVERDEFINED
                if isinstance(container_value, (tuple, list, str, bytes, range)):
                    if isinstance(index_value, int) and not isinstance(
                        index_value, bool
                    ):
                        try:
                            return container_value[index_value]
                        except Exception:
                            return _SCCP_OVERDEFINED
                    return _SCCP_OVERDEFINED
                if isinstance(container_value, dict):
                    try:
                        if index_value in container_value:
                            return container_value[index_value]
                    except Exception:
                        return _SCCP_OVERDEFINED
                return _SCCP_OVERDEFINED
            return _SCCP_OVERDEFINED

        def evaluate_try_behavior(start_idx: int, end_idx: int) -> tuple[bool, bool]:
            known: dict[str, Any] = {}
            may_raise = False
            may_complete_normally = True
            if end_idx <= start_idx + 1:
                return False, True
            for op_idx in range(start_idx + 1, end_idx):
                op = ops[op_idx]
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
                    "LABEL",
                    "STATE_LABEL",
                }:
                    return True, True
                if op.kind in {"GUARD_TAG", "GUARD_TYPE"} and len(op.args) == 2:
                    guarded = op.args[0]
                    expected = op.args[1]
                    if isinstance(guarded, MoltValue) and isinstance(
                        expected, MoltValue
                    ):
                        expected_value = value_lattice(expected.name, known)
                        guarded_tag = value_type_tag(guarded.name, known)
                        if (
                            isinstance(expected_value, int)
                            and guarded_tag is not None
                            and guarded_tag == expected_value
                        ):
                            known[type_fact_key(guarded.name)] = expected_value
                            continue
                    return True, False
                if op.kind == "GUARD_DICT_SHAPE" and len(op.args) == 3:
                    guarded = op.args[0]
                    dict_type = op.args[1]
                    version = op.args[2]
                    if (
                        isinstance(guarded, MoltValue)
                        and isinstance(dict_type, MoltValue)
                        and isinstance(version, MoltValue)
                    ):
                        shape_key = dict_shape_fact_key(guarded.name)
                        expected = (dict_type.name, version.name)
                        known_shape = known.get(shape_key)
                        if isinstance(known_shape, tuple):
                            if known_shape == expected:
                                continue
                            return True, False
                        known[shape_key] = expected
                        continue
                    return True, True
                if op.kind in {"RAISE", "RAISE_CAUSE", "RERAISE"}:
                    return True, False
                out_name = op.result.name
                lattice_value: Any = _SCCP_UNKNOWN
                if out_name != "none":
                    known.pop(out_name, None)
                    known.pop(type_fact_key(out_name), None)
                    known.pop(dict_shape_fact_key(out_name), None)
                    lattice_value = eval_lattice_value(op, known, op_idx)
                    # Promote MISSING sentinels to overdefined in try analysis too.
                    if is_missing_sentinel(lattice_value):
                        lattice_value = _SCCP_OVERDEFINED
                    if (
                        lattice_value is not _SCCP_UNKNOWN
                        and lattice_value is not _SCCP_OVERDEFINED
                    ):
                        known[out_name] = lattice_value
                        tag = self._const_type_tag_for_lattice_value(lattice_value)
                        if tag is not None:
                            known[type_fact_key(out_name)] = tag
                if self._op_may_raise_for_sccp(op.kind):
                    if (
                        lattice_value is _SCCP_OVERDEFINED
                        or lattice_value is _SCCP_UNKNOWN
                    ):
                        may_raise = True
            return may_raise, may_complete_normally

        for try_start_idx, try_end_idx in cfg.control.try_start_to_end.items():
            may_raise, may_complete_normally = evaluate_try_behavior(
                try_start_idx, try_end_idx
            )
            try_exception_possible_by_start[try_start_idx] = may_raise
            try_normal_possible_by_start[try_start_idx] = may_complete_normally
        check_exception_try_owner: dict[int, int] = {}
        for try_start_idx, try_end_idx in cfg.control.try_start_to_end.items():
            for op_idx in range(try_start_idx + 1, try_end_idx):
                if op_idx >= len(ops) or ops[op_idx].kind != "CHECK_EXCEPTION":
                    continue
                owner = check_exception_try_owner.get(op_idx)
                if owner is None or try_start_idx > owner:
                    check_exception_try_owner[op_idx] = try_start_idx

        value_users: dict[str, set[int]] = {}
        for op_idx, op in enumerate(ops):
            block_id = cfg.index_to_block.get(op_idx)
            if block_id is None:
                continue
            for arg in op.args:
                if isinstance(arg, MoltValue):
                    value_users.setdefault(arg.name, set()).add(block_id)

        iterations = 0
        ssa_defs = sum(1 for op in ops if op.result.name != "none")
        if max_iters_override is not None and max_iters_override > 0:
            max_iterations = max_iters_override
        elif self.midend_env.sccp_iter_cap_override is not None:
            max_iterations = self.midend_env.sccp_iter_cap_override
        else:
            # Dynamic cap keeps compile-time bounded while scaling with function/CFG size.
            # Keep the default ceiling conservative so wasm builds cannot stall for
            # minutes in pathological SCCP worklists.
            cfg_edge_count = sum(len(succs) for succs in cfg.successors.values())
            max_iterations = max(
                2048,
                min(
                    131072,
                    (len(cfg.blocks) * 96) + (cfg_edge_count * 48) + (ssa_defs * 24),
                ),
            )
        func_stats = self._midend_function_stats()

        block_queue: deque[int] = deque()
        queued_blocks: set[int] = set()
        edge_queue: deque[tuple[int, int]] = deque()
        queued_edges: set[tuple[int, int]] = set()
        value_queue: deque[str] = deque()
        queued_values: set[str] = set()

        def enqueue_block(block_id: int) -> None:
            if block_id in queued_blocks:
                return
            queued_blocks.add(block_id)
            block_queue.append(block_id)

        def enqueue_edge(src: int, dst: int) -> None:
            edge = (src, dst)
            if edge in executable_edges or edge in queued_edges:
                return
            queued_edges.add(edge)
            edge_queue.append(edge)

        def enqueue_value(name: str) -> None:
            if name in queued_values:
                return
            queued_values.add(name)
            value_queue.append(name)

        if cfg.blocks:
            enqueue_block(0)

        while block_queue or edge_queue or value_queue:
            if edge_queue:
                src, dst = edge_queue.popleft()
                queued_edges.discard((src, dst))
                if (src, dst) in executable_edges:
                    continue
                executable_edges.add((src, dst))
                if dst not in executable_blocks:
                    executable_blocks.add(dst)
                enqueue_block(dst)
                continue

            if value_queue:
                value_name = value_queue.popleft()
                queued_values.discard(value_name)
                for block_id in value_users.get(value_name, ()):
                    if block_id in executable_blocks:
                        enqueue_block(block_id)
                continue

            iterations += 1
            if iterations > max_iterations:
                self.midend_stats["sccp_iteration_cap_hits"] = (
                    self.midend_stats.get("sccp_iteration_cap_hits", 0) + 1
                )
                func_stats["sccp_iteration_cap_hits"] += 1
                all_blocks = {block.id for block in cfg.blocks}
                all_edges = {
                    (src, dst) for src, succs in cfg.successors.items() for dst in succs
                }
                conservative_try = {
                    start_idx: True for start_idx in cfg.control.try_start_to_end
                }
                return SCCPResult(
                    in_values={block.id: {} for block in cfg.blocks},
                    out_values={block.id: {} for block in cfg.blocks},
                    executable_blocks=all_blocks,
                    executable_edges=all_edges,
                    branch_choice_by_if_index={},
                    loop_break_choice_by_index={},
                    try_exception_possible_by_start=conservative_try,
                    try_normal_possible_by_start=dict(conservative_try),
                    guard_fail_indices=set(),
                )

            block_id = block_queue.popleft()
            queued_blocks.discard(block_id)
            if block_id not in executable_blocks:
                continue
            block = cfg.blocks[block_id]

            if block_id == 0:
                new_in: dict[str, Any] = {}
            else:
                exec_preds = [
                    pred
                    for pred in cfg.predecessors.get(block_id, [])
                    if (pred, block_id) in executable_edges
                ]
                pred_states = [out_values[pred] for pred in exec_preds]
                new_in = merge_states(pred_states)

            if new_in != in_values[block_id]:
                in_values[block_id] = new_in

            known = dict(new_in)
            block_traps = False
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if op.kind in {"GUARD_TAG", "GUARD_TYPE"} and len(op.args) == 2:
                    guarded = op.args[0]
                    expected = op.args[1]
                    if isinstance(guarded, MoltValue) and isinstance(
                        expected, MoltValue
                    ):
                        expected_value = known.get(expected.name, _SCCP_UNKNOWN)
                        if isinstance(expected_value, int):
                            guarded_tag = value_type_tag(guarded.name, known)
                            if (
                                guarded_tag is not None
                                and guarded_tag != expected_value
                            ):
                                guard_fail_indices.add(op_idx)
                                block_traps = True
                                break
                            known[type_fact_key(guarded.name)] = expected_value
                    continue
                if op.kind == "GUARD_DICT_SHAPE" and len(op.args) == 3:
                    guarded = op.args[0]
                    dict_type = op.args[1]
                    version = op.args[2]
                    if (
                        isinstance(guarded, MoltValue)
                        and isinstance(dict_type, MoltValue)
                        and isinstance(version, MoltValue)
                    ):
                        shape_key = dict_shape_fact_key(guarded.name)
                        expected_shape = (dict_type.name, version.name)
                        known_shape = known.get(shape_key)
                        if isinstance(known_shape, tuple):
                            if known_shape != expected_shape:
                                guard_fail_indices.add(op_idx)
                                block_traps = True
                                break
                        else:
                            known[shape_key] = expected_shape
                    continue
                out_name = op.result.name
                if out_name == "none":
                    continue
                known.pop(out_name, None)
                known.pop(type_fact_key(out_name), None)
                known.pop(dict_shape_fact_key(out_name), None)
                lattice_value = eval_lattice_value(op, known, op_idx)
                if lattice_value is _SCCP_UNKNOWN:
                    continue
                # MISSING sentinels must not propagate as constants through
                # the lattice — promote to overdefined so no downstream op
                # can constant-fold through a MISSING value.
                if is_missing_sentinel(lattice_value):
                    lattice_value = _SCCP_OVERDEFINED
                known[out_name] = lattice_value
                tag = self._const_type_tag_for_lattice_value(lattice_value)
                if tag is not None:
                    known[type_fact_key(out_name)] = tag
                if (
                    op.kind in {"EQ", "NE"}
                    and isinstance(lattice_value, bool)
                    and len(op.args) == 2
                ):
                    lhs = op.args[0]
                    rhs = op.args[1]
                    for type_side, tag_side in ((lhs, rhs), (rhs, lhs)):
                        if not isinstance(type_side, MoltValue) or not isinstance(
                            tag_side, MoltValue
                        ):
                            continue
                        guarded_name = type_of_origin.get(type_side.name)
                        if guarded_name is None:
                            continue
                        expected_tag = known.get(tag_side.name, _SCCP_UNKNOWN)
                        if not isinstance(expected_tag, int):
                            continue
                        implies_equal = (
                            lattice_value if op.kind == "EQ" else not lattice_value
                        )
                        if implies_equal:
                            known[type_fact_key(guarded_name)] = expected_tag
                if (
                    op.kind == "ISINSTANCE"
                    and lattice_value is True
                    and len(op.args) == 2
                ):
                    guarded_obj = op.args[0]
                    classinfo = op.args[1]
                    if isinstance(guarded_obj, MoltValue) and isinstance(
                        classinfo, MoltValue
                    ):
                        class_value = known.get(classinfo.name, _SCCP_UNKNOWN)
                        if isinstance(class_value, int):
                            known[type_fact_key(guarded_obj.name)] = class_value
                        elif isinstance(class_value, tuple):
                            tags = [
                                item for item in class_value if isinstance(item, int)
                            ]
                            if len(tags) == 1:
                                known[type_fact_key(guarded_obj.name)] = tags[0]

            prior_out = out_values[block_id]
            out_changed_keys: list[str] = []
            if known != prior_out:
                # DETERMINISM (#73, #34 bug class): `out_changed_keys` drives the
                # order values are pushed onto the SCCP `value_queue` (see the
                # `enqueue_value(key)` loop below), which in turn dictates the
                # block-processing schedule of this worklist fixed point.  Built
                # from a `set[str]` union, its iteration order is
                # PYTHONHASHSEED-dependent — and while the SCCP lattice *result*
                # is order-independent (monotone), the NUMBER of node re-visits
                # to reach the fixed point is not.  For a function near the
                # `max_iterations` cap, a worse schedule can exceed the cap and
                # bail to the conservative empty-facts result, whereas a better
                # schedule converges with full const facts.  That flips
                # downstream CSE/const-dedup on or off, so the emitted IR
                # silently diverged across hash seeds.  Sort the changed keys at
                # this construction site so the worklist schedule — and thus the
                # cap behaviour and the compiled IR — is byte-stable.
                all_keys = set(prior_out.keys()) | set(known.keys())
                out_changed_keys = sorted(
                    key
                    for key in all_keys
                    if prior_out.get(key, _SCCP_UNKNOWN)
                    != known.get(key, _SCCP_UNKNOWN)
                )
                out_values[block_id] = known

            succs = cfg.successors.get(block_id, [])
            chosen_succs = succs
            if block_traps:
                chosen_succs = []
            elif block.start < block.end:
                terminator_idx = block.end - 1
                terminator = ops[terminator_idx]
                if terminator.kind == "IF" and len(terminator.args) == 1:
                    cond = terminator.args[0]
                    cond_value: Any = _SCCP_UNKNOWN
                    if isinstance(cond, MoltValue):
                        cond_value = known.get(cond.name, _SCCP_UNKNOWN)
                    if isinstance(cond_value, bool):
                        branch_choice_by_if_index[terminator_idx] = cond_value
                        if cond_value and succs:
                            chosen_succs = [succs[0]]
                        elif not cond_value and len(succs) >= 2:
                            chosen_succs = [succs[1]]
                    else:
                        branch_choice_by_if_index.pop(terminator_idx, None)
                elif (
                    terminator.kind in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE"}
                    and len(terminator.args) == 1
                ):
                    cond = terminator.args[0]
                    cond_value: Any = _SCCP_UNKNOWN
                    if isinstance(cond, MoltValue):
                        cond_value = known.get(cond.name, _SCCP_UNKNOWN)
                    if isinstance(cond_value, bool) and len(succs) >= 2:
                        if terminator.kind == "LOOP_BREAK_IF_TRUE":
                            break_taken = bool(cond_value)
                        else:
                            break_taken = not bool(cond_value)
                        loop_break_choice_by_index[terminator_idx] = break_taken
                        chosen_succs = [succs[1] if break_taken else succs[0]]
                    else:
                        loop_break_choice_by_index.pop(terminator_idx, None)
                elif terminator.kind == "TRY_START":
                    can_raise = try_exception_possible_by_start.get(
                        terminator_idx, True
                    )
                    if not can_raise and succs:
                        chosen_succs = [succs[0]]
                elif terminator.kind == "CHECK_EXCEPTION":
                    owner_start = check_exception_try_owner.get(terminator_idx)
                    if owner_start is not None:
                        can_raise = try_exception_possible_by_start.get(
                            owner_start, True
                        )
                        if not can_raise and succs:
                            chosen_succs = [succs[0]]
                elif terminator.kind == "LOOP_END" and len(succs) >= 2:
                    loop_start_idx = cfg.control.loop_end_to_start.get(terminator_idx)
                    back_succ = (
                        None
                        if loop_start_idx is None
                        else cfg.index_to_block.get(loop_start_idx)
                    )
                    if back_succ is not None:
                        exit_succ = next(
                            (succ for succ in succs if succ != back_succ),
                            None,
                        )
                        back_exec = back_succ in executable_blocks
                        exit_exec = (
                            exit_succ in executable_blocks
                            if exit_succ is not None
                            else False
                        )
                        if back_exec and not exit_exec:
                            chosen_succs = [back_succ]
                        elif exit_succ is not None and exit_exec and not back_exec:
                            chosen_succs = [exit_succ]

            for succ in chosen_succs:
                enqueue_edge(block_id, succ)

            if out_changed_keys:
                for key in out_changed_keys:
                    if not key.startswith("__"):
                        enqueue_value(key)
                for succ in cfg.successors.get(block_id, []):
                    if succ in executable_blocks:
                        enqueue_block(succ)

        return SCCPResult(
            in_values=in_values,
            out_values=out_values,
            executable_blocks=executable_blocks,
            executable_edges=executable_edges,
            branch_choice_by_if_index=branch_choice_by_if_index,
            loop_break_choice_by_index=loop_break_choice_by_index,
            try_exception_possible_by_start=try_exception_possible_by_start,
            try_normal_possible_by_start=try_normal_possible_by_start,
            guard_fail_indices=guard_fail_indices,
        )

    def _sccp_in_const_int_values(self, sccp: SCCPResult) -> dict[int, dict[str, int]]:
        in_int_values: dict[int, dict[str, int]] = {}
        for block_id, known in sccp.in_values.items():
            in_int_values[block_id] = {
                key: value
                for key, value in known.items()
                if (
                    not str(key).startswith("__tag__:")
                    and value is not _SCCP_OVERDEFINED
                    and isinstance(value, int)
                    and not isinstance(value, bool)
                )
            }
        return in_int_values

    def _trim_phi_args_by_executable_edges(
        self,
        ops: list[MoltOp],
        cfg: CFGGraph,
        executable_edges: set[tuple[int, int]],
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        trimmed = 0
        out: list[MoltOp] = []
        for block in cfg.blocks:
            block_preds = cfg.predecessors.get(block.id, [])
            # Look through single-predecessor post-merge blocks: if this
            # block has exactly one predecessor that is itself a merge point
            # (multiple predecessors), the PHI args correspond to the merge
            # block's predecessors, not the direct predecessor.
            effective_preds = block_preds
            edge_target = block.id
            if (
                len(block_preds) == 1
                and len(cfg.predecessors.get(block_preds[0], [])) > 1
            ):
                effective_preds = cfg.predecessors.get(block_preds[0], [])
                edge_target = block_preds[0]
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if (
                    op.kind == "PHI"
                    and op.args
                    and len(op.args) == len(effective_preds)
                    and len(effective_preds) > 1
                ):
                    kept_args = [
                        arg
                        for arg, pred in zip(op.args, effective_preds)
                        if (pred, edge_target) in executable_edges
                    ]
                    normalized_args = kept_args
                    if kept_args and all(
                        isinstance(arg, MoltValue)
                        and isinstance(kept_args[0], MoltValue)
                        and arg.name == kept_args[0].name
                        for arg in kept_args
                    ):
                        normalized_args = [kept_args[0]]
                    if 0 < len(normalized_args) < len(op.args):
                        out.append(
                            MoltOp(
                                kind=op.kind,
                                args=normalized_args,
                                result=op.result,
                                metadata=op.metadata,
                            )
                        )
                        trimmed += len(op.args) - len(normalized_args)
                        continue
                out.append(op)
        return out, trimmed

    def _align_phi_args_to_cfg_predecessors(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        rewrites = 0
        out: list[MoltOp] = []
        for block in cfg.blocks:
            block_preds = cfg.predecessors.get(block.id, [])
            # Look through single-predecessor post-merge blocks to find
            # the effective predecessor count that PHI args should match.
            effective_preds = block_preds
            if (
                len(block_preds) == 1
                and len(cfg.predecessors.get(block_preds[0], [])) > 1
            ):
                effective_preds = cfg.predecessors.get(block_preds[0], [])
            expected = len(effective_preds)
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if op.kind != "PHI" or not op.args:
                    out.append(op)
                    continue
                if expected == 0:
                    out.append(op)
                    continue
                args = list(op.args)
                if len(args) == expected:
                    out.append(op)
                    continue
                if not all(isinstance(arg, MoltValue) for arg in args):
                    out.append(op)
                    continue
                first = cast(MoltValue, args[0])
                all_same = all(
                    isinstance(arg, MoltValue) and arg.name == first.name
                    for arg in args
                )
                if not all_same:
                    out.append(op)
                    continue
                if expected > 0:
                    # Expand to match effective predecessor count, then
                    # collapse identical args back down.
                    expanded = [first for _ in range(expected)]
                    if all(
                        isinstance(a, MoltValue) and a.name == first.name
                        for a in expanded
                    ):
                        normalized = [first]
                    else:
                        normalized = expanded
                    out.append(
                        MoltOp(
                            kind=op.kind,
                            args=normalized,
                            result=op.result,
                            metadata=op.metadata,
                        )
                    )
                    rewrites += abs(len(args) - expected)
                    continue
                out.append(op)
        return out, rewrites

    def _canonicalize_cfg_before_optimization(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        current = ops
        total_rewrites = 0
        for _ in range(8):
            round_rewrites = 0
            round_cfg = build_cfg(current)
            if not round_cfg.blocks:
                break

            step_ops, phi_align = self._align_phi_args_to_cfg_predecessors(
                current, round_cfg
            )
            round_rewrites += phi_align

            step_cfg = build_cfg(step_ops)
            if step_cfg.blocks:
                step_ops, ladder_threads = self._normalize_try_except_join_labels(
                    step_ops, cfg=step_cfg
                )
                round_rewrites += ladder_threads

            step_ops, label_prunes, jump_noops = self._prune_dead_labels_and_noop_jumps(
                step_ops
            )
            round_rewrites += label_prunes + jump_noops

            step_ops, structural_prunes = (
                self._canonicalize_structured_regions_pre_sccp(step_ops)
            )
            round_rewrites += structural_prunes

            if step_ops == current:
                break
            total_rewrites += round_rewrites
            current = step_ops

        return current, total_rewrites

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
