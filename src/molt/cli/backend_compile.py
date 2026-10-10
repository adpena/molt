from __future__ import annotations

from molt.cli.runtime_build_python import BuildPythonAdmission

from molt.cli.native_symbol_inspection import (
    NativeSymbolInspectionError,
)

import os
import subprocess
import sys
import time
from contextlib import nullcontext
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, ContextManager, Mapping

from molt.backend_environment import CodegenSelection
from molt.backend_executable_names import CodegenBackend
from molt.capability_manifest import ResolvedRuntimePolicy
from molt.cli.compiler_identity import CompilerIdentityError
from molt.cli import backend_binary as _backend_binary
from molt.cli import backend_cache_setup as _backend_cache_setup
from molt.cli import factgraph as _factgraph
from molt.cli.backend_cache import (
    _artifact_sync_state_path,
    _backend_daemon_skip_output_sync_flags,
    _read_artifact_sync_state,
    _shared_cache_lock,
    _stage_backend_output_and_caches,
    _temporary_backend_output_path,
    _try_cached_backend_candidates,
)
from molt.cli.backend_daemon_config import _backend_daemon_enabled
from molt.cli.backend_daemon_logs import (
    _backend_daemon_log_mark,
    _backend_daemon_log_since,
)
from molt.cli.backend_daemon_startup import _backend_daemon_start_timeout
from molt.cli.backend_diagnostics import _env_requests_backend_diagnostics
from molt.cli.backend_execution import (
    _backend_bin_path,
    _backend_daemon_config_digest,
    _backend_daemon_identity_path,
    _backend_daemon_log_path,
    _backend_daemon_retryable_error,
    _backend_daemon_socket_path,
    _backend_features_for_target,
    _compile_with_backend_daemon,
    _read_backend_daemon_identity,
    _start_backend_daemon,
)
from molt.cli.build_locks import _build_lock, BuildLockAcquisitionError
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
    ENTRY_OVERRIDE_ENV,
)
from molt.cli.models import (
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    BuildProfile,
    _BackendCacheSetup,
    _BackendExecutionResult,
    _CliFailure,
    _ExternalPackageNativeArtifactPlan,
    _ModuleGraphMetadata,
    _PreparedBackendCompile,
    _PreparedBackendDispatch,
    _PreparedBackendRuntimeContext,
    _PreparedBackendSetup,
    _RuntimeArtifactState,
)
from molt.cli.output import (
    fail as _fail,
    subprocess_output_text as _subprocess_output_text,
)
from molt.cli.runtime_build import _initialize_runtime_artifact_state
from molt.cli.installed_runtime_contract import InstalledRuntimeError
from molt.cli.runtime_features import SOURCE_EXTENSION_RUNTIME_FEATURES
from molt.cli.runtime_wasm_build_policy import runtime_wasm_simd_policy
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.native_link_plan import NativeArtifactKind
from molt.cli.runtime_callable_symbols import (
    _stage_runtime_callable_symbols_for_native_codegen,
)
from molt.cli.runtime_native_build import _maybe_start_native_runtime_lib_ready_async
from molt.cli.runtime_native_codegen import (
    NativeRuntimeCodegenBinding,
    native_runtime_codegen_environment,
)
from molt.cli.runtime_wasm_pair_build import _ensure_runtime_wasm_both
from molt.target_python import TargetPythonVersion
from molt.cli.wasm_codegen_layout import (
    WasmCodegenLayout,
    prepare_wasm_codegen_layout,
)

_BACKEND_COMPILER_FINGERPRINT_ENV = "MOLT_BACKEND_COMPILER_FINGERPRINT"


def _backend_environment_with_compiler_fingerprint(
    base_env: Mapping[str, str],
    fingerprint: str | None,
) -> dict[str, str]:
    backend_env = dict(base_env)
    if fingerprint:
        backend_env[_BACKEND_COMPILER_FINGERPRINT_ENV] = fingerprint
    else:
        backend_env.pop(_BACKEND_COMPILER_FINGERPRINT_ENV, None)
    return backend_env


def _record_pipeline_stage_ms(
    stage_timings_ms: dict[str, float] | None,
    name: str,
    started_at: float,
) -> None:
    if stage_timings_ms is None:
        return
    stage_timings_ms[name] = round(
        max(0.0, (time.perf_counter() - started_at) * 1000.0),
        6,
    )


@dataclass(frozen=True)
class _BackendSelection:
    """The backend compiler one build lane dispatches."""

    cargo_profile: str
    features: tuple[str, ...]
    binary: Path


def _select_backend_binary(
    *,
    molt_root: Path,
    backend_cargo_profile: str,
    is_wasm: bool,
    is_luau_transpile: bool,
    is_rust_transpile: bool,
    codegen_backend: CodegenBackend,
) -> _BackendSelection:
    """Select the backend compiler for a build lane.

    Build setup, backend dispatch, and the ``internal-backend-build`` prewarm
    all select here, so a prewarm admits the exact feature-tagged binary a
    later build runs. The feature authority folds the ``llvm`` codegen backend
    into both the Cargo features and the binary path.
    """
    backend_features = _backend_features_for_target(
        is_wasm=is_wasm,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
        codegen_backend=codegen_backend,
    )
    return _BackendSelection(
        cargo_profile=backend_cargo_profile,
        features=backend_features,
        binary=_backend_bin_path(molt_root, backend_cargo_profile, backend_features),
    )


def _ensure_selected_backend_binary(
    selection: _BackendSelection,
    *,
    molt_root: Path,
    cargo_timeout: float | None,
    json_output: bool,
    stage_timings_ms: dict[str, float] | None = None,
) -> _backend_binary._BackendBinaryEnsureResult:
    return _backend_binary._ensure_backend_binary(
        selection.binary,
        cargo_timeout=cargo_timeout,
        json_output=json_output,
        cargo_profile=selection.cargo_profile,
        project_root=molt_root,
        backend_features=selection.features,
        stage_timings_ms=stage_timings_ms,
    )


def _prepare_backend_setup(
    *,
    is_rust_transpile: bool,
    is_luau_transpile: bool = False,
    is_wasm: bool,
    is_wasm_freestanding: bool = False,
    required_link_features: frozenset[str] = frozenset(),
    emit_mode: str,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
    json_output: bool,
    cargo_timeout: float | None,
    target: str,
    profile: BuildProfile,
    backend_cargo_profile: str,
    linked: bool,
    project_root: Path,
    cache_dir: str | None,
    output_artifact: Path,
    warnings: list[str],
    cache: bool,
    ir: Mapping[str, Any],
    entry_module: str,
    module_graph_metadata: _ModuleGraphMetadata,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
    resolved_modules: set[str] | frozenset[str] | None = None,
    codegen: CodegenSelection,
    resolved_capability_policy: ResolvedRuntimePolicy | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> tuple[_PreparedBackendSetup | None, _CliFailure | None]:
    try:
        artifact_contract = resolve_backend_artifact_contract(
            target=target, emit_mode=emit_mode, target_triple=target_triple
        )
    except ValueError as exc:
        return None, _fail(str(exc), json_output, command="build")
    if artifact_contract.native_target is not None:
        target_triple = artifact_contract.native_target.cargo_target
    extra_runtime_features: tuple[str, ...] = ()
    if native_artifact_plan.artifacts and not is_wasm:
        extra_runtime_features = SOURCE_EXTENSION_RUNTIME_FEATURES
    try:
        runtime_state = _initialize_runtime_artifact_state(
            is_rust_transpile=is_rust_transpile or is_luau_transpile,
            is_wasm=is_wasm,
            emit_mode=emit_mode,
            molt_root=molt_root,
            runtime_cargo_profile=runtime_cargo_profile,
            target_triple=target_triple,
            stdlib_profile=stdlib_profile,
            extra_runtime_features=extra_runtime_features,
        )
    except InstalledRuntimeError as exc:
        return None, _fail(str(exc), json_output, command="build")
    runtime_state.build_python_admission = build_python_admission
    runtime_callable_symbols_digest = ""
    callable_symbols_start = time.perf_counter()
    runtime_callable_symbols_digest, callable_symbols_error = (
        _stage_runtime_callable_symbols_for_native_codegen(
            runtime_state,
            target_triple=target_triple,
            json_output=json_output,
            runtime_cargo_profile=runtime_cargo_profile,
            molt_root=molt_root,
            cargo_timeout=cargo_timeout,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            stage_timings_ms=stage_timings_ms,
        )
    )
    _record_pipeline_stage_ms(
        stage_timings_ms,
        "backend_setup_runtime_callable_symbols",
        callable_symbols_start,
    )
    if callable_symbols_error is not None:
        return None, callable_symbols_error

    backend_selection = _select_backend_binary(
        molt_root=molt_root,
        backend_cargo_profile=backend_cargo_profile,
        is_wasm=is_wasm,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
        codegen_backend=codegen.backend,
    )
    backend_bin = backend_selection.binary
    backend_binary_start = time.perf_counter()
    backend_ensure_result = _ensure_selected_backend_binary(
        backend_selection,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        json_output=json_output,
        stage_timings_ms=stage_timings_ms,
    )
    _record_pipeline_stage_ms(
        stage_timings_ms,
        "backend_setup_ensure_backend_binary",
        backend_binary_start,
    )
    if not backend_ensure_result:
        return None, _fail(backend_ensure_result.message, json_output, command="build")
    if not backend_bin.exists():
        return None, _fail("Backend binary missing", json_output, command="build")
    runtime_wasm_codegen_digest = ""
    if is_wasm:
        # Runtime layout is an input to every WASM cache tier, including cache
        # hits that never dispatch the backend. Bind before admitting artifacts.
        if not _ensure_runtime_wasm_both(
            runtime_state,
            json_output=json_output,
            cargo_profile=runtime_cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
            simd_enabled=runtime_wasm_simd_policy(freestanding=is_wasm_freestanding),
            freestanding=is_wasm_freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=native_artifact_plan.runtime_export_symbols() or None,
            bind_for_codegen=True,
        ):
            return None, _fail(
                "Runtime wasm build failed", json_output, command="build"
            )
        binding = runtime_state.runtime_wasm_codegen_binding
        if binding is None:
            return None, _fail(
                "Runtime WASM code generation lacks a bound pair",
                json_output,
                command="build",
            )
        runtime_wasm_codegen_digest = binding.semantic_digest
    cache_setup_start = time.perf_counter()
    try:
        cache_setup = _backend_cache_setup._prepare_backend_cache_setup(
            backend_bin=backend_bin,
            cache_enabled=cache,
            ir=ir,
            target=target,
            artifact_contract=artifact_contract,
            profile=profile,
            runtime_cargo_profile=runtime_cargo_profile,
            backend_cargo_profile=backend_cargo_profile,
            emit_mode=emit_mode,
            is_wasm=is_wasm,
            linked=linked,
            project_root=project_root,
            cache_dir=cache_dir,
            output_artifact=output_artifact,
            warnings=warnings,
            entry_module=entry_module,
            module_graph_metadata=module_graph_metadata,
            target_python=target_python,
            stdlib_profile=stdlib_profile,
            native_artifact_plan=native_artifact_plan,
            native_runtime_codegen_binding=runtime_state.native_runtime_codegen_binding,
            runtime_wasm_codegen_digest=runtime_wasm_codegen_digest,
            backend_compiler_fingerprint=backend_ensure_result.cache_compiler_fingerprint,
            resolved_capability_policy=resolved_capability_policy,
            codegen=codegen,
            stage_timings_ms=stage_timings_ms,
        )
    except (NativeSymbolInspectionError, OSError, ValueError) as error:
        return None, _fail(str(error), json_output, command="build")
    _record_pipeline_stage_ms(
        stage_timings_ms,
        "backend_setup_prepare_cache",
        cache_setup_start,
    )
    if emit_mode != "obj" and not runtime_callable_symbols_digest:
        _maybe_start_native_runtime_lib_ready_async(
            runtime_state,
            target_triple=target_triple,
            json_output=json_output,
            runtime_cargo_profile=runtime_cargo_profile,
            molt_root=molt_root,
            cargo_timeout=cargo_timeout,
            diagnostics_enabled=False,
            phase_starts=None,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
        )
    return _PreparedBackendSetup(
        backend="llvm" if "llvm" in backend_selection.features else target,
        runtime_state=runtime_state,
        backend_bin=backend_bin,
        cache_setup=cache_setup,
        cache_hit=cache_setup.cache_hit,
        cache_hit_tier=cache_setup.cache_hit_tier,
        cache_key=cache_setup.cache_key,
        function_cache_key=cache_setup.function_cache_key,
        cache_path=cache_setup.cache_path,
        function_cache_path=cache_setup.function_cache_path,
        stdlib_object_path=cache_setup.stdlib_object_path,
        cache_candidates=list(cache_setup.cache_candidates),
        runtime_callable_symbols_digest=runtime_callable_symbols_digest,
        backend_compiler_fingerprint=backend_ensure_result.cache_compiler_fingerprint,
    ), None


def _prepare_backend_runtime_context(
    *,
    prepared_backend_setup: _PreparedBackendSetup,
    is_wasm_freestanding: bool,
    json_output: bool,
    runtime_cargo_profile: str,
    cargo_timeout: float | None,
    molt_root: Path,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    required_link_features: frozenset[str] = frozenset(),
    target_triple: str | None = None,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
) -> tuple[_PreparedBackendRuntimeContext | None, _CliFailure | None]:
    runtime_state = prepared_backend_setup.runtime_state
    native_runtime_exports = native_artifact_plan.runtime_export_symbols()

    def runtime_export_requirements(
        required_exports: set[str] | frozenset[str] | None,
    ) -> set[str] | frozenset[str] | None:
        if not native_runtime_exports:
            return required_exports
        if required_exports is None:
            return native_runtime_exports
        return frozenset(required_exports) | native_runtime_exports

    def ensure_runtime_wasm_both(
        required_exports: set[str] | frozenset[str] | None = None,
    ) -> bool:
        if (
            required_exports is None
            and runtime_state.runtime_wasm_codegen_binding is not None
        ):
            # Setup already admitted the pair before cache lookup. Dispatch
            # consumes it; final emitted imports still take fresh admission below.
            return True
        # The pair authority selects the exact staticlib+cdylib producer once,
        # validates both nested finalizers with per-member Cargo building
        # disabled, and commits one immutable generation. It fails closed rather
        # than falling back to per-member compilation or publication.
        bind_for_codegen = required_exports is None
        required_exports = runtime_export_requirements(required_exports)
        return _ensure_runtime_wasm_both(
            runtime_state,
            json_output=json_output,
            cargo_profile=runtime_cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
            simd_enabled=runtime_wasm_simd_policy(freestanding=is_wasm_freestanding),
            freestanding=is_wasm_freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
            bind_for_codegen=bind_for_codegen,
        )

    if not prepared_backend_setup.runtime_callable_symbols_digest:
        _, callable_symbols_error = _stage_runtime_callable_symbols_for_native_codegen(
            runtime_state,
            target_triple=target_triple,
            json_output=json_output,
            runtime_cargo_profile=runtime_cargo_profile,
            molt_root=molt_root,
            cargo_timeout=cargo_timeout,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            is_wasm_freestanding=is_wasm_freestanding,
        )
        if callable_symbols_error is not None:
            return None, callable_symbols_error

    return _PreparedBackendRuntimeContext(
        runtime_state=runtime_state,
        backend_bin=prepared_backend_setup.backend_bin,
        runtime_lib=runtime_state.runtime_lib,
        ensure_runtime_wasm_both=ensure_runtime_wasm_both,
        cache_setup=prepared_backend_setup.cache_setup,
        cache_hit=prepared_backend_setup.cache_hit,
        cache_hit_tier=prepared_backend_setup.cache_hit_tier,
        cache_key=prepared_backend_setup.cache_key,
        function_cache_key=prepared_backend_setup.function_cache_key,
        cache_path=prepared_backend_setup.cache_path,
        function_cache_path=prepared_backend_setup.function_cache_path,
        stdlib_object_path=prepared_backend_setup.stdlib_object_path,
        backend_compiler_fingerprint=prepared_backend_setup.backend_compiler_fingerprint,
    ), None


def _start_backend_daemon_under_lock(
    backend_bin: Path,
    daemon_socket: Path,
    *,
    cargo_profile: str,
    project_root: Path,
    config_digest: str | None,
    startup_timeout: float | None,
    json_output: bool,
    warnings: list[str],
    backend_env: Mapping[str, str] | None,
    phase: str,
) -> tuple[bool, _CliFailure | None]:
    """One startup/restart acquisition boundary; inner failures retain identity."""
    started = time.perf_counter()
    try:
        with _build_lock(
            project_root,
            f"backend-daemon.{cargo_profile}",
            default_timeout_s=startup_timeout if startup_timeout is not None else 300.0,
        ):
            ready = _start_backend_daemon(
                backend_bin,
                daemon_socket,
                cargo_profile=cargo_profile,
                project_root=project_root,
                config_digest=config_digest,
                startup_timeout=startup_timeout,
                json_output=json_output,
                warnings=warnings,
                backend_env=backend_env,
            )
        return ready, None
    except BuildLockAcquisitionError as exc:
        return False, _fail(
            f"Backend daemon lock acquisition failed: {exc}",
            json_output,
            command="build",
            data={
                "failure": {"phase": phase},
                "stage_timings_ms": {
                    phase: (time.perf_counter() - started) * 1000.0,
                },
            },
        )


def _prepare_backend_dispatch(
    *,
    is_rust_transpile: bool,
    is_luau_transpile: bool = False,
    is_wasm: bool,
    wasm_layout: WasmCodegenLayout | None,
    deterministic: bool,
    profile: BuildProfile,
    cargo_timeout: float | None,
    molt_root: Path,
    target_triple: str | None,
    backend_cargo_profile: str,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    json_output: bool,
    backend_daemon_config_digest: str | None,
    warnings: list[str],
    codegen: CodegenSelection,
    backend_bin: Path | None = None,
    backend_compiler_fingerprint: str | None = None,
    start_daemon: bool = True,
    native_runtime_codegen_binding: NativeRuntimeCodegenBinding | None = None,
) -> tuple[_PreparedBackendDispatch | None, _CliFailure | None]:
    try:
        if (
            not (is_wasm or is_rust_transpile or is_luau_transpile)
            and native_runtime_codegen_binding is None
        ):
            raise ValueError(
                "native backend dispatch requires an admitted runtime binding"
            )
        # The backend process gets its own mapping: the caller's environment
        # plus this build's codegen selection. os.environ is never written.
        backend_env = _backend_environment_with_compiler_fingerprint(
            native_runtime_codegen_environment(
                codegen.environment(os.environ), native_runtime_codegen_binding
            ),
            backend_compiler_fingerprint,
        )
    except (OSError, ValueError) as exc:
        return None, _fail(str(exc), json_output, command="build")
    if is_wasm:
        if wasm_layout is None:
            return None, _fail(
                "WASM backend dispatch requires a bound runtime layout",
                json_output,
                command="build",
            )
        backend_env.pop("MOLT_WASM_DATA_BASE", None)
        backend_env.pop("MOLT_WASM_TABLE_BASE", None)
        backend_env.pop("MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE", None)
        backend_env.pop("MOLT_WASM_RELOCATABLE", None)
        backend_env.update(wasm_layout.backend_environment())
    # Single source of truth (shared with setup, the backend prewarm, and the
    # cache-key binary-identity resolver): the 'llvm' feature is folded in by
    # the helper for the llvm codegen backend so the backend binary is compiled
    # with inkwell/LLVM support and the feature-tagged path/identity stays
    # consistent.
    backend_features: tuple[str, ...] = _backend_features_for_target(
        is_wasm=is_wasm,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
        codegen_backend=codegen.backend,
    )
    if deterministic or profile == "release":
        backend_env.setdefault("SOURCE_DATE_EPOCH", "315532800")
    # Auto-set Cranelift optimization level based on profile for size-critical
    # builds.  speed_and_size balances code quality with binary density.
    if profile in ("release-size", "wasm-release"):
        backend_env.setdefault("MOLT_BACKEND_OPT_LEVEL", "speed_and_size")
    reloc_requested = is_wasm and wasm_layout is not None and wasm_layout.relocatable

    if backend_bin is None:
        backend_selection = _select_backend_binary(
            molt_root=molt_root,
            backend_cargo_profile=backend_cargo_profile,
            is_wasm=is_wasm,
            is_luau_transpile=is_luau_transpile,
            is_rust_transpile=is_rust_transpile,
            codegen_backend=codegen.backend,
        )
        backend_bin = backend_selection.binary
        backend_ensure_result = _ensure_selected_backend_binary(
            backend_selection,
            molt_root=molt_root,
            cargo_timeout=cargo_timeout,
            json_output=json_output,
        )
        if not backend_ensure_result:
            return None, _fail(
                backend_ensure_result.message, json_output, command="build"
            )
        backend_env = _backend_environment_with_compiler_fingerprint(
            backend_env, backend_ensure_result.cache_compiler_fingerprint
        )
    if not backend_bin.exists():
        return None, _fail("Backend binary missing", json_output, command="build")

    daemon_socket: Path | None = None
    daemon_ready = False
    daemon_config_digest = backend_daemon_config_digest
    if (
        start_daemon
        and not is_rust_transpile
        and not is_luau_transpile
        and _backend_daemon_enabled()
    ):
        try:
            daemon_config_digest = _backend_daemon_config_digest(
                molt_root,
                backend_cargo_profile,
                env=backend_env,
                backend_bin=backend_bin,
                target_triple=target_triple,
                backend_features=backend_features,
            )
        except CompilerIdentityError as exc:
            return None, _fail(str(exc), json_output, command="build")
        if diagnostics_enabled and "backend_daemon_setup" not in phase_starts:
            phase_starts["backend_daemon_setup"] = time.perf_counter()
        daemon_socket = _backend_daemon_socket_path(
            molt_root,
            backend_cargo_profile,
            config_digest=daemon_config_digest,
        )
        startup_timeout = _backend_daemon_start_timeout()
        daemon_ready, daemon_lock_failure = _start_backend_daemon_under_lock(
            backend_bin,
            daemon_socket,
            cargo_profile=backend_cargo_profile,
            project_root=molt_root,
            config_digest=daemon_config_digest,
            startup_timeout=startup_timeout,
            json_output=json_output,
            warnings=warnings,
            backend_env=backend_env,
            phase="backend_daemon_start_lock",
        )
        if daemon_lock_failure is not None:
            return None, daemon_lock_failure

    return _PreparedBackendDispatch(
        backend_env=backend_env,
        reloc_requested=reloc_requested,
        backend_bin=backend_bin,
        daemon_socket=daemon_socket,
        daemon_ready=daemon_ready,
        backend_daemon_config_digest=daemon_config_digest,
    ), None


def _execute_backend_compile(
    *,
    cache: bool,
    cache_path: Path | None,
    function_cache_path: Path | None,
    artifacts_root: Path,
    is_rust_transpile: bool,
    is_luau_transpile: bool = False,
    is_wasm: bool,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    daemon_ready: bool,
    daemon_socket: Path | None,
    project_root: Path,
    output_artifact: Path,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_setup: _BackendCacheSetup,
    backend_daemon_config_digest: str | None,
    entry_module: str,
    ir: Mapping[str, Any],
    json_output: bool,
    warnings: list[str],
    verbose: bool,
    backend_bin: Path,
    backend_env: Mapping[str, str],
    backend_timeout: float | None,
    molt_root: Path,
    backend_cargo_profile: str,
    _ensure_backend_ir_file_path: Callable[[], Path],
    cache_hit: bool,
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_health: dict[str, Any] | None,
    codegen: CodegenSelection,
    native_runtime_codegen_binding: NativeRuntimeCodegenBinding | None = None,
) -> tuple[_BackendExecutionResult | None, _CliFailure | None]:
    target_triple = cache_setup.artifact_contract.target_triple
    # A daemon resets its request controls from the backend environment
    # catalog for every request, so each request carries this build's
    # selection; the daemon's startup environment cannot carry it.
    daemon_request_environment = codegen.environment(os.environ)
    try:
        if (
            cache_setup.artifact_contract.is_native
            and native_runtime_codegen_binding is None
        ):
            raise ValueError(
                "native backend execution requires an admitted runtime binding"
            )
        backend_env = native_runtime_codegen_environment(
            backend_env, native_runtime_codegen_binding
        )
    except (OSError, ValueError) as exc:
        return None, _fail(str(exc), json_output, command="build")
    backend_output_ctx: ContextManager[Path]
    # One-shot backend subprocess compilation should always write to a fresh
    # artifact path and stage atomically into cache/output afterward. Writing
    # directly into the cache artifact path couples codegen to cache lifecycle
    # and breaks first-build correctness when a toolchain rebuild invalidates
    # cache directories in the same command.
    backend_output_ctx = _temporary_backend_output_path(
        artifacts_root,
        artifact_contract=cache_setup.artifact_contract,
    )
    native_output_kind = (
        cache_setup.artifact_contract.native_kind or NativeArtifactKind.OBJECT
    )
    with backend_output_ctx as backend_output:
        daemon_identity_path = (
            _backend_daemon_identity_path(
                molt_root,
                backend_cargo_profile,
                config_digest=backend_daemon_config_digest,
            )
            if daemon_socket is not None
            else None
        )
        daemon_identity = (
            _read_backend_daemon_identity(daemon_identity_path)
            if daemon_identity_path is not None
            else None
        )
        backend_compiled = False
        backend_output_written = True
        backend_output_exists = False
        daemon_error: str | None = None
        output_sync_state_path: Path | None = None
        output_sync_state: dict[str, Any] | None = None
        output_artifact_stat: os.stat_result | None = None
        skip_module_output_if_synced = False
        skip_function_output_if_synced = False
        wasm_link = False
        wasm_data_base: int | None = None
        wasm_table_base: int | None = None
        wasm_split_runtime_app_table_base: int | None = None
        if is_wasm:
            wasm_link = backend_env.get("MOLT_WASM_RELOCATABLE") == "1"
            raw_data_base = backend_env.get("MOLT_WASM_DATA_BASE")
            raw_table_base = backend_env.get("MOLT_WASM_TABLE_BASE")
            raw_split_runtime_app_table_base = backend_env.get(
                "MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE"
            )
            try:
                wasm_data_base = (
                    int(raw_data_base) if raw_data_base is not None else None
                )
            except ValueError:
                wasm_data_base = None
            try:
                wasm_table_base = (
                    int(raw_table_base) if raw_table_base is not None else None
                )
            except ValueError:
                wasm_table_base = None
            try:
                wasm_split_runtime_app_table_base = (
                    int(raw_split_runtime_app_table_base)
                    if raw_split_runtime_app_table_base is not None
                    else None
                )
            except ValueError:
                wasm_split_runtime_app_table_base = None
        if daemon_ready and daemon_socket is not None:
            output_sync_state_path = _artifact_sync_state_path(
                project_root, output_artifact
            )
            output_sync_state = _read_artifact_sync_state(output_sync_state_path)
            try:
                output_artifact_stat = output_artifact.stat()
            except OSError:
                output_artifact_stat = None
            (
                skip_module_output_if_synced,
                skip_function_output_if_synced,
            ) = _backend_daemon_skip_output_sync_flags(
                project_root,
                output_artifact,
                artifact_contract=cache_setup.artifact_contract,
                cache_key=cache_key if cache else None,
                function_cache_key=(
                    function_cache_key
                    if cache and function_cache_key != cache_key
                    else None
                ),
                stdlib_object_path=cache_setup.stdlib_object_path,
                stdlib_object_cache_key=cache_setup.stdlib_object_cache_key,
                stdlib_object_manifest=cache_setup.stdlib_object_manifest,
                stdlib_module_symbols=cache_setup.stdlib_module_symbols,
                state_path=output_sync_state_path,
                state=output_sync_state,
                output_stat=output_artifact_stat,
            )
            if diagnostics_enabled and "backend_daemon_compile" not in phase_starts:
                phase_starts["backend_daemon_compile"] = time.perf_counter()
            # Keep probe/full request selection centralized in
            # _compile_with_backend_daemon(). Eagerly encoding the full
            # request here defeats the daemon's probe-only warm-cache path.
            daemon_log_path: Path | None = None
            daemon_log_offset: int | None = None
            # Stream the daemon log delta back to the user when they have
            # explicitly asked for backend diagnostics (--verbose, or any of
            # the diagnostic env knobs like TIR_OPT_STATS=1). Without the
            # env-knob branch the user can set the knob, run a build, and
            # see no output — the daemon writes diagnostics to its log
            # file rather than to the parent's stderr, so the request-scoped
            # delta is the only path that surfaces them.
            forward_daemon_log = verbose or _env_requests_backend_diagnostics(
                os.environ
            )
            if forward_daemon_log and not json_output:
                daemon_log_path = _backend_daemon_log_path(
                    molt_root,
                    backend_cargo_profile,
                    config_digest=backend_daemon_config_digest,
                )
                daemon_log_offset = _backend_daemon_log_mark(daemon_log_path)
            daemon_compile = _compile_with_backend_daemon(
                daemon_socket,
                native_runtime_codegen_binding=native_runtime_codegen_binding,
                project_root=molt_root,
                ir=ir,
                backend_output=backend_output,
                artifact_contract=cache_setup.artifact_contract,
                wasm_link=wasm_link,
                wasm_data_base=wasm_data_base,
                wasm_table_base=wasm_table_base,
                wasm_split_runtime_app_table_base=wasm_split_runtime_app_table_base,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                config_digest=backend_daemon_config_digest,
                skip_module_output_if_synced=skip_module_output_if_synced,
                skip_function_output_if_synced=skip_function_output_if_synced,
                entry_module=entry_module,
                stdlib_object_path=cache_setup.stdlib_object_path,
                stdlib_object_cache_key=cache_setup.stdlib_object_cache_key,
                stdlib_object_manifest=cache_setup.stdlib_object_manifest,
                stdlib_module_symbols_json=cache_setup.stdlib_module_symbols_json,
                stdlib_module_symbols=cache_setup.stdlib_module_symbols,
                timeout=None,
                daemon_identity=daemon_identity,
                request_environment=daemon_request_environment,
            )
            backend_compiled = daemon_compile.ok
            backend_output_written = daemon_compile.output_written
            daemon_error = daemon_compile.error
            backend_output_exists = daemon_compile.output_exists
            # Show only the daemon output produced by this request. Printing
            # a rolling tail replays previous builds and makes warm user-code
            # compiles look like they recompiled stdlib batches.
            if daemon_log_path is not None and daemon_log_offset is not None:
                daemon_log_delta = _backend_daemon_log_since(
                    daemon_log_path, daemon_log_offset
                )
                if daemon_log_delta:
                    print(daemon_log_delta, file=sys.stderr)
            if daemon_compile.cached is not None:
                backend_daemon_cached = daemon_compile.cached
            if daemon_compile.cache_tier is not None:
                backend_daemon_cache_tier = daemon_compile.cache_tier
            daemon_health = daemon_compile.health
            if daemon_health is not None:
                backend_daemon_health = daemon_health
            if (
                not backend_compiled
                and not daemon_compile.full_request_sent
                and _backend_daemon_retryable_error(daemon_error)
            ):
                if diagnostics_enabled and "backend_daemon_restart" not in phase_starts:
                    phase_starts["backend_daemon_restart"] = time.perf_counter()
                restart_timeout = _backend_daemon_start_timeout()
                daemon_ready, daemon_lock_failure = _start_backend_daemon_under_lock(
                    backend_bin,
                    daemon_socket,
                    cargo_profile=backend_cargo_profile,
                    project_root=molt_root,
                    config_digest=backend_daemon_config_digest,
                    startup_timeout=restart_timeout,
                    json_output=json_output,
                    warnings=warnings,
                    backend_env=backend_env,
                    phase="backend_daemon_restart_lock",
                )
                if daemon_lock_failure is not None:
                    return None, daemon_lock_failure

                if daemon_ready:
                    daemon_compile = _compile_with_backend_daemon(
                        daemon_socket,
                        native_runtime_codegen_binding=native_runtime_codegen_binding,
                        project_root=molt_root,
                        ir=ir,
                        backend_output=backend_output,
                        artifact_contract=cache_setup.artifact_contract,
                        wasm_link=wasm_link,
                        wasm_data_base=wasm_data_base,
                        wasm_table_base=wasm_table_base,
                        wasm_split_runtime_app_table_base=wasm_split_runtime_app_table_base,
                        cache_key=cache_key,
                        function_cache_key=function_cache_key,
                        config_digest=backend_daemon_config_digest,
                        skip_module_output_if_synced=skip_module_output_if_synced,
                        skip_function_output_if_synced=skip_function_output_if_synced,
                        entry_module=entry_module,
                        stdlib_object_path=cache_setup.stdlib_object_path,
                        stdlib_object_cache_key=cache_setup.stdlib_object_cache_key,
                        stdlib_object_manifest=cache_setup.stdlib_object_manifest,
                        stdlib_module_symbols_json=cache_setup.stdlib_module_symbols_json,
                        stdlib_module_symbols=cache_setup.stdlib_module_symbols,
                        timeout=None,
                        request_environment=daemon_request_environment,
                        daemon_identity=(
                            _read_backend_daemon_identity(daemon_identity_path)
                            if daemon_identity_path is not None
                            else None
                        ),
                    )
                    backend_compiled = daemon_compile.ok
                    backend_output_written = daemon_compile.output_written
                    daemon_error = daemon_compile.error
                    backend_output_exists = daemon_compile.output_exists
                    if daemon_compile.cached is not None:
                        backend_daemon_cached = daemon_compile.cached
                    if daemon_compile.cache_tier is not None:
                        backend_daemon_cache_tier = daemon_compile.cache_tier
                    daemon_health = daemon_compile.health
                    if daemon_health is not None:
                        backend_daemon_health = daemon_health
            if not backend_compiled:
                detail = (
                    daemon_error
                    or "backend daemon returned no successful compile result"
                )
                return None, _fail(
                    f"Backend daemon compile failed: {detail}",
                    json_output,
                    command="build",
                )
        if not backend_output_written:
            if not (skip_module_output_if_synced or skip_function_output_if_synced):
                return None, _fail(
                    "Backend daemon skipped output write without a synced-artifact contract",
                    json_output,
                    command="build",
                )
            if not output_artifact.exists():
                return None, _fail(
                    "Backend output missing", json_output, command="build"
                )
        if not backend_compiled:
            if diagnostics_enabled and "backend_subprocess_compile" not in phase_starts:
                phase_starts["backend_subprocess_compile"] = time.perf_counter()
            _is_transpile = is_rust_transpile or is_luau_transpile
            if not is_wasm and not _is_transpile and backend_env is not None:
                # Always scrub the partition contract before setting the
                # current build's values so stale ambient state cannot leak
                # into a later native compile.
                backend_env.pop("MOLT_STDLIB_OBJ", None)
                backend_env.pop("MOLT_STDLIB_CACHE_KEY", None)
                backend_env.pop("MOLT_STDLIB_CACHE_MANIFEST", None)
                backend_env.pop("MOLT_STDLIB_MODULE_SYMBOLS", None)
            stdlib_obj_path = cache_setup.stdlib_object_path
            if not is_wasm and not _is_transpile and stdlib_obj_path is not None:
                stdlib_obj_path.parent.mkdir(parents=True, exist_ok=True)
                if backend_env is not None:
                    backend_env["MOLT_STDLIB_OBJ"] = str(stdlib_obj_path)
                    if cache_setup.stdlib_object_cache_key:
                        backend_env["MOLT_STDLIB_CACHE_KEY"] = (
                            cache_setup.stdlib_object_cache_key
                        )
                    else:
                        backend_env.pop("MOLT_STDLIB_CACHE_KEY", None)
                    if cache_setup.stdlib_object_manifest:
                        backend_env["MOLT_STDLIB_CACHE_MANIFEST"] = (
                            cache_setup.stdlib_object_manifest
                        )
                    else:
                        backend_env.pop("MOLT_STDLIB_CACHE_MANIFEST", None)
                    if cache_setup.stdlib_module_symbols_json:
                        backend_env["MOLT_STDLIB_MODULE_SYMBOLS"] = (
                            cache_setup.stdlib_module_symbols_json
                        )
                    else:
                        backend_env.pop("MOLT_STDLIB_MODULE_SYMBOLS", None)
            if not is_wasm and not _is_transpile and backend_env is not None:
                backend_env[ENTRY_OVERRIDE_ENV] = entry_module
                # Limit rayon threads to a fraction of available cores.
                # The batched compilation pipeline may run multiple backend
                # processes; each process's thread pool must share the CPU
                # fairly. Default: half of available cores, minimum 2.
                _default_threads = str(max(2, (os.cpu_count() or 4) // 2))
                backend_env.setdefault("RAYON_NUM_THREADS", _default_threads)
            cmd = _factgraph.backend_command_prefix(
                backend_bin=backend_bin,
                is_luau_transpile=is_luau_transpile,
                is_rust_transpile=is_rust_transpile,
                is_wasm=is_wasm,
                native_output_kind=native_output_kind,
                target_triple=target_triple,
                wasm_link=wasm_link,
                wasm_data_base=wasm_data_base,
                wasm_table_base=wasm_table_base,
                wasm_split_runtime_app_table_base=wasm_split_runtime_app_table_base,
            )
            cmd_with_output = cmd + ["--output", str(backend_output)]
            # Ensure the output directory exists — --rebuild may have
            # cleared the cache tree, and the backend's own
            # ensure_output_parent_dir may race with ld -r timing.
            backend_output.parent.mkdir(parents=True, exist_ok=True)
            _entry_name = entry_module or "program"
            try:
                ir_file_path = _ensure_backend_ir_file_path()
                cmd_with_output.extend(["--ir-file", str(ir_file_path)])
                backend_process = _run_subprocess_captured_to_tempfiles(
                    cmd_with_output,
                    env=backend_env,
                    timeout=backend_timeout,
                    progress_label=None
                    if json_output
                    else (
                        f"Compiling {_entry_name} ({target_triple or 'native'}, {backend_cargo_profile} compiler)"
                    ),
                )
            except subprocess.TimeoutExpired:
                return None, _fail(
                    "Backend compilation timed out",
                    json_output,
                    command="build",
                )
            except OSError as exc:
                return None, _fail(
                    f"Backend IR lease write failed: {exc}",
                    json_output,
                    command="build",
                )
            # Emit each captured stream once. Failed commands expose both
            # streams even without verbosity; JSON diagnostics stay structured.
            backend_stderr = _subprocess_output_text(backend_process.stderr)
            backend_stdout = _subprocess_output_text(backend_process.stdout)
            if not json_output and (verbose or backend_process.returncode != 0):
                if backend_stderr:
                    print(backend_stderr, end="", file=sys.stderr)
                if backend_stdout:
                    print(backend_stdout, end="")
            if backend_process.returncode != 0:
                # Build a more informative error message
                _fail_detail_parts = ["Backend compilation failed"]
                _fail_detail_parts.append(f" (exit code {backend_process.returncode})")
                if not backend_stderr and not backend_stdout:
                    _fail_detail_parts.append(
                        ".\nNo output from the backend. "
                        "Run with --verbose for more details."
                    )
                elif json_output:
                    # For JSON output, include stderr in the message since
                    # we didn't print it above.
                    _stderr_tail = (backend_stderr or "").strip()
                    if _stderr_tail:
                        # Include the last few lines of stderr for context
                        _stderr_lines = _stderr_tail.splitlines()
                        if len(_stderr_lines) > 10:
                            _stderr_tail = "\n".join(
                                ["...(truncated)"] + _stderr_lines[-10:]
                            )
                        _fail_detail_parts.append(f":\n{_stderr_tail}")
                else:
                    _fail_detail_parts.append(".")
                return None, _fail(
                    "".join(_fail_detail_parts),
                    json_output,
                    backend_process.returncode or 1,
                    command="build",
                )
            backend_output_written = True
        if backend_output_written and not (
            daemon_ready and backend_compiled and backend_output_exists
        ):
            if not backend_output.exists():
                return None, _fail(
                    "Backend output missing", json_output, command="build"
                )
        if backend_output_written:
            if diagnostics_enabled and "backend_artifact_stage" not in phase_starts:
                phase_starts["backend_artifact_stage"] = time.perf_counter()
            if cache and cache_path is not None:
                if diagnostics_enabled and "backend_cache_write" not in phase_starts:
                    phase_starts["backend_cache_write"] = time.perf_counter()
            stage_error = _stage_backend_output_and_caches(
                project_root,
                backend_output,
                output_artifact,
                cache_path=cache_path if cache else None,
                cache_key=cache_key if cache else None,
                stdlib_object_cache_key=(
                    cache_setup.stdlib_object_cache_key if cache else None
                ),
                function_cache_path=function_cache_path if cache else None,
                warnings=warnings,
                artifact_contract=cache_setup.artifact_contract,
                output_already_synced=(
                    skip_module_output_if_synced
                    if daemon_ready and cache and cache_key
                    else None
                ),
                state_path=output_sync_state_path,
                state=output_sync_state,
                output_stat=output_artifact_stat,
            )
            if stage_error is not None:
                return None, _fail(stage_error, json_output, command="build")
    return _BackendExecutionResult(
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_health=backend_daemon_health,
    ), None


def _prepare_backend_compile(
    *,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    cache_report: bool,
    verbose: bool,
    json_output: bool,
    cache_setup: _BackendCacheSetup,
    cache_hit: bool,
    cache_hit_tier: str | None,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_path: Path | None,
    function_cache_path: Path | None,
    project_root: Path,
    warnings: list[str],
    is_rust_transpile: bool,
    is_luau_transpile: bool = False,
    is_wasm: bool,
    split_runtime: bool = False,
    output_artifact: Path,
    linked: bool,
    deterministic: bool,
    profile: BuildProfile,
    runtime_state: _RuntimeArtifactState,
    cargo_timeout: float | None,
    molt_root: Path,
    backend_cargo_profile: str,
    backend_timeout: float | None,
    backend_daemon_config_digest: str | None,
    entry_module: str,
    artifacts_root: Path,
    ir: Mapping[str, Any],
    _ensure_backend_ir_file_path: Callable[[], Path],
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_health: dict[str, Any] | None,
    codegen: CodegenSelection,
    backend_bin: Path | None = None,
    backend_compiler_fingerprint: str | None = None,
) -> tuple[_PreparedBackendCompile | None, _CliFailure | None]:
    if diagnostics_enabled:
        phase_starts["cache_lookup"] = time.perf_counter()
    cache_enabled = cache_setup.cache_enabled
    target_triple = cache_setup.artifact_contract.target_triple
    wasm_layout = None
    if is_wasm:
        try:
            wasm_layout = prepare_wasm_codegen_layout(
                runtime_state.runtime_wasm_codegen_binding,
                linked=linked,
                split_runtime=split_runtime,
            )
        except (OSError, ValueError) as exc:
            return None, _fail(str(exc), json_output, command="build")

    if (verbose or cache_report) and not json_output:
        if not cache_enabled:
            print("Cache: disabled")
        elif cache_key:
            cache_state = "hit" if cache_hit else "miss"
            cache_detail = f" ({cache_key})" if cache_key else ""
            if cache_hit and cache_hit_tier:
                cache_detail = f"{cache_detail} [{cache_hit_tier}]"
            print(f"Cache: {cache_state}{cache_detail}")

    compile_lock = (
        _shared_cache_lock(
            f"compile.{cache_key}",
            cache_root=cache_path.parent if cache_path is not None else None,
        )
        if cache_enabled and cache_key is not None
        else nullcontext()
    )
    with compile_lock:
        if not cache_hit and cache_enabled:
            cache_hit, cache_hit_tier = _try_cached_backend_candidates(
                project_root=project_root,
                cache_candidates=cache_setup.cache_candidates,
                output_artifact=output_artifact,
                artifact_contract=cache_setup.artifact_contract,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                stdlib_object_path=cache_setup.stdlib_object_path,
                stdlib_object_cache_key=cache_setup.stdlib_object_cache_key,
                stdlib_object_manifest=cache_setup.stdlib_object_manifest,
                stdlib_module_symbols=cache_setup.stdlib_module_symbols,
                warnings=warnings,
                stdlib_contract_validation_token=(
                    cache_setup.stdlib_contract_validation_token
                ),
            )

        if not cache_hit:
            if diagnostics_enabled:
                now = time.perf_counter()
                if "backend_codegen" not in phase_starts:
                    phase_starts["backend_codegen"] = now
                if "backend_prepare" not in phase_starts:
                    phase_starts["backend_prepare"] = now
            prepared_backend_dispatch, prepared_backend_dispatch_error = (
                _prepare_backend_dispatch(
                    native_runtime_codegen_binding=runtime_state.native_runtime_codegen_binding,
                    is_rust_transpile=is_rust_transpile,
                    is_luau_transpile=is_luau_transpile,
                    is_wasm=is_wasm,
                    wasm_layout=wasm_layout,
                    deterministic=deterministic,
                    profile=profile,
                    cargo_timeout=cargo_timeout,
                    molt_root=molt_root,
                    target_triple=target_triple,
                    backend_cargo_profile=backend_cargo_profile,
                    diagnostics_enabled=diagnostics_enabled,
                    phase_starts=phase_starts,
                    json_output=json_output,
                    backend_daemon_config_digest=backend_daemon_config_digest,
                    warnings=warnings,
                    backend_bin=backend_bin,
                    backend_compiler_fingerprint=backend_compiler_fingerprint,
                    codegen=codegen,
                )
            )
            if prepared_backend_dispatch_error is not None:
                return None, prepared_backend_dispatch_error
            assert prepared_backend_dispatch is not None
            if diagnostics_enabled and "backend_dispatch" not in phase_starts:
                phase_starts["backend_dispatch"] = time.perf_counter()
            backend_execution_result, backend_execution_error = (
                _execute_backend_compile(
                    native_runtime_codegen_binding=runtime_state.native_runtime_codegen_binding,
                    cache=cache_enabled,
                    cache_path=cache_path,
                    function_cache_path=function_cache_path,
                    artifacts_root=artifacts_root,
                    is_rust_transpile=is_rust_transpile,
                    is_luau_transpile=is_luau_transpile,
                    is_wasm=is_wasm,
                    diagnostics_enabled=diagnostics_enabled,
                    phase_starts=phase_starts,
                    daemon_ready=prepared_backend_dispatch.daemon_ready,
                    daemon_socket=prepared_backend_dispatch.daemon_socket,
                    project_root=project_root,
                    output_artifact=output_artifact,
                    cache_key=cache_key,
                    function_cache_key=function_cache_key,
                    cache_setup=cache_setup,
                    backend_daemon_config_digest=(
                        prepared_backend_dispatch.backend_daemon_config_digest
                    ),
                    entry_module=entry_module,
                    ir=ir,
                    json_output=json_output,
                    warnings=warnings,
                    verbose=verbose,
                    backend_bin=prepared_backend_dispatch.backend_bin,
                    backend_env=prepared_backend_dispatch.backend_env,
                    backend_timeout=backend_timeout,
                    molt_root=molt_root,
                    backend_cargo_profile=backend_cargo_profile,
                    _ensure_backend_ir_file_path=_ensure_backend_ir_file_path,
                    cache_hit=cache_hit,
                    backend_daemon_cached=backend_daemon_cached,
                    backend_daemon_cache_tier=backend_daemon_cache_tier,
                    backend_daemon_health=backend_daemon_health,
                    codegen=codegen,
                )
            )
            if backend_execution_error is not None:
                return None, backend_execution_error
            assert backend_execution_result is not None
            backend_daemon_cached = backend_execution_result.backend_daemon_cached
            backend_daemon_cache_tier = (
                backend_execution_result.backend_daemon_cache_tier
            )
            backend_daemon_health = backend_execution_result.backend_daemon_health
            backend_daemon_config_digest = (
                prepared_backend_dispatch.backend_daemon_config_digest
            )

    return _PreparedBackendCompile(
        cache_enabled=cache_enabled,
        cache_hit=cache_hit,
        cache_hit_tier=cache_hit_tier,
        wasm_table_base=wasm_layout.table_base if wasm_layout is not None else None,
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_health=backend_daemon_health,
        backend_daemon_config_digest=backend_daemon_config_digest,
    ), None
