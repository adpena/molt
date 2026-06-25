from __future__ import annotations

import os
import time
from pathlib import Path
from typing import Any, Callable, Mapping, MutableMapping, Sequence, cast

from molt.cli.build_diagnostics import _duration_ms_from_ns
from molt.cli.models import (
    _FrontendLayerPlan,
    _FrontendLayerPolicySummary,
    _FrontendLayerStaticMetrics,
    _FrontendModuleResultTimings,
    _FrontendParallelConfig,
    _FrontendParallelLayerState,
    _WorkerTimingSummary,
)
from molt.cli.module_source import _ModuleSourceCatalog
from molt.cli.module_stdlib_policy import _looks_like_stdlib_module_name


def _fresh_frontend_parallel_layer_state() -> _FrontendParallelLayerState:
    return _FrontendParallelLayerState()


def _resolve_frontend_parallel_module_workers() -> int:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_MODULES", "").strip().lower()
    if not raw:
        return 0
    if raw in {"0", "false", "no", "off"}:
        return 0
    if raw in {"auto", "1", "true", "yes", "on"}:
        cpu_count = os.cpu_count() or 1
        return max(2, cpu_count)
    try:
        parsed = int(raw)
    except ValueError:
        return 0
    if parsed < 2:
        return 0
    return parsed


def _resolve_frontend_parallel_min_modules() -> int:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_MIN_MODULES", "").strip()
    if not raw:
        return 2
    try:
        parsed = int(raw)
    except ValueError:
        return 2
    return max(2, parsed)


def _resolve_frontend_parallel_min_predicted_cost() -> float:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_MIN_PREDICTED_COST", "").strip()
    if not raw:
        return 32768.0
    try:
        parsed = float(raw)
    except ValueError:
        return 32768.0
    return max(0.0, parsed)


def _resolve_frontend_parallel_target_cost_per_worker() -> float:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_TARGET_COST_PER_WORKER", "").strip()
    if not raw:
        return 65536.0
    try:
        parsed = float(raw)
    except ValueError:
        return 65536.0
    return max(1.0, parsed)


def _resolve_frontend_parallel_stdlib_min_cost_scale() -> float:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_STDLIB_MIN_COST_SCALE", "").strip()
    if not raw:
        return 0.5
    try:
        parsed = float(raw)
    except ValueError:
        return 0.5
    return max(0.0, parsed)


def _predict_frontend_module_cost(
    module_name: str,
    module_deps: dict[str, set[str]],
    *,
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _ModuleSourceCatalog | None = None,
    module_graph: Mapping[str, Path] | None = None,
) -> float:
    source_size = 0
    if module_source_catalog is not None:
        source_size = module_source_catalog.source_size(
            module_name,
            module_graph.get(module_name) if module_graph is not None else None,
        )
    elif module_sources is not None:
        source_size = len(module_sources.get(module_name, ""))
    source_cost = max(1.0, float(source_size))
    dep_cost = float(max(0, len(module_deps.get(module_name, set()))) * 512)
    return source_cost + dep_cost


def _choose_frontend_parallel_layer_workers(
    *,
    candidates: list[str],
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _ModuleSourceCatalog | None = None,
    module_graph: Mapping[str, Path] | None = None,
    module_deps: dict[str, set[str]],
    module_costs: Mapping[str, float] | None = None,
    stdlib_like_by_module: Mapping[str, bool] | None = None,
    max_workers: int,
    min_modules: int,
    min_predicted_cost: float,
    target_cost_per_worker: float,
) -> dict[str, Any]:
    candidate_count = len(candidates)
    if candidate_count < min_modules:
        return {
            "enabled": False,
            "workers": 1,
            "reason": "layer_module_count_below_min",
            "predicted_cost_total": 0.0,
            "effective_min_predicted_cost": round(min_predicted_cost, 3),
            "stdlib_candidates": 0,
        }
    predicted_cost_total = 0.0
    for name in candidates:
        if module_costs is not None and name in module_costs:
            predicted_cost_total += module_costs[name]
        else:
            predicted_cost_total += _predict_frontend_module_cost(
                name,
                module_deps,
                module_sources=module_sources,
                module_source_catalog=module_source_catalog,
                module_graph=module_graph,
            )
    stdlib_candidates = sum(
        1
        for name in candidates
        if (
            stdlib_like_by_module[name]
            if stdlib_like_by_module is not None and name in stdlib_like_by_module
            else _looks_like_stdlib_module_name(name)
        )
    )
    effective_min_predicted_cost = float(min_predicted_cost)
    if stdlib_candidates > 0:
        effective_min_predicted_cost *= _resolve_frontend_parallel_stdlib_min_cost_scale()
    if predicted_cost_total < effective_min_predicted_cost:
        return {
            "enabled": False,
            "workers": 1,
            "reason": "layer_predicted_cost_below_min",
            "predicted_cost_total": round(predicted_cost_total, 3),
            "effective_min_predicted_cost": round(effective_min_predicted_cost, 3),
            "stdlib_candidates": stdlib_candidates,
        }
    scaled_workers = int(
        (predicted_cost_total / max(1.0, target_cost_per_worker)) + 0.999
    )
    chosen_workers = min(
        max_workers,
        candidate_count,
        max(2, scaled_workers),
    )
    return {
        "enabled": chosen_workers >= 2,
        "workers": max(1, chosen_workers),
        "reason": "enabled",
        "predicted_cost_total": round(predicted_cost_total, 3),
        "effective_min_predicted_cost": round(effective_min_predicted_cost, 3),
        "stdlib_candidates": stdlib_candidates,
    }


def _summarize_worker_timing_items(
    items: Sequence[Mapping[str, Any]],
) -> _WorkerTimingSummary:
    queue_samples = [float(item.get("queue_ms", 0.0)) for item in items]
    wait_samples = [float(item.get("wait_ms", 0.0)) for item in items]
    exec_samples = [float(item.get("exec_ms", 0.0)) for item in items]
    roundtrip_samples = [float(item.get("roundtrip_ms", 0.0)) for item in items]
    return _WorkerTimingSummary(
        count=len(items),
        queue_ms_total=round(sum(queue_samples), 6),
        queue_ms_max=round(max(queue_samples, default=0.0), 6),
        wait_ms_total=round(sum(wait_samples), 6),
        wait_ms_max=round(max(wait_samples, default=0.0), 6),
        exec_ms_total=round(sum(exec_samples), 6),
        exec_ms_max=round(max(exec_samples, default=0.0), 6),
        roundtrip_ms_total=round(sum(roundtrip_samples), 6),
    )


def _frontend_parallel_layer_detail(
    *,
    layer_index: int,
    mode: str,
    policy_reason: str,
    module_count: int,
    candidate_count: int,
    workers: int,
    cache_hits: int,
    predicted_cost_total: float,
    effective_min_predicted_cost: float,
    stdlib_candidates: int,
    target_cost_per_worker: float,
    timing_summary: _WorkerTimingSummary,
    started_ns: int,
    finished_ns: int,
    fallback_reason: str | None = None,
) -> dict[str, Any]:
    detail: dict[str, Any] = {
        "index": layer_index,
        "mode": mode,
        "policy_reason": policy_reason,
        "module_count": module_count,
        "candidate_count": candidate_count,
        "workers": workers,
        "cache_hits": cache_hits,
        "predicted_cost_total": round(predicted_cost_total, 3),
        "effective_min_predicted_cost": round(effective_min_predicted_cost, 3),
        "stdlib_candidates": stdlib_candidates,
        "target_cost_per_worker": round(target_cost_per_worker, 3),
        "queue_ms_total": timing_summary.queue_ms_total,
        "queue_ms_max": timing_summary.queue_ms_max,
        "wait_ms_total": timing_summary.wait_ms_total,
        "wait_ms_max": timing_summary.wait_ms_max,
        "exec_ms_total": timing_summary.exec_ms_total,
        "exec_ms_max": timing_summary.exec_ms_max,
        "roundtrip_ms_total": timing_summary.roundtrip_ms_total,
        "elapsed_ms": _duration_ms_from_ns(started_ns, finished_ns),
    }
    if fallback_reason:
        detail["fallback_reason"] = fallback_reason
    return detail


def _frontend_result_timings(result: Mapping[str, Any]) -> _FrontendModuleResultTimings:
    timings = cast(Mapping[str, Any], result.get("timings", {}))
    return _FrontendModuleResultTimings(
        visit_s=float(timings.get("visit_s", 0.0)),
        lower_s=float(timings.get("lower_s", 0.0)),
        total_s=float(timings.get("total_s", 0.0)),
    )

def _frontend_layer_policy_summary(
    layer_policy: Mapping[str, Any],
    *,
    default_min_predicted_cost: float,
) -> _FrontendLayerPolicySummary:
    return _FrontendLayerPolicySummary(
        enabled=bool(layer_policy.get("enabled")),
        workers=int(layer_policy.get("workers", 1)),
        reason=str(layer_policy.get("reason", "serial")),
        predicted_cost_total=float(layer_policy.get("predicted_cost_total", 0.0)),
        effective_min_predicted_cost=float(
            layer_policy.get(
                "effective_min_predicted_cost",
                default_min_predicted_cost,
            )
        ),
        stdlib_candidates=int(layer_policy.get("stdlib_candidates", 0)),
    )


def _record_parallel_cached_module_result(
    layer_state: _FrontendParallelLayerState,
    module_name: str,
    cached_result: Mapping[str, Any],
) -> None:
    timings = cast(Mapping[str, Any], cached_result.get("timings", {}))
    total_ms = float(timings.get("total_s", 0.0)) * 1000.0
    layer_state.results[module_name] = {"ok": True, **cached_result}
    layer_state.worker_timings_by_module[module_name] = {
        "mode": "parallel_cache_hit",
        "queue_ms": 0.0,
        "wait_ms": 0.0,
        "exec_ms": round(max(0.0, total_ms), 6),
        "roundtrip_ms": round(max(0.0, total_ms), 6),
        "worker_pid": None,
    }


def _record_parallel_worker_result(
    layer_state: _FrontendParallelLayerState,
    *,
    module_name: str,
    result: Mapping[str, Any],
    submitted_ns: int,
    received_ns: int,
) -> None:
    timings = cast(Mapping[str, Any], result.get("timings", {}))
    worker_meta = cast(Mapping[str, Any], result.get("worker", {}))
    worker_started_ns = worker_meta.get("started_ns")
    worker_finished_ns = worker_meta.get("finished_ns")
    exec_ms = float(timings.get("total_s", 0.0)) * 1000.0
    exec_from_ns = _duration_ms_from_ns(worker_started_ns, worker_finished_ns)
    if exec_from_ns > 0.0:
        exec_ms = exec_from_ns
    layer_state.results[module_name] = dict(result)
    layer_state.worker_timings_by_module[module_name] = {
        "mode": "parallel",
        "queue_ms": _duration_ms_from_ns(submitted_ns, worker_started_ns),
        "wait_ms": _duration_ms_from_ns(worker_finished_ns, received_ns),
        "exec_ms": round(max(0.0, exec_ms), 6),
        "roundtrip_ms": _duration_ms_from_ns(submitted_ns, received_ns),
        "worker_pid": worker_meta.get("pid"),
    }


def _resolve_frontend_parallel_config(
    *,
    module_count: int,
    has_back_edges: bool,
    frontend_phase_timeout: float | None,
) -> _FrontendParallelConfig:
    workers = _resolve_frontend_parallel_module_workers()
    min_modules = _resolve_frontend_parallel_min_modules()
    min_predicted_cost = _resolve_frontend_parallel_min_predicted_cost()
    target_cost_per_worker = _resolve_frontend_parallel_target_cost_per_worker()
    stdlib_min_cost_scale = _resolve_frontend_parallel_stdlib_min_cost_scale()
    enabled = False
    reason = "disabled"
    if workers < 2:
        reason = "workers<2"
    elif module_count < 2:
        reason = "module_count<2"
    elif has_back_edges:
        reason = "dependency_back_edge"
    elif frontend_phase_timeout is not None:
        reason = "phase_timeout_configured"
    else:
        enabled = True
        reason = "enabled"
    return _FrontendParallelConfig(
        workers=workers,
        min_modules=min_modules,
        min_predicted_cost=min_predicted_cost,
        target_cost_per_worker=target_cost_per_worker,
        stdlib_min_cost_scale=stdlib_min_cost_scale,
        enabled=enabled,
        reason=reason,
    )


def _frontend_parallel_policy_payload(
    config: _FrontendParallelConfig,
) -> dict[str, Any]:
    return {
        "min_modules": config.min_modules,
        "min_predicted_cost": round(config.min_predicted_cost, 3),
        "target_cost_per_worker": round(config.target_cost_per_worker, 3),
        "stdlib_min_cost_scale": round(config.stdlib_min_cost_scale, 3),
    }


def _frontend_layer_plan(
    layer: Sequence[str],
    *,
    syntax_error_modules: Mapping[str, Any],
    module_source_catalog: _ModuleSourceCatalog,
    module_graph: Mapping[str, Path],
    module_deps: dict[str, set[str]],
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
    frontend_parallel_config: _FrontendParallelConfig,
    parallel_pool_usable: bool,
) -> _FrontendLayerPlan:
    candidates = tuple(name for name in layer if name not in syntax_error_modules)
    policy = _choose_frontend_parallel_layer_workers(
        candidates=list(candidates),
        module_source_catalog=module_source_catalog,
        module_graph=module_graph,
        module_deps=module_deps,
        module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
        max_workers=frontend_parallel_config.workers,
        min_modules=frontend_parallel_config.min_modules,
        min_predicted_cost=frontend_parallel_config.min_predicted_cost,
        target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
    )
    policy_summary = _frontend_layer_policy_summary(
        policy,
        default_min_predicted_cost=frontend_parallel_config.min_predicted_cost,
    )
    mode = "serial"
    policy_reason = policy_summary.reason
    workers = policy_summary.workers
    if parallel_pool_usable and policy_summary.enabled and len(candidates) > 1:
        mode = "parallel"
        workers = min(workers, len(candidates))
    elif len(candidates) > 1 and not parallel_pool_usable:
        mode = "serial_layer_policy"
        policy_reason = "pool_unavailable_after_error"
    return _FrontendLayerPlan(
        candidates=candidates,
        predicted_cost_total=policy_summary.predicted_cost_total,
        effective_min_predicted_cost=policy_summary.effective_min_predicted_cost,
        stdlib_candidates=policy_summary.stdlib_candidates,
        workers=workers,
        policy_reason=policy_reason,
        mode=mode,
    )


def _worker_timing_summary_payload(summary: _WorkerTimingSummary) -> dict[str, Any]:
    return {
        "count": summary.count,
        "queue_ms_total": summary.queue_ms_total,
        "queue_ms_max": summary.queue_ms_max,
        "wait_ms_total": summary.wait_ms_total,
        "wait_ms_max": summary.wait_ms_max,
        "exec_ms_total": summary.exec_ms_total,
        "exec_ms_max": summary.exec_ms_max,
    }


def _layer_cache_hit_count(items: Sequence[Mapping[str, Any]]) -> int:
    return sum(1 for item in items if item.get("mode") == "parallel_cache_hit")


def _frontend_layer_static_metrics(
    module_names: Sequence[str],
    *,
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
) -> _FrontendLayerStaticMetrics:
    return _FrontendLayerStaticMetrics(
        predicted_cost_total=sum(
            frontend_module_costs.get(name, 0.0) for name in module_names
        ),
        stdlib_candidates=sum(
            1 for name in module_names if stdlib_like_by_module.get(name, False)
        ),
    )


def _record_serial_frontend_worker_timing(
    *,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    recorded_worker_timings: list[dict[str, Any]],
    layer_index: int,
    module_name: str,
    module_path: Path,
    mode: str,
    total_s: float,
) -> None:
    total_ms = total_s * 1000.0
    recorded_worker_timings.append(
        record_frontend_parallel_worker_timing(
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            mode=mode,
            queue_ms=0.0,
            wait_ms=0.0,
            exec_ms=total_ms,
            roundtrip_ms=total_ms,
            worker_pid=None,
        )
    )


def _append_frontend_parallel_layer_detail(
    frontend_parallel_layers: list[dict[str, Any]],
    *,
    layer_index: int,
    layer_mode: str,
    layer_policy_reason: str,
    module_names: Sequence[str],
    candidate_count: int,
    workers: int,
    timing_items: Sequence[Mapping[str, Any]],
    predicted_cost_total: float,
    effective_min_predicted_cost: float,
    stdlib_candidates: int,
    target_cost_per_worker: float,
    started_ns: int,
    finished_ns: int,
    fallback_reason: str | None = None,
) -> None:
    timing_summary = _summarize_worker_timing_items(timing_items)
    frontend_parallel_layers.append(
        _frontend_parallel_layer_detail(
            layer_index=layer_index,
            mode=layer_mode,
            policy_reason=layer_policy_reason,
            module_count=len(module_names),
            candidate_count=candidate_count,
            workers=workers,
            cache_hits=_layer_cache_hit_count(timing_items),
            predicted_cost_total=predicted_cost_total,
            effective_min_predicted_cost=effective_min_predicted_cost,
            stdlib_candidates=stdlib_candidates,
            target_cost_per_worker=target_cost_per_worker,
            timing_summary=timing_summary,
            started_ns=started_ns,
            finished_ns=finished_ns,
            fallback_reason=fallback_reason,
        )
    )


def _initialize_frontend_parallel_details(
    frontend_parallel_details: MutableMapping[str, Any],
    *,
    frontend_parallel_config: _FrontendParallelConfig,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    frontend_parallel_details["enabled"] = frontend_parallel_config.enabled
    frontend_parallel_details["workers"] = frontend_parallel_config.workers
    frontend_parallel_details["mode"] = (
        "process_pool_reused" if frontend_parallel_config.enabled else "serial"
    )
    frontend_parallel_details["reason"] = frontend_parallel_config.reason
    frontend_parallel_details["policy"] = _frontend_parallel_policy_payload(
        frontend_parallel_config
    )
    frontend_parallel_details["layers"] = []
    frontend_parallel_details["worker_timings"] = []
    return (
        cast(list[dict[str, Any]], frontend_parallel_details["layers"]),
        cast(list[dict[str, Any]], frontend_parallel_details["worker_timings"]),
    )


def _summarize_frontend_parallel_worker_timings(
    frontend_parallel_details: MutableMapping[str, Any],
    worker_timings: Sequence[Mapping[str, Any]],
) -> None:
    summary = _summarize_worker_timing_items(worker_timings)
    frontend_parallel_details["worker_summary"] = _worker_timing_summary_payload(
        summary
    )


def _append_frontend_serial_disabled_layer_detail(
    frontend_parallel_layers: list[dict[str, Any]],
    *,
    module_order: Sequence[str],
    serial_layer_state: _FrontendParallelLayerState,
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
    frontend_parallel_config: _FrontendParallelConfig,
    serial_layer_started_ns: int,
) -> None:
    serial_static_metrics = _frontend_layer_static_metrics(
        module_order,
        frontend_module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
    )
    _append_frontend_parallel_layer_detail(
        frontend_parallel_layers,
        layer_index=0,
        layer_mode="serial_disabled",
        layer_policy_reason=frontend_parallel_config.reason,
        module_names=module_order,
        candidate_count=len(module_order),
        workers=1,
        timing_items=serial_layer_state.recorded_worker_timings,
        predicted_cost_total=serial_static_metrics.predicted_cost_total,
        effective_min_predicted_cost=frontend_parallel_config.min_predicted_cost,
        stdlib_candidates=serial_static_metrics.stdlib_candidates,
        target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
        started_ns=serial_layer_started_ns,
        finished_ns=time.time_ns(),
    )


def _fallback_frontend_parallel_layer_to_serial(
    *,
    frontend_parallel_details: MutableMapping[str, Any],
    warnings: list[str],
    failure_detail: str,
) -> _FrontendParallelLayerState:
    frontend_parallel_details["reason"] = "worker_error_fallback_serial"
    warnings.append(
        f"Frontend parallel lowering fallback to serial for layer: {failure_detail}"
    )
    fallback_state = _fresh_frontend_parallel_layer_state()
    fallback_state.fallback_reason = failure_detail
    return fallback_state


def _frontend_parallel_result_error(
    module_name: str,
    result: Mapping[str, Any],
) -> str | None:
    if bool(result.get("ok")):
        return None
    return str(result.get("error", f"Failed to lower module {module_name}"))


def _frontend_parallel_worker_timing_inputs(
    result_timings: _FrontendModuleResultTimings,
    worker_timing: Mapping[str, Any] | None,
) -> tuple[float, float, float, float, str, int | None]:
    total_ms = result_timings.total_s * 1000.0
    queue_ms = float((worker_timing or {}).get("queue_ms", 0.0))
    wait_ms = float((worker_timing or {}).get("wait_ms", 0.0))
    exec_ms = float((worker_timing or {}).get("exec_ms", total_ms))
    roundtrip_ms = float(
        (worker_timing or {}).get("roundtrip_ms", max(queue_ms + wait_ms, exec_ms))
    )
    worker_mode = str((worker_timing or {}).get("mode", "parallel"))
    worker_pid_raw = (worker_timing or {}).get("worker_pid")
    worker_pid = worker_pid_raw if isinstance(worker_pid_raw, int) else None
    return queue_ms, wait_ms, exec_ms, roundtrip_ms, worker_mode, worker_pid


def _take_frontend_parallel_layer_result(
    layer_state: _FrontendParallelLayerState,
    module_name: str,
) -> dict[str, Any] | None:
    return layer_state.results.pop(module_name, None)


def _record_parallel_layer_module_timing(
    *,
    layer_state: _FrontendParallelLayerState,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    layer_index: int,
    module_name: str,
    module_path: Path,
    result_timings: _FrontendModuleResultTimings,
    worker_timing: Mapping[str, Any] | None,
) -> str:
    (
        queue_ms,
        wait_ms,
        exec_ms,
        roundtrip_ms,
        worker_mode,
        worker_pid,
    ) = _frontend_parallel_worker_timing_inputs(result_timings, worker_timing)
    layer_state.recorded_worker_timings.append(
        record_frontend_parallel_worker_timing(
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            mode=worker_mode,
            queue_ms=queue_ms,
            wait_ms=wait_ms,
            exec_ms=exec_ms,
            roundtrip_ms=roundtrip_ms,
            worker_pid=worker_pid,
        )
    )
    return worker_mode


def _frontend_serial_worker_mode(layer_mode: str) -> str:
    if layer_mode == "parallel":
        return "parallel_fallback_serial"
    if layer_mode == "serial_layer_policy":
        return "serial_layer_policy"
    return "serial_disabled"
