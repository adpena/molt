from __future__ import annotations

import ast
import contextlib
import functools
import os
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import Any, Callable, Collection, Mapping, Sequence, cast

from molt.compat import CompatibilityError
from molt.frontend import SimpleTIRGenerator
from molt.type_facts import TypeFacts

from molt.cli import frontend_parallel as _frontend_parallel
from molt.cli import frontend_integration as _frontend_integration
from molt.cli import frontend_worker as _frontend_worker
from molt.cli.models import (
    BuildProfile,
    FallbackPolicy,
    ParseCodec,
    TypeHintPolicy,
    _EntryFrontendLoweringContext,
    _FrontendIntegrationState,
    _FrontendLayerExecutionContext,
    _FrontendLayerPlan,
    _FrontendLayerRunResult,
    _FrontendLayerRuntimeHooks,
    _FrontendModuleResultTimings,
    _FrontendParallelConfig,
    _FrontendParallelLayerState,
    _MidendDiagnosticsState,
    _ModuleGraphMetadata,
    _ParallelWorkerSubmission,
    _PreparedFrontendRunTicket,
    _ScopedLoweringInputs,
    _SerialFrontendLoweringContext,
    _SerialFrontendLoweringHooks,
)
from molt.cli.module_graph import ModuleSyntaxErrorInfo
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import (
    _ModuleSourceCatalog,
    _read_module_source,
)
from molt.cli.module_cache import (
    _build_scoped_known_classes_snapshot,
    _write_persisted_module_lowering,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.target_python import (
    TargetPythonVersion,
    _parse_source_for_target,
)







def _lower_entry_module_as_main(
    *,
    lowering_context: _EntryFrontendLoweringContext,
    integration_state: _FrontendIntegrationState,
    diagnostics_state: _MidendDiagnosticsState,
    record_frontend_timing: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
) -> _CliFailure | None:
    try:
        source = _read_module_source(lowering_context.entry_path)
    except (SyntaxError, UnicodeDecodeError) as exc:
        return fail(
            f"Syntax error in {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )
    except OSError as exc:
        return fail(
            f"Failed to read module {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )
    try:
        tree = _parse_source_for_target(
            source,
            filename=str(lowering_context.entry_path),
            target_python=lowering_context.target_python,
        )
    except SyntaxError as exc:
        return fail(
            f"Syntax error in {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )

    main_gen = SimpleTIRGenerator(
        parse_codec=lowering_context.parse_codec,
        type_hint_policy=lowering_context.type_hint_policy,
        fallback_policy=lowering_context.fallback_policy,
        source_path=str(lowering_context.entry_path),
        type_facts=lowering_context.type_facts,
        type_facts_module=lowering_context.entry_module,
        module_name="__main__",
        module_spec_name=lowering_context.entry_module,
        entry_module=None,
        enable_phi=lowering_context.enable_phi,
        known_modules=set(lowering_context.known_modules),
        known_classes=cast(Any, lowering_context.known_classes),
        stdlib_allowlist=set(lowering_context.stdlib_allowlist),
        known_func_defaults=lowering_context.known_func_defaults,
        known_func_kinds=lowering_context.known_func_kinds,
        module_chunking=lowering_context.module_chunking,
        module_chunk_max_ops=lowering_context.module_chunk_max_ops,
        optimization_profile=cast(BuildProfile, lowering_context.optimization_profile),
        pgo_hot_functions=set(lowering_context.pgo_hot_function_names),
    )
    main_frontend_start = time.perf_counter()
    main_visit_s = 0.0
    main_lower_s = 0.0
    try:
        main_visit_start = time.perf_counter()
        with _frontend_worker._phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name="frontend visit (__main__)",
        ):
            main_gen.visit(tree)
        main_visit_s = time.perf_counter() - main_visit_start
        main_lower_start = time.perf_counter()
        with _frontend_worker._phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name="frontend IR lowering (__main__)",
        ):
            main_ir = main_gen.to_json()
        main_lower_s = time.perf_counter() - main_lower_start
    except TimeoutError as exc:
        record_frontend_timing(
            module_name="__main__",
            module_path=lowering_context.entry_path,
            visit_s=main_visit_s,
            lower_s=main_lower_s,
            total_s=time.perf_counter() - main_frontend_start,
            timed_out=True,
            detail=str(exc),
        )
        return fail(str(exc), json_output, command="build")
    except CompatibilityError as exc:
        return fail(str(exc), json_output, command="build")

    record_frontend_timing(
        module_name="__main__",
        module_path=lowering_context.entry_path,
        visit_s=main_visit_s,
        lower_s=main_lower_s,
        total_s=time.perf_counter() - main_frontend_start,
    )
    main_init = SimpleTIRGenerator.module_init_symbol("__main__")
    local_code_ids = dict(main_gen.func_code_ids)
    if "molt_main" in local_code_ids:
        local_code_ids[main_init] = local_code_ids.pop("molt_main")
    local_id_to_symbol = {code_id: symbol for symbol, code_id in local_code_ids.items()}
    try:
        _frontend_integration._remap_module_code_ops_with_state(
            integration_state,
            "__main__",
            main_ir["functions"],
            local_id_to_symbol,
        )
    except ValueError as exc:
        return fail(str(exc), json_output, command="build")
    for func in main_ir["functions"]:
        if func["name"] == "molt_main":
            func["name"] = main_init
    integration_state.functions.extend(main_ir["functions"])
    _frontend_integration._accumulate_midend_diagnostics_with_state(
        diagnostics_state,
        "__main__",
        policy_outcomes_by_func=dict(main_gen.midend_policy_outcomes_by_function),
        pass_stats_by_func=dict(main_gen.midend_pass_stats_by_function),
    )
    return None


def _prepare_frontend_execution(
    *,
    syntax_error_modules: dict[str, "ModuleSyntaxErrorInfo"],
    module_graph: Mapping[str, Path],
    module_source_catalog: _ModuleSourceCatalog,
    project_root: Path,
    module_resolution_cache: "_ModuleResolutionCache",
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: BuildProfile,
    pgo_hot_function_names: set[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    module_path_stats: dict[str, os.stat_result | None],
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs,
    dirty_lowering_modules: set[str],
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
    known_classes: dict[str, Any],
    module_trees: dict[str, ast.AST],
    generated_module_source_paths: Mapping[str, str],
    frontend_phase_timeout: float | None,
    record_frontend_timing: Callable[..., None],
    fail: Callable[..., int],
    json_output: bool,
    warnings: list[str],
    frontend_parallel_details: dict[str, Any],
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    midend_policy_outcomes_by_function: dict[str, dict[str, Any]],
    midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]],
    target_python: TargetPythonVersion,
) -> tuple[
    _FrontendLayerExecutionContext,
    _FrontendLayerRuntimeHooks,
    _FrontendIntegrationState,
    _MidendDiagnosticsState,
]:
    frontend_layer_execution_context = _FrontendLayerExecutionContext(
        syntax_error_modules=syntax_error_modules,
        module_graph=module_graph,
        module_source_catalog=module_source_catalog,
        project_root=project_root,
        module_resolution_cache=module_resolution_cache,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        module_deps=module_deps,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        module_graph_metadata=module_graph_metadata,
        path_stat_by_module=module_path_stats,
        module_chunking=module_chunking,
        scoped_lowering_inputs=scoped_lowering_inputs,
        dirty_lowering_modules=dirty_lowering_modules,
        frontend_module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
        known_classes=known_classes,
        target_python=target_python,
    )
    serial_frontend_lowering_context = _SerialFrontendLoweringContext(
        syntax_error_modules=syntax_error_modules,
        module_trees=module_trees,
        module_source_catalog=module_source_catalog,
        generated_module_source_paths=generated_module_source_paths,
        module_resolution_cache=module_resolution_cache,
        project_root=project_root,
        dirty_lowering_modules=dirty_lowering_modules,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        module_deps=module_deps,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        scoped_lowering_inputs=scoped_lowering_inputs,
        module_graph_metadata=module_graph_metadata,
        module_path_stats=module_path_stats,
        known_classes=known_classes,
        frontend_phase_timeout=frontend_phase_timeout,
        target_python=target_python,
    )
    serial_frontend_lowering_hooks = _SerialFrontendLoweringHooks(
        record_frontend_timing=record_frontend_timing,
        fail=fail,
        json_output=json_output,
    )
    integration_state = _FrontendIntegrationState(
        functions=[],
        known_classes=known_classes,
    )
    midend_diagnostics_state = _MidendDiagnosticsState(
        policy_outcomes_by_function=midend_policy_outcomes_by_function,
        pass_stats_by_function=midend_pass_stats_by_function,
    )
    def _run_serial_frontend_lower(
        module_name: str,
        module_path: Path,
    ) -> tuple[
        dict[str, Any] | None,
        _FrontendModuleResultTimings | None,
        _CliFailure | None,
    ]:
        return _frontend_worker._run_serial_frontend_lower_with_context(
            module_name,
            module_path,
            lowering_context=serial_frontend_lowering_context,
            lowering_hooks=serial_frontend_lowering_hooks,
        )

    frontend_layer_runtime_hooks = _FrontendLayerRuntimeHooks(
        warnings=warnings,
        frontend_parallel_details=frontend_parallel_details,
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        record_frontend_timing=record_frontend_timing,
        integrate_module_frontend_result=functools.partial(
            _frontend_integration._integrate_module_frontend_result_with_state,
            integration_state,
        ),
        accumulate_midend_diagnostics=functools.partial(
            _frontend_integration._accumulate_midend_diagnostics_with_state,
            midend_diagnostics_state,
        ),
        fail=fail,
        json_output=json_output,
        run_serial_frontend_lower=_run_serial_frontend_lower,
    )
    return (
        frontend_layer_execution_context,
        frontend_layer_runtime_hooks,
        integration_state,
        midend_diagnostics_state,
    )


def _run_frontend_parallel_enabled_layers(
    module_layers: Sequence[Sequence[str]],
    *,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_config: _FrontendParallelConfig,
    frontend_parallel_layers: list[dict[str, Any]],
) -> _CliFailure | None:
    parallel_pool_usable = True
    with ProcessPoolExecutor(max_workers=frontend_parallel_config.workers) as executor:
        for layer_index, layer in enumerate(module_layers):
            layer_started_ns = time.time_ns()
            layer_run_result, layer_error = _run_frontend_layer(
                layer,
                layer_index=layer_index,
                executor=executor,
                execution_context=execution_context,
                runtime_hooks=runtime_hooks,
                frontend_parallel_config=frontend_parallel_config,
                parallel_pool_usable=parallel_pool_usable,
            )
            if layer_error is not None:
                return layer_error
            assert layer_run_result is not None
            layer_state = layer_run_result.layer_state
            layer_plan = layer_run_result.layer_plan
            parallel_pool_usable = layer_run_result.parallel_pool_usable
            _frontend_parallel._append_frontend_parallel_layer_detail(
                frontend_parallel_layers,
                layer_index=layer_index,
                layer_mode=layer_plan.mode,
                layer_policy_reason=layer_plan.policy_reason,
                module_names=layer,
                candidate_count=len(layer_plan.candidates),
                workers=layer_plan.workers,
                timing_items=layer_state.recorded_worker_timings,
                predicted_cost_total=layer_plan.predicted_cost_total,
                effective_min_predicted_cost=layer_plan.effective_min_predicted_cost,
                stdlib_candidates=layer_plan.stdlib_candidates,
                target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
                started_ns=layer_started_ns,
                finished_ns=time.time_ns(),
                fallback_reason=layer_state.fallback_reason,
            )
    return None


def _run_frontend_pipeline(
    *,
    prepared_frontend_run_ticket: _PreparedFrontendRunTicket,
) -> _CliFailure | None:
    frontend_parallel_config = prepared_frontend_run_ticket.frontend_parallel_config
    frontend_parallel_layers = prepared_frontend_run_ticket.frontend_parallel_layers
    frontend_layer_execution_context = (
        prepared_frontend_run_ticket.frontend_layer_execution_context
    )
    frontend_layer_runtime_hooks = (
        prepared_frontend_run_ticket.frontend_layer_runtime_hooks
    )
    if frontend_parallel_config.enabled:
        frontend_layer_error = _run_frontend_parallel_enabled_layers(
            prepared_frontend_run_ticket.module_layers,
            execution_context=frontend_layer_execution_context,
            runtime_hooks=frontend_layer_runtime_hooks,
            frontend_parallel_config=frontend_parallel_config,
            frontend_parallel_layers=frontend_parallel_layers,
        )
    else:
        frontend_layer_error = _run_frontend_serial_disabled_layers(
            prepared_frontend_run_ticket.module_order,
            execution_context=frontend_layer_execution_context,
            runtime_hooks=frontend_layer_runtime_hooks,
            frontend_parallel_layers=frontend_parallel_layers,
            frontend_parallel_config=frontend_parallel_config,
        )
    if frontend_layer_error is not None:
        return frontend_layer_error
    _frontend_parallel._summarize_frontend_parallel_worker_timings(
        prepared_frontend_run_ticket.frontend_parallel_details,
        prepared_frontend_run_ticket.frontend_parallel_worker_timings,
    )
    return None


def _run_frontend_serial_disabled_layers(
    module_order: Sequence[str],
    *,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_layers: list[dict[str, Any]],
    frontend_parallel_config: _FrontendParallelConfig,
) -> _CliFailure | None:
    serial_layer_started_ns = time.time_ns()
    serial_layer_state = _frontend_parallel._fresh_frontend_parallel_layer_state()
    serial_error = _run_frontend_serial_layer_modules(
        module_order,
        module_graph=execution_context.module_graph,
        run_serial_frontend_lower=runtime_hooks.run_serial_frontend_lower,
        record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
        integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
        accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
        fail=runtime_hooks.fail,
        json_output=runtime_hooks.json_output,
        layer_state=serial_layer_state,
        layer_index=0,
        serial_mode="serial_disabled",
    )
    if serial_error is not None:
        return serial_error
    _frontend_parallel._append_frontend_serial_disabled_layer_detail(
        frontend_parallel_layers,
        module_order=module_order,
        serial_layer_state=serial_layer_state,
        frontend_module_costs=execution_context.frontend_module_costs,
        stdlib_like_by_module=execution_context.stdlib_like_by_module,
        frontend_parallel_config=frontend_parallel_config,
        serial_layer_started_ns=serial_layer_started_ns,
    )
    return None


def _run_frontend_parallel_layer_batches(
    candidates: Sequence[str],
    *,
    layer_workers: int,
    executor: Any,
    known_classes_snapshot_source: Mapping[str, Any],
    module_graph: Mapping[str, Path],
    module_source_catalog: _ModuleSourceCatalog,
    project_root: Path | None,
    module_resolution_cache: _ModuleResolutionCache,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    path_stat_by_module: Mapping[str, os.stat_result | None] | None,
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs | None,
    dirty_lowering_modules: Collection[str],
    target_python: TargetPythonVersion,
) -> tuple[_FrontendParallelLayerState, str | None, str | None]:
    layer_state = _frontend_parallel._fresh_frontend_parallel_layer_state()
    known_classes_snapshot = _frontend_parallel._known_classes_snapshot_copy(
        known_classes_snapshot_source
    )
    scoped_known_classes_by_module = _build_scoped_known_classes_snapshot(
        candidates,
        module_deps=module_deps,
        module_dep_closures=module_dep_closures,
        known_classes_snapshot=known_classes_snapshot,
    )
    for batch_start in range(0, len(candidates), layer_workers):
        batch = list(candidates[batch_start : batch_start + layer_workers])
        worker_submissions: list[_ParallelWorkerSubmission] = []
        (
            cached_results,
            worker_payloads,
            context_digest_by_module,
            batch_error,
        ) = _frontend_worker._prepare_frontend_parallel_batch(
            batch,
            module_graph=module_graph,
            module_source_catalog=module_source_catalog,
            project_root=project_root,
            known_classes_snapshot=known_classes_snapshot,
            module_resolution_cache=module_resolution_cache,
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            type_facts=type_facts,
            enable_phi=enable_phi,
            known_modules=known_modules,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            module_deps=module_deps,
            module_chunk_max_ops=module_chunk_max_ops,
            optimization_profile=optimization_profile,
            pgo_hot_function_names=pgo_hot_function_names,
            known_modules_sorted=known_modules_sorted,
            stdlib_allowlist_sorted=stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            module_dep_closures=module_dep_closures,
            module_graph_metadata=module_graph_metadata,
            path_stat_by_module=path_stat_by_module,
            module_chunking=module_chunking,
            scoped_lowering_inputs=scoped_lowering_inputs,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
            dirty_lowering_modules=dirty_lowering_modules,
            target_python=target_python,
        )
        if batch_error is not None:
            return layer_state, batch_error, None
        layer_state.context_digests.update(context_digest_by_module)
        for module_name, cached_result in cached_results.items():
            _frontend_parallel._record_parallel_cached_module_result(
                layer_state,
                module_name,
                cached_result,
            )
        for module_name, payload in worker_payloads:
            worker_submissions.append(
                _ParallelWorkerSubmission(
                    module_name=module_name,
                    submitted_ns=time.time_ns(),
                    future=executor.submit(_frontend_worker._frontend_lower_module_worker, payload),
                )
            )
        for submission in worker_submissions:
            module_name = submission.module_name
            future = submission.future
            try:
                result = future.result()
                received_ns = time.time_ns()
                _frontend_parallel._record_parallel_worker_result(
                    layer_state,
                    module_name=module_name,
                    result=result,
                    submitted_ns=submission.submitted_ns,
                    received_ns=received_ns,
                )
            except Exception as exc:
                return layer_state, None, f"{module_graph[module_name]}: {exc}"
    return layer_state, None, None


def _write_parallel_persisted_module_lowering(
    *,
    project_root: Path | None,
    module_path: Path,
    module_name: str,
    worker_mode: str,
    context_digest: str | None,
    result: Mapping[str, Any],
    target_python: TargetPythonVersion,
) -> None:
    if (
        project_root is None
        or worker_mode == "parallel_cache_hit"
        or context_digest is None
    ):
        return
    with contextlib.suppress(OSError):
        _write_persisted_module_lowering(
            project_root,
            module_path,
            module_name=module_name,
            is_package=module_path.name == "__init__.py",
            context_digest=context_digest,
            result={key: value for key, value in result.items() if key != "ok"},
            target_python=target_python,
        )


def _consume_frontend_module_result(
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    *,
    result_timings: _FrontendModuleResultTimings | None = None,
    record_frontend_timing: Callable[..., None] | None,
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
) -> _CliFailure | None:
    timings = result_timings or _frontend_parallel._frontend_result_timings(result)
    if record_frontend_timing is not None:
        record_frontend_timing(
            module_name=module_name,
            module_path=module_path,
            visit_s=timings.visit_s,
            lower_s=timings.lower_s,
            total_s=timings.total_s,
        )
    integration_error = integrate_module_frontend_result(
        module_name,
        ir_functions=cast(list[dict[str, Any]], result["functions"]),
        func_code_ids=cast(dict[str, int], result["func_code_ids"]),
        local_class_names=cast(list[str], result["local_class_names"]),
        local_classes=cast(dict[str, Any], result["local_classes"]),
    )
    if integration_error is not None:
        return fail(integration_error, json_output, command="build")
    accumulate_midend_diagnostics(
        module_name,
        policy_outcomes_by_func=cast(
            dict[str, dict[str, Any]],
            result.get("midend_policy_outcomes_by_function", {}),
        ),
        pass_stats_by_func=cast(
            dict[str, dict[str, dict[str, Any]]],
            result.get("midend_pass_stats_by_function", {}),
        ),
    )
    return None


def _consume_frontend_parallel_layer_result(
    *,
    layer_state: _FrontendParallelLayerState,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    record_frontend_timing: Callable[..., None],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    project_root: Path | None,
    layer_index: int,
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    target_python: TargetPythonVersion,
) -> _CliFailure | None:
    result_error = _frontend_parallel._frontend_parallel_result_error(module_name, result)
    if result_error is not None:
        return fail(result_error, json_output, command="build")
    result_timings = _frontend_parallel._frontend_result_timings(result)
    worker_mode = _frontend_parallel._record_parallel_layer_module_timing(
        layer_state=layer_state,
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        layer_index=layer_index,
        module_name=module_name,
        module_path=module_path,
        result_timings=result_timings,
        worker_timing=layer_state.worker_timings_by_module.get(module_name),
    )
    _write_parallel_persisted_module_lowering(
        project_root=project_root,
        module_path=module_path,
        module_name=module_name,
        worker_mode=worker_mode,
        context_digest=layer_state.context_digests.get(module_name),
        result=result,
        target_python=target_python,
    )
    return _consume_frontend_module_result(
        module_name=module_name,
        module_path=module_path,
        result=result,
        result_timings=result_timings,
        record_frontend_timing=record_frontend_timing,
        integrate_module_frontend_result=integrate_module_frontend_result,
        accumulate_midend_diagnostics=accumulate_midend_diagnostics,
        fail=fail,
        json_output=json_output,
    )


def _consume_frontend_serial_layer_result(
    *,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    layer_state: _FrontendParallelLayerState,
    layer_index: int,
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    result_timings: _FrontendModuleResultTimings,
    serial_mode: str,
) -> _CliFailure | None:
    _frontend_parallel._record_serial_frontend_worker_timing(
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        recorded_worker_timings=layer_state.recorded_worker_timings,
        layer_index=layer_index,
        module_name=module_name,
        module_path=module_path,
        mode=serial_mode,
        total_s=result_timings.total_s,
    )
    return _consume_frontend_module_result(
        module_name=module_name,
        module_path=module_path,
        result=result,
        result_timings=result_timings,
        record_frontend_timing=None,
        integrate_module_frontend_result=integrate_module_frontend_result,
        accumulate_midend_diagnostics=accumulate_midend_diagnostics,
        fail=fail,
        json_output=json_output,
    )


def _run_frontend_serial_layer_modules(
    module_names: Sequence[str],
    *,
    module_graph: Mapping[str, Path],
    run_serial_frontend_lower: Callable[
        [str, Path],
        tuple[
            dict[str, Any] | None,
            _FrontendModuleResultTimings | None,
            _CliFailure | None,
        ],
    ],
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    layer_state: _FrontendParallelLayerState,
    layer_index: int,
    serial_mode: str,
) -> _CliFailure | None:
    for module_name in module_names:
        module_path = module_graph[module_name]
        result, result_timings, lower_error = run_serial_frontend_lower(
            module_name,
            module_path,
        )
        if lower_error is not None:
            return lower_error
        assert result is not None
        assert result_timings is not None
        consume_error = _consume_frontend_serial_layer_result(
            record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
            integrate_module_frontend_result=integrate_module_frontend_result,
            accumulate_midend_diagnostics=accumulate_midend_diagnostics,
            fail=fail,
            json_output=json_output,
            layer_state=layer_state,
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            result=result,
            result_timings=result_timings,
            serial_mode=serial_mode,
        )
        if consume_error is not None:
            return consume_error
    return None


def _run_frontend_layer(
    layer: Sequence[str],
    *,
    layer_index: int,
    executor: Any | None,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_config: _FrontendParallelConfig,
    parallel_pool_usable: bool,
) -> tuple[_FrontendLayerRunResult | None, _CliFailure | None]:
    layer_state = _frontend_parallel._fresh_frontend_parallel_layer_state()
    layer_plan = _frontend_parallel._frontend_layer_plan(
        layer,
        syntax_error_modules=execution_context.syntax_error_modules,
        module_source_catalog=execution_context.module_source_catalog,
        module_graph=execution_context.module_graph,
        module_deps=execution_context.module_deps,
        frontend_module_costs=execution_context.frontend_module_costs,
        stdlib_like_by_module=execution_context.stdlib_like_by_module,
        frontend_parallel_config=frontend_parallel_config,
        parallel_pool_usable=parallel_pool_usable,
    )
    if layer_plan.mode == "parallel":
        assert executor is not None
        layer_state, batch_error, layer_failure_detail = (
            _run_frontend_parallel_layer_batches(
                layer_plan.candidates,
                layer_workers=layer_plan.workers,
                executor=executor,
                known_classes_snapshot_source=execution_context.known_classes,
                module_graph=execution_context.module_graph,
                module_source_catalog=execution_context.module_source_catalog,
                project_root=execution_context.project_root,
                module_resolution_cache=execution_context.module_resolution_cache,
                parse_codec=execution_context.parse_codec,
                type_hint_policy=execution_context.type_hint_policy,
                fallback_policy=execution_context.fallback_policy,
                type_facts=execution_context.type_facts,
                enable_phi=execution_context.enable_phi,
                known_modules=execution_context.known_modules,
                stdlib_allowlist=execution_context.stdlib_allowlist,
                known_func_defaults=execution_context.known_func_defaults,
                known_func_kinds=execution_context.known_func_kinds,
                module_deps=execution_context.module_deps,
                module_chunk_max_ops=execution_context.module_chunk_max_ops,
                optimization_profile=execution_context.optimization_profile,
                pgo_hot_function_names=execution_context.pgo_hot_function_names,
                known_modules_sorted=execution_context.known_modules_sorted,
                stdlib_allowlist_sorted=execution_context.stdlib_allowlist_sorted,
                pgo_hot_function_names_sorted=execution_context.pgo_hot_function_names_sorted,
                module_dep_closures=execution_context.module_dep_closures,
                module_graph_metadata=execution_context.module_graph_metadata,
                path_stat_by_module=execution_context.path_stat_by_module,
                module_chunking=execution_context.module_chunking,
                scoped_lowering_inputs=execution_context.scoped_lowering_inputs,
                dirty_lowering_modules=execution_context.dirty_lowering_modules,
                target_python=execution_context.target_python,
            )
        )
        if batch_error is not None:
            return None, runtime_hooks.fail(
                batch_error, runtime_hooks.json_output, command="build"
            )
        if layer_failure_detail is not None:
            layer_state = _frontend_parallel._fallback_frontend_parallel_layer_to_serial(
                frontend_parallel_details=runtime_hooks.frontend_parallel_details,
                warnings=runtime_hooks.warnings,
                failure_detail=layer_failure_detail,
            )
            layer_plan = _FrontendLayerPlan(
                candidates=layer_plan.candidates,
                predicted_cost_total=layer_plan.predicted_cost_total,
                effective_min_predicted_cost=layer_plan.effective_min_predicted_cost,
                stdlib_candidates=layer_plan.stdlib_candidates,
                workers=1,
                policy_reason="worker_error_fallback_serial",
                mode="serial_fallback",
            )
            parallel_pool_usable = False

    for module_name in layer:
        module_path = execution_context.module_graph[module_name]
        result = _frontend_parallel._take_frontend_parallel_layer_result(layer_state, module_name)
        if result is not None:
            consume_error = _consume_frontend_parallel_layer_result(
                layer_state=layer_state,
                record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
                record_frontend_timing=runtime_hooks.record_frontend_timing,
                integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
                accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
                fail=runtime_hooks.fail,
                json_output=runtime_hooks.json_output,
                project_root=execution_context.project_root,
                layer_index=layer_index,
                module_name=module_name,
                module_path=module_path,
                result=result,
                target_python=execution_context.target_python,
            )
            if consume_error is not None:
                return None, consume_error
            continue
        serial_error = _run_frontend_serial_layer_modules(
            [module_name],
            module_graph=execution_context.module_graph,
            run_serial_frontend_lower=runtime_hooks.run_serial_frontend_lower,
            record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
            integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
            accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
            fail=runtime_hooks.fail,
            json_output=runtime_hooks.json_output,
            layer_state=layer_state,
            layer_index=layer_index,
            serial_mode=_frontend_parallel._frontend_serial_worker_mode(layer_plan.mode),
        )
        if serial_error is not None:
            return None, serial_error
    return (
        _FrontendLayerRunResult(
            layer_state=layer_state,
            layer_plan=layer_plan,
            parallel_pool_usable=parallel_pool_usable,
        ),
        None,
    )
