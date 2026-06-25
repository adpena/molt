from __future__ import annotations

import argparse
import ast
import codecs
import contextlib
from concurrent.futures import Future, ProcessPoolExecutor
import errno
import datetime as dt
import functools
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
    wasm_runtime_required_import_names,
)
from molt.debug import DebugSubcommand
from molt.dx import DxConfigError, DxProject
from molt.frontend import SimpleTIRGenerator
from molt.cli.completion import _completion_script
from molt.cli import backend_binary as _backend_binary
from molt.cli import backend_ir as _backend_ir
from molt.cli import build_inputs as _build_inputs
from molt.cli import debug_helpers as _debug_helpers
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli import link_pipeline as _link_pipeline
from molt.cli import typecheck as _typecheck
from molt.cli import factgraph as _factgraph
from molt.cli.maintenance import _load_artifact_cleanup_module, clean, show_config
from molt.cli.config_resolution import (
    ENTRY_OVERRIDE_ENV,
    STATIC_IMPORT_MODULES_ENV,
    _coerce_bool,
    _config_value,
    _resolve_build_config,
    _resolve_capabilities_config,
    _resolve_command_config,
)
from molt.cli.arg_helpers import (
    _BUILD_ESSENTIAL_FLAGS,
    _BuildHelpFormatter,
    _MoltHelpFormatter,
    _add_debug_shared_selector_args,
    _build_args_has_cache_flag,
    _build_args_has_capabilities_flag,
    _build_args_has_profile_flag,
    _build_args_has_trusted_flag,
    _cli_hash_seed_reexec_argv,
    _ensure_cli_hash_seed,
    _extract_emit_arg,
    _extract_out_dir_arg,
    _extract_output_arg,
    _flush_standard_streams,
    _is_windows_process_model,
    _process_exit_code,
    _reexec_cli_with_hash_seed,
    _resolve_binary_output,
    _strip_leading_double_dash,
    completion,
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
from molt.cli.backend_daemon_config import (
    _backend_daemon_enabled,
    _backend_daemon_enabled_cached,
)
from molt.cli.backend_daemon_logs import (
    _backend_daemon_log_mark,
    _backend_daemon_log_max_bytes,
    _backend_daemon_log_max_bytes_cached,
    _backend_daemon_log_since,
    _backend_daemon_log_tail,
    _rotate_backend_daemon_log_if_large,
)
from molt.cli.backend_daemon_paths import (
    _backend_daemon_paths as _backend_daemon_paths_bundle,
    _backend_daemon_socket_path_error,
    _short_backend_daemon_socket_dir as _short_backend_daemon_socket_dir_impl,
    _unix_socket_path_exceeds_limit as _unix_socket_path_exceeds_limit,
)
from molt.cli.backend_daemon_startup import (
    _backend_daemon_spawn_probe_timeout,
    _backend_daemon_start_timeout,
    _backend_daemon_start_timeout_cached,
)
from molt.cli.backend_diagnostics import (
    _BACKEND_DIAGNOSTIC_ENV_KNOBS as _BACKEND_DIAGNOSTIC_ENV_KNOBS,
    _FALSY_ENV_VALUES,
    _PYTHON_WARNING_RE as _PYTHON_WARNING_RE,
    _env_requests_backend_diagnostics,
    _forward_compilation_warnings,
)
from molt.cli.build_locks import (
    _acquire_file_lock,
    _build_lock,
    _build_lock_dir_cached,
    _parse_lock_timeout,
    _release_file_lock,
    _try_acquire_file_lock,
)
from molt.cli.cache_fingerprints import (
    _cache_fingerprint,
    _cache_tooling_fingerprint,
)
from molt.cli.cache_keys import (
    _cache_backend_payload_ir,
    _cache_ir_payload_ir,
    _cache_key,
    _function_cache_key,
    _json_ir_default,
    _sorted_ir_functions,
)
from molt.cli.backend_cache import (
    _ARTIFACT_SYNC_STATE_CACHE,
    _DEAD_FUNCTION_ELIM_REFERENCE_KINDS,
    _SHARED_STDLIB_CACHE_SCHEMA_VERSION,
    _SHARED_STDLIB_MANIFEST_SCHEMA_VERSION,
    _SHARED_STDLIB_PARTITION_SCHEMA_VERSION,
    _artifact_sync_state_matches,
    _artifact_sync_state_matches_stat,
    _artifact_sync_state_path,
    _backend_cache_artifact_path,
    _backend_daemon_skip_output_sync_flags,
    _emitted_name_matches_module_symbol,
    _encode_stdlib_module_symbols,
    _is_protected_runtime_entrypoint,
    _is_stdlib_owned_symbol,
    _is_user_owned_symbol,
    _is_valid_cached_backend_artifact,
    _materialize_cached_backend_artifact,
    _module_symbol_name,
    _native_artifact_source_key,
    _native_nm_command,
    _native_object_global_symbol_sets,
    _native_object_global_symbols_result,
    _native_object_has_unresolved_module_chunks,
    _native_stdlib_object_split_enabled,
    _publish_immutable_backend_cache_artifact,
    _read_artifact_sync_state,
    _read_shared_stdlib_partition_functions,
    _read_stdlib_cache_key,
    _reachable_function_names_for_stdlib_cache,
    _remove_shared_stdlib_cache_artifacts,
    _shared_cache_lock,
    _shared_cache_lock_dir_cached,
    _shared_stdlib_cache_key,
    _shared_stdlib_cache_lock,
    _shared_stdlib_cache_matches_key,
    _shared_stdlib_cache_mismatch_detail,
    _shared_stdlib_cache_payload_ir,
    _shared_stdlib_compiler_fingerprint,
    _shared_stdlib_manifest,
    _shared_stdlib_native_symbol_closure_issue,
    _shared_stdlib_publish_lock_path,
    _stage_backend_output_and_caches,
    _stdlib_module_symbols,
    _stdlib_object_cache_path,
    _stdlib_object_count_sidecar_path,
    _stdlib_object_digest_sidecar_path,
    _stdlib_object_key_sidecar_path,
    _stdlib_object_manifest_sidecar_path,
    _stdlib_object_partition_manifest_sidecar_path,
    _temporary_backend_output_path,
    _try_cached_backend_candidates,
    _unresolved_stdlib_module_symbols,
    _validate_shared_stdlib_cache_contract,
    _write_artifact_sync_payload,
    _write_artifact_sync_state,
)
from molt.cli.backend_execution import (
    _BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION,
    _BACKEND_CODEGEN_REQUEST_ENV_KNOBS,
    _BACKEND_DAEMON_ORPHAN_SWEEP_DONE,
    _BACKEND_DAEMON_PROTOCOL_VERSION,
    _BACKEND_REQUEST_ENV_KNOBS,
    _BACKEND_RESOURCE_ENV_KNOBS,
    _DAEMON_CONFIG_DIGEST_SCHEMA_VERSION,
    _NATIVE_CODEGEN_ENV_KNOBS,
    _NATIVE_RELOCATABLE_LINKER_ENV_KEYS,
    _WASM_CODEGEN_ENV_KNOBS,
    _BackendDaemonIdentity,
    _backend_bin_path,
    _backend_bin_path_cached,
    _backend_binary_identity,
    _backend_codegen_env_digest,
    _backend_codegen_env_inputs,
    _backend_codegen_env_inputs_cached,
    _backend_daemon_binary_is_newer,
    _backend_daemon_command_has_socket,
    _backend_daemon_command_matches_identity,
    _backend_daemon_compile_request_bytes,
    _backend_daemon_config_digest,
    _backend_daemon_empty_response_error,
    _backend_daemon_freshness_inputs,
    _backend_daemon_health_from_response,
    _backend_daemon_health_probe,
    _backend_daemon_identity_for_pid,
    _backend_daemon_identity_from_health,
    _backend_daemon_identity_health_matches,
    _backend_daemon_identity_is_verified,
    _backend_daemon_identity_matches_context,
    _backend_daemon_identity_path,
    _backend_daemon_identity_process_matches,
    _backend_daemon_job_failure_message,
    _backend_daemon_log_path,
    _backend_daemon_paths_cached,
    _backend_daemon_ping,
    _backend_daemon_ping_health,
    _backend_daemon_process_command,
    _backend_daemon_request,
    _backend_daemon_request_bytes,
    _backend_daemon_request_on_socket,
    _backend_daemon_request_payload_bytes,
    _backend_daemon_response_failure_message,
    _backend_daemon_retryable_error,
    _backend_daemon_socket_dir,
    _backend_daemon_socket_path,
    _backend_daemon_text_field,
    _backend_daemon_wait_until_ready,
    _backend_features_for_build_target,
    _backend_features_for_target,
    _command_executable_matches_backend,
    _command_has_path_separator,
    _compile_with_backend_daemon,
    _native_relocatable_linker_identity,
    _native_relocatable_linker_selection,
    _path_freshness_fingerprint,
    _pid_alive,
    _read_backend_daemon_identity,
    _runtime_lib_freshness_candidates,
    _short_backend_daemon_socket_dir,
    _source_tree_freshness_fingerprint,
    _split_backend_daemon_command,
    _start_backend_daemon,
    _sweep_orphaned_backend_daemon_locks,
    _sweep_orphaned_backend_daemon_locks_once,
    _terminate_backend_daemon_identity,
    _write_backend_daemon_identity,
    _write_backend_daemon_ir_lease,
    _write_backend_ir_json_file,
    _write_backend_ir_lease,
    _remove_backend_daemon_identity,
)
from molt.cli.cargo_execution import (
    _build_slot,
)
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _CROSS_MEMORY_GUARD_PREFIX,
    _DIFF_MEMORY_GUARD_PREFIX,
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
from molt.cli.capability_spec import (
    CAPABILITY_PROFILES as CAPABILITY_PROFILES,
    CAPABILITY_TOKEN_RE as CAPABILITY_TOKEN_RE,
    CapabilityGrant as CapabilityGrant,
    CapabilityInput,
    CapabilityManifest,
    CapabilitySpec as CapabilitySpec,
    _allowed_capabilities_for_package,
    _allowed_effects_for_package,
    _coerce_effects_list as _coerce_effects_list,
    _coerce_token_list as _coerce_token_list,
    _dedupe_preserve_order,
    _expand_capabilities as _expand_capabilities,
    _format_capabilities_input,
    _materialize_capabilities_arg,
    _merge_optional_list as _merge_optional_list,
    _parse_capabilities,
    _parse_capabilities_spec,
    _parse_capability_manifest_dict as _parse_capability_manifest_dict,
    _parse_fs_block as _parse_fs_block,
    _parse_package_grant as _parse_package_grant,
    _parse_package_grants as _parse_package_grants,
    _resolve_capability_manifest as _resolve_capability_manifest,
    _split_tokens,
)
from molt.cli.cargo_profiles import (
    _CARGO_PROFILE_NAME_RE,
    _active_artifact_profile_dirs,
    _resolve_backend_cargo_profile_name,
    _resolve_backend_cargo_profile_name_cached,
    _resolve_backend_profile,
    _resolve_backend_profile_cached,
    _resolve_cargo_profile_name,
    _resolve_cargo_profile_name_cached,
)
from molt.cli.build_output_layout import (
    _BUILD_OR_DEPLOY_PROFILE_CHOICES,
    _BUILD_PROFILE_CHOICES,
    _DEPLOY_PROFILE_CHOICES,
    _DEPLOY_PROFILE_DEFAULTS,
    _OUTPUT_BASE_SAFE_RE,
    _default_build_root,
    _default_build_root_cached,
    _resolve_build_output_layout,
    _resolve_cache_root,
    _resolve_cache_root_cached,
    _resolve_out_dir,
    _resolve_out_dir_cached,
    _resolve_output_path,
    _resolve_output_roots,
    _resolve_sysroot,
    _resolve_sysroot_cached,
    _safe_output_base,
    _wasm_runtime_root,
    _wasm_runtime_root_cached,
)
from molt.cli.build_diagnostics import (
    _build_allocation_diagnostics_enabled,
    _build_build_diagnostics_payload,
    _build_diagnostics_enabled,
    _build_midend_diagnostics_payload,
    _build_reason_summary,
    _capture_build_allocation_diagnostics,
    _duration_ms_from_ns,
    _emit_build_diagnostics,
    _emit_build_diagnostics_if_present,
    _midend_policy_config_snapshot,
    _midend_sample_p95,
    _midend_sample_percentile,
    _normalize_midend_pass_stat,
    _phase_duration_map,
    _record_frontend_timing_item,
    _resolve_build_diagnostics_path,
    _resolve_build_diagnostics_verbosity,
)
from molt.cli.build_results import (
    _attach_build_metadata,
    _attach_process_output,
    _build_cache_info,
    _build_common_build_json_data,
    _build_native_link_error_data,
    _build_native_link_success_data,
    _emit_build_success_json,
    _emit_native_link_result,
    _emit_non_native_build_result,
    _post_link_strip,
    _write_link_fingerprint_if_needed,
)
from molt.cli.frontend_execution import (
    _fresh_frontend_parallel_layer_state,
    _format_syntax_error_message,
    _syntax_error_stub_ast,
    _resolve_frontend_parallel_module_workers,
    _resolve_frontend_parallel_min_modules,
    _resolve_frontend_parallel_min_predicted_cost,
    _resolve_frontend_parallel_target_cost_per_worker,
    _resolve_frontend_parallel_stdlib_min_cost_scale,
    _predict_frontend_module_cost,
    _choose_frontend_parallel_layer_workers,
    _read_worker_source_lease,
    _frontend_lower_module_worker,
    _module_frontend_payload,
    _module_frontend_generator,
    _known_classes_snapshot_copy,
    _summarize_worker_timing_items,
    _frontend_parallel_layer_detail,
    _frontend_result_timings,
    _frontend_layer_policy_summary,
    _record_parallel_cached_module_result,
    _record_parallel_worker_result,
    _resolve_frontend_parallel_config,
    _frontend_parallel_policy_payload,
    _frontend_layer_plan,
    _worker_timing_summary_payload,
    _layer_cache_hit_count,
    _frontend_layer_static_metrics,
    _record_serial_frontend_worker_timing,
    _append_frontend_parallel_layer_detail,
    _initialize_frontend_parallel_details,
    _summarize_frontend_parallel_worker_timings,
    _append_frontend_serial_disabled_layer_detail,
    _resolve_tree_for_serial_frontend_module,
    _lower_module_serial_with_context,
    _run_serial_frontend_lower_with_context,
    _register_global_code_id_with_state,
    _remap_module_code_ops_with_state,
    _accumulate_midend_diagnostics_with_state,
    _integrate_module_frontend_result_with_state,
    _lower_entry_module_as_main,
    _prepare_frontend_execution,
    _run_frontend_parallel_enabled_layers,
    _run_frontend_pipeline,
    _run_frontend_serial_disabled_layers,
    _run_frontend_parallel_layer_batches,
    _fallback_frontend_parallel_layer_to_serial,
    _frontend_parallel_result_error,
    _write_parallel_persisted_module_lowering,
    _frontend_parallel_worker_timing_inputs,
    _take_frontend_parallel_layer_result,
    _record_parallel_layer_module_timing,
    _consume_frontend_module_result,
    _consume_frontend_parallel_layer_result,
    _consume_frontend_serial_layer_result,
    _run_frontend_serial_layer_modules,
    _run_frontend_layer,
    _frontend_serial_worker_mode,
    _prepare_frontend_parallel_batch,
    _phase_timeout,
)
from molt.cli.default_paths import (
    _default_home_str,
    _default_molt_bin,
    _default_molt_bin_cached,
    _default_molt_cache,
    _default_molt_cache_cached,
    _default_molt_home,
    _default_molt_home_cached,
)
from molt.cli.debug_helpers import (
    _capture_json_cli_result,
    _debug_eval_base_env,
    _emit_debug_payload,
    _load_debug_oracle,
    _merge_debug_manifest,
    _run_debug_eval_command,
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
from molt.cli.file_hashing import _sha256_file
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
from molt.cli.wrapper_build import (
    _build_args_has_json_flag,
    _build_args_has_python_version_flag,
    _emit_wrapper_build_failure,
    _emit_wrapper_build_success_signals,
    _parse_wrapper_build_contract_payload,
    _read_wrapper_build_cache_contract,
    _run_wrapper_build,
    _scoped_environ_updates,
    _wrapper_build_cache_input,
    _wrapper_build_cache_manifest_path,
    _wrapper_build_cache_semantic_env,
    _wrapper_build_default_binary_path,
    _wrapper_target_python,
    _write_wrapper_build_cache_manifest,
)
from molt.cli.native_toolchain import (
    _append_darwin_runtime_frameworks,
    _resolve_macos_sdk_root,
    _run_bolt_post_link,
    _zig_target_query,
)
from molt.cli.native_link_deps import (
    _collect_cargo_native_link_deps,
    _crate_name_from_archive_member,
    _crate_name_from_cargo_build_dir,
    _runtime_archive_crate_names,
)
from molt.cli.native_link_command import (
    _resolve_available_fast_linker,
    _resolve_dev_linker,
    _resolve_native_linker_hint,
)
from molt.cli.native_main_stub import (
    _native_main_stub_snippets,
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
from molt.cli.package_distribution import (
    package,
    publish,
    verify,
)
from molt.cli.profile_feedback import (
    _extract_hot_functions,
    _extract_runtime_feedback_hot_functions,
    _load_pgo_profile,
    _load_runtime_feedback,
    _pgo_hotspot_entries,
)
from molt.cli.lockfiles import (
    _LOCK_CHECK_CACHE_VERSION,
    _cargo_lock_manifest_paths,
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
    _require_molt_root,
    _resolve_root_override,
)
from molt.cli.runtime_paths import (
    _RUNTIME_STDLIB_PROFILE_ALIASES,
    _build_state_root,
    _build_state_root_cached,
    _cargo_target_root_cached,
    _molt_session_id,
    _normalize_runtime_stdlib_profile,
    _runtime_lib_archive_name,
    _runtime_lib_archive_names,
    _runtime_cargo_scratch_lib_name,
    _runtime_cargo_scratch_lib_path,
    _runtime_staticlib_target_is_windows,
    _runtime_wasm_artifact_path,
    _runtime_wasm_artifact_path_cached,
    _session_artifact_component,
)
from molt.cli.runtime_fingerprints import (
    _artifact_needs_rebuild,
    _hash_runtime_file,
    _hash_source_tree_metadata,
    _inspect_wasm_binary,
    _is_valid_static_library_artifact,
    _is_valid_wasm_binary,
    _read_runtime_fingerprint,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint,
    _stored_fingerprint_matches_source_metadata,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
    _wasm_runtime_feature_plan,
)
from molt.cli import runtime_build as _runtime_build
from molt.cli.runtime_build import (
    _RUNTIME_LIB_VERIFIED,
    _ensure_native_runtime_lib_ready_before_link,
    _ensure_runtime_lib,
    _ensure_runtime_wasm_artifact,
    _ensure_runtime_lib_ready,
    _initialize_runtime_artifact_state,
    _maybe_start_native_runtime_lib_ready_async,
)
from molt.cli.runtime_intrinsic_symbols import (
    _runtime_intrinsic_symbols_digest,
    _runtime_intrinsic_symbols_file,
    _stage_runtime_intrinsic_symbols_for_native_codegen,
)
from molt.cli.runtime_wasm_validation import (
    _is_reusable_wasm_artifact,
    _is_valid_runtime_wasm_artifact,
    _is_valid_shared_runtime_wasm_artifact,
    _runtime_wasm_exports_satisfy,
    _runtime_wasm_has_shared_import_abi,
    _runtime_wasm_integrity_sidecar_path,
    _runtime_wasm_missing_exports,
    _try_read_wasm_varuint,
    _validate_wasm_structural,
    _wasm_has_nonempty_code_section,
    _write_runtime_wasm_integrity_sidecar,
)
from molt.cli.toolchain_validation import (
    _VALIDATE_PROOF_BYPASS_ENV,
    _VALIDATE_SUITE_CHOICES,
    _build_toolchain_report,
    _canonical_env_defaults,
    _clang_setup_advice,
    _collect_setup_actions,
    _cargo_setup_advice,
    _default_validate_summary_path,
    _detect_llvm_backend_toolchain,
    _ensure_rustup_target,
    _format_validate_guard_summary,
    _is_path_within,
    _llvm_backend_advice,
    _persist_validate_summary,
    _planned_update_steps,
    _planned_validate_steps,
    _python_setup_advice,
    _resolve_validate_summary_path,
    _resolved_env_dir_from_root,
    _rustup_setup_advice,
    _uv_setup_advice,
    _validate_guard_prefix,
    _validate_proof_bypass_errors,
    _validation_guard_summary,
    doctor,
    setup,
    update_repo,
    validate,
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
    _host_target_triple,
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
from molt.cli.extension_scan import extension_scan
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
    _PersistedModuleGraphState,
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
from molt.cli.target_python import (
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
    _analyze_module_schedule,
    _apply_dead_module_elimination,
    _build_frontend_module_costs,
    _build_module_graph_metadata,
    _build_module_lowering_metadata,
    _build_module_source_catalog,
    _build_stdlib_like_module_flags,
    _case_exact_file,
    _collect_import_star_modules,
    _collect_imports,
    _collect_namespace_parents,
    _collect_package_parents,
    _CORE_STDLIB_MODULES_FULL,
    _CORE_STDLIB_MODULES_MICRO,
    _discover_module_graph,
    _discover_module_graph_from_paths,
    _enforce_intrinsic_stdlib,
    _enforce_profile_feature_availability,
    _ensure_core_stdlib_modules,
    _entry_module_root_for_path,
    ENTRY_OVERRIDE_SPAWN,
    _expand_imports_with_static_package_all_star_children,
    _expand_module_chain,
    _expand_module_chain_cached,
    _explicit_imports_reference_generated_importer,
    _extend_module_graph_with_closure,
    _extend_module_graph_with_static_import_modules,
    _has_namespace_dir,
    _import_scan_cache_path,
    _IMPORT_SCAN_CACHE_SCHEMA_VERSION,
    IMPORTER_MODULE_NAME,
    _infer_module_overrides,
    _INTRINSIC_CALL_NAMES,
    _is_fail_closed_import_policy_gate,
    _is_runtime_owned_module_path,
    _is_stdlib_module,
    _is_stdlib_resolved_path,
    _load_module_imports,
    _logical_generated_module_path,
    _looks_like_stdlib_module_name,
    _materialize_import_plan,
    _compute_reachable_modules,
    _dependent_module_closure,
    _module_graph_cache_key,
    _module_graph_cache_path,
    _MODULE_GRAPH_CACHE_SCHEMA_VERSION,
    _module_graph_import_scan_mode,
    _module_graph_needs_runtime_import_support,
    _module_graph_policy_digest,
    _module_init_scan_nodes,
    _module_dependencies,
    _module_dependencies_from_imports,
    _module_dependency_closure,
    _module_dependency_closures,
    _module_dependency_layers,
    _module_order_has_back_edges,
    _module_name_from_path,
    _module_name_from_relative_parts,
    _module_name_from_resolved_path,
    _module_required_intrinsic_names,
    _module_uses_runtime_import_protocol,
    _ModuleResolutionCache,
    _ModuleSourceCatalog,
    _ModuleSourceLease,
    ModuleSyntaxErrorInfo,
    _namespace_paths,
    _parse_static_import_modules,
    _payload_source_matches,
    PLATFORM_EXCLUDED_SUBMODULES,
    _prepare_entry_module_graph,
    _profile_feature_gap_for_module,
    _read_module_source,
    _read_persisted_import_scan,
    _read_persisted_module_graph,
    _record_module_reason,
    _record_new_module_reasons,
    _relative_parts_if_within,
    _requires_spawn_entry_override,
    _resolve_module_path,
    _resolve_module_path_parts,
    _resolve_relative_import,
    _resolve_static_import_module_paths,
    _resolved_module_cache_key,
    _roots_for_module,
    _RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES,
    _RUNTIME_IMPORT_PROTOCOL_MARKERS,
    _RUNTIME_IMPORT_PROTOCOL_TARGETS,
    _RUNTIME_IMPORT_SUPPORT_ROOT_MODULES,
    _runtime_owned_module_roots,
    _source_content_sha256,
    _source_content_sha256_cached,
    _source_may_use_runtime_import_protocol,
    _spec_parent,
    _static_module_all_exports,
    _static_string_sequence,
    _stdlib_allowlist,
    _stdlib_allowlist_cached,
    _stdlib_module_intrinsic_status,
    STDLIB_NESTED_IMPORT_SCAN_MODULES,
    _STDLIB_POLICY_GATE_STATUS,
    _STDLIB_PROBE_INTRINSIC,
    _stdlib_root_path,
    STUB_MODULES,
    STUB_PARENT_MODULES,
    _reverse_module_dependencies,
    _topo_sort_modules,
    _tree_uses_runtime_import_protocol,
    _write_importer_module,
    _write_namespace_module,
    _write_persisted_import_scan,
    _write_persisted_module_graph,
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
from molt.cli.mlir_backend import (
    _find_mlir_backend_binary,
    _run_mlir_backend_pipeline,
)
from molt.cli.native_binary import (
    _NativeBinaryInvalid,
    _assert_native_binary_valid,
    _expected_binary_format_for_target,
    _smoke_probe_native_binary,
    _target_is_host_executable,
    _validate_native_binary_format,
)
from molt.cli.wasm import (
    _build_wasm_sections,
    _collect_wasm_active_table_function_slots,
    _collect_wasm_export_names,
    _collect_wasm_module_import_names,
    _effective_split_worker_table_base,
    _export_wasm_table_refs,
    _infer_wasm_table_base_from_export_names,
    _parse_wasm_sections,
    _read_wasm_data_end,
    _read_wasm_memory_min_bytes,
    _read_wasm_ref_func_expr,
    _read_wasm_table_min,
    _reserved_wasm_runtime_callable_count,
    _skip_wasm_init_expr,
    _generate_split_worker_js,
    _generate_split_wrangler_jsonc,
    _wasm_export_function_signatures,
    _wasm_import_function_result_kinds,
    _wasm_import_function_signatures,
    _wasm_import_minima,
    _write_wasm_string,
    _write_wasm_varuint,
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
    return project_root / "target" / "sessions" / _session_artifact_component(sid)

def _replace_directory_tree_from_source(
    src: Path,
    dst: Path,
    *,
    ignore: Any = None,
) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    backup_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.old")
    try:
        shutil.copytree(src, tmp_path, ignore=ignore)
        had_existing = dst.exists() or dst.is_symlink()
        if had_existing:
            os.replace(dst, backup_path)
        try:
            os.replace(tmp_path, dst)
        except BaseException:
            if had_existing and backup_path.exists() and not dst.exists():
                os.replace(backup_path, dst)
            raise
        if backup_path.exists():
            _remove_file_or_tree(backup_path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(dst.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                _remove_file_or_tree(tmp_path)
        with contextlib.suppress(OSError):
            if backup_path.exists():
                _remove_file_or_tree(backup_path)

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
    cache_setup = _prepare_backend_cache_setup(
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

def _generate_snapshot_header(
    *,
    output_wasm: Path,
    target_profile: str,
    capabilities_list: list[str] | None,
    verbose: bool,
) -> None:
    """Generate a molt.snapshot.json header alongside the WASM output.

    The header captures mount plan, capability manifest, and module hash
    metadata needed by edge hosts to restore a post-init snapshot (Plan D).
    The binary memory blob capture is deferred to the wasmtime host
    integration.
    """
    import hashlib
    import datetime

    snapshot_dir = output_wasm.parent
    snapshot_path = snapshot_dir / "molt.snapshot.json"

    # Compute module hash from the WASM binary.
    module_hash = "sha256:unknown"
    if output_wasm.exists():
        h = hashlib.sha256()
        with open(output_wasm, "rb") as f:
            for chunk in iter(lambda: f.read(65536), b""):
                h.update(chunk)
        module_hash = f"sha256:{h.hexdigest()}"

    # Default mount plan matching the spec Layer 4 snapshot format.
    mount_plan = [
        {"path": "/bundle", "mount_type": "bundle", "hash": module_hash},
        {"path": "/tmp", "mount_type": "tmp", "quota_mb": 32},
        {"path": "/dev", "mount_type": "dev"},
    ]

    caps = (
        list(capabilities_list)
        if capabilities_list
        else [
            "fs.bundle.read",
            "fs.tmp.read",
            "fs.tmp.write",
        ]
    )

    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": target_profile,
        "module_hash": module_hash,
        "mount_plan": mount_plan,
        "capability_manifest": caps,
        "determinism_stamp": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "init_state_size": 0,
    }

    _atomic_write_json(snapshot_path, header, indent=2)
    if verbose:
        print(f"Wrote snapshot header: {snapshot_path}", file=sys.stderr)

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
    stdlib_profile: str | None = "micro",
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
    )
    if prepared_backend_ir_error is not None:
        return prepared_backend_ir_error
    assert prepared_backend_ir is not None
    ir = prepared_backend_ir.ir
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

    prepared_backend_setup, prepared_backend_setup_error = _prepare_backend_setup(
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
    if prepared_backend_setup_error is not None:
        return prepared_backend_setup_error
    assert prepared_backend_setup is not None
    prepared_backend_runtime_context, prepared_backend_runtime_error = (
        _prepare_backend_runtime_context(
            prepared_backend_setup=prepared_backend_setup,
            is_wasm_freestanding=output_layout.is_wasm_freestanding,
            json_output=json_output,
            runtime_cargo_profile=prepared_build_config.runtime_cargo_profile,
            cargo_timeout=prepared_build_config.cargo_timeout,
            molt_root=prepared_build_roots.molt_root,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
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
            resolved_modules=resolved_modules,
            json_output=json_output,
            verbose=verbose,
            target=target,
            entry_module=resolved_build_entry.entry_module,
            prepare_backend_dispatch=_prepare_backend_dispatch,
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
            _prepare_backend_compile(
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
            _prepare_non_native_build_result(
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
            )
        )
        if prepared_non_native_result_error is not None:
            return prepared_non_native_result_error
        assert prepared_non_native_result is not None

        # -- Snapshot header generation (Plan D) ----------------------------
        if snapshot and output_layout.is_wasm:
            _generate_snapshot_header(
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

def _prepare_non_native_build_result(
    *,
    is_rust_transpile: bool,
    is_luau_transpile: bool,
    is_wasm: bool,
    is_wasm_freestanding: bool = False,
    wasm_opt_enabled: bool = True,
    wasm_opt_level: str = "Oz",
    wasm_table_base: int | None = None,
    linked: bool,
    require_linked: bool,
    linked_output_path: Path | None,
    output_artifact: Path,
    json_output: bool,
    runtime_wasm: Path | None,
    runtime_reloc_wasm: Path | None,
    ensure_runtime_wasm_shared: Callable[[set[str] | frozenset[str] | None], bool],
    ensure_runtime_wasm_reloc: Callable[[set[str] | frozenset[str] | None], bool],
    runtime_cargo_profile: str,
    molt_root: Path,
    split_runtime: bool = False,
    precompile: bool = False,
    project_root: Path | None = None,
    profile: BuildProfile = "dev",
    warnings: list[str] | None = None,
) -> tuple[_PreparedNonNativeResult | None, _CliFailure | None]:
    if is_rust_transpile:
        return _PreparedNonNativeResult(
            primary_output=output_artifact,
            consumer_output=output_artifact,
            bundle_root=None,
            linked_output_path=linked_output_path,
            success_messages=[f"Successfully transpiled {output_artifact}"],
            extra_fields={},
            artifacts={"rust": str(output_artifact)},
        ), None
    if is_luau_transpile:
        return _PreparedNonNativeResult(
            primary_output=output_artifact,
            consumer_output=output_artifact,
            bundle_root=None,
            linked_output_path=linked_output_path,
            success_messages=[f"Successfully built {output_artifact}"],
            extra_fields={},
            artifacts={"luau": str(output_artifact)},
        ), None
    if is_wasm:
        output_wasm = output_artifact
        resolved_linked_output = linked_output_path
        bundle_root: Path | None = None
        artifacts: dict[str, str] = {"wasm": str(output_wasm)}
        _split_runtime = split_runtime or os.environ.get("MOLT_SPLIT_RUNTIME") == "1"
        staged_runtime_wasm: Path | None = None
        if linked:
            required_runtime_exports = _collect_wasm_module_import_names(
                output_wasm, "molt_runtime"
            )
            structural_error = _validate_wasm_structural(output_wasm)
            if structural_error is not None:
                return None, _fail(
                    "Generated wasm module failed structural validation before linking: "
                    + structural_error,
                    json_output,
                    command="build",
                )
            if not ensure_runtime_wasm_reloc(required_runtime_exports):
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if runtime_reloc_wasm is None:
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if resolved_linked_output is None:
                resolved_linked_output = output_wasm.with_name("output_linked.wasm")
            if resolved_linked_output.parent != Path("."):
                resolved_linked_output.parent.mkdir(parents=True, exist_ok=True)
            if not is_wasm_freestanding:
                if runtime_wasm is None:
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if not ensure_runtime_wasm_shared(required_runtime_exports):
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if not runtime_wasm.exists():
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
            tool = molt_root / "tools" / "wasm_link.py"
            link_cmd = [
                sys.executable,
                str(tool),
                "--runtime",
                str(runtime_reloc_wasm),
                "--input",
                str(output_wasm),
                "--output",
                str(resolved_linked_output),
            ]
            if _split_runtime:
                split_dir = output_wasm.parent
                link_cmd.extend(
                    [
                        "--split-runtime",
                        "--split-output-dir",
                        str(split_dir),
                    ]
                )
            if is_wasm_freestanding:
                link_cmd.append("--freestanding")
            if wasm_opt_enabled:
                link_cmd.extend(["--optimize", "--optimize-level", wasm_opt_level])
            link_project_root = project_root or molt_root
            link_fingerprint_path = _link_pipeline._link_fingerprint_path(
                link_project_root,
                resolved_linked_output,
                profile,
                "wasm32-wasip1",
            )
            stored_link_fingerprint = _read_runtime_fingerprint(link_fingerprint_path)
            link_fingerprint = _link_pipeline._link_fingerprint(
                project_root=link_project_root,
                inputs=[output_wasm, runtime_reloc_wasm, tool],
                link_cmd=link_cmd,
                stored_fingerprint=stored_link_fingerprint,
            )
            link_skipped = not _artifact_needs_rebuild(
                resolved_linked_output,
                link_fingerprint,
                stored_link_fingerprint,
            )
            if link_skipped and _split_runtime:
                split_dir = output_wasm.parent
                app_wasm = split_dir / "app.wasm"
                rt_wasm = split_dir / "molt_runtime.wasm"
                link_skipped = _is_reusable_wasm_artifact(
                    app_wasm
                ) and _is_reusable_wasm_artifact(rt_wasm)
            if link_skipped:
                link_process = subprocess.CompletedProcess(link_cmd, 0, "", "")
            else:
                linked_tmp_output: Path | None = None
                link_run_cmd = list(link_cmd)
                if not _split_runtime:
                    linked_tmp_output = resolved_linked_output.with_name(
                        f".{resolved_linked_output.name}."
                        f"{os.getpid()}.{uuid.uuid4().hex}.tmp"
                    )
                    output_arg_index = link_run_cmd.index("--output") + 1
                    link_run_cmd[output_arg_index] = str(linked_tmp_output)
                try:
                    link_process = _run_completed_command(
                        link_run_cmd,
                        cwd=molt_root,
                        env=None,
                        capture_output=True,
                        memory_guard_prefix="MOLT_WASM_LINK",
                    )
                    if link_process.returncode != 0:
                        err = link_process.stderr.strip() or link_process.stdout.strip()
                        msg = "Wasm link failed"
                        if err:
                            msg = f"{msg}: {err}"
                        return None, _fail(msg, json_output, command="build")
                    if linked_tmp_output is not None:
                        if not _is_reusable_wasm_artifact(linked_tmp_output):
                            return None, _fail(
                                f"Wasm link produced invalid artifact: {linked_tmp_output}",
                                json_output,
                                command="build",
                            )
                        os.replace(linked_tmp_output, resolved_linked_output)
                        if os.name == "posix":
                            with contextlib.suppress(OSError):
                                dir_fd = os.open(
                                    resolved_linked_output.parent,
                                    os.O_RDONLY,
                                )
                                try:
                                    os.fsync(dir_fd)
                                finally:
                                    os.close(dir_fd)
                finally:
                    if linked_tmp_output is not None:
                        with contextlib.suppress(OSError):
                            if linked_tmp_output.exists():
                                linked_tmp_output.unlink()
                link_fingerprint_warning = _write_link_fingerprint_if_needed(
                    link_skipped=False,
                    link_fingerprint=link_fingerprint,
                    link_fingerprint_path=link_fingerprint_path,
                    json_output=json_output,
                )
                if link_fingerprint_warning is not None:
                    if warnings is not None:
                        warnings.append(link_fingerprint_warning)
                    if not json_output:
                        print(f"Warning: {link_fingerprint_warning}", file=sys.stderr)
            if require_linked and resolved_linked_output is not None:
                if output_wasm != resolved_linked_output and output_wasm.exists():
                    try:
                        output_wasm.unlink()
                    except OSError as exc:
                        return None, _fail(
                            f"Failed to remove unlinked wasm: {exc}",
                            json_output,
                            command="build",
                        )
        if not is_wasm_freestanding and not _split_runtime and not linked:
            if runtime_wasm is None:
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            required_runtime_exports = _collect_wasm_module_import_names(
                output_wasm, "molt_runtime"
            )
            if not ensure_runtime_wasm_shared(required_runtime_exports):
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if not runtime_wasm.exists():
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            staged_runtime_wasm = output_wasm.with_name("molt_runtime.wasm")
            if staged_runtime_wasm != runtime_wasm:
                try:
                    _atomic_copy_file(runtime_wasm, staged_runtime_wasm)
                except OSError as exc:
                    return None, _fail(
                        f"Failed to stage runtime wasm: {exc}",
                        json_output,
                        command="build",
                    )
            artifacts["runtime_wasm"] = str(staged_runtime_wasm)
        if resolved_linked_output is not None:
            artifacts["linked_wasm"] = str(resolved_linked_output)
        # -- Precompile step: produce .cwasm for faster startup -----------
        cwasm_path: Path | None = None
        if precompile:
            precompile_target = (
                resolved_linked_output
                if resolved_linked_output is not None
                else output_wasm
            )
            cwasm_path = precompile_target.with_suffix(".cwasm")
            wasmtime_bin = shutil.which("wasmtime")
            if wasmtime_bin:
                precompile_proc = _run_completed_command(
                    [
                        wasmtime_bin,
                        "compile",
                        str(precompile_target),
                        "-o",
                        str(cwasm_path),
                    ],
                    cwd=molt_root,
                    env=None,
                    capture_output=True,
                    memory_guard_prefix="MOLT_WASM_LINK",
                    timeout=60,
                )
                if precompile_proc.returncode == 0:
                    print(f"Precompiled to {cwasm_path}", file=sys.stderr)
                else:
                    print(
                        f"Precompilation failed (non-fatal): {precompile_proc.stderr.strip()}",
                        file=sys.stderr,
                    )
                    cwasm_path = None
            else:
                print("wasmtime not found; skipping precompilation", file=sys.stderr)
                cwasm_path = None
        # -- End precompile step -------------------------------------------
        if cwasm_path is not None:
            artifacts["cwasm"] = str(cwasm_path)

        primary_output = output_wasm
        if require_linked and resolved_linked_output is not None:
            primary_output = resolved_linked_output
        consumer_output = resolved_linked_output or primary_output
        success_messages = (
            [f"Successfully built {primary_output}"]
            if require_linked
            else [f"Successfully built {output_wasm}"]
        )
        if resolved_linked_output is not None and not require_linked:
            success_messages.append(f"Successfully linked {resolved_linked_output}")
        if cwasm_path is not None:
            success_messages.append(f"Precompiled {cwasm_path}")

        # --split-runtime: wasm_link.py produces app.wasm + molt_runtime.wasm;
        # generate manifest.json and worker.js shim here.
        if _split_runtime and runtime_reloc_wasm is not None:
            split_dir = output_wasm.parent

            app_wasm = split_dir / "app.wasm"
            rt_wasm = split_dir / "molt_runtime.wasm"
            manifest = split_dir / "manifest.json"

            if not app_wasm.exists() or not rt_wasm.exists():
                return None, _fail(
                    "Split-runtime link did not produce expected artifacts "
                    f"(app.wasm={app_wasm.exists()}, molt_runtime.wasm={rt_wasm.exists()})",
                    json_output,
                    command="build",
                )

            app_size = app_wasm.stat().st_size
            rt_size = rt_wasm.stat().st_size
            app_memory_min, app_table_min = _wasm_import_minima(app_wasm)
            rt_memory_min, rt_table_min = _wasm_import_minima(rt_wasm)
            app_runtime_import_result_kinds = _wasm_import_function_result_kinds(
                app_wasm, module_name="molt_runtime"
            )
            app_runtime_import_signatures = _wasm_import_function_signatures(
                app_wasm, module_name="molt_runtime"
            )
            shared_memory_initial_pages = max(
                app_memory_min or 0,
                rt_memory_min or 0,
            )
            shared_table_initial = max(
                app_table_min or 0,
                rt_table_min or 0,
                8192,
            )
            app_table_ref_signatures = _wasm_export_function_signatures(
                app_wasm, export_name_prefix="__molt_table_ref_"
            )
            runtime_table_ref_signatures = _wasm_export_function_signatures(
                rt_wasm, export_name_prefix="__molt_table_ref_"
            )
            effective_wasm_table_base = _effective_split_worker_table_base(
                wasm_table_base=wasm_table_base,
                runtime_table_min=rt_table_min,
                app_table_ref_signatures=app_table_ref_signatures,
            )

            manifest_data = {
                "version": 2,
                "mode": "split-runtime",
                "tree_shaken": True,
                "shared_memory_initial_pages": shared_memory_initial_pages,
                "shared_table_initial": shared_table_initial,
                "wasm_table_base": effective_wasm_table_base,
                "abi": {
                    "runtime_imports": {
                        "module": "molt_runtime",
                        "names": sorted(app_runtime_import_signatures),
                        "signatures": app_runtime_import_signatures,
                        "result_kinds": app_runtime_import_result_kinds,
                    },
                    "table_refs": {
                        "app": app_table_ref_signatures,
                        "runtime": runtime_table_ref_signatures,
                    },
                },
                "modules": {
                    "runtime": {
                        "path": "molt_runtime.wasm",
                        "size": rt_size,
                    },
                    "app": {
                        "path": "app.wasm",
                        "size": app_size,
                    },
                },
                "total_size": app_size + rt_size,
                "instantiation_order": ["runtime", "app"],
                "entry": {"module": "app", "function": "molt_main"},
            }
            _atomic_write_json(manifest, manifest_data, indent=2)

            # Generate split-runtime Cloudflare Workers shim with full
            # WASI support and multi-module instantiation.
            worker_js = split_dir / "worker.js"
            _atomic_write_text(
                worker_js,
                _generate_split_worker_js(
                    shared_memory_initial_pages=shared_memory_initial_pages,
                    shared_table_initial=shared_table_initial,
                    shared_table_base=effective_wasm_table_base,
                    runtime_import_result_kinds=app_runtime_import_result_kinds,
                    runtime_import_signatures=app_runtime_import_signatures,
                    app_table_ref_signatures=app_table_ref_signatures,
                    runtime_table_ref_signatures=runtime_table_ref_signatures,
                ),
            )
            vfs_support_src = molt_root / "wasm" / "molt_vfs_browser.js"
            vfs_support_dst = split_dir / "molt_vfs_browser.js"
            try:
                _atomic_copy_file(vfs_support_src, vfs_support_dst)
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage split-runtime VFS support: {exc}",
                    json_output,
                    command="build",
                )

            # Generate wrangler.jsonc for Cloudflare Workers deployment.
            # JSONC is the modern Wrangler config shape and matches the
            # live-verification tooling contract.
            wrangler_jsonc = split_dir / "wrangler.jsonc"
            _atomic_write_text(
                wrangler_jsonc,
                _generate_split_wrangler_jsonc(dt.date.today().isoformat()),
            )
            legacy_wrangler_toml = split_dir / "wrangler.toml"
            if legacy_wrangler_toml.exists():
                legacy_wrangler_toml.unlink()
            bundle_root = split_dir
            artifacts.update(
                {
                    "app_wasm": str(app_wasm),
                    "runtime_wasm": str(rt_wasm),
                    "manifest": str(manifest),
                    "worker_js": str(worker_js),
                    "wrangler_config": str(wrangler_jsonc),
                }
            )

            # Cloudflare Workers isolate memory limit: 128MB.
            # Warn if the combined WASM size exceeds a safe threshold.
            combined_mb = (app_size + rt_size) / (1024 * 1024)
            if combined_mb > 100:
                success_messages.append(
                    f"WARNING: Combined WASM size ({combined_mb:.1f}MB) approaches "
                    f"Cloudflare Workers 128MB isolate memory limit. "
                    f"Consider enabling --stdlib-profile micro for smaller builds."
                )
            success_messages.append(
                f"Split runtime: {app_wasm.name} ({app_size // 1024}KB) "
                f"+ {rt_wasm.name} ({rt_size // 1024}KB)"
            )

        return _PreparedNonNativeResult(
            primary_output=primary_output,
            consumer_output=consumer_output,
            bundle_root=bundle_root,
            linked_output_path=resolved_linked_output,
            success_messages=success_messages,
            extra_fields={
                "linked": linked,
                "require_linked": require_linked,
                **(
                    {"linked_output": str(resolved_linked_output)}
                    if resolved_linked_output is not None
                    else {}
                ),
                **({"cwasm_output": str(cwasm_path)} if cwasm_path is not None else {}),
            },
            artifacts=artifacts,
        ), None
    return _PreparedNonNativeResult(
        primary_output=output_artifact,
        consumer_output=output_artifact,
        bundle_root=None,
        linked_output_path=linked_output_path,
        success_messages=[f"Successfully built {output_artifact}"],
        extra_fields={},
        artifacts={"object": str(output_artifact)},
    ), None

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
    stdlib_profile: str | None = "micro",
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
    native_artifact_plan = prepared_frontend_pipeline_bundle[21]
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
            artifacts_root,
            _native_artifact_plan,
        ) = prepared_frontend_pipeline_bundle
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
        )
        if prepared_backend_ir_error is not None:
            return prepared_backend_ir_error
        assert prepared_backend_ir is not None
        return _run_mlir_backend_pipeline(
            ir=prepared_backend_ir.ir,
            output_artifact=output_layout.output_artifact,
            project_root=prepared_build_roots.project_root,
            json_output=json_output,
            verbose=verbose,
        )

    return _run_backend_pipeline(
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

def _build_cache_variant(
    *,
    profile: str,
    runtime_cargo: str,
    backend_cargo: str,
    emit: str,
    stdlib_split: bool,
    codegen_env: str,
    linked: bool,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = "micro",
    partition_mode: bool = False,
    backend_binary_identity: str = "",
    external_static_packages_digest: str = "",
    runtime_intrinsic_symbols_digest: str = "",
    capability_config_digest: str = "",
) -> str:
    """Build a cache variant key from build configuration.

    Changes to any parameter produce a different variant, ensuring cache
    entries for different build configurations never collide.

    ``stdlib_profile`` (micro vs full) MUST be part of the variant: the two
    profiles compile the molt-runtime hub with different Cargo features
    (``stdlib_micro`` + ``no-default-features`` vs ``stdlib_full`` +
    ``default-features``) and the frontend lowers the entry differently
    (e.g. ``_inject_sys_init`` only under full). Two builds whose reachable
    stdlib IR happens to be identical would otherwise collide on the same
    ``stdlib_shared.o`` (and main backend object), so a micro build could
    silently reuse a full build's object and vice versa — a stale cache hit
    that yields the wrong runtime surface or a duplicate/missing-symbol link.

    ``backend_binary_identity`` MUST be part of the variant: it is the stat-based
    identity (path + mtime + size) of the backend binary that will compile these
    objects (see ``_backend_binary_identity``). The variant flows into every
    ``.o`` cache key (stdlib-shared, module, per-function), so binding it here
    makes the cache key change whenever the backend binary changes — closing the
    Finding #4 (design 20 §4.1) confound where a rebuilt backend with different
    codegen silently linked stale objects compiled by the prior binary. The
    backend *source-tree* fingerprint (``_cache_fingerprint``) does not catch
    this when source mtimes are reset by git/worktree ops or when two
    same-source builds produce different binaries.

    ``runtime_intrinsic_symbols_digest`` MUST be part of native binary cache
    identity because the app object embeds the per-app intrinsic resolver. The
    resolver's relocation set is computed against the linked runtime staticlib's
    exact `molt_*` symbol authority; a stale app object emitted against a
    different set can either miss required intrinsics or reference absent ones.
    """
    parts = [
        f"profile={profile}",
        f"runtime_cargo={runtime_cargo}",
        f"backend_cargo={backend_cargo}",
        f"emit={emit}",
        f"stdlib_split={int(stdlib_split)}",
        f"stdlib_profile={_normalize_runtime_stdlib_profile(stdlib_profile)}",
        f"codegen_env={codegen_env}",
        f"target_python={target_python.tag}",
    ]
    if linked:
        parts.append("linked=1")
    if partition_mode:
        parts.append("partitioned=v1")
    if backend_binary_identity:
        parts.append(f"backend_bin={backend_binary_identity}")
    if external_static_packages_digest:
        parts.append(f"external_static_packages={external_static_packages_digest}")
    if runtime_intrinsic_symbols_digest:
        parts.append(f"runtime_intrinsics={runtime_intrinsic_symbols_digest}")
    if capability_config_digest:
        parts.append(f"capability_config={capability_config_digest}")
    return ";".join(parts)

def _prepare_backend_cache_setup(
    *,
    cache_enabled: bool,
    ir: Mapping[str, Any],
    target: str,
    target_triple: str | None,
    profile: str,
    runtime_cargo_profile: str,
    backend_cargo_profile: str,
    emit_mode: str,
    is_wasm: bool,
    linked: bool,
    project_root: Path,
    cache_dir: str | None,
    output_artifact: Path,
    warnings: list[str],
    entry_module: str,
    module_graph_metadata: _ModuleGraphMetadata,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = "micro",
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
    runtime_intrinsic_symbols_digest: str = "",
    capabilities_list: Sequence[str] | None = None,
    capability_profiles: Sequence[str] | None = None,
    manifest_env_vars: Mapping[str, str] | None = None,
    capability_config_digest: str | None = None,
) -> _BackendCacheSetup:
    split_stdlib_object = _native_stdlib_object_split_enabled(
        target=target,
        emit_mode=emit_mode,
    )
    stdlib_module_symbols = _stdlib_module_symbols(module_graph_metadata)
    stdlib_module_symbols_json = (
        _encode_stdlib_module_symbols(stdlib_module_symbols)
        if split_stdlib_object
        else None
    )
    # Bind the cache key to the backend binary the daemon will run, so a rebuilt
    # backend with different codegen never silently reuses .o objects compiled by
    # the prior binary (Finding #4, design 20 §4.1). Resolve the binary path via
    # the same feature mapping the build dispatch uses, so the stamped identity
    # matches the actual daemon executable for this target/profile.
    backend_bin = _backend_bin_path(
        project_root,
        backend_cargo_profile,
        _backend_features_for_build_target(target=target, is_wasm=is_wasm),
    )
    backend_binary_identity = _backend_binary_identity(backend_bin)
    if capability_config_digest is None:
        capability_config_digest = _build_inputs._capability_config_cache_digest(
            capabilities_list=capabilities_list,
            capability_profiles=capability_profiles,
            manifest_env_vars=manifest_env_vars,
        )
    cache_variant = _build_cache_variant(
        profile=profile,
        runtime_cargo=runtime_cargo_profile,
        backend_cargo=backend_cargo_profile,
        emit=emit_mode,
        stdlib_split=split_stdlib_object,
        codegen_env=_backend_codegen_env_digest(is_wasm=is_wasm),
        linked=linked,
        target_python=target_python,
        stdlib_profile=stdlib_profile,
        backend_binary_identity=backend_binary_identity,
        external_static_packages_digest=native_artifact_plan.digest(),
        runtime_intrinsic_symbols_digest=runtime_intrinsic_symbols_digest,
        capability_config_digest=capability_config_digest,
    )
    if not cache_enabled:
        # Even with cache disabled, compute stdlib_object_path so the
        # daemon can partition stdlib functions into stdlib_shared.o and
        # the linker can resolve them.  Without this, the daemon strips
        # stdlib functions but the linker never sees stdlib_shared.o.
        _nocache_stdlib_path = None
        _nocache_stdlib_key = None
        _nocache_stdlib_manifest = None
        if split_stdlib_object:
            _nocache_stdlib_key = _shared_stdlib_cache_key(
                ir,
                entry_module=entry_module,
                stdlib_module_symbols=stdlib_module_symbols,
                target_triple=target_triple,
                cache_variant=cache_variant,
            )
            _nocache_stdlib_manifest = _shared_stdlib_manifest(
                cache_key=_nocache_stdlib_key,
                cache_variant=cache_variant,
                target_triple=target_triple,
            )
            _nocache_cache_root = _resolve_cache_root(project_root, cache_dir)
            try:
                _nocache_cache_root.mkdir(parents=True, exist_ok=True)
            except OSError:
                pass
            _nocache_stub_path = _nocache_cache_root / "__nocache__.o"
            _nocache_stdlib_path = _stdlib_object_cache_path(
                _nocache_stub_path, _nocache_stdlib_key
            )
            if _nocache_stdlib_path is not None:
                _validate_shared_stdlib_cache_contract(
                    _nocache_stdlib_path,
                    project_root,
                    _nocache_stdlib_key,
                    expected_manifest=_nocache_stdlib_manifest,
                    target_triple=target_triple,
                    stdlib_module_symbols=stdlib_module_symbols,
                )
        return _BackendCacheSetup(
            cache_enabled=False,
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            function_cache_path=None,
            stdlib_object_path=_nocache_stdlib_path,
            stdlib_object_cache_key=_nocache_stdlib_key,
            stdlib_object_manifest=_nocache_stdlib_manifest,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
            stdlib_module_symbols_json=stdlib_module_symbols_json,
            stdlib_module_symbols=frozenset(stdlib_module_symbols),
        )
    module_cache_payload_ir = _cache_ir_payload_ir(ir)
    backend_cache_payload_ir = _cache_backend_payload_ir(ir)
    cache_key = _cache_key(
        ir,
        target,
        target_triple,
        cache_variant,
        payload_ir=module_cache_payload_ir,
    )
    function_cache_key = _function_cache_key(
        ir,
        target,
        target_triple,
        cache_variant,
        payload_ir=backend_cache_payload_ir,
    )
    cache_root = _resolve_cache_root(project_root, cache_dir)
    try:
        cache_root.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        warnings.append(f"Cache disabled: {exc}")
        return _BackendCacheSetup(
            cache_enabled=False,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            cache_path=None,
            function_cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            stdlib_object_manifest=None,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
            stdlib_module_symbols_json=stdlib_module_symbols_json,
            stdlib_module_symbols=frozenset(stdlib_module_symbols),
        )
    stdlib_object_path = None
    stdlib_object_cache_key = None
    stdlib_object_manifest = None
    if split_stdlib_object:
        stdlib_object_cache_key = _shared_stdlib_cache_key(
            ir,
            entry_module=entry_module,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            cache_variant=cache_variant,
        )
        stdlib_object_manifest = _shared_stdlib_manifest(
            cache_key=stdlib_object_cache_key,
            cache_variant=cache_variant,
            target_triple=target_triple,
        )
    ext = "wasm" if is_wasm else "o"
    cache_path = _backend_cache_artifact_path(
        cache_root,
        cache_key,
        ext=ext,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    function_cache_path = None
    if function_cache_key and function_cache_key != cache_key:
        function_cache_path = _backend_cache_artifact_path(
            cache_root,
            function_cache_key,
            ext=ext,
            stdlib_object_cache_key=stdlib_object_cache_key,
            is_wasm=is_wasm,
        )
    if split_stdlib_object and stdlib_object_cache_key is not None:
        assert cache_path is not None
        stdlib_object_path = _stdlib_object_cache_path(
            cache_path, stdlib_object_cache_key
        )
        if stdlib_object_path is not None:
            _validate_shared_stdlib_cache_contract(
                stdlib_object_path,
                project_root,
                stdlib_object_cache_key,
                expected_manifest=stdlib_object_manifest,
                target_triple=target_triple,
                stdlib_module_symbols=stdlib_module_symbols,
            )
    cache_candidates: list[tuple[str, Path]] = []
    if cache_path is not None:
        cache_candidates.append(("module", cache_path))
    if function_cache_path is not None and function_cache_path != cache_path:
        cache_candidates.append(("function", function_cache_path))
    cache_hit, cache_hit_tier = _try_cached_backend_candidates(
        project_root=project_root,
        cache_candidates=cache_candidates,
        output_artifact=output_artifact,
        is_wasm=is_wasm,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        cache_path=cache_path,
        stdlib_object_path=stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        warnings=warnings,
    )
    return _BackendCacheSetup(
        cache_enabled=True,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        stdlib_object_path=stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        cache_candidates=tuple(cache_candidates),
        cache_hit=cache_hit,
        cache_hit_tier=cache_hit_tier,
        stdlib_module_symbols_json=stdlib_module_symbols_json,
        stdlib_module_symbols=frozenset(stdlib_module_symbols),
    )
