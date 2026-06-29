from __future__ import annotations

from pathlib import Path
from typing import Any, Callable

from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE
from molt.cli import link_pipeline as _link_pipeline
from molt.cli import non_native_output as _non_native_output
from molt.cli.build_results import _emit_native_link_result, _emit_non_native_build_result
from molt.cli.models import (
    BuildProfile,
    _BuildOutputLayout,
    _ExternalPackageNativeArtifactPlan,
    _PreparedBackendCompile,
    _PreparedBackendRuntimeContext,
    _PreparedBackendSetup,
    _PreparedBuildConfig,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _ResolvedBuildEntry,
)
from molt.cli.output import fail as _fail
from molt.cli.runtime_build import _ensure_native_runtime_lib_ready_before_link


def _emit_backend_pipeline_outputs(
    *,
    prepared_build_preamble: _PreparedBuildPreamble,
    prepared_build_roots: _PreparedBuildRoots,
    prepared_build_config: _PreparedBuildConfig,
    resolved_build_entry: _ResolvedBuildEntry,
    output_layout: _BuildOutputLayout,
    prepared_backend_setup: _PreparedBackendSetup,
    prepared_backend_runtime_context: _PreparedBackendRuntimeContext,
    prepared_backend_compile: _PreparedBackendCompile,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan,
    artifacts_root: Path,
    resolved_modules: frozenset[str],
    build_diagnostics_payload: Callable[[], tuple[Any, Path | None]],
    target: str,
    deterministic: bool,
    trusted: bool,
    verbose: bool,
    require_linked: bool,
    wasm_opt_level: str = "Oz",
    precompile: bool = False,
    snapshot: bool = False,
    profile: BuildProfile = "dev",
    json_output: bool = False,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
) -> int:
    diagnostics_payload, diagnostics_path = build_diagnostics_payload()
    runtime_lib = prepared_backend_runtime_context.runtime_lib
    runtime_wasm = prepared_backend_runtime_context.runtime_wasm
    runtime_reloc_wasm = prepared_backend_runtime_context.runtime_reloc_wasm
    ensure_runtime_wasm_shared = (
        prepared_backend_runtime_context.ensure_runtime_wasm_shared
    )
    ensure_runtime_wasm_reloc = (
        prepared_backend_runtime_context.ensure_runtime_wasm_reloc
    )
    cache = prepared_backend_compile.cache_enabled
    cache_hit = prepared_backend_compile.cache_hit
    cache_key = prepared_backend_runtime_context.cache_key
    function_cache_key = prepared_backend_runtime_context.function_cache_key
    cache_path = prepared_backend_runtime_context.cache_path
    function_cache_path = prepared_backend_runtime_context.function_cache_path
    cache_hit_tier = prepared_backend_compile.cache_hit_tier
    backend_daemon_cached = prepared_backend_compile.backend_daemon_cached
    backend_daemon_cache_tier = prepared_backend_compile.backend_daemon_cache_tier
    backend_daemon_config_digest = prepared_backend_compile.backend_daemon_config_digest
    wasm_table_base = prepared_backend_compile.wasm_table_base

    if (
        output_layout.is_rust_transpile
        or output_layout.is_luau_transpile
        or output_layout.is_wasm
    ):
        prepared_non_native_result, prepared_non_native_result_error = (
            _non_native_output._prepare_non_native_build_result(
                is_rust_transpile=output_layout.is_rust_transpile,
                is_luau_transpile=output_layout.is_luau_transpile,
                is_wasm=output_layout.is_wasm,
                is_wasm_freestanding=output_layout.is_wasm_freestanding,
                wasm_opt_level=wasm_opt_level,
                wasm_table_base=wasm_table_base,
                linked=output_layout.linked,
                require_linked=require_linked,
                linked_output_path=output_layout.linked_output_path,
                output_artifact=output_layout.output_artifact,
                json_output=json_output,
                runtime_wasm=runtime_wasm,
                runtime_reloc_wasm=runtime_reloc_wasm,
                ensure_runtime_wasm_shared=ensure_runtime_wasm_shared,
                ensure_runtime_wasm_reloc=ensure_runtime_wasm_reloc,
                runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
                molt_root=prepared_build_roots.molt_root,
                project_root=prepared_build_roots.project_root,
                profile=profile,
                warnings=prepared_build_preamble.warnings,
                precompile=precompile,
                split_runtime=output_layout.split_runtime,
                native_artifact_plan=native_artifact_plan,
                artifacts_root=artifacts_root,
            )
        )
        if prepared_non_native_result_error is not None:
            return prepared_non_native_result_error
        assert prepared_non_native_result is not None

        # -- Snapshot header generation (Plan D) ----------------------------
        if snapshot and output_layout.is_wasm:
            _non_native_output._generate_snapshot_header(
                output_wasm=prepared_non_native_result.primary_output,
                target_profile=target,
                capabilities_list=prepared_build_config.capabilities_list,
                verbose=verbose,
            )
            prepared_non_native_result.success_messages.append(
                f"Snapshot header: {prepared_non_native_result.primary_output.parent / 'molt.snapshot.json'}"
            )
        # -- End snapshot header generation ----------------------------------

        return _emit_non_native_build_result(
            output=prepared_non_native_result.primary_output,
            consumer_output=prepared_non_native_result.consumer_output,
            bundle_root=prepared_non_native_result.bundle_root,
            cache=cache,
            cache_hit=cache_hit,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            cache_path=cache_path,
            function_cache_path=function_cache_path,
            cache_hit_tier=cache_hit_tier,
            backend_daemon_cached=backend_daemon_cached,
            backend_daemon_cache_tier=backend_daemon_cache_tier,
            backend_daemon_config_digest=backend_daemon_config_digest,
            target=target,
            target_triple=output_layout.target_triple,
            source_path=resolved_build_entry.source_path,
            deterministic=deterministic,
            trusted=trusted,
            capabilities_list=prepared_build_config.capabilities_list,
            capability_profiles=prepared_build_config.capability_profiles,
            capabilities_source=prepared_build_config.capabilities_source,
            sysroot_path=prepared_build_roots.sysroot_path,
            emit_mode=output_layout.emit_mode,
            profile=profile,
            native_arch_perf_enabled=prepared_build_preamble.native_arch_perf_enabled,
            diagnostics_payload=diagnostics_payload,
            diagnostics_path=diagnostics_path,
            pgo_profile_payload=prepared_build_config.pgo_profile_payload,
            runtime_feedback_payload=prepared_build_config.runtime_feedback_payload,
            emit_ir_path=output_layout.emit_ir_path,
            warnings=prepared_build_preamble.warnings,
            json_output=json_output,
            resolved_diagnostics_verbosity=prepared_build_preamble.resolved_diagnostics_verbosity,
            extra_fields=prepared_non_native_result.extra_fields,
            artifacts=prepared_non_native_result.artifacts,
            success_messages=prepared_non_native_result.success_messages,
        )

    if output_layout.emit_mode == "obj":
        prepared_object_output, _partial_link_process, prepared_object_error = (
            _link_pipeline._prepare_native_object_artifact(
                output_artifact=output_layout.output_artifact,
                artifacts_root=artifacts_root,
                stdlib_obj_path=prepared_backend_setup.cache_setup.stdlib_object_path,
                stdlib_object_cache_key=prepared_backend_setup.cache_setup.stdlib_object_cache_key,
                stdlib_object_manifest=prepared_backend_setup.cache_setup.stdlib_object_manifest,
                stdlib_module_symbols=prepared_backend_setup.cache_setup.stdlib_module_symbols,
                json_output=json_output,
                link_timeout=prepared_build_config.link_timeout,
                target_triple=output_layout.target_triple,
                sysroot_path=prepared_build_roots.sysroot_path,
            )
        )
        if prepared_object_error is not None:
            return prepared_object_error
        assert prepared_object_output is not None
        return _emit_non_native_build_result(
            output=prepared_object_output,
            consumer_output=prepared_object_output,
            bundle_root=None,
            cache=cache,
            cache_hit=cache_hit,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            cache_path=cache_path,
            function_cache_path=function_cache_path,
            cache_hit_tier=cache_hit_tier,
            backend_daemon_cached=backend_daemon_cached,
            backend_daemon_cache_tier=backend_daemon_cache_tier,
            backend_daemon_config_digest=backend_daemon_config_digest,
            target=target,
            target_triple=output_layout.target_triple,
            source_path=resolved_build_entry.source_path,
            deterministic=deterministic,
            trusted=trusted,
            capabilities_list=prepared_build_config.capabilities_list,
            capability_profiles=prepared_build_config.capability_profiles,
            capabilities_source=prepared_build_config.capabilities_source,
            sysroot_path=prepared_build_roots.sysroot_path,
            emit_mode=output_layout.emit_mode,
            profile=profile,
            native_arch_perf_enabled=prepared_build_preamble.native_arch_perf_enabled,
            diagnostics_payload=diagnostics_payload,
            diagnostics_path=diagnostics_path,
            pgo_profile_payload=prepared_build_config.pgo_profile_payload,
            runtime_feedback_payload=prepared_build_config.runtime_feedback_payload,
            emit_ir_path=output_layout.emit_ir_path,
            warnings=prepared_build_preamble.warnings,
            json_output=json_output,
            resolved_diagnostics_verbosity=prepared_build_preamble.resolved_diagnostics_verbosity,
            artifacts={"object": str(prepared_object_output)},
            success_messages=[f"Successfully built {prepared_object_output}"],
        )

    stdlib_link_obj_path = prepared_backend_setup.cache_setup.stdlib_object_path

    if not _ensure_native_runtime_lib_ready_before_link(
        prepared_backend_runtime_context.runtime_state,
        target_triple=output_layout.target_triple,
        json_output=json_output,
        runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
        molt_root=prepared_build_roots.molt_root,
        cargo_timeout=prepared_build_config.cargo_timeout,
        diagnostics_enabled=prepared_build_preamble.diagnostics_enabled,
        phase_starts=prepared_build_preamble.phase_starts,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
    ):
        return _fail("Runtime build failed", json_output, command="build")
    if prepared_build_preamble.diagnostics_enabled:
        diagnostics_payload, diagnostics_path = build_diagnostics_payload()
    prepared_native_link, prepared_native_link_error = _link_pipeline._prepare_native_link(
        output_artifact=output_layout.output_artifact,
        trusted=trusted,
        capabilities_list=prepared_build_config.capabilities_list,
        artifacts_root=artifacts_root,
        json_output=json_output,
        output_binary=output_layout.output_binary,
        runtime_lib=runtime_lib,
        molt_root=prepared_build_roots.molt_root,
        runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
        target_triple=output_layout.target_triple,
        sysroot_path=prepared_build_roots.sysroot_path,
        profile=profile,
        project_root=prepared_build_roots.project_root,
        diagnostics_enabled=prepared_build_preamble.diagnostics_enabled,
        phase_starts=prepared_build_preamble.phase_starts,
        link_timeout=prepared_build_config.link_timeout,
        warnings=prepared_build_preamble.warnings,
        stdlib_obj_path=stdlib_link_obj_path,
        stdlib_object_cache_key=prepared_backend_setup.cache_setup.stdlib_object_cache_key,
        stdlib_object_manifest=prepared_backend_setup.cache_setup.stdlib_object_manifest,
        stdlib_module_symbols=prepared_backend_setup.cache_setup.stdlib_module_symbols,
        native_artifact_plan=native_artifact_plan,
        stdlib_profile=stdlib_profile,
    )
    if prepared_native_link_error is not None:
        return prepared_native_link_error
    assert prepared_native_link is not None
    return _emit_native_link_result(
        link_process=prepared_native_link.link_process,
        link_skipped=prepared_native_link.link_skipped,
        link_fingerprint=prepared_native_link.link_fingerprint,
        link_fingerprint_path=prepared_native_link.link_fingerprint_path,
        cache=cache,
        cache_hit=cache_hit,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        cache_hit_tier=cache_hit_tier,
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_config_digest=backend_daemon_config_digest,
        target=target,
        target_triple=output_layout.target_triple,
        source_path=resolved_build_entry.source_path,
        output_binary=prepared_native_link.output_binary,
        deterministic=deterministic,
        trusted=trusted,
        capabilities_list=prepared_build_config.capabilities_list,
        capability_profiles=prepared_build_config.capability_profiles,
        capabilities_source=prepared_build_config.capabilities_source,
        sysroot_path=prepared_build_roots.sysroot_path,
        emit_mode=output_layout.emit_mode,
        profile=profile,
        native_arch_perf_enabled=prepared_build_preamble.native_arch_perf_enabled,
        output_obj=prepared_native_link.output_obj,
        stub_path=prepared_native_link.stub_path,
        runtime_lib=prepared_native_link.runtime_lib,
        external_native_artifacts=prepared_native_link.external_native_artifacts,
        diagnostics_payload=diagnostics_payload,
        diagnostics_path=diagnostics_path,
        pgo_profile_payload=prepared_build_config.pgo_profile_payload,
        runtime_feedback_payload=prepared_build_config.runtime_feedback_payload,
        emit_ir_path=output_layout.emit_ir_path,
        stdlib_obj_path=stdlib_link_obj_path,
        warnings=prepared_build_preamble.warnings,
        json_output=json_output,
        resolved_diagnostics_verbosity=prepared_build_preamble.resolved_diagnostics_verbosity,
    )
