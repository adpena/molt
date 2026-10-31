from __future__ import annotations

from molt.compilation_target import Target

import time

from molt.cli.build_diagnostics import (
    _emit_build_diagnostics_for_result,
    _frontend_lowering_cache_summary,
)
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE
from molt.cli.cache_keys import FunctionDigests
from molt.cli import backend_ir as _backend_ir
from molt.cli import backend_ir_analysis_cache as _backend_ir_analysis_cache
from molt.cli import backend_pipeline as _backend_pipeline
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli import jit_pipeline as _jit_pipeline
from molt.cli import progress as _progress
from molt.cli.build_request import FactGraphRequest
from molt.backend_daemon_paths import (
    _unix_socket_path_exceeds_limit as _unix_socket_path_exceeds_limit,
)
from molt.cli.backend_diagnostics import (
    _BACKEND_DIAGNOSTIC_ENV_KNOBS as _BACKEND_DIAGNOSTIC_ENV_KNOBS,
    _PYTHON_WARNING_RE as _PYTHON_WARNING_RE,
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
from molt.cli.extension_manifest import (
    _abi_version_error as _abi_version_error,
)
from molt.cli.build_request import BuildProfile
from molt.cli.models import (
    _PreparedBuildConfig,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _ResolvedBuildEntry,
)
from molt.cli.build_output_layout import _BuildOutputLayout
from molt.target_python import (
    _SUPPORTED_TARGET_PYTHON_BY_SHORT as _SUPPORTED_TARGET_PYTHON_BY_SHORT,
    _SUPPORTED_TARGET_PYTHON_VERSIONS as _SUPPORTED_TARGET_PYTHON_VERSIONS,
    _project_requires_python as _project_requires_python,
    _target_python_from_requires_python as _target_python_from_requires_python,
)
from molt.cli.mlir_backend import (
    _run_mlir_backend_pipeline,
)


def _run_build_pipeline(
    *,
    prepared_build_preamble: _PreparedBuildPreamble,
    prepared_build_roots: _PreparedBuildRoots,
    prepared_build_config: _PreparedBuildConfig,
    resolved_build_entry: _ResolvedBuildEntry,
    prepared_frontend_pipeline_bundle: _frontend_pipeline._PreparedFrontendPipelineBundle,
    profile: BuildProfile,
    json_output: bool,
    target: Target,
    cache_dir: str | None,
    cache: bool,
    cache_report: bool,
    deterministic: bool,
    trusted: bool,
    verbose: bool,
    require_linked: bool,
    capture_link_inputs: bool = False,
    wasm_opt_level: str | None = "Oz",
    precompile: bool = False,
    snapshot: bool = False,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
    bolt_requested: bool = False,
    bolt_training_cmd: str | None = None,
    fact_graph_request: FactGraphRequest | None = None,
) -> int:
    def return_after_build_diagnostics(result: int) -> int:
        return _emit_build_diagnostics_for_result(
            result,
            diagnostics_enabled=prepared_build_preamble.diagnostics_enabled,
            build_diagnostics_payload=(
                prepared_frontend_pipeline_bundle.build_diagnostics_payload
            ),
            json_output=json_output,
            verbosity=prepared_build_preamble.resolved_diagnostics_verbosity,
        )

    prepared_frontend_run_ticket = (
        prepared_frontend_pipeline_bundle.prepared_frontend_run_ticket
    )
    output_layout: _BuildOutputLayout = prepared_frontend_pipeline_bundle.output_layout
    if capture_link_inputs and (
        fact_graph_request is not None
        or output_layout.emit_mode in {"jit", "obj"}
        or output_layout.is_mlir_emit
        or output_layout.is_rust_transpile
        or output_layout.is_luau_transpile
        or output_layout.is_wasm
        and not output_layout.linked
    ):
        return return_after_build_diagnostics(
            _fail(
                "Link input capture requires a linked native/WASM artifact with retained original emission origin; "
                "JIT, factgraph, object and transpiler capture are not implemented.",
                json_output,
                command="build",
            )
        )
    native_artifact_plan = prepared_frontend_pipeline_bundle.native_artifact_plan
    # The import plan already names every external native artifact, so an
    # output that cannot carry them is refused before the frontend runs.
    native_artifact_custody_error = _external_native_artifact_output_custody_error(
        native_artifact_plan=native_artifact_plan,
        output_layout=output_layout,
        target=target,
    )
    if native_artifact_custody_error is not None:
        return return_after_build_diagnostics(
            _fail(native_artifact_custody_error, json_output, command="build")
        )
    _progress.phase("frontend")
    frontend_run_start = time.perf_counter()
    frontend_layer_error = _run_frontend_pipeline(
        prepared_frontend_run_ticket=prepared_frontend_run_ticket,
    )
    if prepared_build_preamble.diagnostics_enabled:
        prepared_frontend_run_ticket.frontend_parallel_details.setdefault(
            "pipeline_stage_ms", {}
        )["run_frontend_pipeline"] = round(
            max(0.0, (time.perf_counter() - frontend_run_start) * 1000.0),
            6,
        )
    if frontend_layer_error is not None:
        return return_after_build_diagnostics(frontend_layer_error)
    if verbose and not json_output:
        summary = _frontend_lowering_cache_summary(
            {
                "worker_timings": prepared_frontend_run_ticket.frontend_parallel_worker_timings
            }
        )
        if summary is not None:
            print(f"Frontend {summary['message']}")

    _progress.phase("backend_pipeline")

    if output_layout.emit_mode == "jit":
        if fact_graph_request is not None:
            return return_after_build_diagnostics(
                _fail(
                    "factgraph does not support the JIT tier",
                    json_output,
                    command="factgraph",
                )
            )
        return return_after_build_diagnostics(
            _jit_pipeline._run_jit_pipeline(
                prepared_build_preamble=prepared_build_preamble,
                prepared_build_roots=prepared_build_roots,
                prepared_build_config=prepared_build_config,
                resolved_build_entry=resolved_build_entry,
                prepared_frontend_pipeline_bundle=prepared_frontend_pipeline_bundle,
                profile=profile,
                json_output=json_output,
                target=target,
                deterministic=deterministic,
                trusted=trusted,
                stdlib_profile=stdlib_profile,
            )
        )
    # MLIR target: run the frontend to produce TIR, then shell out to the
    # standalone molt-backend-mlir binary. This bypasses the standard backend
    # pipeline entirely because the MLIR crate is out-of-workspace.
    if fact_graph_request is not None and output_layout.is_mlir_emit:
        return return_after_build_diagnostics(
            _fail(
                "factgraph does not support the MLIR backend",
                json_output,
                command="factgraph",
            )
        )
    if output_layout.is_mlir_emit:
        module_graph = prepared_frontend_pipeline_bundle.module_graph
        runtime_import_dispatch_roots = (
            prepared_frontend_pipeline_bundle.runtime_import_dispatch_roots
        )
        stdlib_allowlist = prepared_frontend_pipeline_bundle.stdlib_allowlist
        spawn_enabled = prepared_frontend_pipeline_bundle.spawn_enabled
        known_modules = prepared_frontend_pipeline_bundle.known_modules
        module_order = prepared_frontend_pipeline_bundle.module_order
        integration_state = prepared_frontend_pipeline_bundle.integration_state
        record_binary_image_analysis = (
            prepared_frontend_pipeline_bundle.record_binary_image_analysis
        )
        prepared_backend_ir, prepared_backend_ir_error = (
            _backend_ir._prepare_backend_ir(
                entry_module=resolved_build_entry.entry_module,
                module_graph=module_graph,
                logical_source_path_by_module=(
                    prepared_frontend_run_ticket.frontend_layer_execution_context.module_graph_metadata.logical_source_path_by_module
                ),
                namespace_portions_by_module=(
                    prepared_frontend_run_ticket.frontend_layer_execution_context.module_graph_metadata.namespace_portions_by_module
                ),
                module_source_role_by_module=(
                    prepared_frontend_run_ticket.frontend_layer_execution_context.module_graph_metadata.module_source_role_by_module
                ),
                known_modules=known_modules,
                stdlib_allowlist=stdlib_allowlist,
                integration_state=integration_state,
                fail=_fail,
                json_output=json_output,
                module_order=module_order,
                runtime_import_dispatch_roots=runtime_import_dispatch_roots,
                spawn_enabled=spawn_enabled,
                pgo_profile_summary=prepared_build_config.pgo_profile_summary,
                runtime_feedback_summary=prepared_build_config.runtime_feedback_summary,
                emit_ir_path=output_layout.emit_ir_path,
                validate_inputs=prepared_frontend_pipeline_bundle.module_source_catalog.validate,
                target_python=prepared_build_config.target_python,
                stdlib_profile=stdlib_profile,
                target=target,
                facts_target=output_layout.facts_target,
                native_artifact_plan=native_artifact_plan,
            )
        )
        if prepared_backend_ir_error is not None:
            return return_after_build_diagnostics(prepared_backend_ir_error)
        assert prepared_backend_ir is not None
        if prepared_build_preamble.diagnostics_enabled:
            backend_ir_analysis = _backend_ir_analysis_cache._cached_backend_ir_binary_image_analysis_payload(
                project_root=prepared_build_roots.project_root,
                ir=prepared_backend_ir.ir,
                function_digests=FunctionDigests(
                    prepared_backend_ir.function_digest_seeds
                ),
            )
            record_binary_image_analysis(
                "backend_ir",
                backend_ir_analysis.payload,
            )
            prepared_frontend_run_ticket.frontend_parallel_details[
                "backend_ir_binary_image_analysis_cache"
            ] = {
                "hit": backend_ir_analysis.cache_hit,
                "key": backend_ir_analysis.cache_key[:16],
                "path": str(backend_ir_analysis.cache_path),
            }
        return _run_mlir_backend_pipeline(
            ir=prepared_backend_ir.ir,
            output_artifact=output_layout.output_artifact,
            project_root=prepared_build_roots.project_root,
            molt_root=prepared_build_roots.molt_root,
            json_output=json_output,
            verbose=verbose,
            validate_inputs=prepared_frontend_pipeline_bundle.module_source_catalog.validate,
        )

    return _backend_pipeline._run_backend_pipeline(
        prepared_build_preamble=prepared_build_preamble,
        prepared_build_roots=prepared_build_roots,
        prepared_build_config=prepared_build_config,
        resolved_build_entry=resolved_build_entry,
        prepared_frontend_pipeline_bundle=prepared_frontend_pipeline_bundle,
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
        capture_link_inputs=capture_link_inputs,
        wasm_opt_level=wasm_opt_level,
        precompile=precompile,
        snapshot=snapshot,
        stdlib_profile=stdlib_profile,
        bolt_requested=bolt_requested,
        bolt_training_cmd=bolt_training_cmd,
        fact_graph_request=fact_graph_request,
    )
