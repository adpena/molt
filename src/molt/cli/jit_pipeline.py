"""Prepare one program for the development JIT tier (``molt build --emit jit``).

The JIT tier (HF-371, ``docs/design/dev_jit_tier.md``) skips object emission,
the system link and executable publication. After the shared frontend and
backend IR preparation, this pipeline publishes the program's backend IR and a
launch manifest, admits the runtime image and the ``molt-jit`` runner, and
returns. ``molt run --tier jit`` then runs the runner on the manifest; the
runner compiles the program into its own memory and starts it.
"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
from typing import Any, BinaryIO

from molt import file_publication
from molt.backend_environment import request_environment_inputs
from molt.cli import backend_ir as _backend_ir
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli import runtime_features as _runtime_features
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.backend_diagnostics import _env_requests_backend_diagnostics
from molt.cli.backend_execution import _backend_request_environment
from molt.cli.backend_ir_transport import write_backend_ir
from molt.cli.build_request import BuildProfile
from molt.cli.build_results import (
    _build_common_build_json_data,
    _emit_build_result_json,
)
from molt.cli.jit_runner_tool import ensure_jit_runner
from molt.cli.jit_runtime_image import ensure_jit_runtime_image
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
from molt.cli.run_tier import JIT_LAUNCH_SUFFIX, JIT_PROGRAM_SUFFIX
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.compilation_target import Target, TargetKind
from molt.native_target import NativeTargetSpec, resolve_native_target_spec

JIT_LAUNCH_SCHEMA = "molt.jit-launch.v1"


def jit_program_path(launch_manifest: Path) -> Path:
    """The backend IR beside a launch manifest."""
    name = launch_manifest.name
    stem = (
        name.removesuffix(JIT_LAUNCH_SUFFIX)
        if name.endswith(JIT_LAUNCH_SUFFIX)
        else name
    )
    return launch_manifest.with_name(stem + JIT_PROGRAM_SUFFIX)


# The (os, arch) hosts where the JIT-AOT twin gate ran (HF-371). Any other
# host fails here, before its unverified runtime image link and loader run.
VERIFIED_JIT_HOSTS = frozenset({("macos", "aarch64")})


def _unsupported_jit_construct(
    *,
    target: Target,
    codegen_backend: str,
    has_external_native_artifacts: bool,
    host: NativeTargetSpec | None = None,
) -> str | None:
    """Name the first construct the JIT tier cannot run, or None."""
    if isinstance(target, NativeTargetSpec):
        if not target.is_host:
            return f"the cross target {target.triple} (the JIT tier runs on the host)"
    elif target is not TargetKind.NATIVE:
        return f"the {target.value} target (the JIT tier runs native host code)"
    host = resolve_native_target_spec(None) if host is None else host
    if (host.os, host.arch) not in VERIFIED_JIT_HOSTS:
        return (
            f"the {host.triple} host (the JIT tier is verified only on aarch64 "
            "macOS; HF-371 tracks the other hosts)"
        )
    if codegen_backend != "cranelift":
        return f"the {codegen_backend} code generator (the JIT tier uses Cranelift)"
    if has_external_native_artifacts:
        return "external native package artifacts (they need the system linker)"
    return None


class _HashingWriter:
    """Hash every byte on its way to the file."""

    def __init__(self, handle: BinaryIO) -> None:
        self._handle = handle
        self.digest = hashlib.sha256()

    def write(self, data: bytes) -> int:
        self.digest.update(data)
        return self._handle.write(data)


def _publish_program(path: Path, ir: Any) -> str:
    staged = file_publication.staged_file_path(
        path, purpose="jit-program", suffix=JIT_PROGRAM_SUFFIX
    )
    try:
        with staged.open("xb") as handle:
            writer = _HashingWriter(handle)
            write_backend_ir(writer, ir)
            handle.flush()
            os.fsync(handle.fileno())
        file_publication.durable_namespace_replace(staged, path)
    except BaseException:
        staged.unlink(missing_ok=True)
        raise
    return writer.digest.hexdigest()


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
    unsupported = _unsupported_jit_construct(
        target=target,
        codegen_backend=prepared_build_config.codegen.backend,
        has_external_native_artifacts=bool(native_artifact_plan.artifacts),
    )
    if unsupported is not None:
        return _fail(
            f"The JIT tier does not support {unsupported}. "
            "Run it with `molt run --tier aot` or build it with `molt build`.",
            json_output,
            command="build",
        )
    module_graph = prepared_frontend_pipeline_bundle.module_graph
    prepared_backend_ir, prepared_backend_ir_error = _backend_ir._prepare_backend_ir(
        entry_module=resolved_build_entry.entry_module,
        module_graph=module_graph,
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
            binding=binding,
            target=native_target,
            sysroot_path=prepared_build_roots.sysroot_path,
            profile=profile,
            link_timeout=prepared_build_config.link_timeout,
        )
        runner = ensure_jit_runner(
            molt_root,
            cargo_profile=prepared_build_config.backend_cargo_profile,
            rust_cpu=prepared_build_preamble.rust_cpu,
            cargo_timeout=prepared_build_config.cargo_timeout,
            json_output=json_output,
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
    launch_manifest = output_layout.output_artifact
    program = jit_program_path(launch_manifest)
    plan = NativeLaunchPlan.for_program(
        resolved_capability_policy=prepared_build_config.resolved_capability_policy,
        compiled_source_paths=(
            prepared_frontend_pipeline_bundle.module_source_catalog.source_paths()
        ),
    )
    try:
        # The published program must be the one these sources lowered to.
        prepared_frontend_pipeline_bundle.module_source_catalog.validate()
        program_sha256 = _publish_program(program, prepared_backend_ir.ir)
        _atomic_write_json(
            launch_manifest,
            {
                "schema": JIT_LAUNCH_SCHEMA,
                "program": str(program),
                "program_sha256": program_sha256,
                "runtime_image": str(runtime_image),
                "backend_environment": dict(sorted(backend_environment.items())),
                "compiler_diagnostics": (
                    "stream"
                    if _env_requests_backend_diagnostics(os.environ)
                    else "on_failure"
                ),
                "launch": plan.manifest_payload(),
            },
            indent=2,
        )
    except (OSError, ValueError) as exc:
        return _fail(
            f"Cannot publish the JIT program: {exc}", json_output, command="build"
        )
    data = _build_common_build_json_data(
        target=target,
        target_triple=output_layout.target_triple,
        source_path=resolved_build_entry.source_path,
        output=launch_manifest,
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
    data["consumer_output"] = str(launch_manifest)
    data["messages"] = [f"Prepared JIT program {launch_manifest}"]
    data["artifacts"] = {
        "launch_manifest": str(launch_manifest),
        "program": str(program),
        "runner": str(runner),
        "runtime_image": str(runtime_image),
    }
    return _emit_build_result_json(
        data=data,
        warnings=prepared_build_preamble.warnings,
        json_output=json_output,
        returncode=0,
        diagnostics_error=None,
    )
