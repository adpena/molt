from __future__ import annotations

import os
import subprocess
import sys
import time
from contextlib import nullcontext
from pathlib import Path
from typing import Any, Callable, ContextManager, Mapping, Sequence

from molt._wasm_runtime_exports import wasm_runtime_required_import_names
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
from molt.cli.build_locks import _build_lock
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.config_resolution import ENTRY_OVERRIDE_ENV
from molt.cli.models import (
    BuildProfile,
    _BackendCacheSetup,
    _BackendExecutionResult,
    _CliFailure,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
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
from molt.cli.runtime_build import (
    _ensure_runtime_wasm_artifact,
    _initialize_runtime_artifact_state,
    _maybe_start_native_runtime_lib_ready_async,
)
from molt.cli.runtime_intrinsic_symbols import (
    _stage_runtime_intrinsic_symbols_for_native_codegen,
)
from molt.cli.target_python import TargetPythonVersion
from molt.wasm_artifact import (
    _read_wasm_data_end,
    _read_wasm_memory_min_bytes,
    _read_wasm_table_min,
)


def _prepare_backend_setup(
    *,
    is_rust_transpile: bool,
    is_luau_transpile: bool = False,
    is_wasm: bool,
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
    stdlib_profile: str | None = "micro",
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
    resolved_modules: set[str] | frozenset[str] | None = None,
    capabilities_list: Sequence[str] | None = None,
    capability_profiles: Sequence[str] | None = None,
    manifest_env_vars: Mapping[str, str] | None = None,
    capability_config_digest: str | None = None,
) -> tuple[_PreparedBackendSetup | None, _CliFailure | None]:
    runtime_state = _initialize_runtime_artifact_state(
        is_rust_transpile=is_rust_transpile or is_luau_transpile,
        is_wasm=is_wasm,
        emit_mode=emit_mode,
        molt_root=molt_root,
        runtime_cargo_profile=runtime_cargo_profile,
        target_triple=target_triple,
        stdlib_profile=stdlib_profile,
    )
    runtime_intrinsic_symbols_digest = ""
    runtime_intrinsic_symbols_digest, intrinsic_symbols_error = (
        _stage_runtime_intrinsic_symbols_for_native_codegen(
            runtime_state,
            target_triple=target_triple,
            json_output=json_output,
            runtime_cargo_profile=runtime_cargo_profile,
            molt_root=molt_root,
            cargo_timeout=cargo_timeout,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
        )
    )
    if intrinsic_symbols_error is not None:
        return None, intrinsic_symbols_error
    cache_setup = _backend_cache_setup._prepare_backend_cache_setup(
        cache_enabled=cache,
        ir=ir,
        target=target,
        target_triple=target_triple,
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
        runtime_intrinsic_symbols_digest=runtime_intrinsic_symbols_digest,
        capabilities_list=capabilities_list,
        capability_profiles=capability_profiles,
        manifest_env_vars=manifest_env_vars,
        capability_config_digest=capability_config_digest,
    )
    if emit_mode != "obj" and not runtime_intrinsic_symbols_digest:
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
        runtime_state=runtime_state,
        cache_setup=cache_setup,
        cache_hit=cache_setup.cache_hit,
        cache_hit_tier=cache_setup.cache_hit_tier,
        cache_key=cache_setup.cache_key,
        function_cache_key=cache_setup.function_cache_key,
        cache_path=cache_setup.cache_path,
        function_cache_path=cache_setup.function_cache_path,
        stdlib_object_path=cache_setup.stdlib_object_path,
        cache_candidates=list(cache_setup.cache_candidates),
    ), None


def _prepare_backend_runtime_context(
    *,
    prepared_backend_setup: _PreparedBackendSetup,
    is_wasm_freestanding: bool,
    json_output: bool,
    runtime_cargo_profile: str,
    cargo_timeout: float | None,
    molt_root: Path,
    stdlib_profile: str | None = "micro",
    resolved_modules: set[str] | frozenset[str] | None = None,
    target_triple: str | None = None,
) -> tuple[_PreparedBackendRuntimeContext | None, _CliFailure | None]:
    runtime_state = prepared_backend_setup.runtime_state

    def ensure_runtime_wasm_shared(
        required_exports: set[str] | frozenset[str] | None = None,
    ) -> bool:
        return _ensure_runtime_wasm_artifact(
            runtime_state,
            reloc=False,
            json_output=json_output,
            cargo_profile=runtime_cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
            simd_enabled=not is_wasm_freestanding,
            freestanding=is_wasm_freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_exports=required_exports,
        )

    def ensure_runtime_wasm_reloc(
        required_exports: set[str] | frozenset[str] | None = None,
    ) -> bool:
        return _ensure_runtime_wasm_artifact(
            runtime_state,
            reloc=True,
            json_output=json_output,
            cargo_profile=runtime_cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
            simd_enabled=not is_wasm_freestanding,
            freestanding=is_wasm_freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_exports=required_exports,
        )

    _, intrinsic_symbols_error = _stage_runtime_intrinsic_symbols_for_native_codegen(
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
    if intrinsic_symbols_error is not None:
        return None, intrinsic_symbols_error

    return _PreparedBackendRuntimeContext(
        runtime_state=runtime_state,
        runtime_lib=runtime_state.runtime_lib,
        runtime_wasm=runtime_state.runtime_wasm,
        runtime_reloc_wasm=runtime_state.runtime_reloc_wasm,
        ensure_runtime_wasm_shared=ensure_runtime_wasm_shared,
        ensure_runtime_wasm_reloc=ensure_runtime_wasm_reloc,
        cache_setup=prepared_backend_setup.cache_setup,
        cache_hit=prepared_backend_setup.cache_hit,
        cache_hit_tier=prepared_backend_setup.cache_hit_tier,
        cache_key=prepared_backend_setup.cache_key,
        function_cache_key=prepared_backend_setup.function_cache_key,
        cache_path=prepared_backend_setup.cache_path,
        function_cache_path=prepared_backend_setup.function_cache_path,
        stdlib_object_path=prepared_backend_setup.stdlib_object_path,
    ), None


def _prepare_backend_dispatch(
    *,
    is_rust_transpile: bool,
    is_luau_transpile: bool = False,
    is_wasm: bool,
    split_runtime: bool = False,
    linked: bool,
    deterministic: bool,
    profile: BuildProfile,
    runtime_state: _RuntimeArtifactState,
    runtime_cargo_profile: str,
    cargo_timeout: float | None,
    molt_root: Path,
    target_triple: str | None,
    backend_cargo_profile: str,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    json_output: bool,
    backend_daemon_config_digest: str | None,
    ensure_runtime_wasm_shared: Callable[[set[str] | frozenset[str] | None], bool],
    ensure_runtime_wasm_reloc: Callable[[set[str] | frozenset[str] | None], bool],
    resolved_modules: set[str] | frozenset[str] | None,
    warnings: list[str],
    start_daemon: bool = True,
) -> tuple[_PreparedBackendDispatch | None, _CliFailure | None]:
    backend_env = os.environ.copy() if is_wasm else None
    if backend_env is not None:
        backend_env.pop("MOLT_WASM_DATA_BASE", None)
        backend_env.pop("MOLT_WASM_TABLE_BASE", None)
        backend_env.pop("MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN", None)
    # Single source of truth (shared with the cache-key binary-identity
    # resolver): the 'llvm' feature is folded in by the helper when
    # MOLT_BACKEND == "llvm" so the backend binary is compiled with inkwell/LLVM
    # support and the feature-tagged path/identity stays consistent.
    backend_features: tuple[str, ...] = _backend_features_for_target(
        is_wasm=is_wasm,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
    )
    if deterministic or profile == "release":
        os.environ.setdefault("SOURCE_DATE_EPOCH", "315532800")
    # Auto-set Cranelift optimization level based on profile for size-critical
    # builds.  speed_and_size balances code quality with binary density.
    if profile in ("release-size", "wasm-release"):
        os.environ.setdefault("MOLT_BACKEND_OPT_LEVEL", "speed_and_size")
    reloc_requested = is_wasm and (linked or os.environ.get("MOLT_WASM_LINK") == "1")
    runtime_wasm = runtime_state.runtime_wasm
    runtime_reloc_wasm = runtime_state.runtime_reloc_wasm
    if is_wasm and backend_env is not None:
        extra_required_imports = wasm_runtime_required_import_names(resolved_modules)
        if extra_required_imports:
            backend_env["MOLT_WASM_EXTRA_REQUIRED_IMPORTS"] = ",".join(
                extra_required_imports
            )
        layout_probe_path: Path | None = None
        if reloc_requested and linked and runtime_reloc_wasm is not None:
            if not ensure_runtime_wasm_reloc(None):
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if runtime_reloc_wasm.exists():
                layout_probe_path = runtime_reloc_wasm
        if "MOLT_WASM_DATA_BASE" not in backend_env:
            if layout_probe_path is None:
                if not ensure_runtime_wasm_shared(None):
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if runtime_wasm is not None and runtime_wasm.exists():
                    layout_probe_path = runtime_wasm
        if (
            "MOLT_WASM_DATA_BASE" not in backend_env
            and layout_probe_path is not None
            and layout_probe_path.exists()
        ):
            data_base_candidates: list[int] = []
            data_end = _read_wasm_data_end(layout_probe_path)
            if data_end is not None:
                data_base_candidates.append((data_end + 7) & ~7)
            memory_min = _read_wasm_memory_min_bytes(layout_probe_path)
            if memory_min is not None:
                data_base_candidates.append((memory_min + 7) & ~7)
            if data_base_candidates:
                # Place output data well above the runtime's heap growth
                # region.  In the non-linked (split-runtime) path both
                # modules share linear memory: the runtime's dlmalloc heap
                # starts at __heap_base (near data_end) and grows upward.
                # If the heap reaches the output module's data segments the
                # allocator will hand out pointers inside the data region
                # and subsequent writes corrupt string constants and other
                # read-only data — manifesting as null-byte function
                # metadata on large modules (see MOL-heap-corruption).
                #
                # 64 MB gives ample room; the previous 16 MB was too tight
                # for apps with 1000+ functions where module-init alone can
                # allocate tens of MB of runtime objects.
                _HEAP_SAFETY_MARGIN = 64 * 1024 * 1024  # 64 MB
                raw_base = max(data_base_candidates)
                safe_base = (raw_base + _HEAP_SAFETY_MARGIN + 7) & ~7
                backend_env["MOLT_WASM_DATA_BASE"] = str(safe_base)
            else:
                warnings.append(
                    "Failed to read runtime memory layout; using default data base."
                )
        if (
            linked
            and not split_runtime
            and runtime_wasm is not None
            and not runtime_wasm.exists()
        ):
            if not ensure_runtime_wasm_shared(None):
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
        if "MOLT_WASM_TABLE_BASE" not in backend_env:
            table_probe_path = layout_probe_path or runtime_wasm
            if table_probe_path is not None and table_probe_path.exists():
                table_base = _read_wasm_table_min(table_probe_path)
                if table_base is not None:
                    backend_env["MOLT_WASM_TABLE_BASE"] = str(table_base)
                else:
                    warnings.append(
                        "Failed to read runtime table size; using default table base."
                    )
        if runtime_wasm is not None and runtime_wasm.exists():
            runtime_table_min = _read_wasm_table_min(runtime_wasm)
            if runtime_table_min is not None:
                raw_table_base = backend_env.get("MOLT_WASM_TABLE_BASE")
                try:
                    current_table_base = (
                        int(raw_table_base) if raw_table_base is not None else None
                    )
                except ValueError:
                    current_table_base = None
                if current_table_base is None or current_table_base < runtime_table_min:
                    backend_env["MOLT_WASM_TABLE_BASE"] = str(runtime_table_min)
        if (
            split_runtime
            and "MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN" not in backend_env
        ):
            split_runtime_table_probe = runtime_wasm
            if (
                split_runtime_table_probe is None
                or not split_runtime_table_probe.exists()
            ):
                split_runtime_table_probe = layout_probe_path
            if (
                split_runtime_table_probe is None
                or not split_runtime_table_probe.exists()
            ):
                if not ensure_runtime_wasm_shared(None):
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                split_runtime_table_probe = runtime_wasm
            if (
                split_runtime_table_probe is not None
                and split_runtime_table_probe.exists()
            ):
                runtime_table_min = _read_wasm_table_min(split_runtime_table_probe)
                if runtime_table_min is not None:
                    backend_env["MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN"] = str(
                        runtime_table_min
                    )
    if reloc_requested and backend_env is not None:
        backend_env["MOLT_WASM_LINK"] = "1"

    backend_bin = _backend_bin_path(molt_root, backend_cargo_profile, backend_features)
    if not _backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=cargo_timeout,
        json_output=json_output,
        cargo_profile=backend_cargo_profile,
        project_root=molt_root,
        backend_features=backend_features,
    ):
        return None, _fail("Backend build failed", json_output, command="build")
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
        daemon_config_digest = _backend_daemon_config_digest(
            molt_root,
            backend_cargo_profile,
            backend_bin=backend_bin,
            target_triple=target_triple,
        )
        if diagnostics_enabled and "backend_daemon_setup" not in phase_starts:
            phase_starts["backend_daemon_setup"] = time.perf_counter()
        daemon_socket = _backend_daemon_socket_path(
            molt_root,
            backend_cargo_profile,
            config_digest=daemon_config_digest,
        )
        startup_timeout = _backend_daemon_start_timeout()
        with _build_lock(molt_root, f"backend-daemon.{backend_cargo_profile}"):
            daemon_ready = _start_backend_daemon(
                backend_bin,
                daemon_socket,
                cargo_profile=backend_cargo_profile,
                project_root=molt_root,
                target_triple=target_triple,
                config_digest=daemon_config_digest,
                startup_timeout=startup_timeout,
                json_output=json_output,
                warnings=warnings,
            )
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
    target_triple: str | None,
    backend_daemon_config_digest: str | None,
    entry_module: str,
    ir: Mapping[str, Any],
    json_output: bool,
    warnings: list[str],
    verbose: bool,
    backend_bin: Path,
    backend_env: dict[str, str] | None,
    backend_timeout: float | None,
    molt_root: Path,
    backend_cargo_profile: str,
    _ensure_backend_ir_file_path: Callable[[], Path],
    cache_hit: bool,
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_health: dict[str, Any] | None,
) -> tuple[_BackendExecutionResult | None, _CliFailure | None]:
    backend_output_ctx: ContextManager[Path]
    # One-shot backend subprocess compilation should always write to a fresh
    # artifact path and stage atomically into cache/output afterward. Writing
    # directly into the cache artifact path couples codegen to cache lifecycle
    # and breaks first-build correctness when a toolchain rebuild invalidates
    # cache directories in the same command.
    backend_output_ctx = _temporary_backend_output_path(
        artifacts_root,
        is_wasm=is_wasm,
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
        wasm_split_runtime_runtime_table_min: int | None = None
        if is_wasm and backend_env is not None:
            wasm_link = backend_env.get("MOLT_WASM_LINK") == "1"
            raw_data_base = backend_env.get("MOLT_WASM_DATA_BASE")
            raw_table_base = backend_env.get("MOLT_WASM_TABLE_BASE")
            raw_split_runtime_runtime_table_min = backend_env.get(
                "MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN"
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
                wasm_split_runtime_runtime_table_min = (
                    int(raw_split_runtime_runtime_table_min)
                    if raw_split_runtime_runtime_table_min is not None
                    else None
                )
            except ValueError:
                wasm_split_runtime_runtime_table_min = None
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
                    molt_root, backend_cargo_profile
                )
                daemon_log_offset = _backend_daemon_log_mark(daemon_log_path)
            daemon_compile = _compile_with_backend_daemon(
                daemon_socket,
                project_root=molt_root,
                ir=ir,
                backend_output=backend_output,
                is_wasm=is_wasm,
                wasm_link=wasm_link,
                wasm_data_base=wasm_data_base,
                wasm_table_base=wasm_table_base,
                wasm_split_runtime_runtime_table_min=wasm_split_runtime_runtime_table_min,
                target_triple=target_triple,
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
                request_bytes=None,
                daemon_identity=daemon_identity,
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
                with _build_lock(molt_root, f"backend-daemon.{backend_cargo_profile}"):
                    daemon_ready = _start_backend_daemon(
                        backend_bin,
                        daemon_socket,
                        cargo_profile=backend_cargo_profile,
                        project_root=molt_root,
                        target_triple=target_triple,
                        config_digest=backend_daemon_config_digest,
                        startup_timeout=restart_timeout,
                        json_output=json_output,
                        warnings=warnings,
                    )
                if daemon_ready:
                    daemon_compile = _compile_with_backend_daemon(
                        daemon_socket,
                        project_root=molt_root,
                        ir=ir,
                        backend_output=backend_output,
                        is_wasm=is_wasm,
                        wasm_link=wasm_link,
                        wasm_data_base=wasm_data_base,
                        wasm_table_base=wasm_table_base,
                        wasm_split_runtime_runtime_table_min=wasm_split_runtime_runtime_table_min,
                        target_triple=target_triple,
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
                        request_bytes=None,
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
            if not is_wasm and not _is_transpile and backend_env is None:
                backend_env = os.environ.copy()
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
                target_triple=target_triple,
                wasm_link=wasm_link,
                wasm_data_base=wasm_data_base,
                wasm_table_base=wasm_table_base,
                wasm_split_runtime_runtime_table_min=wasm_split_runtime_runtime_table_min,
            )
            cmd_with_output = cmd + ["--output", str(backend_output)]
            # Ensure the output directory exists — --rebuild may have
            # cleared the cache tree, and the backend's own
            # ensure_output_parent_dir may race with ld -r timing.
            backend_output.parent.mkdir(parents=True, exist_ok=True)
            # Progress indicator for long builds (Issue 2.2 / 7.1).
            if not json_output:
                import sys as _sys

                _entry_name = (
                    entry_module.rsplit(".", 1)[-1] if entry_module else "program"
                )
                print(
                    f"Compiling {_entry_name}...",
                    end="",
                    flush=True,
                    file=_sys.stderr,
                )
            try:
                ir_file_path = _ensure_backend_ir_file_path()
                cmd_with_output.extend(["--ir-file", str(ir_file_path)])
                backend_process = _run_subprocess_captured_to_tempfiles(
                    cmd_with_output,
                    env=backend_env,
                    timeout=backend_timeout,
                    progress_label=None if json_output else "Backend compilation",
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
            # Always surface backend stderr when verbose — debug
            # env vars like MOLT_TRACE_EQ and MOLT_DEBUG_ENTRY_INIT
            # emit to stderr and are invisible without this.
            backend_stderr = _subprocess_output_text(backend_process.stderr)
            backend_stdout = _subprocess_output_text(backend_process.stdout)
            if verbose and not json_output:
                if backend_stderr:
                    print(backend_stderr, end="", file=sys.stderr)
            if backend_process.returncode != 0:
                if not json_output and not verbose:
                    if backend_stderr:
                        print(backend_stderr, end="", file=sys.stderr)
                    if backend_stdout:
                        print(backend_stdout, end="")
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
            if verbose and not json_output:
                backend_stdout = _subprocess_output_text(backend_process.stdout)
                backend_stderr = _subprocess_output_text(backend_process.stderr)
                if backend_stdout:
                    print(backend_stdout, end="")
                if backend_stderr:
                    print(backend_stderr, end="", file=sys.stderr)
            backend_output_written = True
            if not json_output:
                import sys as _sys

                print(" done", file=_sys.stderr)
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
    runtime_cargo_profile: str,
    cargo_timeout: float | None,
    molt_root: Path,
    target_triple: str | None,
    backend_cargo_profile: str,
    backend_timeout: float | None,
    backend_daemon_config_digest: str | None,
    entry_module: str,
    resolved_modules: frozenset[str],
    ensure_runtime_wasm_shared: Callable[[set[str] | frozenset[str] | None], bool],
    ensure_runtime_wasm_reloc: Callable[[set[str] | frozenset[str] | None], bool],
    artifacts_root: Path,
    ir: Mapping[str, Any],
    _ensure_backend_ir_file_path: Callable[[], Path],
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_health: dict[str, Any] | None,
) -> tuple[_PreparedBackendCompile | None, _CliFailure | None]:
    if diagnostics_enabled:
        phase_starts["cache_lookup"] = time.perf_counter()
    cache_enabled = cache_setup.cache_enabled
    wasm_table_base: int | None = None

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
                is_wasm=is_wasm,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                stdlib_object_path=cache_setup.stdlib_object_path,
                stdlib_object_cache_key=cache_setup.stdlib_object_cache_key,
                stdlib_object_manifest=cache_setup.stdlib_object_manifest,
                stdlib_module_symbols=cache_setup.stdlib_module_symbols,
                warnings=warnings,
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
                    is_rust_transpile=is_rust_transpile,
                    is_luau_transpile=is_luau_transpile,
                    is_wasm=is_wasm,
                    split_runtime=split_runtime,
                    linked=linked,
                    deterministic=deterministic,
                    profile=profile,
                    runtime_state=runtime_state,
                    runtime_cargo_profile=runtime_cargo_profile,
                    cargo_timeout=cargo_timeout,
                    molt_root=molt_root,
                    target_triple=target_triple,
                    backend_cargo_profile=backend_cargo_profile,
                    diagnostics_enabled=diagnostics_enabled,
                    phase_starts=phase_starts,
                    json_output=json_output,
                    backend_daemon_config_digest=backend_daemon_config_digest,
                    ensure_runtime_wasm_shared=ensure_runtime_wasm_shared,
                    ensure_runtime_wasm_reloc=ensure_runtime_wasm_reloc,
                    resolved_modules=resolved_modules,
                    warnings=warnings,
                )
            )
            if prepared_backend_dispatch_error is not None:
                return None, prepared_backend_dispatch_error
            assert prepared_backend_dispatch is not None
            if is_wasm and prepared_backend_dispatch.backend_env is not None:
                raw_table_base = prepared_backend_dispatch.backend_env.get(
                    "MOLT_WASM_TABLE_BASE"
                )
                try:
                    wasm_table_base = (
                        int(raw_table_base) if raw_table_base is not None else None
                    )
                except ValueError:
                    wasm_table_base = None
            if diagnostics_enabled and "backend_dispatch" not in phase_starts:
                phase_starts["backend_dispatch"] = time.perf_counter()
            backend_execution_result, backend_execution_error = (
                _execute_backend_compile(
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
                    target_triple=target_triple,
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
        wasm_table_base=wasm_table_base,
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_health=backend_daemon_health,
        backend_daemon_config_digest=backend_daemon_config_digest,
    ), None
