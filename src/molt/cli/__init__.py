from __future__ import annotations

import argparse
import ast
import codecs
import contextlib
from concurrent.futures import Future, ProcessPoolExecutor
import errno
import datetime as dt
import functools
import hashlib
import importlib.util
import io
import tempfile
import json
import os
import pathlib
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tomllib
import time
import threading
import tracemalloc
import tokenize
from types import MappingProxyType
import uuid
import zipfile
from contextlib import contextmanager, nullcontext, redirect_stderr, redirect_stdout
from dataclasses import dataclass, field
from pathlib import Path
from typing import (
    Any,
    Callable,
    Collection,
    ContextManager,
    Iterable,
    Iterator,
    Literal,
    Mapping,
    MutableMapping,
    NamedTuple,
    Sequence,
    cast,
)

from molt.compat import CompatibilityError
from molt import backend_daemon_custody as _daemon_custody
from molt import process_guard as _process_guard
from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt._wasm_runtime_exports import (
    wasm_runtime_export_link_args,
    wasm_runtime_missing_required_exports,
)
from molt.debug import DebugSubcommand
from molt.dx import DxConfigError, DxProject
from molt.frontend import SimpleTIRGenerator

from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli import typecheck as _typecheck
from molt.cli import factgraph as _factgraph
from molt.cli.config_resolution import (
    ENTRY_OVERRIDE_ENV,
    STATIC_IMPORT_MODULES_ENV,
    _coerce_bool,
    _config_value,
    _resolve_build_config,
    _resolve_capabilities_config,
    _resolve_command_config,
    resolve_stdlib_profile,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_link_or_copy_file,
    _atomic_write_bytes,
    _atomic_write_json,
    _atomic_write_text,
    _atomic_zip_file,
    _remove_file_or_tree,
    _write_json_sidecar,
    _write_text_if_changed,
)
from molt.cli.artifact_state import (
    _artifact_state_path,
    _artifact_state_path_cached,
    _artifact_state_path_for_build_state_root,
    _build_state_subdir_cached,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
    _resolved_artifact_hash_key,
    _runtime_fingerprint_path,
    _runtime_target_fingerprint_path,
)
from molt.cli.build_locks import (
    _build_lock,
    _build_lock_dir_cached,
)
from molt.cli.cache_fingerprints import (
    _backend_source_paths,
    _cache_fingerprint,
    _cache_tooling_fingerprint,
    _frontend_semantic_tooling_fingerprint,
    _source_tree_fingerprint_transaction,
)
from molt.cli.cache_keys import (
    _cache_backend_payload_ir,
    _cache_ir_payload_ir,
    _cache_key,
    _function_cache_key,
    _json_ir_default,
    _sorted_ir_functions,
)
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _CROSS_MEMORY_GUARD_PREFIX,
    _DIFF_MEMORY_GUARD_PREFIX,
    _load_cli_harness_memory_guard,
    _resolve_timeout_env,
    _run_completed_command,
    _run_subprocess_captured_to_tempfiles,
    _with_memory_guard_env,
)
from molt.cli.compiler_metadata import (
    _compiler_metadata,
    _compiler_root,
    _git_rev,
    _rustc_version,
)
from molt.capability_policy import (
    CapabilityInput,
    CapabilityInputResolution,
    format_capability_input,
    materialize_capability_input,
    parse_capability_input,
)
from molt._host_capabilities_generated import MAXIMUM_BUILTIN_CAPABILITY_TIER
from molt.cli.default_paths import (
    _default_home_str,
    _default_molt_bin,
    _default_molt_bin_cached,
    _default_molt_cache,
    _default_molt_cache_cached,
    _default_molt_home,
    _default_molt_home_cached,
)
from molt.cli.deps import (
    MOLT_VENV_DIR,
    _NoRedirectHandler,
    _append_feature_notes,
    _classify_tier,
    _clone_git_source,
    _collect_dep_specs,
    _collect_deps,
    _dep_allowlists,
    _download_artifact,
    _git_ref_from_source,
    _is_private_ip,
    _load_toml,
    _lock_package_graph,
    _lock_packages,
    _marker_environment,
    _marker_satisfied,
    _molt_venv_path,
    _normalize_name,
    _parse_requirement,
    _pick_vendor_artifact,
    _read_cached_artifact,
    _resolve_dependency_closure,
    _resolve_git_ref,
    _run_git_source_command,
    _summarize_tiers,
    _vendor_cache_path,
    _write_cached_artifact,
    deps,
    install,
    install_add,
    vendor,
)
from molt.cli.env_paths import (
    _base_env,
    _molt_venv_site_packages,
    _resolve_env_path,
    _resolve_env_path_cached,
    _vendor_roots,
)
from molt.cli.env_overrides import temporary_env_overrides as _temporary_env_overrides
from molt.file_hashing import _sha256_file
from molt.cli.external_native import (
    _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_EXCLUDED_DIRS,
    _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES,
    _extension_path_matches_manifest,
    _external_extension_module_name,
    _external_native_artifact_output_custody_error,
    _external_native_support_source_paths,
    _external_package_dir,
    _external_package_init_source_paths,
    _external_package_source_root,
    _external_staged_path_for_source,
    _find_external_extension_manifest,
    _is_external_package_native_artifact,
    _iter_external_package_native_artifacts,
    _parse_external_static_packages,
    _remove_staged_external_candidate,
    _required_manifest_str,
    _resolve_external_package_native_artifact_plan,
    _resolve_import_admission_policy,
    _stage_external_native_required_file,
    _stage_external_native_support_files,
    _stage_external_package_native_artifacts_for_build,
    _validate_external_package_native_artifact,
)
from molt.cli.output import (
    JSON_SCHEMA_VERSION,
    CliFailure as _CliFailure,
    coerce_process_text as _coerce_process_text,
    emit_json as _emit_json,
    fail as _fail,
    json_payload as _json_payload,
    subprocess_output_text as _subprocess_output_text,
)
from molt.cli.package_registry import (
    _is_remote_registry,
)
from molt.cli.profile_feedback import (
    _extract_hot_functions,
    _load_pgo_profile,
    _load_runtime_feedback,
    _pgo_hotspot_entries,
)
from molt.cli.lockfiles import (
    _LOCK_CHECK_CACHE_VERSION,
    _check_lockfiles,
    _is_lock_check_cache_valid,
    _load_lock_check_cache,
    _lock_check_cache_path,
    _lock_check_cache_path_cached,
    _lock_check_inputs,
    _verify_cargo_lock,
    _verify_uv_lock,
    _write_lock_check_cache,
)
from molt.cli.project_roots import (
    _find_molt_root,
    _find_molt_root_cached,
    _find_project_root,
    _find_project_root_cached,
    _has_molt_repo_markers,
    _has_project_markers,
    _is_path_within,
    _require_molt_root,
    _resolve_root_override,
)
from molt.cli.runtime_paths import (
    _RUNTIME_STDLIB_PROFILE_ALIASES,
    _build_state_root,
    _build_state_root_cached,
    _cargo_profile_dir,
    _cargo_target_root,
    _cargo_target_root_cached,
    _molt_session_id,
    _normalize_runtime_stdlib_profile,
    _runtime_lib_archive_name,
    _runtime_lib_archive_names,
    _runtime_lib_path,
    _runtime_lib_path_cached,
    _runtime_cargo_scratch_lib_name,
    _runtime_cargo_scratch_lib_path,
    _runtime_staticlib_target_is_windows,
    _runtime_wasm_artifact_path,
    _runtime_wasm_artifact_path_cached,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
    _wasm_runtime_feature_plan,
)
from molt.cli.json_contract import (
    _coerce_json_path,
    _extract_json_errors,
    _extract_json_warnings,
    _extract_payload_text_list,
    _wrapper_build_payload_data,
)
from molt.cli.json_cache import (
    _PERSISTED_JSON_OBJECT_CACHE,
    _read_cached_json_object,
    _write_cached_json_object,
)
from molt.cli.extension_manifest import (
    ExtensionManifestValidation,
    _MOLT_C_API_VERSION_RE,
    _abi_version_error as _abi_version_error,
    _coerce_str_list,
    _cpu_baseline,
    _default_molt_c_api_version,
    _extension_binary_suffix,
    _is_extension_manifest,
    _load_manifest,
    _manifest_errors,
    _module_parts,
    _normalize_effects,
    _validate_extension_manifest,
    _wheel_record_line,
    _wheel_token,
    _wheel_version_token,
    _write_zip_member,
)
from molt.cli.extension_audit import extension_audit
from molt.cli.native_link_plan import _host_target_triple
from molt.cli.extension_scan import extension_scan
from molt.cli.extension_seal import extension_seal
from molt.cli.models import (
    BuildProfile,
    EmitMode,
    FallbackPolicy,
    ImportScanMode,
    ParseCodec,
    PgoProfileSummary,
    RuntimeFeedbackSummary,
    Target,
    TypeHintPolicy,
    _BackendCacheSetup,
    _BackendDaemonCompileResult,
    _BackendExecutionResult,
    _BinaryImageScope,
    _BuildDiagnosticsContext,
    _BuildOutputLayout,
    _EntryFrontendLoweringContext,
    _ExternalPackageNativeArtifact,
    _ExternalPackageNativeArtifactPlan,
    _FrontendIntegrationState,
    _FrontendLayerExecutionContext,
    _FrontendLayerPlan,
    _FrontendLayerPolicySummary,
    _FrontendLayerRunResult,
    _FrontendLayerRuntimeHooks,
    _FrontendLayerStaticMetrics,
    _FrontendModuleResultTimings,
    _FrontendParallelConfig,
    _FrontendParallelLayerState,
    _FrontendTimingRecorderConfig,
    _ImportAdmissionPolicy,
    _ImportPlan,
    _MaintenanceStep,
    _MidendDiagnosticsState,
    _ModuleGraphAugmentation,
    _ModuleGraphMetadata,
    _ModuleLowerError,
    _ModuleLoweringExecutionView,
    _ModuleLoweringMetadataView,
    _ModuleRootResolution,
    _ParallelWorkerSubmission,
    _PreparedBackendCompile,
    _PreparedBackendDispatch,
    _PreparedBackendIR,
    _PreparedBackendRuntimeContext,
    _PreparedBackendSetup,
    _PreparedBuildCallbacks,
    _PreparedBuildConfig,
    _PreparedBuildModuleOutputs,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _PreparedEntryModuleGraph,
    _PreparedFrontendAnalysis,
    _PreparedFrontendLoweringConfig,
    _PreparedFrontendRunTicket,
    _PreparedNativeLink,
    _PreparedNonNativeResult,
    _ResolvedBuildEntry,
    _RuntimeArtifactState,
    _RuntimeImportSupportPolicy,
    _ScopedLoweringInputView,
    _ScopedLoweringInputs,
    _SerialFrontendLoweringContext,
    _SerialFrontendLoweringHooks,
    _StagedExternalPackageNativeArtifact,
    _SupportModuleAugmentation,
    _TimedResult,
    _ToolchainReport,
    _ValidationStep,
    _WorkerTimingSummary,
    _WrapperBuildContract,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
)
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _SUPPORTED_TARGET_PYTHON_BY_SHORT as _SUPPORTED_TARGET_PYTHON_BY_SHORT,
    _SUPPORTED_TARGET_PYTHON_VERSIONS as _SUPPORTED_TARGET_PYTHON_VERSIONS,
    _parse_source_for_target,
    _parse_target_python_version,
    _project_requires_python as _project_requires_python,
    _resolve_target_python_version,
    _target_python_from_requires_python as _target_python_from_requires_python,
)
from molt.cli.module_graph import (
    _augment_module_graph_for_entry_and_runtime,
    _augment_support_modules,
    _build_frontend_module_costs,
    _build_module_graph_metadata,
    _build_module_lowering_metadata,
    _collect_namespace_parents,
    _collect_package_parents,
    ENTRY_OVERRIDE_SPAWN,
    _logical_generated_module_path,
    _materialize_import_plan,
    ModuleSyntaxErrorInfo,
    _namespace_paths,
    _prepare_entry_module_graph,
    _requires_spawn_entry_override,
    STUB_PARENT_MODULES,
    _write_importer_module,
    _write_namespace_module,
)
from molt.cli.module_source import (
    _ModuleSourceCatalog,
    _ModuleSourceLease,
    _build_module_source_catalog,
    _payload_source_matches,
    _read_module_source,
    _source_content_sha256,
    _source_content_sha256_cached,
)
from molt.cli.module_cache import (
    _MODULE_ANALYSIS_CACHE_SCHEMA_VERSION,
    _MODULE_ANALYSIS_FUNC_KINDS,
    _MODULE_LOWERING_CACHE_SCHEMA_VERSION,
    _build_scoped_known_classes_snapshot,
    _build_scoped_lowering_inputs,
    _collect_func_defaults,
    _collect_func_kinds,
    _decode_cached_json_value,
    _load_cached_module_lowering_result,
    _load_module_analysis,
    _module_analysis_cache_path,
    _module_lowering_cache_path,
    _module_lowering_context_digest,
    _module_lowering_context_digest_for_module,
    _module_lowering_context_payload,
    _module_lowering_execution_view,
    _module_lowering_metadata_view,
    _module_worker_payload,
    _normalize_backend_ir_functions,
    _read_persisted_module_analysis,
    _read_persisted_module_lowering,
    _scoped_known_classes,
    _scoped_known_classes_view,
    _scoped_known_func_defaults,
    _scoped_known_func_kinds,
    _scoped_known_modules,
    _scoped_lowering_input_view,
    _scoped_pgo_hot_function_names,
    _scoped_type_facts,
    _type_facts_cache_payload,
    _validate_module_func_default_payload,
    _write_persisted_module_analysis,
    _write_persisted_module_lowering,
)

from molt.cli._lazy_facade import (
    _LAZY_REEXPORTS,
    _LazyPostLoweringModule,
    __dir__,
    __getattr__,
    _build_inputs,
    _build_pipeline,
    _scoped_environ_updates,
)


_HASH_SEED_SENTINEL_ENV = "MOLT_HASH_SEED_APPLIED"
_HASH_SEED_OVERRIDE_ENV = "MOLT_HASH_SEED"


def build(
    file_path: str | None,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "check",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
    pgo_profile: str | None = None,
    runtime_feedback: str | None = None,
    output: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    cache: bool = True,
    cache_dir: str | None = None,
    cache_report: bool = False,
    sysroot: str | None = None,
    emit_ir: str | None = None,
    emit: EmitMode | None = None,
    out_dir: str | None = None,
    profile: BuildProfile = "release",
    linked: bool = False,
    linked_output: str | None = None,
    require_linked: bool = False,
    respect_pythonpath: bool = False,
    module: str | None = None,
    diagnostics: bool | None = None,
    diagnostics_file: str | None = None,
    diagnostics_verbosity: str | None = None,
    portable: bool = False,
    wasm_opt_level: str = "Oz",
    precompile: bool = False,
    wasm_profile: str = "auto",
    snapshot: bool = False,
    stdlib_profile: str | None = None,
    tree_shake: bool = True,
    lib_paths: list[str] | None = None,
    split_runtime: bool = False,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    audit_log: str | None = None,
    io_mode: str | None = None,
    type_gate: bool = False,
    python_version: str | None = None,
    build_config: Mapping[str, Any] | None = None,
    bolt: bool = False,
    bolt_training_cmd: str | None = None,
    fact_graph_request: _factgraph.FactGraphRequest | None = None,
) -> int:
    if isinstance(profile, bool):
        profile = "release"
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    if bolt_training_cmd is not None and not bolt:
        return _fail(
            "BOLT training command requires BOLT optimization.",
            json_output,
            command="build",
        )
    if bolt and profile != "release":
        return _fail(
            "BOLT requires the release build profile.",
            json_output,
            command="build",
        )
    if bolt and target in {"wasm", "wasm-freestanding", "rust", "luau", "mlir"}:
        return _fail(
            f"BOLT requires a native Linux ELF target, not {target!r}.",
            json_output,
            command="build",
        )
    # Resolve `stdlib_profile` through the ONE config authority. The CLI
    # dispatcher already resolves it before calling `build()`, but direct
    # library callers may pass `None`; resolving here (honoring the flag value,
    # `MOLT_STDLIB_PROFILE`, the `[tool.molt.build]` config, and the single
    # default) guarantees a concrete value that is both passed to the runtime
    # build and re-exported to the env below, so the module-graph closure reader
    # and the staticlib selector can never disagree.
    stdlib_profile, _ = resolve_stdlib_profile(
        flag=stdlib_profile,
        build_cfg=(
            _resolve_build_config(dict(build_config))
            if build_config is not None
            else None
        ),
    )
    env_updates: dict[str, str] = {}
    if trusted:
        env_updates["MOLT_CAPABILITY_TIER"] = MAXIMUM_BUILTIN_CAPABILITY_TIER
    # --audit-log: propagate audit config via environment variables for the
    # build pipeline only. Several lower layers intentionally read os.environ as
    # the canonical build signal, so keep that custody but restore the caller's
    # process environment when the build returns.
    if audit_log is not None:
        env_updates.update(_build_inputs._parse_audit_log_flag(audit_log))
    # --io-mode: propagate IO mode via environment variable.
    if io_mode is not None:
        env_updates.update(_build_inputs._parse_io_mode_flag(io_mode))
    # --type-gate: propagate type gate to the backend.
    env_updates.update(_build_inputs._parse_type_gate_flag(type_gate))
    # --portable: force baseline ISA for cross-machine reproducible codegen.
    if portable:
        env_updates["MOLT_PORTABLE"] = "1"
    # --split-runtime: signal to the non-native build result handler.
    if split_runtime:
        env_updates["MOLT_SPLIT_RUNTIME"] = "1"
    # --wasm-profile: pass the effective profile to the backend explicitly so
    # CLI, config, deploy defaults, and direct library calls share one
    # import-planning authority without widening ordinary wasm builds to the
    # full manifest surface.
    if target in {"wasm", "wasm-freestanding"} and wasm_profile:
        env_updates["MOLT_WASM_PROFILE"] = wasm_profile
    # --stdlib-profile: propagate the resolved profile to module-graph
    # construction so the closure reader (`_ensure_core_stdlib_modules`) and the
    # runtime-staticlib selector are derived from the same value. `stdlib_profile`
    # is always concrete here (resolved above), so this export is unconditional.
    env_updates["MOLT_STDLIB_PROFILE"] = stdlib_profile
    with _scoped_environ_updates(env_updates), _source_tree_fingerprint_transaction():
        if file_path and module:
            return _fail(
                "Use a file path or --module, not both.", json_output, command="build"
            )
        prepared_build_inputs, prepared_build_inputs_error = (
            _build_inputs._prepare_build_inputs(
                file_path=file_path,
                module=module,
                diagnostics=diagnostics,
                diagnostics_file=diagnostics_file,
                diagnostics_verbosity=diagnostics_verbosity,
                json_output=json_output,
                target=target,
                deterministic=deterministic,
                deterministic_warn=deterministic_warn,
                sysroot=sysroot,
                profile=profile,
                pgo_profile=pgo_profile,
                runtime_feedback=runtime_feedback,
                capabilities=capabilities,
                capability_manifest=capability_manifest,
                require_signed_manifest=require_signed_manifest,
                trusted=trusted,
                audit_log=audit_log,
                io_mode=io_mode,
                respect_pythonpath=respect_pythonpath,
                lib_paths=lib_paths or [],
                python_version=python_version,
                build_config=build_config,
            )
        )
        if prepared_build_inputs_error is not None:
            return prepared_build_inputs_error
        assert prepared_build_inputs is not None
        (
            prepared_build_preamble,
            prepared_build_roots,
            prepared_build_config,
            resolved_build_entry,
        ) = prepared_build_inputs
        prepared_frontend_pipeline_bundle, prepared_frontend_pipeline_error = (
            _frontend_pipeline._prepare_frontend_pipeline(
                prepared_build_preamble=prepared_build_preamble,
                prepared_build_roots=prepared_build_roots,
                prepared_build_config=prepared_build_config,
                resolved_build_entry=resolved_build_entry,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                profile=profile,
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
                tree_shake=tree_shake,
            )
        )
        if prepared_frontend_pipeline_error is not None:
            return prepared_frontend_pipeline_error
        assert prepared_frontend_pipeline_bundle is not None
        return _build_pipeline._run_build_pipeline(
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
            wasm_opt_level=wasm_opt_level,
            precompile=precompile,
            snapshot=snapshot,
            stdlib_profile=stdlib_profile,
            bolt_requested=bolt,
            bolt_training_cmd=bolt_training_cmd,
            fact_graph_request=fact_graph_request,
        )


def main() -> int:
    from molt.cli import entrypoint as _entrypoint

    return _entrypoint.main(build_fn=build)
