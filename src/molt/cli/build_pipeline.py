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
from molt.cli import backend_cache_setup as _backend_cache_setup
from molt.cli import backend_compile as _backend_compile
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

    prepared_backend_setup, prepared_backend_setup_error = _backend_compile._prepare_backend_setup(
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
        _backend_compile._prepare_backend_runtime_context(
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
