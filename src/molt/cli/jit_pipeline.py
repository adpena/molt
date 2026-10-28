"""Prepare one program for the development JIT tier (``molt build --emit jit``).

The JIT tier (HF-371, ``docs/design/dev_jit_tier.md``) skips object emission,
the system link and executable publication. After the shared frontend and
backend IR preparation, this pipeline admits the runtime image and the
``molt-jit`` runner and publishes one JIT image (``jit_image.py``) that holds
the program and how to start it. ``molt run --tier jit`` then runs the runner
on the image; the runner compiles the program into its own memory and starts
it. Build request resolution already admitted the cell
(``run_tier.jit_tier_refusal``).
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from molt.backend_environment import request_environment_inputs
from molt.cli import backend_ir as _backend_ir
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli import runtime_features as _runtime_features
from molt.cli.backend_diagnostics import _env_requests_backend_diagnostics
from molt.cli.backend_execution import _backend_request_environment
from molt.cli.backend_ir_transport import BackendIrProgram
from molt.cli.build_request import BuildProfile
from molt.cli.cache_keys import FunctionDigests
from molt.cli.build_results import (
    _build_common_build_json_data,
    _emit_build_result_json,
)
from molt.cli.jit_image import publish_jit_image
from molt.cli.jit_runner_tool import ensure_jit_runner
from molt.cli.jit_runtime_image import (
    ensure_jit_runtime_image,
    jit_runtime_image_store,
)
from molt.cli.models import (
    _PreparedBuildConfig,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _ResolvedBuildEntry,
)
from molt.cli.native_launch_plan import NativeLaunchPlan
from molt.cli.output import fail as _fail
from molt.cli.runtime_build import _initialize_runtime_artifact_state
from molt.cli.runtime_callable_symbols import (
    _stage_runtime_callable_symbols_for_native_codegen,
)
from molt.cli.runtime_native_codegen import native_runtime_codegen_environment
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.compilation_target import Target
from molt.default_paths import _default_molt_cache
from molt.native_target import NativeTargetSpec, resolve_native_target_spec


def jit_image_header(
    *,
    runtime_image: Path,
    backend_environment: Mapping[str, str],
    launch: Mapping[str, Any],
) -> dict[str, Any]:
    """The JIT image header ``runtime/molt-jit/src/image.rs`` reads."""
    return {
        "runtime_image": str(runtime_image),
        "backend_environment": dict(sorted(backend_environment.items())),
        "compiler_diagnostics": (
            "stream" if _env_requests_backend_diagnostics(os.environ) else "on_failure"
        ),
        # The runner's environment and current directory are the program's, so
        # the CLI names the cache root it resolved; the runner keeps its TIR
        # cache under it.
        "tir_cache_root": str(_default_molt_cache()),
        "launch": dict(launch),
    }


def _run_jit_pipeline(
    *,
    prepared_build_preamble: _PreparedBuildPreamble,
    prepared_build_roots: _PreparedBuildRoots,
    prepared_build_config: _PreparedBuildConfig,
    resolved_build_entry: _ResolvedBuildEntry,
    prepared_frontend_pipeline_bundle: _frontend_pipeline._PreparedFrontendPipelineBundle,
    profile: BuildProfile,
    json_output: bool,
    target: Target,
    deterministic: bool,
    trusted: bool,
    stdlib_profile: str | None,
) -> int:
    output_layout = prepared_frontend_pipeline_bundle.output_layout
    native_artifact_plan = prepared_frontend_pipeline_bundle.native_artifact_plan
    module_graph = prepared_frontend_pipeline_bundle.module_graph
    # The AOT backend pipeline's one source of logical paths (HF-372).
    graph_metadata = prepared_frontend_pipeline_bundle.prepared_frontend_run_ticket.frontend_layer_execution_context.module_graph_metadata
    logical_source_path_by_module = graph_metadata.logical_source_path_by_module
    prepared_backend_ir, prepared_backend_ir_error = _backend_ir._prepare_backend_ir(
        entry_module=resolved_build_entry.entry_module,
        module_graph=module_graph,
        logical_source_path_by_module=logical_source_path_by_module,
        module_is_namespace_by_module=graph_metadata.module_is_namespace_by_module,
        module_source_role_by_module=graph_metadata.module_source_role_by_module,
        known_modules=prepared_frontend_pipeline_bundle.known_modules,
        stdlib_allowlist=prepared_frontend_pipeline_bundle.stdlib_allowlist,
        integration_state=prepared_frontend_pipeline_bundle.integration_state,
        fail=_fail,
        json_output=json_output,
        module_order=prepared_frontend_pipeline_bundle.module_order,
        runtime_import_dispatch_roots=(
            prepared_frontend_pipeline_bundle.runtime_import_dispatch_roots
        ),
        spawn_enabled=prepared_frontend_pipeline_bundle.spawn_enabled,
        pgo_profile_summary=prepared_build_config.pgo_profile_summary,
        runtime_feedback_summary=prepared_build_config.runtime_feedback_summary,
        emit_ir_path=output_layout.emit_ir_path,
        validate_inputs=prepared_frontend_pipeline_bundle.module_source_catalog.validate,
        target_python=prepared_build_config.target_python,
        stdlib_profile=stdlib_profile,
        target=target,
        target_triple=output_layout.target_triple,
        native_artifact_plan=native_artifact_plan,
    )
    if prepared_backend_ir_error is not None:
        return prepared_backend_ir_error
    assert prepared_backend_ir is not None
    molt_root = prepared_build_roots.molt_root
    # Admit the runner first: its probe refuses a host that blocks executable
    # memory before this build builds the runtime and links its 37 MiB image.
    try:
        runner = ensure_jit_runner(
            molt_root,
            cargo_profile=prepared_build_config.backend_cargo_profile,
            rust_cpu=prepared_build_preamble.rust_cpu,
            cargo_timeout=prepared_build_config.cargo_timeout,
            json_output=json_output,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        return _fail(str(exc), json_output, command="build")
    runtime_stdlib_profile = (
        _runtime_features.runtime_stdlib_profile_for_required_features(
            stdlib_profile,
            prepared_backend_ir.required_link_features,
            target_triple=output_layout.target_triple,
        )
    )
    try:
        runtime_state = _initialize_runtime_artifact_state(
            is_rust_transpile=False,
            is_wasm=False,
            emit_mode="bin",
            molt_root=molt_root,
            runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
            target_triple=output_layout.target_triple,
            stdlib_profile=runtime_stdlib_profile,
            rust_cpu=prepared_build_preamble.rust_cpu,
        )
    except (OSError, ValueError) as exc:
        return _fail(str(exc), json_output, command="build")
    _digest, callable_symbols_error = (
        _stage_runtime_callable_symbols_for_native_codegen(
            runtime_state,
            target_triple=output_layout.target_triple,
            json_output=json_output,
            runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
            molt_root=molt_root,
            cargo_timeout=prepared_build_config.cargo_timeout,
            stdlib_profile=runtime_stdlib_profile,
            resolved_modules=frozenset(module_graph),
        )
    )
    if callable_symbols_error is not None:
        return callable_symbols_error
    binding = runtime_state.native_runtime_codegen_binding
    if binding is None:
        return _fail(
            "The JIT tier found no admitted native runtime for this program",
            json_output,
            command="build",
        )
    try:
        native_target = (
            target
            if isinstance(target, NativeTargetSpec)
            else resolve_native_target_spec(output_layout.target_triple)
        )
        runtime_image = ensure_jit_runtime_image(
            store=jit_runtime_image_store(molt_root),
            binding=binding,
            target=native_target,
            sysroot_path=prepared_build_roots.sysroot_path,
            profile=profile,
            link_timeout=prepared_build_config.link_timeout,
        )
        backend_environment = request_environment_inputs(
            _backend_request_environment(
                native_runtime_codegen_environment(
                    prepared_build_config.codegen.environment(
                        os.environ, profile=profile
                    ),
                    binding,
                ),
                entry_module=None,
            )
        )
    except (OSError, RuntimeError, ValueError) as exc:
        return _fail(str(exc), json_output, command="build")
    image = output_layout.output_artifact
    plan = NativeLaunchPlan.for_program(
        resolved_capability_policy=prepared_build_config.resolved_capability_policy,
        compiled_source_paths=(
            prepared_frontend_pipeline_bundle.module_source_catalog.source_paths(
                logical_source_path_by_module.values()
            )
        ),
    )
    try:
        # The published program must be the one these sources lowered to.
        prepared_frontend_pipeline_bundle.module_source_catalog.validate()
        publish_jit_image(
            image,
            runner=runner,
            header=jit_image_header(
                runtime_image=runtime_image,
                backend_environment=backend_environment,
                launch=plan.image_header_payload(),
            ),
            program=BackendIrProgram(
                prepared_backend_ir.ir,
                FunctionDigests(prepared_backend_ir.function_digest_seeds),
            ),
        )
    except (OSError, ValueError) as exc:
        return _fail(
            f"Cannot publish the JIT image: {exc}", json_output, command="build"
        )
    data = _build_common_build_json_data(
        target=target,
        target_triple=output_layout.target_triple,
        source_path=resolved_build_entry.source_path,
        output=image,
        deterministic=deterministic,
        trusted=trusted,
        resolved_capability_policy=prepared_build_config.resolved_capability_policy,
        capabilities_source=prepared_build_config.capabilities_source,
        sysroot_path=prepared_build_roots.sysroot_path,
        cache_info={},
        emit_mode=output_layout.emit_mode,
        profile=profile,
        native_arch_perf_enabled=(
            prepared_build_preamble.rust_cpu is RustCpuPolicy.NATIVE
        ),
    )
    data["consumer_output"] = str(image)
    data["messages"] = [f"Prepared JIT image {image}"]
    data["artifacts"] = {
        "jit_image": str(image),
        "runner": str(runner),
        "runtime_image": str(runtime_image),
        "callable_symbols": str(binding.callable_symbols.path),
    }
    return _emit_build_result_json(
        data=data,
        warnings=prepared_build_preamble.warnings,
        json_output=json_output,
        returncode=0,
        diagnostics_error=None,
    )
