from __future__ import annotations

import os
import sys
import time
import tracemalloc
from collections.abc import Mapping
from pathlib import Path
from typing import Any, cast

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.config_resolution import _coerce_bool
from molt.cli.models import (
    BuildProfile,
    _BuildDiagnosticsContext,
    _FrontendTimingRecorderConfig,
)


def _build_reason_summary(
    module_reasons: Mapping[str, set[str]],
) -> dict[str, int]:
    summary: dict[str, int] = {}
    for reasons in module_reasons.values():
        for reason in reasons:
            summary[reason] = summary.get(reason, 0) + 1
    return {name: summary[name] for name in sorted(summary)}


def _build_diagnostics_enabled() -> bool:
    return _coerce_bool(os.environ.get("MOLT_BUILD_DIAGNOSTICS", ""), False)


def _build_allocation_diagnostics_enabled() -> bool:
    return _coerce_bool(os.environ.get("MOLT_BUILD_ALLOCATIONS", ""), False)


def _resolve_build_diagnostics_verbosity(raw: str | None) -> str:
    value = (raw or "").strip().lower()
    if value in {"", "default", "normal", "standard"}:
        return "default"
    if value in {"summary", "compact", "brief"}:
        return "summary"
    if value in {"full", "verbose", "detailed"}:
        return "full"
    return "default"


def _phase_duration_map(phase_starts: Mapping[str, float]) -> dict[str, float]:
    if not phase_starts:
        return {}
    starts = sorted(phase_starts.items(), key=lambda item: item[1])
    durations: dict[str, float] = {}
    for idx, (name, started) in enumerate(starts):
        if idx + 1 < len(starts):
            ended = starts[idx + 1][1]
        else:
            ended = time.perf_counter()
        durations[name] = round(max(0.0, ended - started), 6)
    return durations


def _resolve_build_diagnostics_path(
    output_spec: str,
    artifacts_root: Path,
) -> Path:
    path = Path(output_spec).expanduser()
    if not path.is_absolute():
        path = artifacts_root / path
    return path


def _capture_build_allocation_diagnostics(*, top_n: int = 10) -> dict[str, Any] | None:
    if not tracemalloc.is_tracing():
        return None
    current_bytes, peak_bytes = tracemalloc.get_traced_memory()
    snapshot = tracemalloc.take_snapshot()
    top_allocations: list[dict[str, Any]] = []
    for stat in snapshot.statistics("lineno")[: max(0, top_n)]:
        frame = stat.traceback[0]
        top_allocations.append(
            {
                "file": frame.filename,
                "line": frame.lineno,
                "size_bytes": stat.size,
                "count": stat.count,
            }
        )
    return {
        "current_bytes": current_bytes,
        "peak_bytes": peak_bytes,
        "top": top_allocations,
    }


def _emit_build_diagnostics(
    *,
    diagnostics: dict[str, Any] | None,
    diagnostics_path: Path | None,
    json_output: bool,
    verbosity: str = "default",
) -> None:
    if diagnostics is None:
        return
    if diagnostics_path is not None:
        _atomic_write_json(diagnostics_path, diagnostics, indent=2)
    if json_output:
        return
    resolved_verbosity = _resolve_build_diagnostics_verbosity(verbosity)
    summary_only = resolved_verbosity == "summary"
    full_details = resolved_verbosity == "full"
    phase_sec = diagnostics.get("phase_sec", {})
    total_sec = diagnostics.get("total_sec")
    module_count = diagnostics.get("module_count")
    reason_summary = diagnostics.get("module_reason_summary", {})
    midend = diagnostics.get("midend", {})
    frontend_parallel = diagnostics.get("frontend_parallel", {})
    frontend_modules_top = diagnostics.get("frontend_module_timings_top", [])
    allocations = diagnostics.get("allocations", {})
    print("Build diagnostics:", file=sys.stderr)
    if isinstance(total_sec, (int, float)):
        print(f"- total_sec: {total_sec:.6f}", file=sys.stderr)
    if isinstance(module_count, int):
        print(f"- modules: {module_count}", file=sys.stderr)
    if isinstance(phase_sec, dict):
        for name in sorted(phase_sec):
            value = phase_sec[name]
            if isinstance(value, (int, float)):
                print(f"- phase.{name}: {value:.6f}s", file=sys.stderr)
    if isinstance(reason_summary, dict):
        for name in sorted(reason_summary):
            value = reason_summary[name]
            if isinstance(value, int):
                print(f"- reason.{name}: {value}", file=sys.stderr)
    if isinstance(allocations, dict):
        current_bytes = allocations.get("current_bytes")
        peak_bytes = allocations.get("peak_bytes")
        if isinstance(current_bytes, int):
            print(f"- alloc.current_bytes: {current_bytes}", file=sys.stderr)
        if isinstance(peak_bytes, int):
            print(f"- alloc.peak_bytes: {peak_bytes}", file=sys.stderr)
        top_allocations = allocations.get("top")
        if not summary_only and isinstance(top_allocations, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(top_allocations[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                file_name = str(item.get("file", ""))
                line_no = int(item.get("line", 0))
                size_bytes = int(item.get("size_bytes", 0))
                count = int(item.get("count", 0))
                print(
                    "- alloc.top."
                    f"{idx}: {file_name}:{line_no} size_bytes={size_bytes} count={count}",
                    file=sys.stderr,
                )
    if isinstance(frontend_modules_top, list):
        limit = 20 if full_details else 10
        for idx, item in enumerate(frontend_modules_top[:limit], start=1):
            if not isinstance(item, dict):
                continue
            module_name = str(item.get("module", ""))
            total_s = float(item.get("total_s", 0.0))
            visit_s = float(item.get("visit_s", 0.0))
            lower_s = float(item.get("lower_s", 0.0))
            print(
                "- frontend.hotspot."
                f"{idx}: {module_name} total_s={total_s:.6f} "
                f"visit_s={visit_s:.6f} lower_s={lower_s:.6f}",
                file=sys.stderr,
            )
    if isinstance(frontend_parallel, dict):
        enabled = bool(frontend_parallel.get("enabled", False))
        workers = int(frontend_parallel.get("workers", 0))
        mode = str(frontend_parallel.get("mode", "serial"))
        print(
            f"- frontend_parallel: enabled={enabled} workers={workers} mode={mode}",
            file=sys.stderr,
        )
        reason = frontend_parallel.get("reason")
        if isinstance(reason, str) and reason:
            print(f"- frontend_parallel.reason: {reason}", file=sys.stderr)
        policy = frontend_parallel.get("policy")
        if isinstance(policy, dict):
            min_modules = int(policy.get("min_modules", 0))
            min_predicted_cost = float(policy.get("min_predicted_cost", 0.0))
            target_cost = float(policy.get("target_cost_per_worker", 0.0))
            print(
                "- frontend_parallel.policy: "
                f"min_modules={min_modules} "
                f"min_predicted_cost={min_predicted_cost:.3f} "
                f"target_cost_per_worker={target_cost:.3f}",
                file=sys.stderr,
            )
        layer_stats = frontend_parallel.get("layers")
        if not summary_only and isinstance(layer_stats, list):
            limit = 20 if full_details else 10
            print(f"- frontend_parallel.layers: {len(layer_stats)}", file=sys.stderr)
            for item in layer_stats[:limit]:
                if not isinstance(item, dict):
                    continue
                layer_index = int(item.get("index", 0)) + 1
                layer_mode = str(item.get("mode", "serial"))
                layer_modules = int(item.get("module_count", 0))
                layer_candidates = int(item.get("candidate_count", 0))
                layer_workers = int(item.get("workers", 0))
                queue_ms_total = float(item.get("queue_ms_total", 0.0))
                wait_ms_total = float(item.get("wait_ms_total", 0.0))
                exec_ms_total = float(item.get("exec_ms_total", 0.0))
                print(
                    "- frontend_parallel.layer."
                    f"{layer_index}: mode={layer_mode} modules={layer_modules} "
                    f"candidates={layer_candidates} workers={layer_workers} "
                    f"queue_ms={queue_ms_total:.3f} wait_ms={wait_ms_total:.3f} "
                    f"exec_ms={exec_ms_total:.3f}",
                    file=sys.stderr,
                )
            if len(layer_stats) > limit:
                print(
                    f"- frontend_parallel.layers_truncated: {len(layer_stats) - limit}",
                    file=sys.stderr,
                )
        worker_stats = frontend_parallel.get("worker_summary")
        if not summary_only and isinstance(worker_stats, dict):
            worker_count = int(worker_stats.get("count", 0))
            queue_ms_total = float(worker_stats.get("queue_ms_total", 0.0))
            wait_ms_total = float(worker_stats.get("wait_ms_total", 0.0))
            exec_ms_total = float(worker_stats.get("exec_ms_total", 0.0))
            queue_ms_max = float(worker_stats.get("queue_ms_max", 0.0))
            wait_ms_max = float(worker_stats.get("wait_ms_max", 0.0))
            exec_ms_max = float(worker_stats.get("exec_ms_max", 0.0))
            print(
                "- frontend_parallel.worker_ms: "
                f"count={worker_count} queue_total={queue_ms_total:.3f} "
                f"wait_total={wait_ms_total:.3f} exec_total={exec_ms_total:.3f} "
                f"queue_max={queue_ms_max:.3f} wait_max={wait_ms_max:.3f} "
                f"exec_max={exec_ms_max:.3f}",
                file=sys.stderr,
            )
    if isinstance(midend, dict):
        requested_profile = midend.get("requested_profile")
        if isinstance(requested_profile, str) and requested_profile:
            print(f"- midend.profile: {requested_profile}", file=sys.stderr)
        policy_config = midend.get("policy_config")
        if isinstance(policy_config, dict):
            profile_override = policy_config.get("profile_override")
            if isinstance(profile_override, str) and profile_override:
                print(
                    f"- midend.policy.profile_override: {profile_override}",
                    file=sys.stderr,
                )
            hot_tier_promotion_enabled = policy_config.get("hot_tier_promotion_enabled")
            if isinstance(hot_tier_promotion_enabled, bool):
                print(
                    "- midend.policy.hot_tier_promotion_enabled: "
                    f"{hot_tier_promotion_enabled}",
                    file=sys.stderr,
                )
            work_budget_override = policy_config.get("work_budget_override")
            if isinstance(work_budget_override, (int, float)):
                print(
                    f"- midend.policy.work_budget_override: {work_budget_override:.4f}",
                    file=sys.stderr,
                )
            budget_alpha = policy_config.get("budget_alpha")
            budget_beta = policy_config.get("budget_beta")
            budget_scale = policy_config.get("budget_scale")
            if all(
                isinstance(value, (int, float))
                for value in (budget_alpha, budget_beta, budget_scale)
            ):
                print(
                    "- midend.policy.budget_formula: "
                    f"alpha={float(budget_alpha):.4f} "
                    f"beta={float(budget_beta):.4f} "
                    f"scale={float(budget_scale):.4f}",
                    file=sys.stderr,
                )
        degraded_functions = midend.get("degraded_functions")
        if isinstance(degraded_functions, int):
            print(
                f"- midend.degraded_functions: {degraded_functions}",
                file=sys.stderr,
            )
        tier_summary = midend.get("tier_summary")
        if isinstance(tier_summary, dict):
            for tier in sorted(tier_summary):
                value = tier_summary[tier]
                if isinstance(value, int):
                    print(f"- midend.tier.{tier}: {value}", file=sys.stderr)
        tier_base_summary = midend.get("tier_base_summary")
        if isinstance(tier_base_summary, dict):
            for tier in sorted(tier_base_summary):
                value = tier_base_summary[tier]
                if isinstance(value, int):
                    print(f"- midend.tier_base.{tier}: {value}", file=sys.stderr)
        promoted_functions = midend.get("promoted_functions")
        if isinstance(promoted_functions, int):
            print(
                f"- midend.promoted_functions: {promoted_functions}",
                file=sys.stderr,
            )
        promotion_source_summary = midend.get("promotion_source_summary")
        if isinstance(promotion_source_summary, dict):
            for source in sorted(promotion_source_summary):
                value = promotion_source_summary[source]
                if isinstance(value, int):
                    print(
                        f"- midend.promotion_source.{source}: {value}",
                        file=sys.stderr,
                    )
        reason_counts = midend.get("degrade_reason_summary")
        if isinstance(reason_counts, dict):
            for reason in sorted(reason_counts):
                value = reason_counts[reason]
                if isinstance(value, int):
                    print(f"- midend.degrade_reason.{reason}: {value}", file=sys.stderr)
        hotspots = midend.get("pass_hotspots_top")
        if not summary_only and isinstance(hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                pass_name = str(item.get("pass", ""))
                total_ms = float(item.get("ms_total", 0.0))
                p95_ms = float(item.get("ms_p95", 0.0))
                print(
                    "- midend.hotspot."
                    f"{idx}: {module_name}::{function_name}:{pass_name} "
                    f"total_ms={total_ms:.3f} p95_ms={p95_ms:.3f}",
                    file=sys.stderr,
                )
        function_hotspots = midend.get("function_hotspots_top")
        if not summary_only and isinstance(function_hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(function_hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                spent_ms = float(item.get("spent_ms", 0.0))
                budget_ms = float(item.get("budget_ms", 0.0))
                work_units = float(item.get("work_units_spent", 0.0))
                work_budget = float(item.get("work_budget", 0.0))
                degraded = bool(item.get("degraded", False))
                print(
                    "- midend.function_hotspot."
                    f"{idx}: {module_name}::{function_name} "
                    f"spent_ms={spent_ms:.3f} budget_ms={budget_ms:.3f} "
                    f"work_units={work_units:.3f} work_budget={work_budget:.3f} "
                    f"degraded={degraded}",
                    file=sys.stderr,
                )
        promotion_hotspots = midend.get("promotion_hotspots_top")
        if not summary_only and isinstance(promotion_hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(promotion_hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                tier_base = str(item.get("tier_base", ""))
                tier_effective = str(item.get("tier_effective", ""))
                source = str(item.get("source", ""))
                signal = str(item.get("signal", ""))
                spent_ms = float(item.get("spent_ms", 0.0))
                print(
                    "- midend.promotion_hotspot."
                    f"{idx}: {module_name}::{function_name} "
                    f"{tier_base}->{tier_effective} source={source} "
                    f"signal={signal} spent_ms={spent_ms:.3f}",
                    file=sys.stderr,
                )
        degrade_hotspots = midend.get("degrade_event_hotspots_top")
        if not summary_only and isinstance(degrade_hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(degrade_hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                reason = str(item.get("reason", ""))
                action = str(item.get("action", ""))
                spent_ms = float(item.get("spent_ms", 0.0))
                print(
                    "- midend.degrade_hotspot."
                    f"{idx}: {module_name}::{function_name} reason={reason} "
                    f"action={action} spent_ms={spent_ms:.3f}",
                    file=sys.stderr,
                )
        telemetry_budget_util_avg = midend.get("telemetry_budget_utilization_avg")
        telemetry_budget_util_p95 = midend.get("telemetry_budget_utilization_p95")
        over_telemetry_budget = midend.get("functions_over_telemetry_budget")
        under_50_telemetry_budget = midend.get("functions_under_50pct_telemetry_budget")
        work_budget_util_avg = midend.get("work_budget_utilization_avg")
        work_budget_util_p95 = midend.get("work_budget_utilization_p95")
        over_work_budget = midend.get("functions_over_work_budget")
        under_50_work_budget = midend.get("functions_under_50pct_work_budget")
        if isinstance(telemetry_budget_util_avg, (int, float)):
            print(
                "- midend.telemetry_budget_utilization_avg: "
                f"{telemetry_budget_util_avg:.4f}",
                file=sys.stderr,
            )
        if isinstance(telemetry_budget_util_p95, (int, float)):
            print(
                "- midend.telemetry_budget_utilization_p95: "
                f"{telemetry_budget_util_p95:.4f}",
                file=sys.stderr,
            )
        if isinstance(over_telemetry_budget, int):
            print(
                f"- midend.functions_over_telemetry_budget: {over_telemetry_budget}",
                file=sys.stderr,
            )
        if isinstance(under_50_telemetry_budget, int):
            print(
                "- midend.functions_under_50pct_telemetry_budget: "
                f"{under_50_telemetry_budget}",
                file=sys.stderr,
            )
        if isinstance(work_budget_util_avg, (int, float)):
            print(
                f"- midend.work_budget_utilization_avg: {work_budget_util_avg:.4f}",
                file=sys.stderr,
            )
        if isinstance(work_budget_util_p95, (int, float)):
            print(
                f"- midend.work_budget_utilization_p95: {work_budget_util_p95:.4f}",
                file=sys.stderr,
            )
        if isinstance(over_work_budget, int):
            print(
                f"- midend.functions_over_work_budget: {over_work_budget}",
                file=sys.stderr,
            )
        if isinstance(under_50_work_budget, int):
            print(
                f"- midend.functions_under_50pct_work_budget: {under_50_work_budget}",
                file=sys.stderr,
            )
        telemetry_budget_ranked_functions: list[dict[str, Any]] = []
        work_budget_ranked_functions: list[dict[str, Any]] = []
        if not summary_only and isinstance(function_hotspots, list):
            for item in function_hotspots:
                if not isinstance(item, dict):
                    continue
                b_ms = float(item.get("budget_ms", 0.0))
                s_ms = float(item.get("spent_ms", 0.0))
                work_units = float(item.get("work_units_spent", 0.0))
                work_budget = float(item.get("work_budget", 0.0))
                if b_ms > 0.0:
                    telemetry_budget_ranked_functions.append(
                        {
                            "module": str(item.get("module", "")),
                            "function": str(item.get("function", "")),
                            "ratio": s_ms / b_ms,
                            "spent_ms": s_ms,
                            "budget_ms": b_ms,
                        }
                    )
                if work_budget > 0.0:
                    work_budget_ranked_functions.append(
                        {
                            "module": str(item.get("module", "")),
                            "function": str(item.get("function", "")),
                            "ratio": work_units / work_budget,
                            "work_units": work_units,
                            "work_budget": work_budget,
                        }
                    )
            telemetry_budget_ranked_functions.sort(key=lambda x: -x["ratio"])
            work_budget_ranked_functions.sort(key=lambda x: -x["ratio"])
            limit = 10 if full_details else 5
            for idx, item in enumerate(
                telemetry_budget_ranked_functions[:limit], start=1
            ):
                print(
                    "- midend.telemetry_budget_top."
                    f"{idx}: {item['module']}::{item['function']} "
                    f"ratio={item['ratio']:.4f} "
                    f"spent_ms={item['spent_ms']:.3f} "
                    f"budget_ms={item['budget_ms']:.3f}",
                    file=sys.stderr,
                )
            for idx, item in enumerate(work_budget_ranked_functions[:limit], start=1):
                print(
                    "- midend.work_budget_top."
                    f"{idx}: {item['module']}::{item['function']} "
                    f"ratio={item['ratio']:.4f} "
                    f"work_units={item['work_units']:.3f} "
                    f"work_budget={item['work_budget']:.3f}",
                    file=sys.stderr,
                )
        pass_wall_ranked = midend.get("pass_wall_time_ranked")
        if not summary_only and isinstance(pass_wall_ranked, list):
            limit = 10 if full_details else 3
            for idx, item in enumerate(pass_wall_ranked[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                pass_name = str(item.get("pass", ""))
                ms_total = float(item.get("ms_total", 0.0))
                print(
                    "- midend.pass_wall_top."
                    f"{idx}: {pass_name} ms_total={ms_total:.3f}",
                    file=sys.stderr,
                )
        promo_candidates = midend.get("promotion_candidates")
        if not summary_only and isinstance(promo_candidates, list) and promo_candidates:
            print(
                f"- midend.promotion_candidates: {len(promo_candidates)}",
                file=sys.stderr,
            )
    if diagnostics_path is not None:
        print(f"- wrote: {diagnostics_path}", file=sys.stderr)


def _midend_sample_percentile(samples: list[float], pct: float) -> float:
    if not samples:
        return 0.0
    ordered = sorted(samples)
    idx = max(0, min(len(ordered) - 1, int((len(ordered) - 1) * pct)))
    return float(ordered[idx])


def _midend_sample_p95(samples: list[float]) -> float:
    return _midend_sample_percentile(samples, 0.95)


def _midend_policy_config_snapshot() -> dict[str, Any]:
    profile_override = os.environ.get("MOLT_MIDEND_PROFILE", "").strip().lower()
    hot_promotion_enabled = os.environ.get(
        "MOLT_MIDEND_HOT_TIER_PROMOTION", "1"
    ).strip().lower() not in {"0", "false", "no", "off"}
    work_budget_override_raw = os.environ.get("MOLT_MIDEND_WORK_BUDGET", "").strip()
    work_budget_override: float | None = None
    if work_budget_override_raw:
        try:
            work_budget_override = max(0.0, float(work_budget_override_raw))
        except ValueError:
            work_budget_override = None

    def _float_env(name: str, default: float) -> float:
        raw = os.environ.get(name, "").strip()
        if not raw:
            return default
        try:
            return float(raw)
        except ValueError:
            return default

    return {
        "profile_override": profile_override or None,
        "hot_tier_promotion_enabled": hot_promotion_enabled,
        "work_budget_override": work_budget_override,
        "budget_alpha": _float_env("MOLT_MIDEND_BUDGET_ALPHA", 0.03),
        "budget_beta": _float_env("MOLT_MIDEND_BUDGET_BETA", 0.75),
        "budget_scale": _float_env("MOLT_MIDEND_BUDGET_SCALE", 1.0),
    }


def _duration_ms_from_ns(start_ns: Any, end_ns: Any) -> float:
    if not isinstance(start_ns, int):
        return 0.0
    if not isinstance(end_ns, int):
        return 0.0
    delta_ns = end_ns - start_ns
    if delta_ns <= 0:
        return 0.0
    return round(delta_ns / 1_000_000.0, 6)


def _normalize_midend_pass_stat(raw: dict[str, Any]) -> dict[str, Any]:
    samples = [
        float(sample)
        for sample in raw.get("samples_ms", [])
        if isinstance(sample, (int, float))
    ]
    ms_total = float(raw.get("ms_total", 0.0))
    ms_max = float(raw.get("ms_max", 0.0))
    return {
        "attempted": int(raw.get("attempted", 0)),
        "accepted": int(raw.get("accepted", 0)),
        "rejected": int(raw.get("rejected", 0)),
        "degraded": int(raw.get("degraded", 0)),
        "ms_total": round(max(0.0, ms_total), 6),
        "ms_max": round(max(0.0, ms_max), 6),
        "ms_p50": round(_midend_sample_percentile(samples, 0.50), 6),
        "ms_p75": round(_midend_sample_percentile(samples, 0.75), 6),
        "ms_p90": round(_midend_sample_percentile(samples, 0.90), 6),
        "ms_p95": round(_midend_sample_percentile(samples, 0.95), 6),
        "ms_p99": round(_midend_sample_percentile(samples, 0.99), 6),
        "sample_count": len(samples),
    }


def _build_midend_diagnostics_payload(
    *,
    requested_profile: BuildProfile,
    policy_outcomes_by_function: Mapping[str, dict[str, Any]],
    pass_stats_by_function: Mapping[str, dict[str, dict[str, Any]]],
) -> dict[str, Any] | None:
    if not policy_outcomes_by_function and not pass_stats_by_function:
        return None

    normalized_policy: dict[str, dict[str, Any]] = {}
    tier_summary: dict[str, int] = {}
    tier_base_summary: dict[str, int] = {}
    reason_summary: dict[str, int] = {}
    promotion_source_summary: dict[str, int] = {}
    effective_profiles: set[str] = set()
    degraded_functions = 0
    promoted_functions = 0
    function_hotspots: list[dict[str, Any]] = []
    degrade_event_hotspots: list[dict[str, Any]] = []
    promotion_hotspots: list[dict[str, Any]] = []

    for function_key in sorted(policy_outcomes_by_function):
        module_name, _, function_name = function_key.partition("::")
        raw_outcome = policy_outcomes_by_function[function_key]
        degrade_events: list[dict[str, Any]] = []
        for event in raw_outcome.get("degrade_events", []):
            if not isinstance(event, dict):
                continue
            reason = str(event.get("reason", ""))
            stage = str(event.get("stage", ""))
            action = str(event.get("action", ""))
            spent_ms = float(event.get("spent_ms", 0.0))
            normalized_event = {
                "reason": reason,
                "stage": stage,
                "action": action,
                "spent_ms": spent_ms,
            }
            if "value" in event:
                normalized_event["value"] = event["value"]
            degrade_events.append(normalized_event)
            degrade_event_hotspots.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "reason": reason,
                    "stage": stage,
                    "action": action,
                    "spent_ms": round(max(0.0, spent_ms), 6),
                }
            )
            if reason:
                reason_summary[reason] = reason_summary.get(reason, 0) + 1
        profile = str(raw_outcome.get("profile", ""))
        tier = str(
            raw_outcome.get(
                "tier_effective",
                raw_outcome.get("tier", ""),
            )
        )
        tier_base = str(raw_outcome.get("tier_base", tier))
        tier_source = str(raw_outcome.get("tier_source", ""))
        promoted = bool(raw_outcome.get("promoted", False))
        promotion_source = str(raw_outcome.get("promotion_source", ""))
        promotion_signal = str(raw_outcome.get("promotion_signal", ""))
        if profile:
            effective_profiles.add(profile)
        if tier:
            tier_summary[tier] = tier_summary.get(tier, 0) + 1
        if tier_base:
            tier_base_summary[tier_base] = tier_base_summary.get(tier_base, 0) + 1
        degraded = bool(raw_outcome.get("degraded", False))
        if degraded:
            degraded_functions += 1
        if promoted:
            promoted_functions += 1
            if promotion_source:
                promotion_source_summary[promotion_source] = (
                    promotion_source_summary.get(promotion_source, 0) + 1
                )
        spent_ms = float(raw_outcome.get("spent_ms", 0.0))
        budget_ms = float(raw_outcome.get("budget_ms", 0.0))
        work_budget = float(raw_outcome.get("work_budget", 0.0))
        work_units_spent = float(raw_outcome.get("work_units_spent", 0.0))
        function_hotspots.append(
            {
                "module": module_name,
                "function": function_name or module_name,
                "profile": profile,
                "tier": tier,
                "tier_base": tier_base,
                "spent_ms": round(max(0.0, spent_ms), 6),
                "budget_ms": round(max(0.0, budget_ms), 6),
                "work_budget": round(max(0.0, work_budget), 6),
                "work_units_spent": round(max(0.0, work_units_spent), 6),
                "degraded": degraded,
                "promoted": promoted,
            }
        )
        if promoted:
            promotion_hotspots.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "tier_base": tier_base,
                    "tier_effective": tier,
                    "source": promotion_source,
                    "signal": promotion_signal,
                    "spent_ms": round(max(0.0, spent_ms), 6),
                }
            )
        normalized_policy[function_key] = {
            "profile": profile,
            "tier": tier,
            "tier_effective": tier,
            "tier_base": tier_base,
            "tier_source": tier_source,
            "promoted": promoted,
            "promotion_source": promotion_source,
            "promotion_signal": promotion_signal,
            "budget_ms": budget_ms,
            "spent_ms": spent_ms,
            "work_budget": work_budget,
            "work_units_spent": work_units_spent,
            "degraded": degraded,
            "degrade_events": degrade_events,
        }

    normalized_pass_stats: dict[str, dict[str, dict[str, Any]]] = {}
    hotspots: list[dict[str, Any]] = []
    for function_key in sorted(pass_stats_by_function):
        module_name, _, function_name = function_key.partition("::")
        per_pass = pass_stats_by_function[function_key]
        normalized_per_pass: dict[str, dict[str, Any]] = {}
        for pass_name in sorted(per_pass):
            normalized = _normalize_midend_pass_stat(per_pass[pass_name])
            normalized_per_pass[pass_name] = normalized
            hotspots.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "pass": pass_name,
                    "ms_total": normalized["ms_total"],
                    "ms_p95": normalized["ms_p95"],
                    "attempted": normalized["attempted"],
                    "accepted": normalized["accepted"],
                    "degraded": normalized["degraded"],
                }
            )
        normalized_pass_stats[function_key] = normalized_per_pass

    hotspots.sort(
        key=lambda item: (
            -float(item["ms_total"]),
            item["module"],
            item["function"],
            item["pass"],
        )
    )
    p95_hotspots = sorted(
        hotspots,
        key=lambda item: (
            -float(item["ms_p95"]),
            item["module"],
            item["function"],
            item["pass"],
        ),
    )
    function_hotspots.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
        )
    )
    degrade_event_hotspots.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
            item["reason"],
            item["action"],
        )
    )
    promotion_hotspots.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
            item["tier_base"],
            item["tier_effective"],
        )
    )

    promotion_candidates: list[dict[str, Any]] = []
    telemetry_budget_utilizations: list[float] = []
    work_budget_utilizations: list[float] = []
    functions_over_telemetry_budget = 0
    functions_under_50pct_telemetry_budget = 0
    functions_over_work_budget = 0
    functions_under_50pct_work_budget = 0
    for function_key in sorted(policy_outcomes_by_function):
        raw_outcome = policy_outcomes_by_function[function_key]
        module_name, _, function_name = function_key.partition("::")
        allow_hot = bool(raw_outcome.get("allow_hot_promotion", False))
        was_promoted = bool(raw_outcome.get("promoted", False))
        if allow_hot and not was_promoted:
            promotion_candidates.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "tier": str(raw_outcome.get("tier", "")),
                    "budget_ms": round(
                        max(0.0, float(raw_outcome.get("budget_ms", 0.0))), 6
                    ),
                    "spent_ms": round(
                        max(0.0, float(raw_outcome.get("spent_ms", 0.0))), 6
                    ),
                    "work_budget": round(
                        max(0.0, float(raw_outcome.get("work_budget", 0.0))), 6
                    ),
                    "work_units_spent": round(
                        max(0.0, float(raw_outcome.get("work_units_spent", 0.0))),
                        6,
                    ),
                }
            )
        s_ms = max(0.0, float(raw_outcome.get("spent_ms", 0.0)))
        b_ms = max(0.0, float(raw_outcome.get("budget_ms", 0.0)))
        if b_ms > 0.0:
            utilization = s_ms / b_ms
            telemetry_budget_utilizations.append(utilization)
            if s_ms > b_ms:
                functions_over_telemetry_budget += 1
            if s_ms < 0.5 * b_ms:
                functions_under_50pct_telemetry_budget += 1
        work_units = max(0.0, float(raw_outcome.get("work_units_spent", 0.0)))
        work_budget = max(0.0, float(raw_outcome.get("work_budget", 0.0)))
        if work_budget > 0.0:
            work_utilization = work_units / work_budget
            work_budget_utilizations.append(work_utilization)
            if work_units > work_budget:
                functions_over_work_budget += 1
            if work_units < 0.5 * work_budget:
                functions_under_50pct_work_budget += 1
    promotion_candidates.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
        )
    )
    telemetry_budget_utilization_avg = 0.0
    telemetry_budget_utilization_p95 = 0.0
    if telemetry_budget_utilizations:
        telemetry_budget_utilization_avg = sum(telemetry_budget_utilizations) / len(
            telemetry_budget_utilizations
        )
        telemetry_budget_utilization_p95 = _midend_sample_percentile(
            telemetry_budget_utilizations, 0.95
        )
    work_budget_utilization_avg = 0.0
    work_budget_utilization_p95 = 0.0
    if work_budget_utilizations:
        work_budget_utilization_avg = sum(work_budget_utilizations) / len(
            work_budget_utilizations
        )
        work_budget_utilization_p95 = _midend_sample_percentile(
            work_budget_utilizations, 0.95
        )

    pass_aggregate_wall_ms: dict[str, float] = {}
    for function_key in pass_stats_by_function:
        per_pass = pass_stats_by_function[function_key]
        for pass_name, raw_stat in per_pass.items():
            ms_total = float(raw_stat.get("ms_total", 0.0))
            pass_aggregate_wall_ms[pass_name] = pass_aggregate_wall_ms.get(
                pass_name, 0.0
            ) + max(0.0, ms_total)
    pass_wall_ranked = sorted(pass_aggregate_wall_ms.items(), key=lambda kv: -kv[1])

    return {
        "requested_profile": requested_profile,
        "effective_profiles": sorted(effective_profiles),
        "policy_config": _midend_policy_config_snapshot(),
        "function_count": max(
            len(normalized_policy),
            len(normalized_pass_stats),
        ),
        "degraded_functions": degraded_functions,
        "promoted_functions": promoted_functions,
        "tier_summary": {name: tier_summary[name] for name in sorted(tier_summary)},
        "tier_base_summary": {
            name: tier_base_summary[name] for name in sorted(tier_base_summary)
        },
        "promotion_source_summary": {
            name: promotion_source_summary[name]
            for name in sorted(promotion_source_summary)
        },
        "degrade_reason_summary": {
            name: reason_summary[name] for name in sorted(reason_summary)
        },
        "telemetry_budget_utilization_avg": round(telemetry_budget_utilization_avg, 6),
        "telemetry_budget_utilization_p95": round(telemetry_budget_utilization_p95, 6),
        "functions_over_telemetry_budget": functions_over_telemetry_budget,
        "functions_under_50pct_telemetry_budget": (
            functions_under_50pct_telemetry_budget
        ),
        "work_budget_utilization_avg": round(work_budget_utilization_avg, 6),
        "work_budget_utilization_p95": round(work_budget_utilization_p95, 6),
        "functions_over_work_budget": functions_over_work_budget,
        "functions_under_50pct_work_budget": functions_under_50pct_work_budget,
        "promotion_candidates": promotion_candidates[:20],
        "pass_wall_time_ranked": [
            {"pass": name, "ms_total": round(ms, 6)} for name, ms in pass_wall_ranked
        ],
        "policy_outcomes_by_function": normalized_policy,
        "pass_stats_by_function": normalized_pass_stats,
        "function_hotspots_top": function_hotspots[:10],
        "promotion_hotspots_top": promotion_hotspots[:10],
        "degrade_event_hotspots_top": degrade_event_hotspots[:10],
        "pass_hotspots_top": hotspots[:10],
        "pass_hotspots_p95_top": p95_hotspots[:10],
    }


def _emit_build_diagnostics_if_present(
    *,
    diagnostics_payload: dict[str, Any] | None,
    diagnostics_path: Path | None,
    json_output: bool,
    verbosity: str,
) -> None:
    _emit_build_diagnostics(
        diagnostics=diagnostics_payload,
        diagnostics_path=diagnostics_path,
        json_output=json_output,
        verbosity=verbosity,
    )


def _build_build_diagnostics_payload(
    diagnostics_context: _BuildDiagnosticsContext,
) -> tuple[dict[str, Any] | None, Path | None]:
    if not diagnostics_context.diagnostics_enabled:
        return None, None
    module_reason_map = {
        name: sorted(reasons)
        for name, reasons in sorted(diagnostics_context.module_reasons.items())
    }
    payload: dict[str, Any] = {
        "enabled": True,
        "total_sec": round(
            time.perf_counter() - diagnostics_context.diagnostics_start, 6
        ),
        "phase_sec": _phase_duration_map(diagnostics_context.phase_starts),
        "module_count": len(diagnostics_context.module_graph),
        "module_reason_summary": _build_reason_summary(
            diagnostics_context.module_reasons
        ),
        "module_reasons": module_reason_map,
    }
    frontend_module_timings = list(diagnostics_context.frontend_module_timings)
    if frontend_module_timings:
        payload["frontend_module_timings"] = frontend_module_timings
        payload["frontend_module_timings_top"] = sorted(
            frontend_module_timings,
            key=lambda item: float(item.get("total_s", 0.0)),
            reverse=True,
        )[:20]
    if diagnostics_context.allocation_diagnostics_enabled:
        allocations_payload = _capture_build_allocation_diagnostics()
        if allocations_payload is not None:
            payload["allocations"] = allocations_payload
    payload["frontend_parallel"] = dict(diagnostics_context.frontend_parallel_details)
    midend_payload = _build_midend_diagnostics_payload(
        requested_profile=cast(BuildProfile, diagnostics_context.profile),
        policy_outcomes_by_function=dict(
            diagnostics_context.midend_policy_outcomes_by_function
        ),
        pass_stats_by_function=dict(diagnostics_context.midend_pass_stats_by_function),
    )
    if midend_payload is not None:
        payload["midend"] = midend_payload
    if diagnostics_context.backend_daemon_health is not None:
        payload["backend_daemon"] = diagnostics_context.backend_daemon_health
    if (
        diagnostics_context.backend_daemon_cached is not None
        or diagnostics_context.backend_daemon_cache_tier is not None
        or diagnostics_context.backend_daemon_config_digest is not None
    ):
        daemon_compile_info: dict[str, Any] = {}
        if diagnostics_context.backend_daemon_cached is not None:
            daemon_compile_info["cached"] = diagnostics_context.backend_daemon_cached
        if diagnostics_context.backend_daemon_cache_tier is not None:
            daemon_compile_info["cache_tier"] = (
                diagnostics_context.backend_daemon_cache_tier
            )
        if diagnostics_context.backend_daemon_config_digest is not None:
            daemon_compile_info["config_digest"] = (
                diagnostics_context.backend_daemon_config_digest
            )
        payload["backend_daemon_compile"] = daemon_compile_info
    out_path: Path | None = None
    if diagnostics_context.diagnostics_path_spec:
        out_path = _resolve_build_diagnostics_path(
            diagnostics_context.diagnostics_path_spec,
            diagnostics_context.artifacts_root,
        )
    return payload, out_path


def _record_frontend_timing_item(
    frontend_module_timings: list[dict[str, Any]],
    *,
    config: _FrontendTimingRecorderConfig,
    module_name: str,
    module_path: Path,
    visit_s: float,
    lower_s: float,
    total_s: float,
    timed_out: bool = False,
    detail: str | None = None,
) -> None:
    if not config.enabled:
        return
    item: dict[str, Any] = {
        "module": module_name,
        "path": str(module_path),
        "visit_s": round(max(0.0, visit_s), 6),
        "lower_s": round(max(0.0, lower_s), 6),
        "total_s": round(max(0.0, total_s), 6),
        "timed_out": timed_out,
    }
    if detail:
        item["detail"] = detail
    frontend_module_timings.append(item)
    if (
        config.raw
        and (timed_out or total_s >= config.threshold)
        and not config.json_output
    ):
        suffix = f" timeout={detail}" if timed_out and detail else ""
        print(
            "frontend module timing: "
            f"{module_name} visit={visit_s:.3f}s lower={lower_s:.3f}s "
            f"total={total_s:.3f}s{suffix}",
            file=sys.stderr,
        )
