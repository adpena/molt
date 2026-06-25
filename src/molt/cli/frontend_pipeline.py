from __future__ import annotations

import ast
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Callable, Collection, Mapping, MutableMapping, Sequence, cast

from molt.type_facts import TypeFacts, load_type_facts

from molt.cli import frontend_execution as _frontend_execution
from molt.cli import frontend_parallel as _frontend_parallel
from molt.cli import typecheck as _typecheck
from molt.cli.build_diagnostics import (
    _build_build_diagnostics_payload,
    _record_frontend_timing_item,
)
from molt.cli.build_output_layout import (
    _resolve_build_output_layout,
    _resolve_out_dir,
    _resolve_output_roots,
)
from molt.cli.external_native import _resolve_import_admission_policy
from molt.cli.module_cache import (
    _build_scoped_lowering_inputs,
    _load_module_analysis,
)
from molt.cli.module_graph import (
    ModuleSyntaxErrorInfo,
    _build_module_graph_metadata,
    _materialize_import_plan,
    _prepare_entry_module_graph,
)
from molt.cli.module_dependencies import (
    _analyze_module_schedule,
    _apply_dead_module_elimination,
    _dependent_module_closure,
    _module_dependencies_from_imports,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
from molt.cli.models import (
    BuildProfile,
    EmitMode,
    FallbackPolicy,
    ParseCodec,
    Target,
    TypeHintPolicy,
    _BuildDiagnosticsContext,
    _BuildOutputLayout,
    _BinaryImageScope,
    _ExternalPackageNativeArtifactPlan,
    _FrontendIntegrationState,
    _FrontendTimingRecorderConfig,
    _ImportPlan,
    _MidendDiagnosticsState,
    _ModuleGraphMetadata,
    _PreparedBuildCallbacks,
    _PreparedBuildConfig,
    _PreparedBuildModuleOutputs,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _PreparedEntryModuleGraph,
    _PreparedFrontendAnalysis,
    _PreparedFrontendLoweringConfig,
    _PreparedFrontendRunTicket,
    _ResolvedBuildEntry,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.target_python import TargetPythonVersion
from molt.cli.toolchain_validation import _ensure_rustup_target

_PreparedFrontendPipelineBundle = tuple[
    _PreparedFrontendRunTicket,
    Mapping[str, Path],
    Collection[str],
    Collection[str],
    bool,
    _BuildOutputLayout,
    Collection[str],
    Mapping[str, str],
    dict[str, dict[str, dict[str, Any]]],
    list[str],
    TypeFacts | None,
    dict[str, Any],
    bool,
    int,
    bool,
    _FrontendIntegrationState,
    _MidendDiagnosticsState,
    Callable[..., None],
    Callable[[], tuple[dict[str, Any] | None, Path | None]],
    Path,
    _ExternalPackageNativeArtifactPlan,
]

def _output_base_for_entry(entry_module: str, source_path: Path) -> str:
    base = entry_module.rsplit(".", 1)[-1] or source_path.stem
    if base == "__main__" and "." in entry_module:
        base = entry_module.rsplit(".", 2)[-2]
    return base

def _syntax_error_info_from_exception(
    exc: Exception, *, path: Path
) -> ModuleSyntaxErrorInfo:
    if isinstance(exc, SyntaxError):
        message = exc.msg or str(exc)
        lineno = exc.lineno
        offset = exc.offset
        text = exc.text
        filename = exc.filename or str(path)
    elif isinstance(exc, UnicodeDecodeError):
        message = str(exc)
        lineno = 1
        offset = exc.start + 1 if exc.start is not None else None
        text = None
        filename = str(path)
    else:
        message = str(exc)
        lineno = None
        offset = None
        text = None
        filename = str(path)
    if isinstance(text, str):
        text = text.rstrip("\n")
    return ModuleSyntaxErrorInfo(
        message=message,
        filename=filename,
        lineno=lineno,
        offset=offset,
        text=text,
    )

def _prepare_build_module_outputs(
    *,
    prepared_module_graph: _PreparedEntryModuleGraph,
    module_reasons: MutableMapping[str, set[str]],
    stdlib_root: Path,
    artifacts_root: Path,
    entry_module: str,
    diagnostics_enabled: bool,
    target: str,
    trusted: bool,
    split_runtime: bool,
    require_linked: bool,
    linked: bool,
    linked_output: str | None,
    emit: EmitMode | None,
    output: str | None,
    emit_ir: str | None,
    bin_root: Path,
    output_root: Path,
    output_base: str,
    out_dir_path: Path | None,
    project_root: Path,
) -> tuple[_PreparedBuildModuleOutputs | None, str | None]:
    import_plan = _materialize_import_plan(
        prepared_module_graph=prepared_module_graph,
        module_reasons=module_reasons,
        stdlib_root=stdlib_root,
        artifacts_root=artifacts_root,
        entry_module=entry_module,
        diagnostics_enabled=diagnostics_enabled,
    )
    try:
        output_layout = _resolve_build_output_layout(
            target=target,
            trusted=trusted,
            split_runtime=split_runtime,
            require_linked=require_linked,
            linked=linked,
            linked_output=linked_output,
            emit=emit,
            output=output,
            emit_ir=emit_ir,
            artifacts_root=artifacts_root,
            bin_root=bin_root,
            output_root=output_root,
            output_base=output_base,
            out_dir_path=out_dir_path,
            project_root=project_root,
        )
    except ValueError as exc:
        return None, str(exc)
    return _PreparedBuildModuleOutputs(
        import_plan=import_plan,
        output_layout=output_layout,
    ), None

def _prepare_frontend_analysis(
    *,
    module_graph: Mapping[str, Path],
    module_graph_metadata: _ModuleGraphMetadata,
    module_resolution_cache: "_ModuleResolutionCache",
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    project_root: Path,
    entry_module: str,
    json_output: bool,
    target_python: TargetPythonVersion,
    capability_config_digest: str = "",
) -> tuple[_PreparedFrontendAnalysis | None, _CliFailure | None]:
    module_deps: dict[str, set[str]] = {}
    module_sources: dict[str, str] = {}
    module_source_leases: dict[str, _ModuleSourceLease] = {}
    known_func_defaults: dict[str, dict[str, dict[str, Any]]] = {}
    known_func_kinds: dict[str, dict[str, str]] = {}
    module_trees: dict[str, ast.AST] = {}
    module_path_stats: dict[str, os.stat_result | None] = {}
    syntax_error_modules: dict[str, ModuleSyntaxErrorInfo] = {}
    analysis_cache_miss_modules: set[str] = set()
    interface_changed_modules: set[str] = set()
    for module_name, module_path in module_graph.items():
        try:
            (
                tree,
                module_imports,
                func_defaults,
                func_kinds,
                source,
                analysis_cache_hit,
                interface_changed,
                path_stat,
            ) = _load_module_analysis(
                module_path,
                module_name=module_name,
                is_package=module_graph_metadata.module_is_package_by_module[
                    module_name
                ],
                import_scan_mode="full",
                source=None,
                logical_source_path=module_graph_metadata.logical_source_path_by_module[
                    module_name
                ],
                resolution_cache=module_resolution_cache,
                project_root=project_root,
                retain_source=False,
                retain_tree=False,
                roots=roots,
                stdlib_root=stdlib_root,
                stdlib_allowlist=stdlib_allowlist,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
            module_path_stats[module_name] = path_stat
            module_source_leases[module_name] = _ModuleSourceLease.path_backed(
                module_path, path_stat
            )
            if not analysis_cache_hit:
                analysis_cache_miss_modules.add(module_name)
            if interface_changed:
                interface_changed_modules.add(module_name)
        except SyntaxError as exc:
            if module_name == entry_module:
                return None, _fail(
                    f"Syntax error in {module_path}: {exc}",
                    json_output,
                    command="build",
                )
            syntax_error_modules[module_name] = _syntax_error_info_from_exception(
                exc, path=module_path
            )
            module_deps[module_name] = set()
            known_func_defaults[module_name] = {}
            known_func_kinds[module_name] = {}
            module_path_stats[module_name] = None
            module_source_leases[module_name] = _ModuleSourceLease.path_backed(
                module_path
            )
            continue
        except OSError as exc:
            return None, _fail(
                f"Failed to read module {module_path}: {exc}",
                json_output,
                command="build",
            )
        if tree is not None:
            module_trees[module_name] = tree
        module_deps[module_name] = _module_dependencies_from_imports(
            module_name,
            module_graph,
            module_imports,
        )
        known_func_defaults[module_name] = func_defaults
        known_func_kinds[module_name] = func_kinds
    (
        module_order,
        reverse_module_deps,
        has_back_edges,
        module_layers,
        module_dep_closures,
    ) = _analyze_module_schedule(module_graph, module_deps)
    module_source_catalog = _ModuleSourceCatalog(leases=module_source_leases)
    dirty_lowering_modules = set(analysis_cache_miss_modules)
    dirty_lowering_modules.update(
        _dependent_module_closure(
            interface_changed_modules,
            module_deps,
            module_graph,
            reverse_module_deps=reverse_module_deps,
        )
    )
    return _PreparedFrontendAnalysis(
        module_graph_metadata=module_graph_metadata,
        module_deps=module_deps,
        module_sources=module_sources,
        module_source_catalog=module_source_catalog,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        module_trees=module_trees,
        module_path_stats=module_path_stats,
        syntax_error_modules=syntax_error_modules,
        module_order=module_order,
        reverse_module_deps=reverse_module_deps,
        has_back_edges=has_back_edges,
        module_layers=module_layers,
        module_dep_closures=module_dep_closures,
        dirty_lowering_modules=dirty_lowering_modules,
    ), None

def _prepare_frontend_lowering_config(
    *,
    type_facts_path: str | None,
    type_hint_policy: TypeHintPolicy,
    module_graph: Mapping[str, Path],
    source_path: Path,
    json_output: bool,
    warnings: list[str],
    module_deps: dict[str, set[str]],
    module_dep_closures: dict[str, frozenset[str]],
    has_back_edges: bool,
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    pgo_hot_function_names: set[str],
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
    module_source_catalog: _ModuleSourceCatalog,
    is_wasm: bool,
    target_triple: str | None,
    frontend_parallel_details: dict[str, Any],
    frontend_phase_timeout: float | None,
) -> tuple[_PreparedFrontendLoweringConfig | None, _CliFailure | None]:
    type_facts: TypeFacts | None = None
    if type_facts_path is None and type_hint_policy in {"trust", "check"}:
        type_facts, ty_ok = _typecheck._collect_type_facts_for_build(
            list(module_graph.values()), type_hint_policy, source_path
        )
        if type_facts is None and type_hint_policy == "trust":
            return None, _fail(
                "Type facts unavailable; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "trust" and not ty_ok:
            return None, _fail(
                "ty check failed; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "check" and not ty_ok:
            warning = "ty check failed; continuing with guarded hints only."
            warnings.append(warning)
            if not json_output:
                print(warning, file=sys.stderr)
    if type_facts_path is not None:
        facts_path = Path(type_facts_path)
        if not facts_path.exists():
            return None, _fail(
                f"Type facts not found: {facts_path}",
                json_output,
                command="build",
            )
        try:
            type_facts = load_type_facts(facts_path)
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            return None, _fail(
                f"Failed to load type facts: {exc}",
                json_output,
                command="build",
            )

    known_classes: dict[str, Any] = {}
    scoped_lowering_inputs = _build_scoped_lowering_inputs(
        module_graph,
        module_deps=module_deps,
        module_dep_closures=module_dep_closures,
        known_modules=known_modules,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        pgo_hot_function_names=pgo_hot_function_names,
        type_facts=cast(TypeFacts | None, type_facts),
    )
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
        module_source_catalog=module_source_catalog,
        module_deps=module_deps,
    )
    frontend_module_costs = module_graph_metadata.frontend_module_costs
    stdlib_like_by_module = module_graph_metadata.stdlib_like_by_module
    assert frontend_module_costs is not None
    assert stdlib_like_by_module is not None
    frontend_module_costs_snapshot = dict(frontend_module_costs)
    stdlib_like_by_module_snapshot = dict(stdlib_like_by_module)

    enable_phi = not is_wasm
    module_chunk_max_ops = 0
    if is_wasm:
        module_chunk_max_ops = 2000
        env_chunk_ops = os.environ.get("MOLT_WASM_MODULE_CHUNK_OPS")
        if env_chunk_ops:
            try:
                module_chunk_max_ops = max(0, int(env_chunk_ops))
            except ValueError:
                warnings.append(
                    "Invalid MOLT_WASM_MODULE_CHUNK_OPS; using default of 2000."
                )
    # Also support module chunking for native builds via MOLT_MODULE_CHUNK_OPS.
    # Large stdlib modules like _collections_abc have init functions that balloon
    # to 23+ MB of native code when all class definitions are compiled into one
    # monolithic function.  Chunking splits these into smaller callable pieces
    # that are invoked eagerly and in order during module initialization, so all
    # module-level names (including ABC) are fully defined before any downstream
    # module tries to access them.
    if not is_wasm:
        # Default native chunk size: 1400 ops per module init function.
        # Native stdlib bootstrap functions accumulate dense check_exception /
        # label CFG and post-lowering metadata, so even a 2000-op frontend
        # chunk can still arrive at the backend as a ~2800-op megafunction.
        # Tightening the frontend budget keeps those exception-heavy stdlib
        # chunks under the backend's practical Cranelift sweet spot without
        # changing runtime semantics.
        module_chunk_max_ops = 1400
        env_native_chunk_ops = os.environ.get("MOLT_MODULE_CHUNK_OPS")
        if env_native_chunk_ops:
            try:
                module_chunk_max_ops = max(0, int(env_native_chunk_ops))
            except ValueError:
                warnings.append("Invalid MOLT_MODULE_CHUNK_OPS; using default of 3000.")
    module_chunking = module_chunk_max_ops > 0
    if target_triple:
        _ensure_rustup_target(target_triple, warnings)

    frontend_parallel_config = _frontend_parallel._resolve_frontend_parallel_config(
        module_count=len(module_graph),
        has_back_edges=has_back_edges,
        frontend_phase_timeout=frontend_phase_timeout,
    )
    frontend_parallel_layers, frontend_parallel_worker_timings = (
        _frontend_parallel._initialize_frontend_parallel_details(
            frontend_parallel_details,
            frontend_parallel_config=frontend_parallel_config,
        )
    )
    return _PreparedFrontendLoweringConfig(
        type_facts=type_facts,
        known_classes=known_classes,
        scoped_lowering_inputs=scoped_lowering_inputs,
        module_graph_metadata=module_graph_metadata,
        frontend_module_costs=frontend_module_costs_snapshot,
        stdlib_like_by_module=stdlib_like_by_module_snapshot,
        enable_phi=enable_phi,
        module_chunk_max_ops=module_chunk_max_ops,
        module_chunking=module_chunking,
        frontend_parallel_config=frontend_parallel_config,
        frontend_parallel_layers=frontend_parallel_layers,
        frontend_parallel_worker_timings=frontend_parallel_worker_timings,
    ), None

def _prepare_build_callbacks(
    *,
    frontend_module_timings: list[dict[str, Any]],
    frontend_timing_enabled: bool,
    frontend_timing_raw: str,
    frontend_timing_threshold: float,
    json_output: bool,
    diagnostics_enabled: bool,
    diagnostics_start: float,
    phase_starts: dict[str, float],
    module_graph: Mapping[str, Path],
    module_reasons: Mapping[str, set[str]],
    allocation_diagnostics_enabled: bool,
    frontend_parallel_details: dict[str, Any],
    profile: BuildProfile,
    midend_policy_outcomes_by_function: dict[str, dict[str, Any]],
    midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]],
    backend_daemon_health: dict[str, Any] | None,
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_config_digest: str | None,
    diagnostics_path_spec: str,
    artifacts_root: Path,
    image_scope: _BinaryImageScope | None,
) -> _PreparedBuildCallbacks:
    timing_config = _FrontendTimingRecorderConfig(
        enabled=frontend_timing_enabled,
        raw=bool(frontend_timing_raw),
        threshold=frontend_timing_threshold,
        json_output=json_output,
    )

    def _record_frontend_timing(
        *,
        module_name: str,
        module_path: Path,
        visit_s: float,
        lower_s: float,
        total_s: float,
        timed_out: bool = False,
        detail: str | None = None,
    ) -> None:
        _record_frontend_timing_item(
            frontend_module_timings,
            config=timing_config,
            module_name=module_name,
            module_path=module_path,
            visit_s=visit_s,
            lower_s=lower_s,
            total_s=total_s,
            timed_out=timed_out,
            detail=detail,
        )

    def _build_diagnostics_payload() -> tuple[dict[str, Any] | None, Path | None]:
        return _build_build_diagnostics_payload(
            _BuildDiagnosticsContext(
                diagnostics_enabled=diagnostics_enabled,
                diagnostics_start=diagnostics_start,
                phase_starts=phase_starts,
                image_scope=image_scope,
                module_graph=module_graph,
                module_reasons=module_reasons,
                frontend_module_timings=frontend_module_timings,
                allocation_diagnostics_enabled=allocation_diagnostics_enabled,
                frontend_parallel_details=frontend_parallel_details,
                profile=profile,
                midend_policy_outcomes_by_function=midend_policy_outcomes_by_function,
                midend_pass_stats_by_function=midend_pass_stats_by_function,
                backend_daemon_health=backend_daemon_health,
                backend_daemon_cached=backend_daemon_cached,
                backend_daemon_cache_tier=backend_daemon_cache_tier,
                backend_daemon_config_digest=backend_daemon_config_digest,
                diagnostics_path_spec=diagnostics_path_spec,
                artifacts_root=artifacts_root,
            )
        )

    return _PreparedBuildCallbacks(
        record_frontend_timing=_record_frontend_timing,
        build_diagnostics_payload=_build_diagnostics_payload,
    )

def _prepare_frontend_stage_state(
    *,
    prepared_build_preamble: _PreparedBuildPreamble,
    prepared_build_roots: _PreparedBuildRoots,
    prepared_build_config: _PreparedBuildConfig,
    resolved_build_entry: _ResolvedBuildEntry,
    json_output: bool,
    target: Target,
    verbose: bool,
    out_dir: str | None,
    trusted: bool,
    split_runtime: bool,
    require_linked: bool,
    linked: bool,
    linked_output: str | None,
    emit: EmitMode | None,
    output: str | None,
    emit_ir: str | None,
    type_facts_path: str | None,
    type_hint_policy: TypeHintPolicy,
    profile: BuildProfile,
) -> tuple[
    tuple[
        _ImportPlan,
        _PreparedBuildModuleOutputs,
        _PreparedFrontendAnalysis,
        _PreparedFrontendLoweringConfig,
        Callable[..., None],
        Callable[[], tuple[dict[str, Any] | None, Path | None]],
        Path,
    ]
    | None,
    _CliFailure | None,
]:
    source_path = resolved_build_entry.source_path
    entry_module = resolved_build_entry.entry_module
    module_roots = resolved_build_entry.module_roots
    stdlib_root = prepared_build_preamble.stdlib_root
    project_root = prepared_build_roots.project_root
    entry_tree = resolved_build_entry.entry_tree
    module_reasons = prepared_build_preamble.module_reasons
    diagnostics_enabled = prepared_build_preamble.diagnostics_enabled
    frontend_module_timings = prepared_build_preamble.frontend_module_timings
    frontend_timing_enabled = prepared_build_preamble.frontend_timing_enabled
    frontend_timing_raw = prepared_build_preamble.frontend_timing_raw
    frontend_timing_threshold = prepared_build_preamble.frontend_timing_threshold
    diagnostics_start = prepared_build_preamble.diagnostics_start
    phase_starts = prepared_build_preamble.phase_starts
    allocation_diagnostics_enabled = (
        prepared_build_preamble.allocation_diagnostics_enabled
    )
    frontend_parallel_details = prepared_build_preamble.frontend_parallel_details
    midend_policy_outcomes_by_function = (
        prepared_build_preamble.midend_policy_outcomes_by_function
    )
    midend_pass_stats_by_function = (
        prepared_build_preamble.midend_pass_stats_by_function
    )
    backend_daemon_health = prepared_build_preamble.backend_daemon_health
    backend_daemon_cached = prepared_build_preamble.backend_daemon_cached
    backend_daemon_cache_tier = prepared_build_preamble.backend_daemon_cache_tier
    backend_daemon_config_digest = prepared_build_preamble.backend_daemon_config_digest
    diagnostics_path_spec = prepared_build_preamble.diagnostics_path_spec
    warnings = prepared_build_preamble.warnings
    pgo_hot_function_names = prepared_build_config.pgo_hot_function_names
    frontend_phase_timeout = prepared_build_config.frontend_phase_timeout
    import_admission_policy, import_admission_policy_error = (
        _resolve_import_admission_policy(
            external_module_roots=resolved_build_entry.external_module_roots,
            json_output=json_output,
        )
    )
    if import_admission_policy_error is not None:
        return None, import_admission_policy_error
    assert import_admission_policy is not None
    if diagnostics_enabled:
        phase_starts["module_graph"] = time.perf_counter()
    prepared_module_graph, prepared_module_graph_error = _prepare_entry_module_graph(
        source_path=source_path,
        entry_module=entry_module,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        entry_tree=entry_tree,
        module_reasons=module_reasons,
        diagnostics_enabled=diagnostics_enabled,
        json_output=json_output,
        target=target,
        import_admission_policy=import_admission_policy,
        target_python=prepared_build_config.target_python,
        capability_config_digest=prepared_build_config.capability_config_cache_digest,
        image_scope=resolved_build_entry.image_scope,
    )
    if prepared_module_graph_error is not None:
        return None, prepared_module_graph_error
    assert prepared_module_graph is not None
    output_base = _output_base_for_entry(entry_module, source_path)
    out_dir_path = _resolve_out_dir(project_root, out_dir)
    artifacts_root, bin_root, output_root = _resolve_output_roots(
        project_root, out_dir_path, output_base
    )
    prepared_build_outputs, prepared_build_outputs_error = (
        _prepare_build_module_outputs(
            prepared_module_graph=prepared_module_graph,
            module_reasons=module_reasons,
            stdlib_root=stdlib_root,
            artifacts_root=artifacts_root,
            entry_module=entry_module,
            diagnostics_enabled=diagnostics_enabled,
            target=target,
            trusted=trusted,
            split_runtime=split_runtime,
            require_linked=require_linked,
            linked=linked,
            linked_output=linked_output,
            emit=emit,
            output=output,
            emit_ir=emit_ir,
            bin_root=bin_root,
            output_root=output_root,
            output_base=output_base,
            out_dir_path=out_dir_path,
            project_root=project_root,
        )
    )
    if prepared_build_outputs_error is not None:
        return None, _fail(prepared_build_outputs_error, json_output, command="build")
    assert prepared_build_outputs is not None
    import_plan = prepared_build_outputs.import_plan
    if verbose and not json_output:
        print(f"Project root: {project_root}")
        print(f"Module roots: {', '.join(str(root) for root in module_roots)}")
        print(f"Modules discovered: {len(import_plan.module_graph)}")
    prepared_build_callbacks = _prepare_build_callbacks(
        frontend_module_timings=frontend_module_timings,
        frontend_timing_enabled=frontend_timing_enabled,
        frontend_timing_raw=frontend_timing_raw,
        frontend_timing_threshold=frontend_timing_threshold,
        json_output=json_output,
        diagnostics_enabled=diagnostics_enabled,
        diagnostics_start=diagnostics_start,
        phase_starts=phase_starts,
        module_graph=import_plan.module_graph,
        module_reasons=module_reasons,
        allocation_diagnostics_enabled=allocation_diagnostics_enabled,
        frontend_parallel_details=frontend_parallel_details,
        profile=profile,
        midend_policy_outcomes_by_function=midend_policy_outcomes_by_function,
        midend_pass_stats_by_function=midend_pass_stats_by_function,
        backend_daemon_health=backend_daemon_health,
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_config_digest=backend_daemon_config_digest,
        diagnostics_path_spec=diagnostics_path_spec,
        artifacts_root=artifacts_root,
        image_scope=resolved_build_entry.image_scope,
    )
    if diagnostics_enabled:
        phase_starts["module_analysis"] = time.perf_counter()
    prepared_frontend_analysis, prepared_frontend_analysis_error = (
        _prepare_frontend_analysis(
            module_graph=dict(import_plan.module_graph),
            module_graph_metadata=import_plan.module_graph_metadata,
            module_resolution_cache=import_plan.module_resolution_cache,
            roots=import_plan.roots,
            stdlib_root=import_plan.stdlib_root,
            stdlib_allowlist=set(import_plan.stdlib_allowlist),
            project_root=project_root,
            entry_module=entry_module,
            json_output=json_output,
            target_python=prepared_build_config.target_python,
            capability_config_digest=prepared_build_config.capability_config_cache_digest,
        )
    )
    if prepared_frontend_analysis_error is not None:
        return None, prepared_frontend_analysis_error
    assert prepared_frontend_analysis is not None
    if diagnostics_enabled:
        phase_starts["ir_lowering"] = time.perf_counter()
    prepared_frontend_lowering_config, prepared_frontend_lowering_config_error = (
        _prepare_frontend_lowering_config(
            type_facts_path=type_facts_path,
            type_hint_policy=type_hint_policy,
            module_graph=import_plan.module_graph,
            source_path=source_path,
            json_output=json_output,
            warnings=warnings,
            module_deps=prepared_frontend_analysis.module_deps,
            module_dep_closures=prepared_frontend_analysis.module_dep_closures,
            has_back_edges=prepared_frontend_analysis.has_back_edges,
            known_modules=set(import_plan.known_modules),
            known_func_defaults=prepared_frontend_analysis.known_func_defaults,
            known_func_kinds=prepared_frontend_analysis.known_func_kinds,
            pgo_hot_function_names=pgo_hot_function_names,
            generated_module_source_paths=dict(
                import_plan.generated_module_source_paths
            ),
            entry_module=entry_module,
            namespace_module_names=set(import_plan.namespace_module_names),
            module_source_catalog=prepared_frontend_analysis.module_source_catalog,
            is_wasm=prepared_build_outputs.output_layout.is_wasm,
            target_triple=prepared_build_outputs.output_layout.target_triple,
            frontend_parallel_details=frontend_parallel_details,
            frontend_phase_timeout=frontend_phase_timeout,
        )
    )
    if prepared_frontend_lowering_config_error is not None:
        return None, prepared_frontend_lowering_config_error
    assert prepared_frontend_lowering_config is not None
    return (
        (
            import_plan,
            prepared_build_outputs,
            prepared_frontend_analysis,
            prepared_frontend_lowering_config,
            prepared_build_callbacks.record_frontend_timing,
            prepared_build_callbacks.build_diagnostics_payload,
            artifacts_root,
        ),
        None,
    )

def _prepare_frontend_pipeline(
    *,
    prepared_build_preamble: _PreparedBuildPreamble,
    prepared_build_roots: _PreparedBuildRoots,
    prepared_build_config: _PreparedBuildConfig,
    resolved_build_entry: _ResolvedBuildEntry,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    profile: BuildProfile,
    json_output: bool,
    target: Target,
    verbose: bool,
    out_dir: str | None,
    trusted: bool,
    split_runtime: bool,
    require_linked: bool,
    linked: bool,
    linked_output: str | None,
    emit: EmitMode | None,
    output: str | None,
    emit_ir: str | None,
    type_facts_path: str | None,
) -> tuple[_PreparedFrontendPipelineBundle | None, _CliFailure | None]:
    prepared_frontend_stage_bundle, prepared_frontend_stage_state_error = (
        _prepare_frontend_stage_state(
            prepared_build_preamble=prepared_build_preamble,
            prepared_build_roots=prepared_build_roots,
            prepared_build_config=prepared_build_config,
            resolved_build_entry=resolved_build_entry,
            json_output=json_output,
            target=target,
            verbose=verbose,
            out_dir=out_dir,
            trusted=trusted,
            split_runtime=split_runtime,
            require_linked=require_linked,
            linked=linked,
            linked_output=linked_output,
            emit=emit,
            output=output,
            emit_ir=emit_ir,
            type_facts_path=type_facts_path,
            type_hint_policy=type_hint_policy,
            profile=profile,
        )
    )
    if prepared_frontend_stage_state_error is not None:
        return None, prepared_frontend_stage_state_error
    assert prepared_frontend_stage_bundle is not None
    (
        import_plan,
        prepared_build_outputs,
        prepared_frontend_analysis,
        prepared_frontend_lowering_config,
        record_frontend_timing,
        build_diagnostics_payload,
        artifacts_root,
    ) = prepared_frontend_stage_bundle
    midend_policy_outcomes_by_function = (
        prepared_build_preamble.midend_policy_outcomes_by_function
    )
    midend_pass_stats_by_function = (
        prepared_build_preamble.midend_pass_stats_by_function
    )
    frontend_parallel_worker_timings = (
        prepared_frontend_lowering_config.frontend_parallel_worker_timings
    )

    def _record_frontend_parallel_worker_timing(
        *,
        layer_index: int,
        module_name: str,
        module_path: Path,
        mode: str,
        queue_ms: float,
        wait_ms: float,
        exec_ms: float,
        roundtrip_ms: float,
        worker_pid: int | None,
    ) -> dict[str, Any]:
        item: dict[str, Any] = {
            "layer": layer_index,
            "module": module_name,
            "path": str(module_path),
            "mode": mode,
            "queue_ms": round(max(0.0, queue_ms), 6),
            "wait_ms": round(max(0.0, wait_ms), 6),
            "exec_ms": round(max(0.0, exec_ms), 6),
            "roundtrip_ms": round(max(0.0, roundtrip_ms), 6),
        }
        if isinstance(worker_pid, int):
            item["worker_pid"] = worker_pid
        frontend_parallel_worker_timings.append(item)
        return item

    compile_module_order: list[str] = list(prepared_frontend_analysis.module_order)
    compile_module_layers: list[list[str]] = [
        list(layer) for layer in prepared_frontend_analysis.module_layers
    ]
    if os.environ.get("MOLT_DEAD_MODULE_ELIMINATION") == "1":
        dme_roots = (
            import_plan.runtime_import_dispatch_roots
            | import_plan.declared_root_modules
            | import_plan.runtime_support_modules
            | import_plan.stdlib_support_modules
            | import_plan.package_parent_modules
            | import_plan.namespace_module_names
        )
        compile_module_order, compile_module_layers, dme_eliminated = (
            _apply_dead_module_elimination(
                compile_module_order,
                compile_module_layers,
                entry_module=resolved_build_entry.entry_module,
                module_deps=prepared_frontend_analysis.module_deps,
                module_names=set(import_plan.module_graph),
                extra_roots=dme_roots,
            )
        )
        if dme_eliminated > 0:
            import sys as _dme_sys

            print(
                f"[molt] dead module elimination: skipping {dme_eliminated} "
                f"unreachable modules (compiling {len(compile_module_order)} of "
                f"{len(prepared_frontend_analysis.module_order)})",
                file=_dme_sys.stderr,
            )
    try:
        import_plan = import_plan.with_compile_modules(compile_module_order)
    except ValueError as exc:
        return None, _fail(
            f"internal error: binary image closure plan is invalid: {exc}",
            json_output,
            command="build",
        )
    compile_module_graph = {
        name: path
        for name, path in import_plan.module_graph.items()
        if name in import_plan.compile_modules
    }
    compile_generated_module_source_paths = {
        name: path
        for name, path in import_plan.generated_module_source_paths.items()
        if name in import_plan.compile_modules
    }
    (
        frontend_layer_execution_context,
        frontend_layer_runtime_hooks,
        integration_state,
        midend_diagnostics_state,
    ) = _frontend_execution._prepare_frontend_execution(
        syntax_error_modules=prepared_frontend_analysis.syntax_error_modules,
        module_graph=compile_module_graph,
        module_source_catalog=prepared_frontend_analysis.module_source_catalog,
        project_root=prepared_build_roots.project_root,
        module_resolution_cache=import_plan.module_resolution_cache,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=prepared_frontend_lowering_config.type_facts,
        enable_phi=prepared_frontend_lowering_config.enable_phi,
        known_modules=set(import_plan.compile_modules),
        stdlib_allowlist=set(import_plan.stdlib_allowlist),
        known_func_defaults=prepared_frontend_analysis.known_func_defaults,
        known_func_kinds=prepared_frontend_analysis.known_func_kinds,
        module_deps=prepared_frontend_analysis.module_deps,
        module_chunk_max_ops=prepared_frontend_lowering_config.module_chunk_max_ops,
        optimization_profile=profile,
        pgo_hot_function_names=prepared_build_config.pgo_hot_function_names,
        known_modules_sorted=tuple(sorted(import_plan.compile_modules)),
        stdlib_allowlist_sorted=import_plan.stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=(
            prepared_build_config.pgo_hot_function_names_sorted
        ),
        module_dep_closures=prepared_frontend_analysis.module_dep_closures,
        module_graph_metadata=prepared_frontend_lowering_config.module_graph_metadata,
        module_path_stats=prepared_frontend_analysis.module_path_stats,
        module_chunking=prepared_frontend_lowering_config.module_chunking,
        scoped_lowering_inputs=prepared_frontend_lowering_config.scoped_lowering_inputs,
        dirty_lowering_modules=prepared_frontend_analysis.dirty_lowering_modules,
        frontend_module_costs=prepared_frontend_lowering_config.frontend_module_costs,
        stdlib_like_by_module=prepared_frontend_lowering_config.stdlib_like_by_module,
        known_classes=prepared_frontend_lowering_config.known_classes,
        module_trees=prepared_frontend_analysis.module_trees,
        generated_module_source_paths=compile_generated_module_source_paths,
        frontend_phase_timeout=prepared_build_config.frontend_phase_timeout,
        record_frontend_timing=record_frontend_timing,
        fail=_fail,
        json_output=json_output,
        warnings=prepared_build_preamble.warnings,
        frontend_parallel_details=prepared_build_preamble.frontend_parallel_details,
        record_frontend_parallel_worker_timing=_record_frontend_parallel_worker_timing,
        midend_policy_outcomes_by_function=midend_policy_outcomes_by_function,
        midend_pass_stats_by_function=midend_pass_stats_by_function,
        target_python=prepared_build_config.target_python,
    )

    prepared_frontend_run_ticket = _PreparedFrontendRunTicket(
        module_order=compile_module_order,
        module_layers=compile_module_layers,
        frontend_parallel_config=(
            prepared_frontend_lowering_config.frontend_parallel_config
        ),
        frontend_parallel_layers=compile_module_layers,
        frontend_parallel_worker_timings=frontend_parallel_worker_timings,
        frontend_parallel_details=prepared_build_preamble.frontend_parallel_details,
        frontend_layer_execution_context=frontend_layer_execution_context,
        frontend_layer_runtime_hooks=frontend_layer_runtime_hooks,
    )
    return (
        (
            prepared_frontend_run_ticket,
            dict(compile_module_graph),
            set(import_plan.runtime_import_dispatch_roots),
            set(import_plan.stdlib_allowlist),
            import_plan.spawn_enabled,
            prepared_build_outputs.output_layout,
            set(import_plan.compile_modules),
            dict(compile_generated_module_source_paths),
            prepared_frontend_analysis.known_func_defaults,
            prepared_frontend_analysis.known_func_kinds,
            compile_module_order,
            prepared_frontend_lowering_config.type_facts,
            prepared_frontend_lowering_config.known_classes,
            prepared_frontend_lowering_config.enable_phi,
            prepared_frontend_lowering_config.module_chunk_max_ops,
            prepared_frontend_lowering_config.module_chunking,
            integration_state,
            midend_diagnostics_state,
            record_frontend_timing,
            build_diagnostics_payload,
            artifacts_root,
            import_plan.native_artifact_plan,
        ),
        None,
    )
