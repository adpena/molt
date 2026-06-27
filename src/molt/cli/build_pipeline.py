from __future__ import annotations

from pathlib import Path

from molt.dx import session_scoped_target_dir
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE
from molt.cli import backend_ir as _backend_ir
from molt.cli import backend_pipeline as _backend_pipeline
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli import factgraph as _factgraph
from molt.cli.backend_daemon_paths import (
    _unix_socket_path_exceeds_limit as _unix_socket_path_exceeds_limit,
)
from molt.compiler_analysis import backend_ir_binary_image_analysis_payload
from molt.cli.backend_diagnostics import (
    _BACKEND_DIAGNOSTIC_ENV_KNOBS as _BACKEND_DIAGNOSTIC_ENV_KNOBS,
    _PYTHON_WARNING_RE as _PYTHON_WARNING_RE,
)
from molt.cli.capability_spec import (
    CAPABILITY_PROFILES as CAPABILITY_PROFILES,
    CAPABILITY_TOKEN_RE as CAPABILITY_TOKEN_RE,
    CapabilityGrant as CapabilityGrant,
    CapabilitySpec as CapabilitySpec,
    _coerce_effects_list as _coerce_effects_list,
    _coerce_token_list as _coerce_token_list,
    _expand_capabilities as _expand_capabilities,
    _merge_optional_list as _merge_optional_list,
    _parse_capability_manifest_dict as _parse_capability_manifest_dict,
    _parse_fs_block as _parse_fs_block,
    _parse_package_grant as _parse_package_grant,
    _parse_package_grants as _parse_package_grants,
    _resolve_capability_manifest as _resolve_capability_manifest,
)
from molt.cli.frontend_execution import (
    _run_frontend_pipeline,
)
from molt.cli.external_native import (
    _external_native_artifact_output_custody_error,
)
from molt.cli.output import (
    fail as _fail,
)
from molt.cli.runtime_paths import (
    _molt_session_id,
)
from molt.cli.extension_manifest import (
    _abi_version_error as _abi_version_error,
)
from molt.cli.models import (
    BuildProfile,
    FallbackPolicy,
    ParseCodec,
    TypeHintPolicy,
    _BuildOutputLayout,
    _PreparedBuildConfig,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _ResolvedBuildEntry,
)
from molt.cli.target_python import (
    _SUPPORTED_TARGET_PYTHON_BY_SHORT as _SUPPORTED_TARGET_PYTHON_BY_SHORT,
    _SUPPORTED_TARGET_PYTHON_VERSIONS as _SUPPORTED_TARGET_PYTHON_VERSIONS,
    _project_requires_python as _project_requires_python,
    _target_python_from_requires_python as _target_python_from_requires_python,
)
from molt.cli.mlir_backend import (
    _run_mlir_backend_pipeline,
)


def _session_target_dir(project_root: Path) -> Path | None:
    """Return a per-session CARGO_TARGET_DIR, or None for default.

    When MOLT_SESSION_ID is set, returns
    project_root/target/sessions/<session_id>.
    This keeps session-isolated Cargo output under the canonical target root
    while still eliminating lock contention between concurrent builds.
    """
    sid = _molt_session_id()
    if sid is None:
        return None
    return session_scoped_target_dir(project_root / "target", sid)


def _run_build_pipeline(
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
    prepared_frontend_run_ticket = prepared_frontend_pipeline_bundle[0]
    frontend_layer_error = _run_frontend_pipeline(
        prepared_frontend_run_ticket=prepared_frontend_run_ticket,
    )
    if frontend_layer_error is not None:
        return frontend_layer_error

    # MLIR target: run the frontend to produce TIR, then shell out to the
    # standalone molt-backend-mlir binary. This bypasses the standard backend
    # pipeline entirely because the MLIR crate is out-of-workspace.
    output_layout: _BuildOutputLayout = prepared_frontend_pipeline_bundle[5]
    native_artifact_plan = prepared_frontend_pipeline_bundle[22]
    native_artifact_custody_error = _external_native_artifact_output_custody_error(
        native_artifact_plan=native_artifact_plan,
        output_layout=output_layout,
        target=target,
    )
    if native_artifact_custody_error is not None:
        return _fail(native_artifact_custody_error, json_output, command="build")
    if fact_graph_request is not None and output_layout.is_mlir_emit:
        return _fail(
            "factgraph does not support the MLIR backend",
            json_output,
            command="factgraph",
        )
    if output_layout.is_mlir_emit:
        (
            _frt,
            module_graph,
            runtime_import_dispatch_roots,
            stdlib_allowlist,
            spawn_enabled,
            _ol,
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
            _build_diagnostics_payload,
            record_binary_image_analysis,
            artifacts_root,
            _native_artifact_plan,
        ) = prepared_frontend_pipeline_bundle
        prepared_backend_ir, prepared_backend_ir_error = (
            _backend_ir._prepare_backend_ir(
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
            )
        )
        if prepared_backend_ir_error is not None:
            return prepared_backend_ir_error
        assert prepared_backend_ir is not None
        if prepared_build_preamble.diagnostics_enabled:
            record_binary_image_analysis(
                "backend_ir",
                backend_ir_binary_image_analysis_payload(prepared_backend_ir.ir),
            )
        return _run_mlir_backend_pipeline(
            ir=prepared_backend_ir.ir,
            output_artifact=output_layout.output_artifact,
            project_root=prepared_build_roots.project_root,
            json_output=json_output,
            verbose=verbose,
        )

    return _backend_pipeline._run_backend_pipeline(
        prepared_build_preamble=prepared_build_preamble,
        prepared_build_roots=prepared_build_roots,
        prepared_build_config=prepared_build_config,
        resolved_build_entry=resolved_build_entry,
        prepared_frontend_pipeline_bundle=prepared_frontend_pipeline_bundle,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        profile=profile,
        json_output=json_output,
        target=target,
        cache_dir=cache_dir,
        cache=cache,
        cache_report=cache_report,
        deterministic=deterministic,
        trusted=trusted,
        verbose=verbose,
        require_linked=require_linked,
        wasm_opt_level=wasm_opt_level,
        precompile=precompile,
        snapshot=snapshot,
        stdlib_profile=stdlib_profile,
        fact_graph_request=fact_graph_request,
    )
