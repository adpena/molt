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
    wasm_runtime_required_import_names,
)
from molt.debug import DebugSubcommand
from molt.dx import DxConfigError, DxProject
from molt.frontend import SimpleTIRGenerator
from molt.cli.completion import _completion_script
from molt.cli import backend_ir as _backend_ir
from molt.cli import build_inputs as _build_inputs
from molt.cli import build_pipeline as _build_pipeline
from molt.cli import commands as _commands
from molt.cli import debug_helpers as _debug_helpers
from molt.cli import frontend_pipeline as _frontend_pipeline
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
    _backend_source_paths,
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
    _shared_stdlib_cache_matches_key_locked,
    _shared_stdlib_cache_mismatch_detail,
    _shared_stdlib_cache_payload_ir,
    _shared_stdlib_compiler_fingerprint,
    _shared_stdlib_manifest,
    _shared_stdlib_native_symbol_closure_issue,
    _shared_stdlib_publish_lock_path,
    _stage_backend_output_and_caches,
    _stage_shared_stdlib_object_for_link,
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
    _DEFAULT_BACKEND_FEATURES,
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
    _cargo_build_env,
    _maybe_enable_native_cpu,
    _maybe_enable_sccache,
    _run_cargo_with_sccache_retry,
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
    _codesign_binary,
    _resolve_macos_sdk_root,
    _run_bolt_post_link,
    _zig_target_query,
)
from molt.cli.native_link_deps import (
    _collect_cargo_native_link_deps,
    _crate_name_from_archive_member,
    _crate_name_from_cargo_build_dir,
    _native_target_is_windows,
    _runtime_archive_crate_names,
)
from molt.cli.native_link_command import (
    _build_native_link_command,
    _build_native_link_driver_command,
    _resolve_available_fast_linker,
    _resolve_dev_linker,
    _resolve_native_linker_hint,
    _windows_coff_library_command,
)
from molt.cli.native_main_stub import (
    _native_main_stub_snippets,
    _render_native_main_stub,
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
    _session_artifact_component,
)
from molt.cli.runtime_fingerprints import (
    _artifact_content_looks_valid,
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
    _llvm_sys_prefix_env_var,
    _persist_validate_summary,
    _planned_update_steps,
    _planned_validate_steps,
    _python_setup_advice,
    _required_llvm_backend_major,
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
    _darwin_binary_imports_validation_error,
    _darwin_binary_magic_error,
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
    wasm_profile: str = "full",
    snapshot: bool = False,
    stdlib_profile: str | None = "micro",
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
    fact_graph_request: _factgraph.FactGraphRequest | None = None,
) -> int:
    if isinstance(profile, bool):
        profile = "release"
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    env_updates: dict[str, str] = {}
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
    # --wasm-profile: pass the effective profile to the backend explicitly.
    # The backend defaults to WasmProfile::Auto when the env var is absent,
    # so omitting the "full" case silently changes semantics.
    if target in {"wasm", "wasm-freestanding"} and wasm_profile:
        env_updates["MOLT_WASM_PROFILE"] = wasm_profile
    # --stdlib-profile: propagate to module graph construction so that the
    # micro profile can exclude heavy core modules from the dependency closure.
    if stdlib_profile:
        env_updates["MOLT_STDLIB_PROFILE"] = stdlib_profile
    with _scoped_environ_updates(env_updates):
        if file_path and module:
            return _fail(
                "Use a file path or --module, not both.", json_output, command="build"
            )
        if not file_path and not module:
            return _fail("Missing entry file or module.", json_output, command="build")
        prepared_build_inputs, prepared_build_inputs_error = _build_inputs._prepare_build_inputs(
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
            respect_pythonpath=respect_pythonpath,
            lib_paths=lib_paths or [],
            python_version=python_version,
            build_config=build_config,
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


def main() -> int:
    _ensure_cli_hash_seed()
    from molt import __version__

    parser = argparse.ArgumentParser(
        prog="molt",
        usage="molt [-h] [--version] <command> [options]",
        description="The Molt Python compiler",
        formatter_class=_MoltHelpFormatter,
        epilog=(
            "Run 'molt <command> --help' for more information on a command.\n"
            "\n"
            "Examples:\n"
            "  molt build app.py                  Build a Python program\n"
            "  molt run app.py                    Build and run\n"
            "  molt run app.py --release          Build optimized and run\n"
            "  molt build app.py --target wasm    Build for WebAssembly\n"
            "  molt deploy cloudflare app.py      Deploy to Cloudflare Workers\n"
            "  molt test                          Run test suites\n"
        ),
    )
    parser.add_argument("--version", action="version", version=f"molt {__version__}")
    # Don't require command — show help when no args (like `go` with no args).
    subparsers = parser.add_subparsers(dest="command", title="commands")

    build_parser = subparsers.add_parser(
        "build",
        help="Build a Python program",
        description="Compile a Python file to a native binary, WASM module, Luau script, or MLIR text.",
        formatter_class=_BuildHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt build app.py                      Build with default settings\n"
            "  molt build app.py --release             Optimized release build\n"
            "  molt build app.py --target wasm         Build for WebAssembly\n"
            "  molt build app.py --target luau         Build for Luau/Roblox\n"
            "  molt build app.py --target mlir         Emit MLIR text (requires LLVM 22)\n"
            "  molt build --module mypackage           Build a package by module name\n"
        ),
    )
    build_parser.add_argument("file", nargs="?", help="Path to Python source")
    build_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    build_parser.add_argument(
        "--target",
        default=None,
        help=(
            "Build target: native (default), wasm, luau, mlir, or a target triple "
            "(e.g., aarch64-unknown-linux-gnu, x86_64-unknown-linux-musl)."
        ),
    )
    build_parser.add_argument(
        "--release",
        action="store_true",
        default=False,
        help="Optimized release build (alias for --build-profile release).",
    )
    build_parser.add_argument(
        "--codec",
        choices=["msgpack", "cbor", "json"],
        default=None,
        help="Structured codec for parse calls (default from config or msgpack).",
    )
    build_parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default=None,
        help="Apply type annotations to guide lowering and specialization.",
    )
    build_parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default=None,
        help="Fallback policy for unsupported constructs.",
    )
    build_parser.add_argument(
        "--type-facts",
        help="Path to type facts JSON from `molt check`.",
    )
    build_parser.add_argument(
        "--python-version",
        default=None,
        help=(
            "Target Python semantics (3.12, 3.13, or 3.14). Defaults from "
            "[tool.molt.build] or project.requires-python."
        ),
    )
    build_parser.add_argument(
        "--pgo-profile",
        help="Path to a Molt profile artifact (molt_profile.json) for PGO hints.",
    )
    build_parser.add_argument(
        "--pgo-collect",
        action="store_true",
        default=False,
        help=(
            "Instrument the compiled binary to collect PGO counters at runtime. "
            "The instrumented binary writes branch counts, call counts, and loop "
            "iteration counts to a profile JSON file on exit."
        ),
    )
    build_parser.add_argument(
        "--pgo-collect-output",
        help=(
            "Output path for the PGO collection profile (default: "
            "molt_pgo_collected.json in the project root). Only used with --pgo-collect."
        ),
    )
    build_parser.add_argument(
        "--runtime-feedback",
        help=(
            "Path to a Molt runtime feedback artifact "
            "(molt_runtime_feedback.json) for measured hot-function promotion hints."
        ),
    )
    build_parser.add_argument(
        "--output",
        help=(
            "Output path for the native binary or wasm artifact "
            "(relative to --out-dir when set, otherwise the project root for explicit paths; "
            "default final artifacts land under dist/ when omitted). "
            "If the path is a directory (or ends with a path separator), "
            "the default filename is used within that directory."
        ),
    )
    build_parser.add_argument(
        "--out-dir",
        help=(
            "Output directory for final artifacts (binary/wasm/object). "
            "Intermediates stay under MOLT_HOME/build/<entry> by default. "
            "Native binaries otherwise default to MOLT_BIN."
        ),
    )
    build_parser.add_argument(
        "--sysroot",
        help=(
            "Sysroot path for native linking (relative paths resolve under the project "
            "root; defaults to MOLT_SYSROOT or MOLT_CROSS_SYSROOT when set)."
        ),
    )
    build_parser.add_argument(
        "--emit",
        choices=["bin", "obj", "wasm"],
        default=None,
        help="Select which artifact to emit (native: bin/obj, wasm: wasm).",
    )
    build_parser.add_argument(
        "--linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a linked wasm artifact (output_linked.wasm) alongside output.wasm.",
    )
    build_parser.add_argument(
        "--linked-output",
        help=(
            "Output path for the linked wasm artifact "
            "(relative to --out-dir when set, otherwise the project root for explicit paths; "
            "the default linked artifact lands under dist/ when omitted)."
        ),
    )
    build_parser.add_argument(
        "--require-linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require linked wasm output for wasm targets (fails if linking is unavailable).",
    )
    build_parser.add_argument(
        "--wasm-opt-level",
        choices=["Oz", "O3"],
        default="Oz",
        help=(
            "WASM optimization profile: Oz for size-focused (default, "
            "recommended for browser deployment), O3 for speed-focused "
            "(recommended for server/edge deployment)."
        ),
    )
    build_parser.add_argument(
        "--precompile",
        action="store_true",
        default=False,
        help=(
            "After linking, run wasmtime compile to produce a precompiled "
            ".cwasm artifact for 10-50x faster startup in production."
        ),
    )
    build_parser.add_argument(
        "--snapshot",
        action="store_true",
        default=False,
        help=(
            "Generate a molt.snapshot.json header alongside the WASM output "
            "for sub-millisecond cold starts on edge platforms. "
            "Records mount plan, capabilities, and module hash metadata."
        ),
    )
    build_parser.add_argument(
        "--split-runtime",
        action="store_true",
        default=False,
        help=(
            "Produce separate runtime and app WASM modules instead of a single "
            "linked binary. The runtime is tree-shaken to include only the "
            "builtins and runtime exports your program uses, then both split "
            "artifacts are deforested with post-link cleanup and wasm-opt. "
            "Outputs app.wasm + molt_runtime.wasm + worker.js + manifest.json."
        ),
    )
    build_parser.add_argument(
        "--wasm-profile",
        choices=["full", "pure"],
        default="full",
        help=(
            "WASM import profile: full (default) registers all host imports; "
            "pure omits IO/ASYNC/TIME imports for minimal pure-computation modules."
        ),
    )
    build_parser.add_argument(
        "--stdlib-profile",
        choices=["full", "micro"],
        default=None,
        help="Runtime stdlib profile (full=all modules, micro=core only for smallest binary)",
    )
    build_parser.add_argument(
        "--emit-ir",
        help="Write the lowered IR JSON to a file path.",
    )
    build_parser.add_argument(
        "--build-profile",
        choices=_BUILD_PROFILE_CHOICES,
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    build_parser.add_argument(
        "--backend",
        choices=["cranelift", "llvm", "auto"],
        default="auto",
        help="Compilation backend (auto=cranelift; llvm is opt-in and requires an LLVM toolchain).",
    )
    build_parser.add_argument(
        "--profile",
        choices=_BUILD_OR_DEPLOY_PROFILE_CHOICES,
        default=None,
        help=(
            "Build profile (dev/release) or legacy deployment platform/profile "
            "(cloudflare/browser/wasi/fastly)."
        ),
    )
    build_parser.add_argument(
        "--platform",
        choices=_DEPLOY_PROFILE_CHOICES,
        default=None,
        help="Deployment platform/profile (sets optimization defaults for the target platform).",
    )
    build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    build_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    build_parser.add_argument(
        "--portable",
        action="store_true",
        default=False,
        help="Use baseline ISA (no host-specific CPU features). Ensures cross-machine reproducible codegen at ~5-15%% runtime cost.",
    )
    build_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments (native only).",
    )
    build_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Enable build cache under MOLT_CACHE (defaults to the OS cache).",
    )
    build_parser.add_argument(
        "--cache-dir",
        help="Override the build cache directory (default: MOLT_CACHE).",
    )
    build_parser.add_argument(
        "--cache-report",
        action="store_true",
        help="Print cache hit/miss details even without --verbose.",
    )
    build_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable the build cache (alias for --no-cache).",
    )
    build_parser.add_argument(
        "--respect-pythonpath",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Include PYTHONPATH entries as module roots during compilation.",
    )
    build_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    build_parser.add_argument(
        "--capability-manifest",
        help="Path to a capability manifest file (toml/json/yaml) for build-time configuration.",
    )
    build_parser.add_argument(
        "--require-signed-manifest",
        action="store_true",
        default=False,
        help="Reject unsigned capability manifests. Requires --capability-manifest.",
    )
    build_parser.add_argument(
        "--audit-log",
        metavar="SINK:OUTPUT",
        help="Enable audit logging (e.g., 'jsonl:stderr', 'stderr:stderr')",
    )
    build_parser.add_argument(
        "--io-mode",
        choices=["real", "virtual", "callback"],
        default=None,
        help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
    )
    build_parser.add_argument(
        "--type-gate",
        action="store_true",
        default=False,
        help="Reject compilation if capability-touching code paths contain untyped variables",
    )
    build_parser.add_argument(
        "--diagnostics",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Enable compile diagnostics payloads (phase timings, module reasons, "
            "frontend/midend summaries)."
        ),
    )
    build_parser.add_argument(
        "--diagnostics-file",
        help=(
            "Optional path for compile diagnostics JSON (relative paths resolve "
            "under the build artifacts root). Implies --diagnostics."
        ),
    )
    build_parser.add_argument(
        "--diagnostics-verbosity",
        choices=["summary", "default", "full"],
        default=None,
        help=(
            "Select stderr build diagnostics detail level. "
            "JSON/file diagnostics remain complete."
        ),
    )
    build_parser.add_argument(
        "--lib-path",
        action="append",
        default=[],
        help="Additional directories to search for Python packages (repeatable).",
    )
    build_parser.add_argument(
        "--bolt",
        action="store_true",
        default=False,
        help=(
            "Run BOLT post-link optimization on the output binary. "
            "Instruments, profiles with a training run, and reorders "
            "functions/basic blocks for optimal icache utilization. "
            "Requires llvm-bolt (brew install llvm / apt install llvm-bolt). "
            "Native targets only."
        ),
    )
    build_parser.add_argument(
        "--bolt-training-cmd",
        default=None,
        help=(
            "Custom training command for BOLT profiling (default: run the "
            "output binary with no arguments). Only used with --bolt."
        ),
    )
    build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    _factgraph.add_factgraph_parser(
        subparsers,
        formatter_class=_BuildHelpFormatter,
        build_profile_choices=_BUILD_PROFILE_CHOICES,
    )

    extension_parser = subparsers.add_parser(
        "extension",
        help="Build and audit C extensions compiled against libmolt.",
    )
    extension_subparsers = extension_parser.add_subparsers(
        dest="extension_command", required=True
    )
    extension_build_parser = extension_subparsers.add_parser(
        "build",
        help="Compile a C extension against libmolt and emit a wheel + sidecar.",
    )
    extension_build_parser.add_argument(
        "--project",
        help="Project directory containing pyproject.toml (default: cwd).",
    )
    extension_build_parser.add_argument(
        "--out-dir",
        help="Output directory for wheel + extension_manifest.json (default: dist/).",
    )
    extension_build_parser.add_argument(
        "--molt-abi",
        help=(
            "Molt C-API ABI version override "
            "(default: tool.molt.extension.molt_c_api_version or MOLT_C_API_VERSION)."
        ),
    )
    extension_build_parser.add_argument(
        "--target",
        help="Target triple for extension build (default: native host target).",
    )
    extension_build_parser.add_argument(
        "--capabilities",
        help=(
            "Capabilities allowlist/profiles override "
            "(default: tool.molt.extension.capabilities)."
        ),
    )
    extension_build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic lockfile and reproducible wheel checks.",
    )
    extension_build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_audit_parser = extension_subparsers.add_parser(
        "audit",
        help="Audit an extension manifest and wheel for ABI/capability compatibility.",
    )
    extension_audit_parser.add_argument(
        "--path",
        required=True,
        help="Path to a wheel, extension_manifest.json, or directory containing it.",
    )
    extension_audit_parser.add_argument(
        "--require-capabilities",
        action="store_true",
        help="Fail when the manifest capability list is empty.",
    )
    extension_audit_parser.add_argument(
        "--require-abi",
        help="Require an exact molt_c_api_version match.",
    )
    extension_audit_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Require wheel and extension checksums in the manifest.",
    )
    extension_audit_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_audit_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_scan_parser = extension_subparsers.add_parser(
        "scan",
        help=(
            "Scan extension sources for unsupported Py* C-API usage "
            "against include/molt/Python.h."
        ),
    )
    extension_scan_parser.add_argument(
        "--project",
        help="Project directory containing pyproject.toml (default: cwd).",
    )
    extension_scan_parser.add_argument(
        "--source",
        action="append",
        help=(
            "Source path to scan (repeatable). If omitted, uses "
            "tool.molt.extension.sources from pyproject.toml."
        ),
    )
    extension_scan_parser.add_argument(
        "--fail-on-missing",
        action="store_true",
        help="Return non-zero if unsupported Py* C-API symbols are detected.",
    )
    extension_scan_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_scan_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    internal_batch_parser = subparsers.add_parser(
        "internal-batch-build-server",
        help=argparse.SUPPRESS,
    )
    internal_batch_parser.add_argument(
        "--json", action="store_true", help=argparse.SUPPRESS
    )
    internal_batch_parser.add_argument(
        "--verbose", action="store_true", help=argparse.SUPPRESS
    )

    debug_parser = subparsers.add_parser(
        "debug",
        help="Inspect and retain canonical compiler debug artifacts.",
    )
    debug_subparsers = debug_parser.add_subparsers(
        dest="debug_subcommand",
        title="debug commands",
        required=True,
    )
    for debug_subcommand in DebugSubcommand:
        subparser = debug_subparsers.add_parser(
            debug_subcommand.value,
            help=f"Run canonical `{debug_subcommand.value}` debug flow.",
        )
        _add_debug_shared_selector_args(subparser)
        if debug_subcommand == DebugSubcommand.IR:
            subparser.add_argument("source", help="Python source file to compile.")
            subparser.add_argument(
                "--stage",
                choices=["pre-midend", "post-midend", "all"],
                default="all",
                help="Which compilation stage(s) to dump.",
            )
        if debug_subcommand == DebugSubcommand.REPRO:
            subparser.add_argument(
                "source", help="Python source file to execute as a repro."
            )
        if debug_subcommand == DebugSubcommand.TRACE:
            subparser.add_argument("source", help="Python source file to trace.")
        if debug_subcommand == DebugSubcommand.TRACE:
            subparser.add_argument(
                "--family",
                action="append",
                help=(
                    "Trace family to enable. Repeat for multiple families; "
                    "defaults to all supported trace families."
                ),
            )
            subparser.add_argument(
                "--rebuild",
                action="store_true",
                help="Force a no-cache rebuild before executing the traced repro.",
            )
            subparser.add_argument(
                "--assert-no-pending-on-success",
                action="store_true",
                help="Enable the success-path pending-exception trap during traced execution.",
            )
        if debug_subcommand == DebugSubcommand.REPRO:
            subparser.add_argument(
                "--compare",
                action="store_true",
                help="Compare the repro against CPython instead of only running Molt.",
            )
            subparser.add_argument(
                "--python",
                help="Python executable used for compare mode.",
            )
            subparser.add_argument(
                "--rebuild",
                action="store_true",
                help="Force a no-cache rebuild before executing the repro.",
            )
        if debug_subcommand in {
            DebugSubcommand.REDUCE,
            DebugSubcommand.BISECT,
        }:
            subparser.add_argument(
                "input_path",
                help="Source or prior manifest path to inspect.",
            )
            subparser.add_argument(
                "--oracle-json",
                help="Canonical reduction/bisection oracle as a JSON object.",
            )
            subparser.add_argument(
                "--oracle-file",
                help="Path to a JSON file containing the canonical oracle.",
            )
            subparser.add_argument(
                "--eval-command",
                help=(
                    "Command executed for each candidate. It receives context via "
                    "MOLT_DEBUG_EVAL_* environment variables and may emit JSON on stdout."
                ),
            )
            subparser.add_argument(
                "--eval-timeout",
                type=int,
                default=30,
                help="Per-candidate evaluator timeout in seconds.",
            )
        if debug_subcommand == DebugSubcommand.BISECT:
            subparser.add_argument(
                "--passes",
                help="Comma-separated pass list for first-bad-pass bisection.",
            )
            subparser.add_argument(
                "--baseline-json",
                help="Baseline backend/profile/IC configuration as JSON.",
            )
            subparser.add_argument(
                "--failing-json",
                help="Known failing backend/profile/IC configuration as JSON.",
            )
        if debug_subcommand == DebugSubcommand.VERIFY:
            subparser.add_argument(
                "--require-probe-execution",
                action="store_true",
                help="Require required differential probes to have executed successfully.",
            )
            subparser.add_argument(
                "--probe-rss-metrics",
                help="Path to rss_metrics.jsonl from differential runs.",
            )
            subparser.add_argument(
                "--probe-run-id",
                help="Optional differential run_id to validate for probe execution.",
            )
            subparser.add_argument(
                "--failure-queue",
                help="Path to the differential failure queue file.",
            )
        if debug_subcommand == DebugSubcommand.DIFF:
            subparser.add_argument(
                "summary_path",
                help="Path to a diff summary.json artifact to inspect.",
            )
            subparser.add_argument(
                "--failure-queue",
                help="Optional path to the diff failure queue file.",
            )
        if debug_subcommand == DebugSubcommand.PERF:
            subparser.add_argument(
                "files",
                nargs="+",
                help="Profile JSON/log files containing runtime feedback.",
            )

    check_parser = subparsers.add_parser(
        "check",
        help="Type-check without compiling",
        description=(
            "Analyze a Python file or package and emit type facts without compiling.\n"
            "Type facts can be fed into `molt build --type-facts` for guided specialization."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt check src/app.py                  Type-check a file\n"
            "  molt check src/                        Type-check a package directory\n"
            "  molt check src/app.py --strict         Emit strict-tier type facts\n"
            "  molt check src/app.py --output facts.json\n"
            "                                         Write facts to a custom path\n"
        ),
    )
    check_parser.add_argument("path", help="Python file or package directory")
    check_parser.add_argument(
        "--output",
        default="type_facts.json",
        help="Output path for type facts JSON.",
    )
    check_parser.add_argument(
        "--strict",
        action="store_true",
        help="Mark facts as trusted (strict tier).",
    )
    check_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    check_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    check_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    check_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    run_parser = subparsers.add_parser(
        "run",
        help="Build and run a Python program",
        description=(
            "Compile a Python file with Molt and execute it.\n"
            "Supports native, WASM (via wasmtime), and Luau (via lune) targets."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt run app.py                       Build and run natively\n"
            "  molt run app.py --release              Optimized build and run\n"
            "  molt run app.py --target wasm          Build and run with wasmtime\n"
            "  molt run app.py --target luau          Build and run with lune\n"
            "  molt run app.py --target mlir          Build and JIT via MLIR\n"
            "  molt run app.py -- --arg1 val          Pass args to your script\n"
        ),
    )
    run_parser.add_argument("file", nargs="?", help="Path to Python source")
    run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    run_parser.add_argument(
        "--target",
        default=None,
        help=(
            "Build target: native (default), wasm (build + run with wasmtime), "
            "luau (build + run with lune), mlir (build + JIT via MLIR), "
            "or a target triple."
        ),
    )
    run_parser.add_argument(
        "--release",
        action="store_true",
        default=False,
        help="Optimized release build (alias for --build-profile release).",
    )
    run_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build`.",
    )
    run_parser.add_argument(
        "--python-version",
        default=None,
        help=("Target Python semantics for the build side (3.12, 3.13, or 3.14)."),
    )
    run_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile passed to `molt build` (default: dev).",
    )
    run_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for `molt build`.",
    )
    run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary (compile + run).",
    )
    run_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    run_parser.add_argument(
        "--capability-manifest",
        help="Path to a capability manifest file (toml/json/yaml) for runtime configuration.",
    )
    run_parser.add_argument(
        "--require-signed-manifest",
        action="store_true",
        default=False,
        help="Reject unsigned capability manifests. Requires --capability-manifest.",
    )
    run_parser.add_argument(
        "--audit-log",
        metavar="SINK:OUTPUT",
        help="Enable audit logging (e.g., 'jsonl:stderr', 'stderr:stderr')",
    )
    run_parser.add_argument(
        "--io-mode",
        choices=["real", "virtual", "callback"],
        default=None,
        help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
    )
    run_parser.add_argument(
        "--type-gate",
        action="store_true",
        default=False,
        help="Reject compilation if capability-touching code paths contain untyped variables",
    )
    run_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    run_parser.add_argument(
        "--backend",
        choices=["cranelift", "llvm", "auto"],
        default=None,
        help="Compilation backend passed to `molt build` (auto=cranelift; llvm is opt-in and requires an LLVM toolchain).",
    )
    run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    repl_parser = subparsers.add_parser(
        "repl",
        help="Start the guarded Molt REPL",
        description=(
            "Start an interactive Molt REPL. Each submitted snippet is compiled "
            "and executed through the shared adaptive memory guard."
        ),
    )
    repl_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    repl_parser.add_argument(
        "--io-mode",
        choices=["real", "virtual", "callback"],
        default="real",
        help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
    )
    repl_parser.add_argument(
        "--molt-cmd",
        help=(
            "Override the Molt command used for snippet execution. Defaults to "
            "the current Python interpreter running `-m molt.cli`."
        ),
    )
    repl_parser.add_argument(
        "--timeout-sec",
        type=float,
        default=None,
        help="Per-snippet timeout in seconds (default: MOLT_REPL_TIMEOUT_SEC or 30).",
    )

    compare_parser = subparsers.add_parser(
        "compare",
        help="Compare CPython vs Molt output",
        description="Build and run a Python file with both CPython and Molt, then compare output.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt compare app.py                    Compare output side by side\n"
            "  molt compare app.py --python 3.13      Compare against Python 3.13\n"
            "  molt compare app.py -- --flag           Pass args to both interpreters\n"
        ),
    )
    compare_parser.add_argument("file", nargs="?", help="Path to Python source")
    compare_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    compare_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build` for the Molt side.",
    )
    compare_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile passed to `molt build` (default: dev).",
    )
    compare_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for the Molt build.",
    )
    compare_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    compare_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    compare_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    compare_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    compare_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    parity_run_parser = subparsers.add_parser(
        "parity-run", help="Run the entrypoint with CPython (no Molt compilation)"
    )
    parity_run_parser.add_argument("file", nargs="?", help="Path to Python source")
    parity_run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    parity_run_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    parity_run_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    parity_run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary for the CPython run.",
    )
    parity_run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    parity_run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    parity_run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    test_parser = subparsers.add_parser(
        "test",
        help="Discover and run tests",
        description=(
            "Discover and run test suites.\n"
            "Supports Molt's built-in dev suite, CPython differential tests, and pytest."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt test                             Run the default dev test suite\n"
            "  molt test --suite diff                Run differential tests against CPython\n"
            "  molt test --suite pytest              Run tests with pytest\n"
            "  molt test tests/test_math.py          Run a specific test file\n"
            "  molt test --suite diff --profile release\n"
            "                                        Diff tests with release builds\n"
        ),
    )
    test_parser.add_argument(
        "--suite",
        choices=["dev", "diff", "pytest"],
        default="dev",
        help="Test suite to run.",
    )
    test_parser.add_argument(
        "--python-version",
        help="Python version for diff suite (e.g. 3.13).",
    )
    test_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for Molt builds in suite=diff (default: dev).",
    )
    test_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    test_parser.add_argument("path", nargs="?", help="Optional test path.")
    test_parser.add_argument(
        "pytest_args",
        nargs=argparse.REMAINDER,
        help="Extra pytest args when --suite pytest (use -- to separate).",
    )
    test_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    test_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    diff_parser = subparsers.add_parser(
        "diff",
        help="Run differential tests against CPython",
        description="Run differential tests that compare Molt output against CPython.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt diff                              Run all diff tests\n"
            "  molt diff tests/parity/               Run diff tests in a directory\n"
            "  molt diff --python-version 3.13        Test against Python 3.13\n"
        ),
    )
    diff_parser.add_argument("path", nargs="?", help="File or directory to test.")
    diff_parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)."
    )
    diff_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for Molt builds in the diff harness (default: dev).",
    )
    diff_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    diff_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    diff_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    bench_parser = subparsers.add_parser(
        "bench",
        help="Run benchmarks",
        description=(
            "Run performance benchmarks.\n"
            "Uses the native bench harness by default, or the WASM harness with --wasm."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt bench                             Run all benchmarks\n"
            "  molt bench --wasm                      Run WASM benchmarks\n"
            "  molt bench --script bench/fib.py       Benchmark a custom script\n"
            "  molt bench -- --filter sort             Pass args to bench tool\n"
        ),
    )
    bench_parser.add_argument(
        "--wasm", action="store_true", help="Use the WASM bench harness."
    )
    bench_parser.add_argument(
        "--script",
        action="append",
        dest="bench_script",
        default=[],
        help="Benchmark a custom script path (repeatable).",
    )
    bench_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    bench_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    bench_parser.add_argument(
        "bench_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the bench tool (use -- to separate).",
    )

    profile_parser = subparsers.add_parser(
        "profile",
        help="Profile benchmarks",
        description="Profile Molt benchmarks with detailed performance instrumentation.",
    )
    profile_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    profile_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    profile_parser.add_argument(
        "profile_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the profile tool (use -- to separate).",
    )

    lint_parser = subparsers.add_parser(
        "lint",
        help="Run linting checks",
        description="Run Molt-specific linting checks on the project.",
    )
    lint_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    lint_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    setup_parser = subparsers.add_parser(
        "setup",
        help="Prepare the host toolchain and canonical Molt environment",
        description=(
            "Report and remediate the toolchains, environment variables, and\n"
            "backend readiness required for Molt development and release work."
        ),
    )
    setup_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing required setup items.",
    )
    setup_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    setup_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    doctor_parser = subparsers.add_parser(
        "doctor",
        help="Check toolchain setup",
        description=(
            "Verify that the Molt toolchain is installed and configured correctly.\n"
            "Checks for Rust/Cargo, wasm-opt, wasmtime, and other dependencies."
        ),
    )
    doctor_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing requirements.",
    )
    doctor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    doctor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    update_parser = subparsers.add_parser(
        "update",
        help="Refresh toolchains and dependency state",
        description=(
            "Refresh repo-level toolchains and dependency state.\n"
            "By default this updates rustup-managed toolchains plus Cargo/uv lockfiles.\n"
            "Use --all to also upgrade Rust dependency requirements in Cargo.toml."
        ),
    )
    update_parser.add_argument(
        "--all",
        action="store_true",
        help="Include manifest requirement upgrades (may be breaking).",
    )
    update_parser.add_argument(
        "--toolchains",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Refresh rustup-managed toolchains and wasm targets (default: enabled).",
    )
    update_parser.add_argument(
        "--locks",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Refresh Cargo.lock and uv.lock (default: enabled).",
    )
    update_parser.add_argument(
        "--manifests",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Upgrade Rust dependency requirements in Cargo.toml files.",
    )
    update_parser.add_argument(
        "--check",
        action="store_true",
        help="Print the planned update steps without executing them.",
    )
    update_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    update_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    validate_parser = subparsers.add_parser(
        "validate",
        help="Run the canonical end-to-end local validation matrix",
        description=(
            "Run the release-readiness matrix across CLI smoke, backend parity,\n"
            "conformance, and benchmark lanes."
        ),
    )
    validate_parser.add_argument(
        "--suite",
        choices=_VALIDATE_SUITE_CHOICES,
        default="full",
        help="Validation scope (default: full).",
    )
    validate_parser.add_argument(
        "--backend",
        choices=["all", "native", "llvm", "wasm", "luau"],
        default="all",
        help="Restrict validation to one backend family.",
    )
    validate_parser.add_argument(
        "--profile",
        choices=["all", "dev", "release"],
        default="all",
        help="Restrict validation to one build profile where applicable.",
    )
    validate_parser.add_argument(
        "--check",
        action="store_true",
        help="Print the validation plan without executing it.",
    )
    validate_parser.add_argument(
        "--summary-out",
        help=(
            "Write the validation JSON summary to this path. Executed runs default "
            "to logs/validate-<suite>-<backend>-<profile>.json; check-only runs "
            "write only when this option is provided."
        ),
    )
    validate_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    validate_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    package_parser = subparsers.add_parser(
        "package", help="Bundle a distributable package"
    )
    package_parser.add_argument("artifact", help="Path to the package artifact.")
    package_parser.add_argument(
        "manifest",
        help=(
            "Path to manifest JSON (fields per "
            "docs/spec/areas/compat/contracts/package_abi_contract.md)."
        ),
    )
    package_parser.add_argument(
        "--output",
        help="Output .moltpkg path (default dist/<name>-<version>-<target>.moltpkg).",
    )
    package_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic package metadata.",
    )
    package_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    package_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    package_parser.add_argument(
        "--sbom",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a CycloneDX SBOM sidecar (default: enabled).",
    )
    package_parser.add_argument(
        "--sbom-output",
        help="Override the SBOM output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sbom-format",
        choices=["cyclonedx", "spdx"],
        default="cyclonedx",
        help="SBOM format to emit (default: cyclonedx).",
    )
    package_parser.add_argument(
        "--signature",
        help="Path to a signature file to attach and record in metadata.",
    )
    package_parser.add_argument(
        "--signature-output",
        help="Override the signature sidecar output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sign",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Sign the artifact with cosign or codesign.",
    )
    package_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the signing tool (default: auto).",
    )
    package_parser.add_argument(
        "--signing-key",
        help="Signing key path for cosign (or set COSIGN_KEY).",
    )
    package_parser.add_argument(
        "--signing-identity",
        help="Signing identity for codesign (or set MOLT_CODESIGN_IDENTITY).",
    )
    package_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    package_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    publish_parser = subparsers.add_parser("publish", help="Publish to registry")
    publish_parser.add_argument("package", help="Path to the .moltpkg file.")
    publish_parser.add_argument(
        "--registry",
        default="dist/registry",
        help="Registry directory, file path, or HTTP(S) URL.",
    )
    publish_parser.add_argument(
        "--registry-token",
        help=(
            "Bearer token for remote registry auth (or MOLT_REGISTRY_TOKEN; "
            "prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-user",
        help="Username for basic auth (or MOLT_REGISTRY_USER).",
    )
    publish_parser.add_argument(
        "--registry-password",
        help=(
            "Password for basic auth (or MOLT_REGISTRY_PASSWORD; prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-timeout",
        type=float,
        help="Registry request timeout in seconds (or MOLT_REGISTRY_TIMEOUT).",
    )
    publish_parser.add_argument(
        "--dry-run", action="store_true", help="Print the publish plan only."
    )
    publish_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package determinism before publishing.",
    )
    publish_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    publish_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    publish_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature when publishing.",
    )
    publish_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when publishing.",
    )
    publish_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    publish_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    publish_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    publish_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    publish_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    verify_parser = subparsers.add_parser(
        "verify", help="Verify a package manifest and checksum"
    )
    verify_parser.add_argument(
        "--package",
        help="Path to the .moltpkg archive (alternative to --manifest/--artifact).",
    )
    verify_parser.add_argument("--manifest", help="Manifest JSON path.")
    verify_parser.add_argument("--artifact", help="Artifact path.")
    verify_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Fail when checksum is missing.",
    )
    verify_parser.add_argument(
        "--extension-metadata",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Treat manifest as extension metadata and enforce extension ABI/wheel "
            "checks (default: auto-detect from manifest keys)."
        ),
    )
    verify_parser.add_argument(
        "--require-extension-capabilities",
        action="store_true",
        help="Fail when extension manifest capability list is empty.",
    )
    verify_parser.add_argument(
        "--require-extension-abi",
        help="Require an exact extension molt_c_api_version match.",
    )
    verify_parser.add_argument(
        "--require-deterministic",
        action="store_true",
        help="Fail when manifest is not deterministic.",
    )
    verify_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature.",
    )
    verify_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when present.",
    )
    verify_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    verify_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    verify_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    verify_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    verify_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    verify_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    deps_parser = subparsers.add_parser("deps", help="Show dependency info")
    deps_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    deps_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deps_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    vendor_parser = subparsers.add_parser(
        "vendor", help="Vendor pure-Python dependencies"
    )
    vendor_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    vendor_parser.add_argument(
        "--output",
        help="Output directory for vendored artifacts (default: vendor).",
    )
    vendor_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show vendoring plan without downloading artifacts.",
    )
    vendor_parser.add_argument(
        "--allow-non-tier-a",
        action="store_true",
        help="Proceed even if non-Tier A dependencies are present.",
    )
    vendor_parser.add_argument(
        "--extras",
        action="append",
        help="Extras to include from project optional-dependencies.",
    )
    vendor_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    vendor_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    vendor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    vendor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    install_parser = subparsers.add_parser(
        "install",
        help="Install packages into .molt-venv/ using UV",
        description=(
            "Manage third-party Python packages with UV.\n\n"
            "Without arguments, syncs dependencies from pyproject.toml and\n"
            "requirements.txt into the .molt-venv/ virtual environment.\n"
            "Installed packages are automatically available to `molt build`\n"
            "and `molt run`.\n\n"
            "Use `molt install add <pkg>` to install a package AND persist it\n"
            "to pyproject.toml."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt install                        Sync deps from pyproject.toml\n"
            "  molt install requests flask          Install specific packages\n"
            "  molt install -r requirements.txt     Install from requirements file\n"
            "  molt install add requests            Add and persist a dependency\n"
        ),
    )
    install_parser.add_argument(
        "packages",
        nargs="*",
        default=[],
        help=(
            "Package(s) to install (e.g. requests, 'flask>=2.0'), "
            "or 'add <pkg>...' to add and persist to pyproject.toml."
        ),
    )
    install_parser.add_argument(
        "-r",
        "--requirements",
        help="Path to a requirements.txt file.",
    )
    install_parser.add_argument(
        "--sync",
        action="store_true",
        help="Sync venv to match pyproject.toml + requirements.txt.",
    )
    install_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    install_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    clean_parser = subparsers.add_parser(
        "clean",
        help="Dry-run or apply canonical ignored artifact/cache cleanup",
    )
    clean_parser.add_argument(
        "--apply",
        action="store_true",
        help="Delete ignored artifacts. Default is a dry run.",
    )
    clean_parser.add_argument(
        "--kill-processes",
        action="store_true",
        help="Run the repo process sentinel before cleanup.",
    )
    clean_parser.add_argument(
        "--extra-path",
        action="append",
        default=[],
        help="Additional repo-relative git-clean pathspec. Still removes ignored files only.",
    )
    clean_parser.add_argument(
        "--list-paths",
        action="store_true",
        help="Print canonical cleanup pathspecs and exit.",
    )
    clean_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    clean_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    config_parser = subparsers.add_parser("config", help="Show/set configuration")
    config_parser.add_argument(
        "--file",
        help="Resolve project root from a source file path.",
    )
    config_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    config_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    completion_parser = subparsers.add_parser(
        "completion", help="Generate shell completions"
    )
    completion_parser.add_argument(
        "--shell",
        choices=["bash", "zsh", "fish"],
        default="bash",
        help="Shell type to emit.",
    )
    completion_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    completion_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    # --- deploy command ---
    deploy_parser = subparsers.add_parser(
        "deploy",
        help="Build and deploy to a platform",
        description=(
            "Build and deploy a Python program to a target platform.\n"
            "Automatically sets the correct build target and optimization defaults."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt deploy cloudflare src/app.py      Deploy to Cloudflare Workers\n"
            "  molt deploy roblox src/game.py          Deploy to Roblox Studio\n"
            "  molt deploy cloudflare app.py --release  Optimized production deploy\n"
            "  molt deploy roblox app.py --roblox-project ./my-game\n"
            "                                          Deploy and copy to Roblox project\n"
            "  molt deploy cloudflare app.py --dry-run  Build only, skip wrangler\n"
            "\n"
            "Platforms:\n"
            "  cloudflare    Build as WASM with --split-runtime, deploy via wrangler\n"
            "  roblox        Build as Luau, optionally copy to a Roblox project dir\n"
        ),
    )
    deploy_parser.add_argument(
        "platform",
        choices=["cloudflare", "roblox"],
        help="Deployment target: cloudflare (WASM Workers) or roblox (Luau).",
    )
    deploy_parser.add_argument("file", nargs="?", help="Path to Python source")
    deploy_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    deploy_parser.add_argument(
        "--release",
        action="store_true",
        default=False,
        help="Optimized release build (alias for --build-profile release).",
    )
    deploy_parser.add_argument(
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    deploy_parser.add_argument(
        "--output",
        help="Output path for the build artifact.",
    )
    deploy_parser.add_argument(
        "--out-dir",
        help="Output directory for build artifacts.",
    )
    deploy_parser.add_argument(
        "--roblox-project",
        help="Path to the Roblox project directory to copy Luau output into.",
    )
    deploy_parser.add_argument(
        "--wrangler-args",
        default="",
        help="Extra arguments passed to wrangler deploy (cloudflare only).",
    )
    deploy_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Build only; do not run wrangler deploy or copy to project.",
    )
    deploy_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build`.",
    )
    deploy_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deploy_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    # --- harness command ---
    harness_parser = subparsers.add_parser(
        "harness",
        help="Run the Molt quality harness",
        description="Run layered quality checks (compile, lint, test, fuzz, etc.).",
    )
    harness_parser.add_argument(
        "profile",
        nargs="?",
        default="standard",
        choices=["quick", "standard", "deep"],
        help="Test profile (default: standard).",
    )
    harness_parser.add_argument(
        "--no-fail-fast",
        action="store_true",
        help="Continue running layers after a failure.",
    )
    harness_parser.add_argument(
        "--json", action="store_true", help="Print JSON report to stdout."
    )
    harness_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    args = parser.parse_args()
    if args.command is None:
        parser.print_help()
        sys.exit(0)

    config_root = _find_project_root(Path.cwd())
    if getattr(args, "file", None):
        try:
            config_root = _find_project_root(Path(args.file).resolve())
        except OSError:
            config_root = _find_project_root(Path.cwd())
    config = _build_inputs._load_molt_config(config_root)
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    cfg_capabilities = _resolve_capabilities_config(config)

    if args.command == "internal-batch-build-server":
        return _commands._internal_batch_build_server(
            json_output=args.json,
            verbose=args.verbose,
            build_fn=build,
        )

    if args.command == "debug":
        return _debug_helpers._handle_debug_command(args)

    if args.command == "build":
        target = args.target or build_cfg.get("target") or "native"
        codec = args.codec or build_cfg.get("codec") or "msgpack"
        type_hints = args.type_hints or build_cfg.get("type_hints") or "check"
        fallback = args.fallback or build_cfg.get("fallback") or "error"
        output = args.output or build_cfg.get("output")
        out_dir = args.out_dir or build_cfg.get("out_dir") or build_cfg.get("out-dir")
        sysroot = (
            args.sysroot
            or build_cfg.get("sysroot")
            or build_cfg.get("sysroot_path")
            or build_cfg.get("sysroot-path")
        )
        emit = args.emit or build_cfg.get("emit")
        emit_ir = args.emit_ir or build_cfg.get("emit_ir") or build_cfg.get("emit-ir")
        pgo_profile = (
            args.pgo_profile
            or build_cfg.get("pgo_profile")
            or build_cfg.get("pgo-profile")
        )
        runtime_feedback = (
            args.runtime_feedback
            or build_cfg.get("runtime_feedback")
            or build_cfg.get("runtime-feedback")
        )
        profile_arg = getattr(args, "profile", None)
        platform_arg = getattr(args, "platform", None)
        cli_profile_build_profile: str | None = None
        deploy_profile: str | None = None
        if profile_arg in _BUILD_PROFILE_CHOICES:
            cli_profile_build_profile = profile_arg
        elif profile_arg in _DEPLOY_PROFILE_DEFAULTS:
            deploy_profile = profile_arg
        elif profile_arg is not None:
            return _fail(
                f"Invalid build profile or platform profile: {profile_arg}",
                args.json,
                command="build",
            )
        if platform_arg is not None:
            if deploy_profile is not None and deploy_profile != platform_arg:
                return _fail(
                    "Conflicting deployment profiles: "
                    f"--profile {deploy_profile} and --platform {platform_arg}",
                    args.json,
                    command="build",
                )
            deploy_profile = platform_arg
        if (
            cli_profile_build_profile is not None
            and args.build_profile is not None
            and cli_profile_build_profile != args.build_profile
        ):
            return _fail(
                "Conflicting build profiles: "
                f"--profile {cli_profile_build_profile} and "
                f"--build-profile {args.build_profile}",
                args.json,
                command="build",
            )
        cli_build_profile = args.build_profile or cli_profile_build_profile
        if (
            getattr(args, "release", False)
            and cli_build_profile is not None
            and cli_build_profile != "release"
        ):
            return _fail(
                f"Conflicting build profiles: --release and {cli_build_profile}",
                args.json,
                command="build",
            )
        build_profile = (
            ("release" if getattr(args, "release", False) else None)
            or cli_build_profile
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "release"
        )
        backend_choice = getattr(args, "backend", "auto") or "auto"
        linked_output_path = (
            args.linked_output
            or build_cfg.get("linked_output")
            or build_cfg.get("linked-output")
        )
        require_linked = args.require_linked
        if require_linked is None:
            require_linked = _coerce_bool(
                build_cfg.get("require_linked") or build_cfg.get("require-linked"),
                False,
            )
        type_facts = args.type_facts or build_cfg.get("type_facts")
        deterministic = (
            args.deterministic
            if args.deterministic is not None
            else _coerce_bool(build_cfg.get("deterministic"), True)
        )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(build_cfg.get("trusted"), False)
        linked = args.linked
        if linked is None:
            linked = _coerce_bool(build_cfg.get("linked"), False)
        cache = (
            args.cache
            if args.cache is not None
            else _coerce_bool(build_cfg.get("cache"), True)
        )
        if args.rebuild:
            cache = False
        cache_dir = (
            args.cache_dir or build_cfg.get("cache_dir") or build_cfg.get("cache-dir")
        )
        cache_report = args.cache_report or _coerce_bool(
            build_cfg.get("cache_report") or build_cfg.get("cache-report"), False
        )
        respect_pythonpath = args.respect_pythonpath
        if respect_pythonpath is None:
            respect_pythonpath = _coerce_bool(
                build_cfg.get("respect_pythonpath")
                or build_cfg.get("respect-pythonpath"),
                False,
            )
        diagnostics = args.diagnostics
        if diagnostics is None:
            diagnostics_cfg = build_cfg.get("diagnostics")
            if diagnostics_cfg is None:
                diagnostics_cfg = build_cfg.get("build_diagnostics")
            if diagnostics_cfg is None:
                diagnostics_cfg = build_cfg.get("build-diagnostics")
            if diagnostics_cfg is not None:
                diagnostics = _coerce_bool(diagnostics_cfg, False)
        diagnostics_file_raw = (
            args.diagnostics_file
            or build_cfg.get("diagnostics_file")
            or build_cfg.get("diagnostics-file")
            or build_cfg.get("build_diagnostics_file")
            or build_cfg.get("build-diagnostics-file")
        )
        diagnostics_file = (
            diagnostics_file_raw.strip()
            if isinstance(diagnostics_file_raw, str)
            else None
        )
        if diagnostics_file == "":
            diagnostics_file = None
        diagnostics_verbosity = (
            args.diagnostics_verbosity
            or build_cfg.get("diagnostics_verbosity")
            or build_cfg.get("diagnostics-verbosity")
            or build_cfg.get("build_diagnostics_verbosity")
            or build_cfg.get("build-diagnostics-verbosity")
        )
        capabilities = (
            args.capabilities or build_cfg.get("capabilities") or cfg_capabilities
        )
        cfg_lib_paths = build_cfg.get("lib_paths") or build_cfg.get("lib-paths") or []
        if isinstance(cfg_lib_paths, str):
            cfg_lib_paths = [cfg_lib_paths]
        lib_paths: list[str] = list(args.lib_path) + list(cfg_lib_paths)
        if args.file and args.module:
            return _fail(
                "Use a file path or --module, not both.", args.json, command="build"
            )
        if not args.file and not args.module:
            return _fail("Missing entry file or module.", args.json, command="build")

        wasm_opt_level_raw = getattr(args, "wasm_opt_level", "Oz")
        wasm_opt_level = (
            wasm_opt_level_raw if isinstance(wasm_opt_level_raw, str) else "Oz"
        )
        precompile = bool(getattr(args, "precompile", False))
        wasm_profile_raw = getattr(args, "wasm_profile", "full")
        wasm_profile = wasm_profile_raw if isinstance(wasm_profile_raw, str) else "full"
        stdlib_profile_raw = getattr(args, "stdlib_profile", None)
        stdlib_profile = (
            stdlib_profile_raw if isinstance(stdlib_profile_raw, str) else None
        )
        # When `--stdlib-profile` is not given on the command line, honor the
        # `MOLT_STDLIB_PROFILE` environment variable as the single canonical
        # source of truth. The module-graph construction reads this env var
        # directly (`_ensure_core_stdlib_modules`, the core-module closure), so
        # the runtime-staticlib build profile MUST be derived from the same
        # signal — otherwise the two diverge: an env-only `full` request pulls
        # full-profile modules (e.g. `hashlib`) into the dependency closure
        # while the staticlib is still built `micro`, leaving the full-profile
        # crypto intrinsics (`molt_pbkdf2_hmac`, `molt_scrypt`, ...) undefined
        # and the link failing on `_..._molt_trampoline_*_import`. The explicit
        # arg still wins over the env; the env wins over the deploy-profile
        # default and the `micro` fallback below.
        if stdlib_profile is None:
            env_stdlib_profile = os.environ.get("MOLT_STDLIB_PROFILE")
            if env_stdlib_profile in ("full", "micro"):
                stdlib_profile = env_stdlib_profile

        if deploy_profile and deploy_profile in _DEPLOY_PROFILE_DEFAULTS:
            defaults = _DEPLOY_PROFILE_DEFAULTS[deploy_profile]
            # Only apply defaults for arguments that weren't explicitly set
            if args.wasm_opt_level == "Oz" and "wasm_opt_level" not in sys.argv:
                # wasm_opt_level has argparse default "Oz"; check if user passed it
                _wasm_opt_explicitly_set = any(
                    a.startswith("--wasm-opt-level") for a in sys.argv
                )
                if not _wasm_opt_explicitly_set:
                    default_wasm_opt_level = defaults.get("wasm_opt_level")
                    if isinstance(default_wasm_opt_level, str):
                        wasm_opt_level = default_wasm_opt_level
            if not any(a == "--precompile" for a in sys.argv):
                precompile = bool(defaults.get("precompile", precompile))
            if not any(a.startswith("--wasm-profile") for a in sys.argv):
                default_wasm_profile = defaults.get("wasm_profile")
                if isinstance(default_wasm_profile, str):
                    wasm_profile = default_wasm_profile
            if stdlib_profile is None and "stdlib_profile" in defaults:
                default_stdlib_profile = defaults.get("stdlib_profile")
                if isinstance(default_stdlib_profile, str):
                    stdlib_profile = default_stdlib_profile
        if stdlib_profile is None:
            stdlib_profile = "micro"

        # `--target llvm` is an alias for "native binary, LLVM backend": the
        # LLVM backend emits host-native objects, so the runtime staticlib and
        # the entire native link path are identical to `--target native`; the
        # only difference is the codegen backend.  Canonicalize it to the
        # `native` target (so every downstream `target == "native"` branch —
        # runtime triple, stdlib object split, native link driver — fires) and
        # route the backend selection through MOLT_BACKEND below.  Without this,
        # "llvm" leaks into the cargo `--target` slot, which expects a rustc
        # target triple, and the runtime build fails with "could not find
        # specification for target \"llvm\"".
        if target == "llvm":
            if backend_choice not in {"auto", "llvm"}:
                return _fail(
                    "`--target llvm` selects the LLVM backend; it conflicts "
                    f"with `--backend {backend_choice}`. Use `--target native "
                    "--backend llvm` to mix, or drop one flag.",
                    args.json,
                    command="build",
                )
            backend_choice = "llvm"
            target = "native"
        # --backend: resolve effective backend and propagate via MOLT_BACKEND.
        # "auto" defaults to cranelift for all builds. LLVM remains opt-in
        # until its end-to-end parity and operational tooling are on the same
        # footing as the default Cranelift lane.
        effective_backend = backend_choice
        if effective_backend == "auto":
            effective_backend = "cranelift"
        os.environ["MOLT_BACKEND"] = effective_backend

        build_rc = build(
            args.file,
            target,
            codec,
            type_hints,
            fallback,
            type_facts,
            pgo_profile,
            runtime_feedback,
            output,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            trusted,
            capabilities,
            cache,
            cache_dir,
            cache_report,
            sysroot,
            emit_ir,
            emit,
            out_dir,
            build_profile,
            linked,
            linked_output_path,
            require_linked,
            respect_pythonpath,
            args.module,
            diagnostics,
            diagnostics_file,
            diagnostics_verbosity,
            portable=getattr(args, "portable", False),
            wasm_opt_level=wasm_opt_level,
            precompile=precompile,
            wasm_profile=wasm_profile,
            snapshot=getattr(args, "snapshot", False),
            stdlib_profile=stdlib_profile,
            lib_paths=lib_paths or None,
            split_runtime=getattr(args, "split_runtime", False),
            capability_manifest=getattr(args, "capability_manifest", None),
            require_signed_manifest=getattr(args, "require_signed_manifest", False),
            audit_log=getattr(args, "audit_log", None),
            io_mode=getattr(args, "io_mode", None),
            type_gate=getattr(args, "type_gate", False),
            python_version=getattr(args, "python_version", None),
            build_config=build_cfg,
        )

        # --bolt: post-link BOLT optimization for native targets.
        bolt_requested = getattr(args, "bolt", False)
        if bolt_rc := _run_bolt_post_link(
            bolt_requested=bolt_requested,
            bolt_training_cmd=getattr(args, "bolt_training_cmd", None),
            target=target,
            output=output,
            out_dir=out_dir,
            build_rc=build_rc,
            json_output=args.json,
        ):
            return bolt_rc
        return build_rc
    if args.command == "factgraph":
        return _factgraph.run_factgraph_command(
            args=args,
            build=build,
            build_config=build_cfg,
            config_capabilities=cfg_capabilities,
            coerce_bool=_coerce_bool,
            fail=_fail,
        )
    if args.command == "extension":
        if args.extension_command == "build":
            deterministic = (
                args.deterministic
                if args.deterministic is not None
                else _coerce_bool(extension_cfg.get("deterministic"), True)
            )
            capabilities = (
                args.capabilities
                or extension_cfg.get("capabilities")
                or cfg_capabilities
            )
            molt_abi = (
                args.molt_abi
                or extension_cfg.get("molt_abi")
                or extension_cfg.get("molt-abi")
            )
            return _commands.extension_build(
                project=args.project or extension_cfg.get("project"),
                out_dir=args.out_dir
                or extension_cfg.get("out_dir")
                or extension_cfg.get("out-dir"),
                molt_abi=molt_abi,
                capabilities=capabilities,
                deterministic=deterministic,
                target=args.target or extension_cfg.get("target"),
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "audit":
            require_abi = (
                args.require_abi
                or extension_cfg.get("require_abi")
                or extension_cfg.get("require-abi")
            )
            require_capabilities = args.require_capabilities
            if not require_capabilities:
                require_capabilities = _coerce_bool(
                    extension_cfg.get("require_capabilities")
                    or extension_cfg.get("require-capabilities"),
                    False,
                )
            require_checksum = args.require_checksum
            if not require_checksum:
                require_checksum = _coerce_bool(
                    extension_cfg.get("require_checksum")
                    or extension_cfg.get("require-checksum"),
                    False,
                )
            return extension_audit(
                path=args.path,
                require_capabilities=require_capabilities,
                require_abi=require_abi,
                require_checksum=require_checksum,
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "scan":
            fail_on_missing = args.fail_on_missing
            if not fail_on_missing:
                fail_on_missing = _coerce_bool(
                    extension_cfg.get("scan_fail_on_missing")
                    or extension_cfg.get("scan-fail-on-missing"),
                    False,
                )
            return extension_scan(
                project=args.project or extension_cfg.get("project"),
                sources=args.source,
                fail_on_missing=fail_on_missing,
                json_output=args.json,
                verbose=args.verbose,
            )
        return _fail(
            "Missing extension subcommand (build|audit|scan).",
            args.json,
            command="extension",
        )
    if args.command == "check":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return _typecheck.check(
            args.path,
            args.output,
            args.strict,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
        )
    if args.command == "run":
        build_args = _strip_leading_double_dash(args.build_arg)
        if args.rebuild and not _build_args_has_cache_flag(build_args):
            build_args.append("--no-cache")
        # Forward --backend to the build subprocess when specified.
        run_backend = getattr(args, "backend", None)
        if run_backend and not any(a.startswith("--backend") for a in build_args):
            build_args.extend(["--backend", run_backend])
        run_python_version = (
            getattr(args, "python_version", None)
            or run_cfg.get("python_version")
            or run_cfg.get("python-version")
        )
        if run_python_version and not _build_args_has_python_version_flag(build_args):
            build_args.extend(["--python-version", str(run_python_version)])
        run_target = getattr(args, "target", None) or run_cfg.get("target") or "native"
        run_profile = (
            ("release" if getattr(args, "release", False) else None)
            or args.profile
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if run_profile is not None and run_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid run profile: {run_profile}",
                args.json,
                command="run",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        capabilities = (
            args.capabilities or run_cfg.get("capabilities") or cfg_capabilities
        )
        if run_target in ("wasm", "luau"):
            # Inject --target into build_args so run_script_cross handles it
            if not any(a.startswith("--target") for a in build_args):
                build_args.extend(["--target", run_target])
            return _commands._run_script_cross(
                run_target,
                args.file,
                args.module,
                _strip_leading_double_dash(args.script_args),
                args.json,
                args.verbose,
                args.timing,
                trusted,
                capabilities,
                getattr(args, "capability_manifest", None),
                getattr(args, "require_signed_manifest", False),
                build_args,
                cast(BuildProfile | None, run_profile),
                audit_log=getattr(args, "audit_log", None),
                io_mode=getattr(args, "io_mode", None),
                type_gate=getattr(args, "type_gate", False),
            )
        if run_target == "mlir":
            # MLIR target: build to get MLIR text (no JIT in run mode yet).
            if not any(a.startswith("--target") for a in build_args):
                build_args.extend(["--target", "mlir"])
            return _commands._run_script_cross(
                run_target,
                args.file,
                args.module,
                _strip_leading_double_dash(args.script_args),
                args.json,
                args.verbose,
                args.timing,
                trusted,
                capabilities,
                getattr(args, "capability_manifest", None),
                getattr(args, "require_signed_manifest", False),
                build_args,
                cast(BuildProfile | None, run_profile),
                audit_log=getattr(args, "audit_log", None),
                io_mode=getattr(args, "io_mode", None),
                type_gate=getattr(args, "type_gate", False),
            )
        return _commands.run_script(
            args.file,
            args.module,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
            trusted,
            capabilities,
            getattr(args, "capability_manifest", None),
            getattr(args, "require_signed_manifest", False),
            build_args,
            cast(BuildProfile | None, run_profile),
            audit_log=getattr(args, "audit_log", None),
            io_mode=getattr(args, "io_mode", None),
            type_gate=getattr(args, "type_gate", False),
        )
    if args.command == "repl":
        from molt.repl import run_repl

        molt_cmd: str | Sequence[str]
        if args.molt_cmd:
            molt_cmd = args.molt_cmd
        else:
            molt_cmd = [sys.executable, "-m", "molt.cli"]
        return run_repl(
            capabilities=args.capabilities,
            io_mode=args.io_mode,
            molt_cmd=molt_cmd,
            timeout_sec=args.timeout_sec,
        )
    if args.command == "compare":
        python_exe = args.python or args.python_version
        build_args = _strip_leading_double_dash(args.build_arg)
        compare_target_python = args.python_version
        if compare_target_python is None and args.python:
            with contextlib.suppress(ValueError):
                compare_target_python = _parse_target_python_version(args.python).short
        if compare_target_python and not _build_args_has_python_version_flag(
            build_args
        ):
            build_args.extend(["--python-version", compare_target_python])
        compare_profile = (
            args.profile
            or compare_cfg.get("profile")
            or compare_cfg.get("build_profile")
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if compare_profile is not None and compare_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid compare profile: {compare_profile}",
                args.json,
                command="compare",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(
                compare_cfg.get("trusted", run_cfg.get("trusted")),
                False,
            )
        capabilities = (
            args.capabilities
            or compare_cfg.get("capabilities")
            or run_cfg.get("capabilities")
            or cfg_capabilities
        )
        return _commands.compare(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            trusted,
            capabilities,
            build_args,
            args.rebuild,
            cast(BuildProfile | None, compare_profile),
        )
    if args.command == "parity-run":
        python_exe = args.python or args.python_version
        return _commands.parity_run(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
        )
    if args.command == "test":
        pytest_args = _strip_leading_double_dash(args.pytest_args)
        if args.suite == "dev" and (args.path or pytest_args) and args.verbose:
            print("Ignoring extra args for suite=dev.")
        test_profile = (
            args.profile
            or test_cfg.get("profile")
            or test_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if test_profile is not None and test_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid test profile: {test_profile}",
                args.json,
                command="test",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(test_cfg.get("trusted"), False)
        return _commands.test(
            args.suite,
            args.path,
            args.python_version,
            pytest_args,
            cast(BuildProfile | None, test_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "diff":
        diff_profile = (
            args.profile
            or diff_cfg.get("profile")
            or diff_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if diff_profile is not None and diff_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid diff profile: {diff_profile}",
                args.json,
                command="diff",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(diff_cfg.get("trusted"), False)
        return _commands.diff(
            args.path,
            args.python_version,
            cast(BuildProfile | None, diff_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        return _commands.bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.bench_script,
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        return _commands.profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        return _commands.lint(args.json, args.verbose)
    if args.command == "setup":
        return setup(args.json, args.verbose, args.strict)
    if args.command == "doctor":
        return doctor(args.json, args.verbose, args.strict)
    if args.command == "update":
        include_manifests = args.manifests or args.all
        return update_repo(
            json_output=args.json,
            verbose=args.verbose,
            check_only=args.check,
            include_toolchains=args.toolchains,
            include_locks=args.locks,
            include_manifests=include_manifests,
        )
    if args.command == "validate":
        return validate(
            suite=cast(
                Literal["full", "smoke", "commands", "conformance", "bench"],
                args.suite,
            ),
            backend=cast(
                Literal["all", "native", "llvm", "wasm", "luau"],
                args.backend,
            ),
            profile=cast(Literal["all", "dev", "release"], args.profile),
            json_output=args.json,
            verbose=args.verbose,
            check_only=args.check,
            summary_out=args.summary_out,
        )
    if args.command == "package":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        capabilities = args.capabilities or cfg_capabilities
        sbom_enabled = args.sbom
        if sbom_enabled is None:
            sbom_enabled = True
        return package(
            args.artifact,
            args.manifest,
            args.output,
            json_output=args.json,
            verbose=args.verbose,
            deterministic=deterministic,
            deterministic_warn=deterministic_warn,
            capabilities=capabilities,
            sbom=sbom_enabled,
            sbom_output=args.sbom_output,
            sbom_format=args.sbom_format,
            signature=args.signature,
            signature_output=args.signature_output,
            sign=args.sign,
            signer=args.signer,
            signing_key=args.signing_key,
            signing_identity=args.signing_identity,
        )
    if args.command == "publish":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(
                publish_cfg.get("deterministic") or build_cfg.get("deterministic"),
                True,
            )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                publish_cfg.get("deterministic_warn")
                or publish_cfg.get("deterministic-warn")
                or build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        explicit_require = args.require_signature is not None
        explicit_verify = args.verify_signature is not None
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = _coerce_bool(
                publish_cfg.get("require_signature")
                or publish_cfg.get("require-signature")
                or os.environ.get("MOLT_REQUIRE_SIGNATURE"),
                False,
            )
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = _coerce_bool(
                publish_cfg.get("verify_signature")
                or publish_cfg.get("verify-signature")
                or os.environ.get("MOLT_VERIFY_SIGNATURE"),
                False,
            )
        if explicit_require and not require_signature and not explicit_verify:
            verify_signature = False
        trusted_signers = (
            args.trusted_signers
            or publish_cfg.get("trusted_signers")
            or publish_cfg.get("trusted-signers")
            or os.environ.get("MOLT_TRUSTED_SIGNERS")
        )
        if _is_remote_registry(args.registry):
            if not explicit_require:
                require_signature = True
            if not explicit_verify and require_signature:
                verify_signature = True
            if trusted_signers is None and (require_signature or verify_signature):
                return _fail(
                    "Remote publish requires --trusted-signers or MOLT_TRUSTED_SIGNERS "
                    "(disable with --no-require-signature/--no-verify-signature).",
                    args.json,
                    command="publish",
                )
        capabilities = (
            args.capabilities or publish_cfg.get("capabilities") or cfg_capabilities
        )
        return publish(
            args.package,
            args.registry,
            args.dry_run,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            args.signer,
            args.signing_key,
            args.registry_token,
            args.registry_user,
            args.registry_password,
            args.registry_timeout,
        )
    if args.command == "verify":
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = False
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = False
        return verify(
            args.package,
            args.manifest,
            args.artifact,
            args.require_checksum,
            args.json,
            args.verbose,
            args.require_deterministic,
            args.capabilities or cfg_capabilities,
            require_signature,
            verify_signature,
            args.trusted_signers,
            args.signer,
            args.signing_key,
            args.require_extension_capabilities,
            args.require_extension_abi,
            args.extension_metadata,
        )
    if args.command == "deps":
        return deps(args.include_dev, args.json, args.verbose)
    if args.command == "install":
        pkgs = args.packages or []
        if pkgs and pkgs[0] == "add":
            add_pkgs = pkgs[1:]
            if not add_pkgs:
                return _fail(
                    "molt install add requires at least one package name.",
                    args.json,
                    command="install",
                )
            return install_add(
                add_pkgs,
                json_output=args.json,
                verbose=args.verbose,
            )
        return install(
            packages=pkgs or None,
            requirements=args.requirements,
            json_output=args.json,
            verbose=args.verbose,
            sync=args.sync,
        )
    if args.command == "vendor":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return vendor(
            args.include_dev,
            args.json,
            args.verbose,
            args.output,
            args.dry_run,
            args.allow_non_tier_a,
            args.extras,
            deterministic,
            deterministic_warn,
        )
    if args.command == "clean":
        return clean(
            args.json,
            args.verbose,
            apply=args.apply,
            kill_processes=args.kill_processes,
            extra_paths=args.extra_path,
            list_paths=args.list_paths,
        )
    if args.command == "config":
        return show_config(config_root, config, args.json, args.verbose)
    if args.command == "completion":
        return completion(args.shell, args.json, args.verbose)

    if args.command == "harness":
        from molt.harness import main as harness_main

        harness_args = [getattr(args, "profile", "standard")]
        if getattr(args, "no_fail_fast", False):
            harness_args.append("--no-fail-fast")
        if getattr(args, "verbose", False):
            harness_args.append("--verbose")
        if getattr(args, "json", False):
            harness_args.append("--json")
        return harness_main(harness_args)

    if args.command == "deploy":
        deploy_build_profile = args.build_profile
        if getattr(args, "release", False) and not deploy_build_profile:
            deploy_build_profile = "release"
        return _commands._deploy(
            platform=args.platform,
            file_path=args.file,
            module=args.module,
            build_profile=deploy_build_profile,
            output=args.output,
            out_dir=args.out_dir,
            roblox_project=getattr(args, "roblox_project", None),
            wrangler_args=getattr(args, "wrangler_args", ""),
            dry_run=args.dry_run,
            build_args=_strip_leading_double_dash(args.build_arg),
            json_output=args.json,
            verbose=args.verbose,
        )

    return 2






if __name__ == "__main__":
    raise SystemExit(main())
