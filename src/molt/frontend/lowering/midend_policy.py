"""MidendPolicyMixin: frontend IR midend state, environment policy, tiering, and telemetry."""

from __future__ import annotations

import os
import sys
from typing import TYPE_CHECKING, Any, cast

from molt.frontend._types import (
    MidendEnvConfig,
    MidendFunctionPolicy,
    MidendProfile,
    MidendTier,
    MidendTierClassification,
    MoltOp,
    _MIDEND_DEGRADE_CHECKPOINTS,
    _MIDEND_ENV_KEYS,
    _MIDEND_WORK_BASE_UNITS_PER_MS,
    _MIDEND_WORK_GROWTH_HEADROOM,
    _MOLT_MODULE_CHUNK_PREFIX,
    _TrackedOpsList,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class MidendPolicyMixin(_MixinBase):
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
