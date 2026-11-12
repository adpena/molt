from __future__ import annotations

import itertools
import json
import os
import sys
import time
import tracemalloc
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.config_resolution import _coerce_bool
from molt.cli.models import (
    ModuleReason,
    _BuildDiagnosticsContext,
    _FrontendTimingRecorderConfig,
)
from molt.cli.runtime_wasm_cache_diagnostics import (
    _runtime_wasm_cache_diagnostics_snapshot,
)
from molt.cli.runtime_wasm_build_timings import _runtime_wasm_build_timings_snapshot


def _build_reason_summary(
    module_reasons: Mapping[str, set[ModuleReason]],
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


def _phase_duration_map(
    phase_starts: Mapping[str, float], *, ended_at: float
) -> dict[str, float]:
    if not phase_starts:
        return {}
    starts = sorted(phase_starts.items(), key=lambda item: item[1])
    durations: dict[str, float] = {}
    for idx, (name, started) in enumerate(starts):
        if idx + 1 < len(starts):
            ended = starts[idx + 1][1]
        else:
            ended = ended_at
        durations[name] = round(max(0.0, ended - started), 6)
    return durations


def _build_phase_attribution(
    *,
    total_sec: float,
    phase_sec: Mapping[str, float],
    pipeline_stage_ms: Mapping[str, Any],
) -> dict[str, Any]:
    frontend_names = ("resolve_entry", "module_graph", "module_analysis", "ir_lowering")
    # Preserve every observed sequential phase. Backend preparation, runtime
    # admission and native linking must not disappear from attribution or be
    # charged to the frontend merely because codegen hit its cache.
    phases: dict[str, float] = {
        name: round(max(0.0, float(value)), 6) for name, value in phase_sec.items()
    }
    for name in frontend_names:
        phases.setdefault(name, 0.0)
    phases["frontend_lowering"] = round(sum(phases[name] for name in frontend_names), 6)
    phases["backend_codegen"] = round(
        max(0.0, float(phase_sec.get("backend_codegen", 0.0))), 6
    )
    phases["final_app_codegen"] = phases["backend_codegen"]
    phases.setdefault("seal", 0.0)
    for name, value in pipeline_stage_ms.items():
        if name in {
            "wasm_reloc_preflight",
            "wasm_link_total",
            "split_runtime_processing",
            "wasm_strip",
            "fail_closed_validation",
        } and isinstance(value, (int, float)):
            phases[name] = round(max(0.0, float(value)) / 1000.0, 6)
    for name in (
        "wasm_reloc_preflight",
        "wasm_link_total",
        "split_runtime_processing",
        "wasm_strip",
        "fail_closed_validation",
    ):
        phases.setdefault(name, 0.0)
    optimizer_wall_ms = pipeline_stage_ms.get("wasm_link_optimizer_wall_ms")
    if isinstance(optimizer_wall_ms, (int, float)):
        # This is the observed combined-artifact optimizer invocation, outside
        # split_runtime_processing. Missing means unobserved/disabled (including
        # final-link reuse); an observed early failure can have zero elapsed time.
        phases["wasm_link_optimizer"] = round(
            max(0.0, float(optimizer_wall_ms)) / 1000.0, 6
        )
    link_total = phases.get("wasm_link_total")
    if link_total is not None:
        children = sum(
            phases.get(name, 0.0)
            for name in (
                "split_runtime_processing",
                "wasm_strip",
                "fail_closed_validation",
                "wasm_link_optimizer",
            )
        )
        phases["wasm_link_core"] = round(max(0.0, link_total - children), 6)
    denominator = max(total_sec, 0.0)
    shares = {
        name: round(value / denominator, 8) if denominator > 0.0 else 0.0
        for name, value in phases.items()
    }
    ranked = sorted(phases, key=lambda name: (-phases[name], name))
    return {
        "schema_version": 1,
        "total_sec": round(total_sec, 6),
        "phase_sec": {name: phases[name] for name in sorted(phases)},
        "phase_share": {name: shares[name] for name in sorted(shares)},
        "ranked_phases": ranked,
    }


_DIAGNOSTICS_SEQUENCE = itertools.count()


def _resolve_build_diagnostics_path(
    output_spec: str,
    artifacts_root: Path,
) -> Path:
    """The diagnostics file; a spec ending in a separator names a directory
    that receives one file per build, so parallel builds never collide."""
    path = Path(output_spec).expanduser()
    if not path.is_absolute():
        path = artifacts_root / path
    if output_spec.endswith(("/", os.sep)):
        path.mkdir(parents=True, exist_ok=True)
        return path / f"{os.getpid()}-{next(_DIAGNOSTICS_SEQUENCE)}.json"
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
    known_module_count = diagnostics.get("known_module_count")
    compile_module_count = diagnostics.get("compile_module_count")
    reason_summary = diagnostics.get("module_reason_summary", {})
    frontend_parallel = diagnostics.get("frontend_parallel", {})
    frontend_modules_top = diagnostics.get("frontend_module_timings_top", [])
    allocations = diagnostics.get("allocations", {})
    runtime_wasm_cache = diagnostics.get("runtime_wasm_cache", {})
    runtime_wasm_build = diagnostics.get("runtime_wasm_build", {})
    print("Build diagnostics:", file=sys.stderr)
    compiler = diagnostics.get("compiler")
    program = diagnostics.get("program")
    if isinstance(compiler, dict) and isinstance(program, dict):
        print(
            f"- compiler: {compiler.get('cargo_profile')} | "
            f"program: {program.get('guest_profile')} ({program.get('backend')}/{program.get('target')}) | "
            f"runtime: {program.get('runtime_profile')}",
            file=sys.stderr,
        )
    if isinstance(total_sec, (int, float)):
        print(f"- total_sec: {total_sec:.6f}", file=sys.stderr)
    if isinstance(module_count, int):
        print(f"- modules: {module_count}", file=sys.stderr)
    if isinstance(known_module_count, int):
        print(f"- known_modules: {known_module_count}", file=sys.stderr)
    if isinstance(compile_module_count, int):
        print(f"- compile_modules: {compile_module_count}", file=sys.stderr)
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
    if isinstance(runtime_wasm_cache, dict):
        admit_attempts = runtime_wasm_cache.get("admit_attempts")
        admit_hits = runtime_wasm_cache.get("admit_hits")
        admit_misses = runtime_wasm_cache.get("admit_misses")
        admit_failures = runtime_wasm_cache.get("admit_failures")
        admit_hit_rate = runtime_wasm_cache.get("admit_hit_rate")
        publish_attempts = runtime_wasm_cache.get("publish_attempts")
        publish_successes = runtime_wasm_cache.get("publish_successes")
        publish_failures = runtime_wasm_cache.get("publish_failures")
        publish_success_rate = runtime_wasm_cache.get("publish_success_rate")
        if (
            isinstance(admit_attempts, int)
            and isinstance(admit_hits, int)
            and isinstance(admit_misses, int)
            and isinstance(admit_failures, int)
            and isinstance(admit_hit_rate, (int, float))
            and isinstance(publish_attempts, int)
            and isinstance(publish_successes, int)
            and isinstance(publish_failures, int)
            and isinstance(publish_success_rate, (int, float))
        ):
            print(
                "- runtime_wasm_cache: "
                f"admit_hits={admit_hits}/{admit_attempts} "
                f"admit_misses={admit_misses} "
                f"admit_failures={admit_failures} "
                f"admit_hit_rate={float(admit_hit_rate):.6f} "
                f"publish_successes={publish_successes}/{publish_attempts} "
                f"publish_failures={publish_failures} "
                f"publish_success_rate={float(publish_success_rate):.6f}",
                file=sys.stderr,
            )
        last_publish_failure = runtime_wasm_cache.get("last_publish_failure")
        if isinstance(last_publish_failure, str) and last_publish_failure:
            print(
                f"- runtime_wasm_cache.last_publish_failure: {last_publish_failure}",
                file=sys.stderr,
            )
    if isinstance(runtime_wasm_build, dict) and runtime_wasm_build:
        cargo_builds = runtime_wasm_build.get("cargo_compile_builds")
        cargo_reuses = runtime_wasm_build.get("cargo_compile_reuses")
        build_wall = runtime_wasm_build.get("cargo_compile_build_wall_s")
        if (
            isinstance(cargo_builds, int)
            and isinstance(cargo_reuses, int)
            and isinstance(build_wall, (int, float))
        ):
            print(
                "- runtime_wasm_build: "
                f"cargo_compiles={cargo_builds} (reused={cargo_reuses}) "
                f"cargo_compile_wall_s={float(build_wall):.6f}",
                file=sys.stderr,
            )
        phases = runtime_wasm_build.get("phases")
        if not summary_only and isinstance(phases, list):
            for item in phases:
                if not isinstance(item, dict):
                    continue
                print(
                    "- runtime_wasm_build.phase: "
                    f"{item.get('phase')} kind={item.get('kind')} "
                    f"mode={item.get('mode')} wall_s={float(item.get('wall_s', 0.0)):.6f}",
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
            worker_selection = policy.get("worker_selection")
            worker_memory_bytes = policy.get("worker_memory_bytes")
            worker_memory_headroom_bytes = policy.get("worker_memory_headroom_bytes")
            if (
                isinstance(worker_selection, str)
                and isinstance(worker_memory_bytes, int)
                and isinstance(worker_memory_headroom_bytes, int)
            ):
                mib = 1024 * 1024
                resource_fields = [
                    f"selection={worker_selection}",
                    f"worker_memory_mib={worker_memory_bytes / mib:.1f}",
                    "worker_memory_headroom_mib="
                    f"{worker_memory_headroom_bytes / mib:.1f}",
                ]
                requested = policy.get("worker_requested")
                resource_fields.append(
                    "requested=auto" if requested is None else f"requested={requested}"
                )
                for key, label in (
                    ("worker_cpu_count", "cpu_count"),
                    ("worker_memory_ceiling", "memory_ceiling"),
                ):
                    value = policy.get(key)
                    if isinstance(value, int):
                        resource_fields.append(f"{label}={value}")
                for key, label in (
                    ("system_total_memory_bytes", "system_total_mib"),
                    ("system_available_memory_bytes", "system_available_mib"),
                ):
                    value = policy.get(key)
                    if isinstance(value, int):
                        resource_fields.append(f"{label}={value / mib:.1f}")
                print(
                    "- frontend_parallel.resources: " + " ".join(resource_fields),
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
    if diagnostics_path is not None:
        print(f"- wrote: {diagnostics_path}", file=sys.stderr)


def _duration_ms_from_ns(start_ns: Any, end_ns: Any) -> float:
    if not isinstance(start_ns, int):
        return 0.0
    if not isinstance(end_ns, int):
        return 0.0
    delta_ns = end_ns - start_ns
    if delta_ns <= 0:
        return 0.0
    return round(delta_ns / 1_000_000.0, 6)


def _emit_build_diagnostics_if_present(
    *,
    diagnostics_enabled: bool,
    build_diagnostics_payload: Callable[[], tuple[dict[str, Any] | None, Path | None]],
    json_output: bool,
    verbosity: str,
    artifact_analysis: Callable[[], dict[str, Any] | None] | None = None,
) -> tuple[dict[str, Any] | None, str | None]:
    """Attempt terminal diagnostics once, retaining explicit reporting failures."""
    if not diagnostics_enabled:
        return None, None
    diagnostics_payload = None
    payload_validated = False
    try:
        # Requested artifact inspection belongs before the builder's cutoff.
        artifacts = artifact_analysis() if artifact_analysis is not None else None
        diagnostics_payload, diagnostics_path = build_diagnostics_payload()
        if artifacts is not None:
            from molt.cli.binary_image_analysis import (
                _merge_binary_image_analysis_stage,
            )

            _merge_binary_image_analysis_stage(
                diagnostics_payload, "artifacts", artifacts
            )
        # Validate every route before publication or primary JSON embedding.
        # A rejected destination must not discard a valid captured payload.
        json.dumps(diagnostics_payload, allow_nan=False)
        payload_validated = True
        _emit_build_diagnostics(
            diagnostics=diagnostics_payload,
            diagnostics_path=diagnostics_path,
            json_output=json_output,
            verbosity=verbosity,
        )
    except (OSError, TypeError, ValueError) as exc:
        # Serialization failures may mean the payload itself cannot be emitted.
        # Never send invalid diagnostics through the primary JSON error path.
        if not payload_validated:
            diagnostics_payload = None
        return (
            diagnostics_payload,
            f"Build diagnostics failed: {type(exc).__name__}: {exc}",
        )
    return diagnostics_payload, None


def _emit_build_diagnostics_for_result(
    result: int,
    *,
    diagnostics_enabled: bool,
    build_diagnostics_payload: Callable[[], tuple[dict[str, Any] | None, Path | None]],
    json_output: bool,
    verbosity: str,
) -> int:
    """Flush diagnostics without replacing an already-emitted primary failure."""
    _, diagnostics_error = _emit_build_diagnostics_if_present(
        diagnostics_enabled=diagnostics_enabled,
        build_diagnostics_payload=build_diagnostics_payload,
        json_output=json_output,
        verbosity=verbosity,
    )
    if diagnostics_error is not None:
        print(diagnostics_error, file=sys.stderr)
    return result


def _wasm_link_operation_counts(
    pipeline_stage_ms: Mapping[str, object],
) -> dict[str, int]:
    return dict(
        sorted(
            {
                str(name): int(value)
                for name, value in pipeline_stage_ms.items()
                if (
                    str(name).startswith("split_app_")
                    or str(name).startswith("runtime_tree_shake_cache_")
                    or str(name).startswith("wasm_whole_artifact_")
                    or (
                        str(name).startswith("wasm_link_optimizer_")
                        and not str(name).endswith("_wall_ms")
                    )
                    or name
                    in {
                        "wasm_reloc_preflight_invocations",
                        "wasm_link_timeouts",
                        "wasm_link_failures",
                        "wasm_link_identity_errors",
                    }
                )
                and isinstance(value, (int, float))
                and not isinstance(value, bool)
                and float(value).is_integer()
            }.items()
        )
    )


def _build_build_diagnostics_payload(
    diagnostics_context: _BuildDiagnosticsContext,
) -> tuple[dict[str, Any] | None, Path | None]:
    if not diagnostics_context.diagnostics_enabled:
        return None, None
    completed_at = time.perf_counter()
    module_reason_map = {
        name: sorted(reasons)
        for name, reasons in sorted(diagnostics_context.module_reasons.items())
    }
    total_sec = round(completed_at - diagnostics_context.diagnostics_start, 6)
    phase_sec = _phase_duration_map(
        diagnostics_context.phase_starts, ended_at=completed_at
    )
    payload: dict[str, Any] = {
        "enabled": True,
        "entry": {
            "module": diagnostics_context.entry_module,
            "path": os.fspath(diagnostics_context.entry_path),
        },
        "timing_scope": "build_preamble_to_terminal_result",
        "total_sec": total_sec,
        "phase_sec": phase_sec,
        "module_count": len(diagnostics_context.module_graph),
        "module_graph_operation_counts": dict(
            sorted(diagnostics_context.module_graph_operation_counts.items())
        ),
        "module_reason_summary": _build_reason_summary(
            diagnostics_context.module_reasons
        ),
        "module_reasons": module_reason_map,
    }
    closure_payload = diagnostics_context.binary_image_closure
    if closure_payload is not None:
        payload["binary_image_closure"] = dict(closure_payload)
        image_payload = closure_payload.get("image")
        if isinstance(image_payload, Mapping):
            payload["binary_image"] = dict(image_payload)
        known_modules = closure_payload.get("known_modules")
        if isinstance(known_modules, list):
            payload["known_module_count"] = len(known_modules)
        compile_modules = closure_payload.get("compile_modules")
        if isinstance(compile_modules, list):
            payload["compile_module_count"] = len(compile_modules)
    elif diagnostics_context.image_scope is not None:
        payload["binary_image"] = diagnostics_context.image_scope.diagnostic_payload()
    if diagnostics_context.binary_image_analysis is not None:
        payload["binary_image_analysis"] = dict(
            diagnostics_context.binary_image_analysis
        )
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
    frontend_parallel_payload = dict(diagnostics_context.frontend_parallel_details)
    payload["frontend_parallel"] = frontend_parallel_payload
    pipeline_stage_ms = frontend_parallel_payload.get("pipeline_stage_ms", {})
    if not isinstance(pipeline_stage_ms, Mapping):
        pipeline_stage_ms = {}
    payload["phase_attribution"] = _build_phase_attribution(
        total_sec=total_sec,
        phase_sec=phase_sec,
        pipeline_stage_ms=pipeline_stage_ms,
    )
    wasm_link_operation_counts = _wasm_link_operation_counts(pipeline_stage_ms)
    if wasm_link_operation_counts:
        payload["wasm_link_operation_counts"] = wasm_link_operation_counts
    lowering_cache_summary = _frontend_lowering_cache_summary(frontend_parallel_payload)
    if lowering_cache_summary is not None:
        payload["frontend_lowering_cache"] = lowering_cache_summary
    runtime_wasm_cache = _runtime_wasm_cache_diagnostics_snapshot()
    if runtime_wasm_cache is not None:
        payload["runtime_wasm_cache"] = runtime_wasm_cache
    runtime_wasm_build = _runtime_wasm_build_timings_snapshot()
    if runtime_wasm_build is not None:
        payload["runtime_wasm_build"] = runtime_wasm_build
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


def _frontend_lowering_cache_summary(
    frontend_parallel_details: Mapping[str, Any],
) -> dict[str, Any] | None:
    worker_timings = frontend_parallel_details.get("worker_timings")
    if not isinstance(worker_timings, list):
        return None
    hits = 0
    reused_ms = 0.0
    relowered_ms = 0.0
    for raw_item in worker_timings:
        if not isinstance(raw_item, Mapping):
            continue
        mode = raw_item.get("mode")
        if mode in {"parallel_cache_hit", "serial_cache_hit"}:
            hits += 1
            reused_ms += float(raw_item.get("reused_ms", 0.0))
        else:
            relowered_ms += float(raw_item.get("exec_ms", 0.0))
    observed = sum(1 for item in worker_timings if isinstance(item, Mapping))
    if observed == 0:
        return None
    misses = max(0, observed - hits)
    # Machine-readable effectiveness signal: the fraction of lowered modules
    # served from the persistent cache. A warm rebuild should report a hit_rate
    # near 1.0; a regression that silently disables persistence collapses it
    # toward 0.0, which the effectiveness gate
    # (test_cli_module_frontend_shared_cache) asserts against.
    hit_rate = round(hits / observed, 6) if observed else 0.0
    return {
        "hits": hits,
        "misses": misses,
        "observed": observed,
        "hit_rate": hit_rate,
        "reused_s": round(reused_ms / 1000.0, 6),
        "relowered_s": round(relowered_ms / 1000.0, 6),
        "message": (
            f"lowering cache: {hits}/{observed} hits (hit_rate={hit_rate:.3f}), "
            f"{reused_ms / 1000.0:.6f}s reused / "
            f"{relowered_ms / 1000.0:.6f}s re-lowered"
        ),
    }


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
