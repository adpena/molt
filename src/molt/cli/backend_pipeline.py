from __future__ import annotations

import contextlib
from pathlib import Path

from molt.cli import backend_compile as _backend_compile
from molt.cli import backend_ir as _backend_ir
from molt.cli import backend_output_pipeline as _backend_output_pipeline
from molt.cli import factgraph as _factgraph
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli.backend_execution import _write_backend_ir_lease
from molt.compiler_analysis import backend_ir_binary_image_analysis_payload
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE, ENTRY_OVERRIDE_ENV
from molt.cli.external_native import _external_native_artifact_output_custody_error
from molt.cli.models import (
    BuildProfile,
    FallbackPolicy,
    ParseCodec,
    TypeHintPolicy,
    _PreparedBuildConfig,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _ResolvedBuildEntry,
)
from molt.cli.output import (
    emit_json as _emit_json,
    fail as _fail,
    json_payload as _json_payload,
    subprocess_output_text as _subprocess_output_text,
)


def _run_backend_pipeline(
    *,
    prepared_build_preamble: _PreparedBuildPreamble,
    prepared_build_roots: _PreparedBuildRoots,
    prepared_build_config: _PreparedBuildConfig,
    resolved_build_entry: _ResolvedBuildEntry,
    prepared_frontend_pipeline_bundle: _frontend_pipeline._PreparedFrontendPipelineBundle,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    profile: BuildProfile,
    json_output: bool,
    target: str,
    cache_dir: str | None,
    cache: bool,
    cache_report: bool,
    deterministic: bool,
    trusted: bool,
    verbose: bool,
    require_linked: bool,
    wasm_opt_level: str = "Oz",
    precompile: bool = False,
    snapshot: bool = False,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
    fact_graph_request: _factgraph.FactGraphRequest | None = None,
) -> int:
    (
        prepared_frontend_run_ticket,
        module_graph,
        runtime_import_dispatch_roots,
        stdlib_allowlist,
        spawn_enabled,
        output_layout,
        known_modules,
        generated_module_source_paths,
        known_func_defaults,
        known_func_kinds,
        module_order,
        type_facts,
        known_classes,
        enable_phi,
        module_chunk_max_ops,
        module_chunking,
        integration_state,
        diagnostics_state,
        record_frontend_timing,
        build_diagnostics_payload,
        record_binary_image_analysis,
        artifacts_root,
        native_artifact_plan,
    ) = prepared_frontend_pipeline_bundle
    native_artifact_custody_error = _external_native_artifact_output_custody_error(
        native_artifact_plan=native_artifact_plan,
        output_layout=output_layout,
        target=target,
    )
    if native_artifact_custody_error is not None:
        return _fail(native_artifact_custody_error, json_output, command="build")
    prepared_backend_ir, prepared_backend_ir_error = _backend_ir._prepare_backend_ir(
        entry_module=resolved_build_entry.entry_module,
        module_graph=module_graph,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        known_classes=known_classes,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=profile,
        pgo_hot_function_names=prepared_build_config.pgo_hot_function_names,
        frontend_phase_timeout=prepared_build_config.frontend_phase_timeout,
        integration_state=integration_state,
        diagnostics_state=diagnostics_state,
        record_frontend_timing=record_frontend_timing,
        fail=_fail,
        json_output=json_output,
        module_order=module_order,
        runtime_import_dispatch_roots=runtime_import_dispatch_roots,
        generated_module_source_paths=generated_module_source_paths,
        spawn_enabled=spawn_enabled,
        pgo_profile_summary=prepared_build_config.pgo_profile_summary,
        runtime_feedback_summary=prepared_build_config.runtime_feedback_summary,
        emit_ir_path=output_layout.emit_ir_path,
        target_python=prepared_build_config.target_python,
        stdlib_profile=stdlib_profile,
        target=target,
        native_artifact_plan=native_artifact_plan,
    )
    if prepared_backend_ir_error is not None:
        return prepared_backend_ir_error
    assert prepared_backend_ir is not None
    ir = prepared_backend_ir.ir
    required_link_features = prepared_backend_ir.required_link_features
    if prepared_build_preamble.diagnostics_enabled:
        record_binary_image_analysis(
            "backend_ir",
            backend_ir_binary_image_analysis_payload(ir),
        )
    resolved_modules = frozenset(module_graph)
    backend_ir_file_path: Path | None = None

    def _ensure_backend_ir_file_path() -> Path:
        nonlocal backend_ir_file_path
        if backend_ir_file_path is None:
            backend_ir_file_path = _write_backend_ir_lease(
                prepared_build_roots.project_root, ir
            )
        return backend_ir_file_path

    def _cleanup_backend_ir_file_path() -> None:
        if backend_ir_file_path is not None:
            with contextlib.suppress(OSError):
                backend_ir_file_path.unlink()

    prepared_backend_setup, prepared_backend_setup_error = (
        _backend_compile._prepare_backend_setup(
            is_rust_transpile=output_layout.is_rust_transpile,
            is_luau_transpile=output_layout.is_luau_transpile,
            is_wasm=output_layout.is_wasm,
            emit_mode=output_layout.emit_mode,
            molt_root=prepared_build_roots.molt_root,
            runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
            target_triple=output_layout.target_triple,
            json_output=json_output,
            cargo_timeout=prepared_build_config.cargo_timeout,
            target=target,
            profile=profile,
            backend_cargo_profile=prepared_build_config.backend_cargo_profile,
            linked=output_layout.linked,
            project_root=prepared_build_roots.project_root,
            cache_dir=cache_dir,
            output_artifact=output_layout.output_artifact,
            warnings=prepared_build_preamble.warnings,
            cache=cache,
            ir=ir,
            entry_module=resolved_build_entry.entry_module,
            module_graph_metadata=prepared_frontend_run_ticket.frontend_layer_execution_context.module_graph_metadata,
            target_python=prepared_build_config.target_python,
            stdlib_profile=stdlib_profile,
            native_artifact_plan=native_artifact_plan,
            resolved_modules=resolved_modules,
            capabilities_list=prepared_build_config.capabilities_list,
            capability_profiles=prepared_build_config.capability_profiles,
            manifest_env_vars=prepared_build_config.manifest_env_vars,
            capability_config_digest=prepared_build_config.capability_config_cache_digest,
        )
    )
    if prepared_backend_setup_error is not None:
        return prepared_backend_setup_error
    assert prepared_backend_setup is not None
    prepared_backend_runtime_context, prepared_backend_runtime_error = (
        _backend_compile._prepare_backend_runtime_context(
            prepared_backend_setup=prepared_backend_setup,
            is_wasm_freestanding=output_layout.is_wasm_freestanding,
            json_output=json_output,
            runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
            cargo_timeout=prepared_build_config.cargo_timeout,
            molt_root=prepared_build_roots.molt_root,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            target_triple=output_layout.target_triple,
        )
    )
    if prepared_backend_runtime_error is not None:
        return prepared_backend_runtime_error
    assert prepared_backend_runtime_context is not None
    if fact_graph_request is not None:
        return _factgraph.emit_pipeline_fact_graph(
            request=fact_graph_request,
            output_layout=output_layout,
            deterministic=deterministic,
            profile=profile,
            runtime_context=prepared_backend_runtime_context,
            build_config=prepared_build_config,
            build_roots=prepared_build_roots,
            build_preamble=prepared_build_preamble,
            ir=ir,
            resolved_modules=resolved_modules,
            json_output=json_output,
            verbose=verbose,
            target=target,
            entry_module=resolved_build_entry.entry_module,
            prepare_backend_dispatch=_backend_compile._prepare_backend_dispatch,
            ensure_backend_ir_file_path=_ensure_backend_ir_file_path,
            cleanup_backend_ir_file_path=_cleanup_backend_ir_file_path,
            run_subprocess_captured_to_tempfiles=_run_subprocess_captured_to_tempfiles,
            subprocess_output_text=_subprocess_output_text,
            fail=_fail,
            emit_json=_emit_json,
            json_payload=_json_payload,
            entry_override_env=ENTRY_OVERRIDE_ENV,
        )
    try:
        prepared_backend_compile, prepared_backend_compile_error = (
            _backend_compile._prepare_backend_compile(
                diagnostics_enabled=prepared_build_preamble.diagnostics_enabled,
                phase_starts=prepared_build_preamble.phase_starts,
                cache_report=cache_report,
                verbose=verbose,
                json_output=json_output,
                cache_setup=prepared_backend_runtime_context.cache_setup,
                cache_hit=prepared_backend_runtime_context.cache_hit,
                cache_hit_tier=prepared_backend_runtime_context.cache_hit_tier,
                cache_key=prepared_backend_runtime_context.cache_key,
                function_cache_key=prepared_backend_runtime_context.function_cache_key,
                cache_path=prepared_backend_runtime_context.cache_path,
                function_cache_path=prepared_backend_runtime_context.function_cache_path,
                project_root=prepared_build_roots.project_root,
                warnings=prepared_build_preamble.warnings,
                is_rust_transpile=output_layout.is_rust_transpile,
                is_luau_transpile=output_layout.is_luau_transpile,
                is_wasm=output_layout.is_wasm,
                split_runtime=output_layout.split_runtime,
                output_artifact=output_layout.output_artifact,
                linked=output_layout.linked,
                deterministic=deterministic,
                profile=profile,
                runtime_state=prepared_backend_runtime_context.runtime_state,
                runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
                cargo_timeout=prepared_build_config.cargo_timeout,
                molt_root=prepared_build_roots.molt_root,
                target_triple=output_layout.target_triple,
                backend_cargo_profile=prepared_build_config.backend_cargo_profile,
                backend_timeout=prepared_build_config.backend_timeout,
                backend_daemon_config_digest=prepared_build_preamble.backend_daemon_config_digest,
                entry_module=resolved_build_entry.entry_module,
                resolved_modules=resolved_modules,
                ensure_runtime_wasm_shared=prepared_backend_runtime_context.ensure_runtime_wasm_shared,
                ensure_runtime_wasm_reloc=prepared_backend_runtime_context.ensure_runtime_wasm_reloc,
                artifacts_root=artifacts_root,
                ir=ir,
                _ensure_backend_ir_file_path=_ensure_backend_ir_file_path,
                backend_daemon_cached=prepared_build_preamble.backend_daemon_cached,
                backend_daemon_cache_tier=prepared_build_preamble.backend_daemon_cache_tier,
                backend_daemon_health=prepared_build_preamble.backend_daemon_health,
            )
        )
    finally:
        if backend_ir_file_path is not None:
            with contextlib.suppress(OSError):
                backend_ir_file_path.unlink()
    if prepared_backend_compile_error is not None:
        return prepared_backend_compile_error
    assert prepared_backend_compile is not None
    return _backend_output_pipeline._emit_backend_pipeline_outputs(
        prepared_build_preamble=prepared_build_preamble,
        prepared_build_roots=prepared_build_roots,
        prepared_build_config=prepared_build_config,
        resolved_build_entry=resolved_build_entry,
        output_layout=output_layout,
        prepared_backend_setup=prepared_backend_setup,
        prepared_backend_runtime_context=prepared_backend_runtime_context,
        prepared_backend_compile=prepared_backend_compile,
        native_artifact_plan=native_artifact_plan,
        artifacts_root=artifacts_root,
        resolved_modules=resolved_modules,
        build_diagnostics_payload=build_diagnostics_payload,
        target=target,
        deterministic=deterministic,
        trusted=trusted,
        verbose=verbose,
        require_linked=require_linked,
        wasm_opt_level=wasm_opt_level,
        precompile=precompile,
        snapshot=snapshot,
        profile=profile,
        json_output=json_output,
        stdlib_profile=stdlib_profile,
    )
