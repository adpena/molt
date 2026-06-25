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


def _resolve_python_exe(python_exe: str | None) -> str:
    if not python_exe:
        return sys.executable
    if python_exe[0].isdigit() and os.sep not in python_exe:
        python_exe = f"python{python_exe}"
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _run_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    json_output: bool = False,
    verbose: bool = False,
    label: str | None = None,
    warnings: list[str] | None = None,
    memory_guard_prefix: str | None = None,
) -> int:
    cmd = [str(part) for part in cmd]
    if verbose and not json_output:
        print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
    capture = json_output
    result = _run_completed_command(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture,
        memory_guard_prefix=memory_guard_prefix,
    )
    if json_output:
        data: dict[str, Any] = {"returncode": result.returncode}
        if result.stdout:
            data["stdout"] = result.stdout
        if result.stderr:
            data["stderr"] = result.stderr
        payload = _json_payload(
            label or cmd[0],
            "ok" if result.returncode == 0 else "error",
            data=data,
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    return result.returncode



def _run_command_timed(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    verbose: bool = False,
    capture_output: bool = False,
    memory_guard_prefix: str | None = None,
) -> _TimedResult:
    cmd = [str(part) for part in cmd]
    if verbose:
        print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
    start = time.perf_counter()
    result = _run_completed_command(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture_output,
        memory_guard_prefix=memory_guard_prefix,
    )
    duration = getattr(result, "elapsed_s", None)
    if duration is None:
        duration = time.perf_counter() - start
    return _TimedResult(
        result.returncode,
        result.stdout or "",
        result.stderr or "",
        duration,
    )


def _format_duration(seconds: float) -> str:
    if seconds < 0:
        seconds = 0.0
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} µs"
    if seconds < 1:
        return f"{seconds * 1000:.1f} ms"
    if seconds < 60:
        return f"{seconds:.3f} s"
    return f"{seconds / 60:.2f} min"


















































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


def _backend_fingerprint_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix=f"{cargo_profile}",
        extension="fingerprint",
    )


def _link_fingerprint_path(
    project_root: Path,
    artifact: Path,
    profile: BuildProfile,
    target_triple: str | None,
) -> Path:
    target = (target_triple or "native").replace(os.sep, "_").replace(":", "_")
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="link_fingerprints",
        stem_suffix=f"{profile}.{target}",
        extension="fingerprint",
    )


def _run_native_link_command(
    *,
    link_cmd: Sequence[str],
    json_output: bool,
    link_timeout: float | None,
) -> subprocess.CompletedProcess[str]:
    result = _run_completed_command(
        list(link_cmd),
        capture_output=json_output,
        env=None,
        cwd=None,
        timeout=link_timeout,
        memory_guard_prefix="MOLT_BUILD",
    )
    harness_memory_guard = _load_cli_harness_memory_guard(None)
    if (
        link_timeout is not None
        and result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    ):
        raise subprocess.TimeoutExpired(
            list(link_cmd),
            link_timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result


def _run_native_partial_link_command(
    *,
    input_objects: Sequence[Path],
    output_path: Path,
    json_output: bool,
    link_timeout: float | None,
    target_triple: str | None = None,
    sysroot_path: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    if _native_target_is_windows(target_triple):
        link_cmd = _windows_coff_library_command(
            input_objects=input_objects,
            output_path=output_path,
        )
        return _run_native_link_command(
            link_cmd=link_cmd,
            json_output=json_output,
            link_timeout=link_timeout,
        )
    primary_object = input_objects[0] if input_objects else None
    link_cmd, _linker_hint, _normalized_target = _build_native_link_driver_command(
        output_obj=primary_object,
        target_triple=target_triple,
        sysroot_path=None,
        profile="dev",
    )
    link_cmd = [arg for arg in link_cmd if not arg.startswith("-fuse-ld=")]
    link_cmd.extend(
        ["-Wl,-r", "-o", str(output_path), *[str(path) for path in input_objects]]
    )
    return _run_native_link_command(
        link_cmd=link_cmd,
        json_output=json_output,
        link_timeout=link_timeout,
    )


def _prepare_native_object_artifact(
    *,
    output_artifact: Path,
    artifacts_root: Path,
    stdlib_obj_path: Path | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    json_output: bool,
    link_timeout: float | None,
    target_triple: str | None = None,
    sysroot_path: Path | None = None,
) -> tuple[Path | None, subprocess.CompletedProcess[str] | None, _CliFailure | None]:
    if stdlib_obj_path is None or not stdlib_obj_path.exists():
        return output_artifact, None, None
    if not _shared_stdlib_cache_matches_key_locked(
        stdlib_obj_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
    ):
        return (
            None,
            None,
            _fail(
                "Shared stdlib cache mismatch before native object link",
                json_output,
                command="build",
            ),
        )
    merged_output = artifacts_root / (
        f".{output_artifact.stem}_linked."
        f"{os.getpid()}.{uuid.uuid4().hex}{output_artifact.suffix}"
    )
    try:
        link_process = _run_native_partial_link_command(
            input_objects=[output_artifact, stdlib_obj_path],
            output_path=merged_output,
            json_output=json_output,
            link_timeout=link_timeout,
            target_triple=target_triple,
            sysroot_path=sysroot_path,
        )
    except subprocess.TimeoutExpired:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
        return (
            None,
            None,
            _fail(
                "Native object partial link timed out",
                json_output,
                command="build",
            ),
        )
    except RuntimeError as exc:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
        return None, None, _fail(str(exc), json_output, command="build")
    if link_process.returncode != 0:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
        err = (link_process.stderr or "").strip() or (link_process.stdout or "").strip()
        msg = "Native object partial link failed"
        if err:
            msg = f"{msg}: {err}"
        return None, link_process, _fail(msg, json_output, command="build")
    try:
        os.replace(merged_output, output_artifact)
    finally:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
    return output_artifact, link_process, None


def _retry_native_link_without_hint(
    *,
    link_cmd: Sequence[str],
    linker_hint: str | None,
    json_output: bool,
    link_timeout: float | None,
) -> tuple[subprocess.CompletedProcess[str] | None, list[str]]:
    if linker_hint is None:
        return None, list(link_cmd)
    retry_cmd = [
        arg
        for arg in link_cmd
        if arg != f"-fuse-ld={linker_hint}" and arg != "-Wl,--icf=safe"
    ]
    if retry_cmd == list(link_cmd):
        return None, retry_cmd
    retry_process = _run_native_link_command(
        link_cmd=retry_cmd,
        json_output=json_output,
        link_timeout=link_timeout,
    )
    return retry_process, retry_cmd


def _darwin_link_validation_failure(
    *,
    output_binary: Path,
    kind: str,
) -> str | None:
    if kind == "magic":
        detail = _darwin_binary_magic_error(output_binary)
        if detail is None:
            return None
        return "Generated binary failed Mach-O header validation.\n" + detail + "\n"
    detail = _darwin_binary_imports_validation_error(output_binary)
    if detail is None:
        return None
    return "Generated binary failed dyld import validation.\n" + detail + "\n"


def _validate_darwin_link_output(
    *,
    link_process: subprocess.CompletedProcess[str],
    link_cmd: Sequence[str],
    linker_hint: str | None,
    output_binary: Path,
    validation_kind: str,
    json_output: bool,
    link_timeout: float | None,
    warnings: list[str],
) -> subprocess.CompletedProcess[str]:
    validation_error = _darwin_link_validation_failure(
        output_binary=output_binary,
        kind=validation_kind,
    )
    if (
        validation_error is not None
        and linker_hint is not None
        and any(arg == f"-fuse-ld={linker_hint}" for arg in link_cmd)
    ):
        retry_process, _ = _retry_native_link_without_hint(
            link_cmd=link_cmd,
            linker_hint=linker_hint,
            json_output=json_output,
            link_timeout=link_timeout,
        )
        if retry_process is not None:
            if retry_process.returncode == 0:
                retry_validation_error = _darwin_link_validation_failure(
                    output_binary=output_binary,
                    kind=validation_kind,
                )
                if retry_validation_error is None:
                    label = (
                        "invalid output"
                        if validation_kind == "magic"
                        else "invalid dyld imports"
                    )
                    warnings.append(
                        "Linker fallback: "
                        f"-fuse-ld={linker_hint} produced {label}; "
                        "retried default linker."
                    )
                    return retry_process
                link_process = retry_process
                validation_error = retry_validation_error
            else:
                return retry_process
    if validation_error is None:
        return link_process
    failure_stderr = (link_process.stderr or "") + "\n" + validation_error
    return subprocess.CompletedProcess(
        args=list(link_cmd),
        returncode=1,
        stdout=link_process.stdout,
        stderr=failure_stderr,
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
    if not _ensure_backend_binary(
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
            _prepare_native_object_artifact(
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
    if stdlib_link_obj_path is not None and stdlib_link_obj_path.exists():
        try:
            staged_stdlib_obj = _stage_shared_stdlib_object_for_link(
                stdlib_link_obj_path,
                stdlib_object_cache_key=prepared_backend_setup.cache_setup.stdlib_object_cache_key,
                stdlib_object_manifest=prepared_backend_setup.cache_setup.stdlib_object_manifest,
                stdlib_module_symbols=prepared_backend_setup.cache_setup.stdlib_module_symbols,
                artifacts_root=artifacts_root,
            )
        except OSError as exc:
            return _fail(
                f"Failed to stage shared stdlib object before native link: {exc}",
                json_output,
                command="build",
            )
        stdlib_link_obj_path = staged_stdlib_obj

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
    prepared_native_link, prepared_native_link_error = _prepare_native_link(
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
            link_fingerprint_path = _link_fingerprint_path(
                link_project_root,
                resolved_linked_output,
                profile,
                "wasm32-wasip1",
            )
            stored_link_fingerprint = _read_runtime_fingerprint(link_fingerprint_path)
            link_fingerprint = _link_fingerprint(
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


def _prepare_native_link(
    *,
    output_artifact: Path,
    trusted: bool,
    capabilities_list: list[str] | None,
    artifacts_root: Path,
    json_output: bool,
    output_binary: Path | None,
    runtime_lib: Path | None,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
    sysroot_path: Path | None,
    profile: BuildProfile,
    project_root: Path,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    link_timeout: float | None,
    warnings: list[str],
    stdlib_obj_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
    stdlib_profile: str | None = "micro",
) -> tuple[_PreparedNativeLink | None, _CliFailure | None]:
    output_obj = output_artifact
    link_stdlib_obj = stdlib_obj_path
    if stdlib_obj_path is not None:
        if stdlib_obj_path.exists() and not _shared_stdlib_cache_matches_key_locked(
            stdlib_obj_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
        ):
            return None, _fail(
                "Shared stdlib cache key mismatch before native link",
                json_output,
                command="build",
            )
        if stdlib_obj_path.exists() and stdlib_obj_path.parent != artifacts_root:
            try:
                staged_stdlib_obj = _stage_shared_stdlib_object_for_link(
                    stdlib_obj_path,
                    stdlib_object_cache_key=stdlib_object_cache_key,
                    stdlib_object_manifest=stdlib_object_manifest,
                    stdlib_module_symbols=stdlib_module_symbols,
                    artifacts_root=artifacts_root,
                )
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage shared stdlib object for native link: {exc}",
                    json_output,
                    command="build",
                )
            link_stdlib_obj = staged_stdlib_obj
    try:
        staged_external_native_artifacts = (
            _stage_external_package_native_artifacts_for_build(
                native_artifact_plan,
                artifacts_root=artifacts_root,
            )
        )
    except OSError as exc:
        return None, _fail(
            f"Failed to stage external native artifacts for native build: {exc}",
            json_output,
            command="build",
        )
    main_c_content = _render_native_main_stub(
        trusted=trusted,
        capabilities_list=capabilities_list,
        runtime_module_roots=tuple(
            dict.fromkeys(
                artifact.runtime_root for artifact in staged_external_native_artifacts
            )
        ),
    )
    stub_path = artifacts_root / "main_stub.c"
    _write_text_if_changed(stub_path, main_c_content)

    if output_binary is None:
        return None, _fail("Binary output unavailable", json_output, command="build")
    if output_binary.parent != Path("."):
        output_binary.parent.mkdir(parents=True, exist_ok=True)
    resolved_runtime_lib = runtime_lib
    if resolved_runtime_lib is None:
        resolved_runtime_lib = _runtime_lib_path(
            molt_root,
            runtime_cargo_profile,
            target_triple,
            stdlib_profile=stdlib_profile,
        )
    try:
        link_cmd, linker_hint, normalized_target = _build_native_link_command(
            output_obj=output_obj,
            stub_path=stub_path,
            runtime_lib=resolved_runtime_lib,
            output_binary=output_binary,
            target_triple=target_triple,
            sysroot_path=sysroot_path,
            profile=profile,
            stdlib_obj_path=link_stdlib_obj,
        )
    except RuntimeError as exc:
        return None, _fail(str(exc), json_output, command="build")
    if os.environ.get("MOLT_TRACE_NATIVE_LINK") == "1":
        stdlib_exists = (
            link_stdlib_obj.exists() if link_stdlib_obj is not None else False
        )
        print(
            "native-link trace: "
            f"output_obj={output_obj} "
            f"stdlib_obj={link_stdlib_obj} "
            f"stdlib_exists={stdlib_exists} "
            f"runtime_lib={resolved_runtime_lib} "
            f"output_binary={output_binary}",
            file=sys.stderr,
        )
        print(f"native-link cmd: {link_cmd}", file=sys.stderr)
    if (
        normalized_target is not None
        and target_triple is not None
        and normalized_target != target_triple
    ):
        warnings.append(
            f"Zig target normalized to {normalized_target} from {target_triple}."
        )

    link_fingerprint_path = _link_fingerprint_path(
        project_root, output_binary, profile, target_triple
    )
    stored_link_fingerprint = _read_runtime_fingerprint(link_fingerprint_path)
    external_native_fingerprint_inputs = [
        path
        for artifact in staged_external_native_artifacts
        for path in (
            artifact.staged_path,
            artifact.staged_manifest_path,
            *artifact.staged_support_paths,
        )
    ]
    link_fingerprint = _link_fingerprint(
        project_root=project_root,
        inputs=[
            stub_path,
            output_obj,
            resolved_runtime_lib,
            *(
                [link_stdlib_obj]
                if link_stdlib_obj is not None and link_stdlib_obj.exists()
                else []
            ),
            *external_native_fingerprint_inputs,
        ],
        link_cmd=link_cmd,
        stored_fingerprint=stored_link_fingerprint,
    )
    link_skipped = not _artifact_needs_rebuild(
        output_binary,
        link_fingerprint,
        stored_link_fingerprint,
    )
    # Staleness guard: even when the fingerprint matches, the cached binary
    # may be stale if ANY link input was rebuilt after the binary was linked.
    # This catches backend changes that produce identical .o files (from TIR
    # cache) but changed runtime internals. Comparing mtimes is O(1) and
    # eliminates the entire class of stale-binary bugs.
    if link_skipped and output_binary.exists():
        try:
            binary_mtime = output_binary.stat().st_mtime
            # Include the backend binary: when function_compiler.rs changes,
            # the backend is rebuilt, which changes how .o files are generated.
            # Even if the .o content is identical (TIR cache), the binary must
            # be relinked because the runtime library was also rebuilt.
            backend_cargo_profile, _ = _resolve_backend_cargo_profile_name(profile)
            backend_bin = _backend_bin_path(molt_root, backend_cargo_profile)
            deps = [
                resolved_runtime_lib,
                output_obj,
                stub_path,
                backend_bin,
                *external_native_fingerprint_inputs,
            ]
            for dep in deps:
                if dep.exists() and dep.stat().st_mtime > binary_mtime:
                    link_skipped = False
                    break
        except OSError:
            pass
    if link_skipped:
        link_process = subprocess.CompletedProcess(
            args=link_cmd,
            returncode=0,
            stdout="",
            stderr="",
        )
    else:
        if diagnostics_enabled and "link" not in phase_starts:
            phase_starts["link"] = time.perf_counter()
        try:
            link_process = _run_native_link_command(
                link_cmd=link_cmd,
                json_output=json_output,
                link_timeout=link_timeout,
            )
        except subprocess.TimeoutExpired:
            return None, _fail("Linker timed out", json_output, command="build")
        if link_process.returncode != 0 and linker_hint is not None:
            try:
                retry_process, _ = _retry_native_link_without_hint(
                    link_cmd=link_cmd,
                    linker_hint=linker_hint,
                    json_output=json_output,
                    link_timeout=link_timeout,
                )
            except subprocess.TimeoutExpired:
                return None, _fail("Linker timed out", json_output, command="build")
            if retry_process is not None and retry_process.returncode == 0:
                warnings.append(
                    f"Linker fallback: -fuse-ld={linker_hint} failed; retried default linker."
                )
                link_process = retry_process
        if (
            link_process.returncode == 0
            and sys.platform == "darwin"
            and not target_triple
        ):
            try:
                link_process = _validate_darwin_link_output(
                    link_process=link_process,
                    link_cmd=link_cmd,
                    linker_hint=linker_hint,
                    output_binary=output_binary,
                    validation_kind="magic",
                    json_output=json_output,
                    link_timeout=link_timeout,
                    warnings=warnings,
                )
            except subprocess.TimeoutExpired:
                return None, _fail("Linker timed out", json_output, command="build")
        if (
            link_process.returncode == 0
            and sys.platform == "darwin"
            and not target_triple
        ):
            try:
                link_process = _validate_darwin_link_output(
                    link_process=link_process,
                    link_cmd=link_cmd,
                    linker_hint=linker_hint,
                    output_binary=output_binary,
                    validation_kind="dyld",
                    json_output=json_output,
                    link_timeout=link_timeout,
                    warnings=warnings,
                )
            except subprocess.TimeoutExpired:
                return None, _fail("Linker timed out", json_output, command="build")
    return _PreparedNativeLink(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=resolved_runtime_lib,
        output_binary=output_binary,
        external_native_artifacts=staged_external_native_artifacts,
        link_cmd=link_cmd,
        linker_hint=linker_hint,
        normalized_target=normalized_target,
        link_fingerprint_path=link_fingerprint_path,
        link_fingerprint=link_fingerprint,
        link_skipped=link_skipped,
        link_process=link_process,
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


def _link_fingerprint(
    *,
    project_root: Path,
    inputs: list[Path],
    link_cmd: list[str],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    inputs_meta = _hash_source_tree_metadata(inputs, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    meta_digest = hashlib.sha256("\0".join(link_cmd).encode("utf-8")).hexdigest()
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=None,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": None,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
        }
    hasher = hashlib.sha256()
    hasher.update("\0".join(link_cmd).encode("utf-8"))
    hasher.update(b"\0")
    try:
        for path in inputs:
            _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": None,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
    }


def _backend_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    rustflags: str,
    backend_features: tuple[str, ...],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    meta = f"profile:{cargo_profile}\n"
    meta += f"rustflags:{rustflags}\n"
    meta += f"features:{','.join(backend_features)}\n"
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    source_paths = _backend_source_paths(project_root, backend_features)
    rustc_info = _rustc_version()
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        for path in sorted(source_paths, key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
    }


def _ensure_backend_binary(
    backend_bin: Path,
    *,
    cargo_timeout: float | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    backend_features: tuple[str, ...],
) -> bool:
    # MOLT_SKIP_RUNTIME_REBUILD=1 also skips the backend fingerprint check.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        if backend_bin.exists():
            return True
    rustflags = os.environ.get("RUSTFLAGS", "")
    fingerprint_path = _backend_fingerprint_path(
        project_root, backend_bin, cargo_profile
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        rustflags=rustflags,
        backend_features=backend_features,
        stored_fingerprint=stored_fingerprint,
    )
    features_tag = "_".join(sorted(backend_features)) if backend_features else "default"
    lock_name = f"backend.{cargo_profile}.{features_tag}"
    with _build_lock(project_root, lock_name):

        def _canonical_cargo_backend_output() -> Path:
            exe_suffix = ".exe" if os.name == "nt" else ""
            return backend_bin.parent / f"molt-backend{exe_suffix}"

        def _materialize_backend_binary_from(source: Path) -> bool:
            if not source.exists():
                return False
            if source != backend_bin:
                _atomic_copy_file(source, backend_bin, codesign=True)
            else:
                _codesign_binary(backend_bin)
            return backend_bin.exists()

        def _materialize_rebuilt_backend_binary() -> bool:
            return _materialize_backend_binary_from(_canonical_cargo_backend_output())

        def _backend_probe_target() -> str:
            if "wasm-backend" in backend_features:
                return "wasm"
            if "luau-backend" in backend_features:
                return "luau"
            if "rust-backend" in backend_features:
                return "rust"
            return "native"

        def _probe_backend_binary_support(
            probe_target: str,
            *,
            binary_path: Path | None = None,
        ) -> tuple[bool, str]:
            probe_ir = json.dumps(
                {
                    "functions": [],
                    "module": "__probe__",
                    "entry": "main",
                    "metadata": {"target": probe_target, "deterministic": True},
                }
            ).encode()
            probe_suffix = ".o"
            if probe_target == "wasm":
                probe_suffix = ".wasm"
            elif probe_target == "luau":
                probe_suffix = ".luau"
            elif probe_target == "rust":
                probe_suffix = ".rs"
            probe_tmp = tempfile.NamedTemporaryFile(
                prefix="molt_backend_probe_",
                suffix=probe_suffix,
                delete=False,
            )
            probe_path = Path(probe_tmp.name)
            probe_tmp.close()
            probe_cmd = [str(binary_path or backend_bin), "--output", str(probe_path)]
            if probe_target == "wasm":
                probe_cmd.extend(["--target", "wasm"])
            elif probe_target == "luau":
                probe_cmd.extend(["--target", "luau"])
            elif probe_target == "rust":
                probe_cmd.extend(["--target", "rust"])
            try:
                probe = _run_subprocess_captured_to_tempfiles(
                    probe_cmd,
                    input=probe_ir,
                    cwd=project_root,
                    timeout=10,
                    memory_guard_prefix="MOLT_BUILD",
                )
            except (subprocess.TimeoutExpired, OSError) as exc:
                return False, str(exc)
            finally:
                try:
                    probe_path.unlink()
                except OSError:
                    pass
            stderr = probe.stderr.decode(errors="replace")
            stdout = probe.stdout.decode(errors="replace")
            return probe.returncode == 0, (stderr or stdout).strip()

        def _refresh_feature_tagged_backend_alias(probe_target: str) -> None:
            if backend_features == _DEFAULT_BACKEND_FEATURES:
                return
            cargo_output = _canonical_cargo_backend_output()
            if cargo_output == backend_bin or not cargo_output.exists():
                return
            try:
                cargo_mtime = cargo_output.stat().st_mtime_ns
            except OSError:
                return
            try:
                alias_mtime = backend_bin.stat().st_mtime_ns
            except OSError:
                alias_mtime = -1
            if alias_mtime >= cargo_mtime:
                return
            probe_ok, _probe_detail = _probe_backend_binary_support(
                probe_target,
                binary_path=cargo_output,
            )
            if probe_ok:
                _materialize_backend_binary_from(cargo_output)

        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if not _artifact_needs_rebuild(backend_bin, fingerprint, stored_fingerprint):
            # Force a real compile-path probe. An empty stdin-only probe can
            # miss feature-lane poisoning because it never exercises output
            # emission for the requested target.
            _quick_target = _backend_probe_target()
            _refresh_feature_tagged_backend_alias(_quick_target)
            _probe_ok, _probe_detail = _probe_backend_binary_support(_quick_target)
            if _probe_ok:
                return True
        canonical_target_root = _canonical_target_root(project_root)
        canonical_backend_bin = (
            canonical_target_root / _cargo_profile_dir(cargo_profile) / backend_bin.name
        )
        canonical_fingerprint_path = _artifact_state_path_for_build_state_root(
            _canonical_build_state_root(project_root),
            canonical_backend_bin,
            subdir="backend_fingerprints",
            stem_suffix=f"{cargo_profile}",
            extension="fingerprint",
        )
        if _maybe_hydrate_artifact_from_canonical_target(
            artifact=backend_bin,
            fingerprint=fingerprint,
            fingerprint_path=fingerprint_path,
            candidate_artifact=canonical_backend_bin,
            candidate_fingerprint_path=canonical_fingerprint_path,
        ):
            _probe_target = _backend_probe_target()
            _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
            if _probe_ok:
                return True
        # Cargo always writes the executable as `molt-backend`; Molt keeps
        # feature-specific aliases beside it so native/wasm/rust lanes cannot
        # poison each other.  When CI or a developer prebuilds the correct
        # feature lane with cargo, materialize the alias after probing the
        # canonical binary instead of rebuilding the backend.
        if backend_features != _DEFAULT_BACKEND_FEATURES:
            cargo_output = _canonical_cargo_backend_output()
            if _artifact_newer_than_sources(
                cargo_output,
                _backend_source_paths(project_root, backend_features),
            ):
                _probe_target = _backend_probe_target()
                _probe_ok, _probe_detail = _probe_backend_binary_support(
                    _probe_target,
                    binary_path=cargo_output,
                )
                if _probe_ok and _materialize_backend_binary_from(cargo_output):
                    if fingerprint is not None:
                        try:
                            fingerprint_path.parent.mkdir(
                                parents=True,
                                exist_ok=True,
                            )
                            _write_runtime_fingerprint(fingerprint_path, fingerprint)
                        except OSError:
                            if not json_output:
                                print(
                                    "Warning: failed to write backend fingerprint metadata.",
                                    file=sys.stderr,
                                )
                    return True
        # Fast path: if the backend binary exists and is newer than every
        # source file that contributes to the fingerprint, skip the expensive
        # cargo build and just update the stored fingerprint.  This handles
        # the common case of running `cargo build` manually before `molt build`.
        if _artifact_newer_than_sources(
            backend_bin,
            _backend_source_paths(project_root, backend_features),
        ):
            _probe_target = _backend_probe_target()
            _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
            if _probe_ok:
                assert fingerprint is not None
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
                return True
        if not json_output:
            print("Backend sources changed; rebuilding backend...", file=sys.stderr)
        # Cache entries include backend/tooling/runtime identity in their keys.
        # A backend rebuild therefore invalidates by selecting new keys, not by
        # deleting shared immutable cache artifacts that concurrent sessions may
        # still be reading. Size/age retention belongs to `molt clean`.
        cmd = [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            cargo_profile,
        ]
        if backend_features:
            cmd.append("--no-default-features")
            cmd.extend(["--features", ",".join(backend_features)])
        build_env = _cargo_build_env()
        # Per-session build isolation: route cargo output to
        # target/sessions/<id>/ under the canonical target root
        # when MOLT_SESSION_ID is active to prevent concurrent agents from
        # clobbering each other's backend artifacts.
        build_env["CARGO_TARGET_DIR"] = str(_cargo_target_root(project_root))
        # When building with the LLVM feature, ensure the pinned llvm-sys
        # prefix env var points at the matching Homebrew install so
        # inkwell/llvm-sys can link without extra shell setup.
        if "llvm" in backend_features:
            llvm_major = _required_llvm_backend_major(project_root)
            if llvm_major is not None:
                llvm_prefix_env = _llvm_sys_prefix_env_var(llvm_major)
                if llvm_prefix_env not in build_env:
                    llvm_prefix = f"/opt/homebrew/opt/llvm@{llvm_major}"
                    if os.path.isdir(llvm_prefix):
                        build_env[llvm_prefix_env] = llvm_prefix
        _maybe_enable_sccache(build_env)
        _maybe_enable_native_cpu(build_env)
        try:
            build = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Backend build",
            )
        except subprocess.TimeoutExpired:
            if not json_output:
                timeout_note = (
                    f"Backend build timed out after {cargo_timeout:.1f}s."
                    if cargo_timeout is not None
                    else "Backend build timed out."
                )
                print(timeout_note, file=sys.stderr)
            return False
        if build.returncode != 0:
            if not json_output:
                err = build.stderr.strip() or build.stdout.strip()
                if err:
                    print(err, file=sys.stderr)
            return False
        # Cargo always produces target/<profile>/molt-backend regardless of
        # features.  When the requested feature set is non-default, copy
        # the freshly-built binary to the feature-tagged path so that
        # concurrent or sequential builds with different feature sets
        # (native vs wasm vs rust) do not overwrite each other.
        if not _materialize_rebuilt_backend_binary():
            if not json_output:
                print("Backend binary missing after rebuild.", file=sys.stderr)
            return False
        # -- Post-build feature probe (defense-in-depth) -----------------
        # Cargo's incremental cache may skip recompilation when only
        # features change, leaving a binary built for the wrong target.
        # Probe the binary and, on mismatch, clean + rebuild once.
        _probe_target = _backend_probe_target()
        _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
        if not _probe_ok:
            if not json_output:
                print(
                    "Backend feature mismatch detected; cleaning and rebuilding...",
                    file=sys.stderr,
                )
            # Skip cargo clean: the deterministic rebuild path plus post-build
            # feature probe is the authority, while cargo clean would hold the
            # Cargo lock and block concurrent sessions.
            try:
                rebuild = _run_cargo_with_sccache_retry(
                    cmd,
                    cwd=project_root,
                    env=build_env,
                    timeout=cargo_timeout,
                    json_output=json_output,
                    label="Backend rebuild (feature fix)",
                )
            except subprocess.TimeoutExpired:
                if not json_output:
                    print("Backend rebuild timed out.", file=sys.stderr)
                return False
            if rebuild.returncode != 0:
                if not json_output:
                    err = rebuild.stderr.strip() or rebuild.stdout.strip()
                    if err:
                        print(err, file=sys.stderr)
                return False
            if not _materialize_rebuilt_backend_binary():
                if not json_output:
                    print("Backend binary missing after rebuild.", file=sys.stderr)
                return False
            _reprobe_ok, _reprobe_detail = _probe_backend_binary_support(_probe_target)
            if not _reprobe_ok:
                if not json_output and _reprobe_detail:
                    print(_reprobe_detail, file=sys.stderr)
                return False
        # -- End post-build feature probe --------------------------------
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write backend fingerprint metadata.",
                        file=sys.stderr,
                    )
    return True


def _artifact_newer_than_sources(
    artifact: Path,
    source_paths: list[Path],
) -> bool:
    """Return True if *artifact* exists and is newer than every file in *source_paths*.

    Handles both individual files and directories (recursed for all files).
    Returns False on any OS error or if no source files are found.
    """
    try:
        lib_mtime = artifact.stat().st_mtime
    except OSError:
        return False
    if not _artifact_content_looks_valid(artifact):
        return False
    newest_src = 0.0
    for path in source_paths:
        try:
            if path.is_dir():
                for item in path.rglob("*"):
                    if item.is_file():
                        newest_src = max(newest_src, item.stat().st_mtime)
            elif path.exists():
                newest_src = max(newest_src, path.stat().st_mtime)
        except OSError:
            return False
    return newest_src > 0 and lib_mtime > newest_src



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
        return _run_build_pipeline(
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


def _run_script_cross(
    target: str,
    file_path: str | None,
    module: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    build_args: list[str] | None = None,
    build_profile: BuildProfile | None = None,
    audit_log: str | None = None,
    io_mode: str | None = None,
    type_gate: bool = False,
) -> int:
    """Build with a cross target (wasm or luau) and run with the appropriate runtime."""
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="run"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="run")

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    build_args = list(build_args or [])
    resolved_build_entry, resolved_build_entry_error = _build_inputs._resolve_wrapper_build_entry(
        file_path=file_path,
        module=module,
        project_root=project_root,
        json_output=json_output,
        command="run",
        build_args=build_args,
    )
    if resolved_build_entry_error is not None:
        return resolved_build_entry_error
    assert resolved_build_entry is not None
    molt_root = _find_molt_root(project_root, Path.cwd())
    source_path = resolved_build_entry.source_path
    env = _base_env(
        project_root,
        source_path if file_path else None,
        molt_root=molt_root,
    )
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="run",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    if capability_manifest is not None:
        from molt.capability_manifest import load_manifest

        try:
            manifest_obj = load_manifest(
                capability_manifest, require_signed=require_signed_manifest
            )
            env.update(manifest_obj.to_env_vars())
        except Exception as e:
            return _fail(
                f"Invalid capability manifest: {e}",
                json_output,
                command="run",
            )

    # --audit-log flag (overrides manifest audit config)
    if audit_log is not None:
        env.update(_build_inputs._parse_audit_log_flag(audit_log))

    # --io-mode flag (overrides manifest io config)
    if io_mode is not None:
        env.update(_build_inputs._parse_io_mode_flag(io_mode))

    # --type-gate flag
    env.update(_build_inputs._parse_type_gate_flag(type_gate))

    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--build-profile", build_profile])
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])

    if not json_output:
        target_label = (
            "WASM" if target == "wasm" else "MLIR" if target == "mlir" else "Luau"
        )
        print(f"Building for {target_label}...", file=sys.stderr)
    try:
        build_contract, t_build, build_error = _run_wrapper_build(
            file_path=file_path,
            module=module,
            build_args=build_args,
            env=env,
            project_root=project_root,
            json_output=json_output,
            command="run",
            verbose=verbose,
            resolved_build_entry=resolved_build_entry,
            memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_error is not None:
        return build_error
    assert build_contract is not None

    if target == "wasm":
        run_artifact = build_contract.consumer_output
        if not run_artifact.exists():
            return _fail(
                f"WASM artifact not found: {run_artifact}\n"
                "Hint: the build may have succeeded but placed output elsewhere. "
                "Try `molt build --target wasm --verbose` to see the output path.",
                json_output,
                command="run",
            )
        wasmtime = shutil.which("wasmtime")
        if wasmtime is None:
            return _fail(
                "wasmtime not found on PATH. Install it: https://wasmtime.dev\n"
                "Hint: curl https://wasmtime.dev/install.sh -sSf | bash",
                json_output,
                command="run",
            )
        run_cmd = [wasmtime, "run", str(run_artifact), "--", *script_args]
    elif target == "luau":
        luau_artifact = build_contract.artifacts.get(
            "luau", build_contract.consumer_output
        )
        if not luau_artifact.exists():
            return _fail(
                f"Luau artifact not found: {luau_artifact}\n"
                "Hint: the build may have succeeded but placed output elsewhere. "
                "Try `molt build --target luau --verbose` to see the output path.",
                json_output,
                command="run",
            )
        lune = shutil.which("lune")
        if lune is None:
            return _fail(
                "lune not found on PATH. Install it: https://lune-org.github.io/docs/getting-started/installation\n"
                "Hint: cargo install lune",
                json_output,
                command="run",
            )
        run_cmd = [lune, "run", str(luau_artifact), "--", *script_args]
    elif target == "mlir":
        # MLIR target: the build phase produces .mlir text. There is no
        # separate run phase -- the MLIR output is the artifact.
        mlir_artifact = build_contract.consumer_output
        if not mlir_artifact.exists():
            return _fail(
                f"MLIR artifact not found: {mlir_artifact}\n"
                "Hint: the build may have succeeded but placed output elsewhere. "
                "Try `molt build --target mlir --verbose` to see the output path.",
                json_output,
                command="run",
            )
        if not json_output:
            print(f"MLIR output: {mlir_artifact}", file=sys.stderr)
        if timing and not json_output:
            print(
                f"\n--- timing: build {t_build:.3f}s ---",
                file=sys.stderr,
            )
        return 0
    else:
        return _fail(f"Unsupported cross target: {target}", json_output, command="run")

    if not json_output and verbose:
        print(f"Running: {shlex.join(run_cmd)}", file=sys.stderr)

    t_run_start = time.monotonic()
    run_res = _run_completed_command(
        run_cmd,
        env=env,
        cwd=project_root,
        capture_output=False,
        memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
    )
    t_run = getattr(run_res, "elapsed_s", None)
    if t_run is None:
        t_run = time.monotonic() - t_run_start

    if timing and not json_output:
        print(
            f"\n--- timing: build {t_build:.3f}s | run {t_run:.3f}s | "
            f"total {t_build + t_run:.3f}s ---",
            file=sys.stderr,
        )
    return run_res.returncode


def _deploy(
    platform: str,
    file_path: str | None,
    module: str | None,
    build_profile: str | None,
    output: str | None,
    out_dir: str | None,
    roblox_project: str | None,
    wrangler_args: str,
    dry_run: bool,
    build_args: list[str],
    json_output: bool,
    verbose: bool,
) -> int:
    """Build and deploy to the specified platform."""
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="deploy"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="deploy")

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    build_cmd_args = list(build_args)
    resolved_build_entry, resolved_build_entry_error = _build_inputs._resolve_wrapper_build_entry(
        file_path=file_path,
        module=module,
        project_root=project_root,
        json_output=json_output,
        command="deploy",
        build_args=build_cmd_args,
    )
    if resolved_build_entry_error is not None:
        return resolved_build_entry_error
    assert resolved_build_entry is not None
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(
        project_root,
        resolved_build_entry.source_path if file_path else None,
        molt_root=molt_root,
    )
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))

    # Construct build command
    if platform == "cloudflare":
        if not any(a.startswith("--target") for a in build_cmd_args):
            build_cmd_args.extend(["--target", "wasm"])
        if not any(
            a.startswith("--profile") or a.startswith("--platform")
            for a in build_cmd_args
        ):
            build_cmd_args.extend(["--profile", "cloudflare"])
        if not any(a == "--split-runtime" for a in build_cmd_args):
            build_cmd_args.append("--split-runtime")
    elif platform == "roblox":
        if not any(a.startswith("--target") for a in build_cmd_args):
            build_cmd_args.extend(["--target", "luau"])

    if build_profile and not _build_args_has_profile_flag(build_cmd_args):
        build_cmd_args.extend(["--build-profile", build_profile])
    if output:
        build_cmd_args.extend(["--output", output])
    if out_dir:
        build_cmd_args.extend(["--out-dir", out_dir])
    if verbose:
        build_cmd_args.append("--verbose")

    if not json_output:
        platform_label = "Cloudflare Workers" if platform == "cloudflare" else "Roblox"
        print(f"Building for {platform_label}...", file=sys.stderr)
    build_contract, _t_build, build_error = _run_wrapper_build(
        file_path=file_path,
        module=module,
        build_args=build_cmd_args,
        env=env,
        project_root=project_root,
        json_output=json_output,
        command="deploy",
        verbose=verbose,
        resolved_build_entry=resolved_build_entry,
        memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
    )
    if build_error is not None:
        return build_error
    assert build_contract is not None

    if dry_run:
        if not json_output:
            print("Build succeeded (dry run, skipping deploy).", file=sys.stderr)
        return 0

    if platform == "cloudflare":
        wrangler = shutil.which("wrangler")
        if wrangler is None:
            return _fail(
                "wrangler not found on PATH. Install it:\n"
                "  npm install -g wrangler\n"
                "  # or: npx wrangler deploy",
                json_output,
                command="deploy",
            )
        bundle_root = build_contract.bundle_root
        if bundle_root is None:
            return _fail(
                "Build JSON missing bundle_root for Cloudflare deploy.",
                json_output,
                command="deploy",
            )
        wrangler_config = build_contract.artifacts.get("wrangler_config")
        if wrangler_config is None:
            return _fail(
                "Build JSON missing wrangler_config for Cloudflare deploy.",
                json_output,
                command="deploy",
            )
        if not bundle_root.is_dir():
            return _fail(
                f"Cloudflare bundle root not found: {bundle_root}",
                json_output,
                command="deploy",
            )
        if not wrangler_config.exists():
            return _fail(
                f"Cloudflare wrangler config not found: {wrangler_config}",
                json_output,
                command="deploy",
            )
        deploy_cmd_parts = [wrangler, "deploy", "--config", str(wrangler_config)]
        if wrangler_args:
            deploy_cmd_parts.extend(shlex.split(wrangler_args))
        if not json_output:
            print("Deploying with wrangler...", file=sys.stderr)
            if verbose:
                print(
                    f"Deploy command: {shlex.join(deploy_cmd_parts)}", file=sys.stderr
                )
        return _run_command(
            deploy_cmd_parts,
            env=env,
            cwd=bundle_root,
            json_output=json_output,
            label="deploy",
            memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
        )

    elif platform == "roblox":
        if roblox_project is None:
            if not json_output:
                print(
                    "Build succeeded. Use --roblox-project <dir> to auto-copy "
                    "Luau output into your Roblox project.",
                    file=sys.stderr,
                )
            return 0
        roblox_dir = Path(roblox_project)
        if not roblox_dir.is_dir():
            return _fail(
                f"Roblox project directory not found: {roblox_dir}",
                json_output,
                command="deploy",
            )
        luau_artifact = build_contract.artifacts.get(
            "luau", build_contract.consumer_output
        )
        if not luau_artifact.exists():
            return _fail(
                f"Luau artifact not found at {luau_artifact}. "
                "Build may have placed it elsewhere; check --verbose output.",
                json_output,
                command="deploy",
            )
        dest = roblox_dir / luau_artifact.name
        try:
            _atomic_copy_file(luau_artifact, dest)
        except OSError as exc:
            return _fail(
                f"Failed to publish Roblox Luau artifact: {exc}",
                json_output,
                command="deploy",
            )
        if not json_output:
            print(f"Copied {luau_artifact.name} -> {dest}", file=sys.stderr)
        return 0

    return _fail(f"Unknown deploy platform: {platform}", json_output, command="deploy")


def run_script(
    file_path: str | None,
    module: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    build_args: list[str] | None = None,
    build_profile: BuildProfile | None = None,
    audit_log: str | None = None,
    io_mode: str | None = None,
    type_gate: bool = False,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="run"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="run")
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    build_args = list(build_args or [])
    resolved_build_entry, resolved_build_entry_error = _build_inputs._resolve_wrapper_build_entry(
        file_path=file_path,
        module=module,
        project_root=project_root,
        json_output=json_output,
        command="run",
        build_args=build_args,
    )
    if resolved_build_entry_error is not None:
        return resolved_build_entry_error
    assert resolved_build_entry is not None
    molt_root = _find_molt_root(project_root, Path.cwd())
    source_path = resolved_build_entry.source_path
    env = _base_env(
        project_root,
        source_path if file_path else None,
        molt_root=molt_root,
    )
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="run",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    if capability_manifest is not None:
        from molt.capability_manifest import load_manifest

        try:
            manifest_obj = load_manifest(
                capability_manifest, require_signed=require_signed_manifest
            )
            env.update(manifest_obj.to_env_vars())
        except Exception as e:
            return _fail(
                f"Invalid capability manifest: {e}",
                json_output,
                command="run",
            )

    # --audit-log flag (overrides manifest audit config)
    if audit_log is not None:
        env.update(_build_inputs._parse_audit_log_flag(audit_log))

    # --io-mode flag (overrides manifest io config)
    if io_mode is not None:
        env.update(_build_inputs._parse_io_mode_flag(io_mode))

    # --type-gate flag
    env.update(_build_inputs._parse_type_gate_flag(type_gate))

    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--build-profile", build_profile])
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    try:
        build_contract, build_duration_s, build_error = _run_wrapper_build(
            file_path=file_path,
            module=module,
            build_args=build_args,
            env=env,
            project_root=project_root,
            json_output=json_output,
            command="run",
            verbose=verbose,
            resolved_build_entry=resolved_build_entry,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_error is not None:
        return build_error
    assert build_contract is not None
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compiled run requires emit=bin (got {emit_arg})",
            json_output,
            command="run",
        )
    output_binary = _resolve_binary_output(str(build_contract.consumer_output))
    if output_binary is None:
        return _fail(
            f"Compiled binary not found at {build_contract.consumer_output}.",
            json_output,
            command="run",
        )
    if timing:
        run_res = _run_command_timed(
            [str(output_binary), *script_args],
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        if json_output:
            data: dict[str, Any] = {
                "returncode": run_res.returncode,
                "timing": {
                    "build_s": build_duration_s,
                    "run_s": run_res.duration_s,
                    "total_s": build_duration_s + run_res.duration_s,
                },
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (compiled):", file=sys.stderr)
            print(f"- build: {_format_duration(build_duration_s)}", file=sys.stderr)
            print(
                f"- run: {_format_duration(run_res.duration_s)}",
                file=sys.stderr,
            )
            total = build_duration_s + run_res.duration_s
            print(f"- total: {_format_duration(total)}", file=sys.stderr)
        return run_res.returncode
    return _run_command(
        [str(output_binary), *script_args],
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="run",
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )


def compare(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    build_args: list[str] | None = None,
    rebuild: bool = False,
    build_profile: BuildProfile | None = None,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="compare",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="compare")
    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}", json_output, command="compare"
            )
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="compare",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    requested_python_selector = python_exe
    python_exe = _resolve_python_exe(python_exe)
    if module:
        cpy_cmd = [python_exe, "-m", module, *script_args]
    else:
        cpy_cmd = [python_exe, str(source_path), *script_args]
    cpy_res = _run_command_timed(
        cpy_cmd,
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )

    build_args = list(build_args or [])
    if (
        requested_python_selector is not None
        and not _build_args_has_python_version_flag(build_args)
    ):
        with contextlib.suppress(ValueError):
            build_args.extend(
                [
                    "--python-version",
                    _parse_target_python_version(requested_python_selector).short,
                ]
            )
    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--build-profile", build_profile])
    if rebuild and not _build_args_has_cache_flag(build_args):
        build_args.append("--no-cache")
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compare requires emit=bin (got {emit_arg})",
            json_output,
            command="compare",
        )
    build_cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        *build_args,
    ]
    if module:
        build_cmd.extend(["--module", module])
    else:
        assert file_path is not None
        build_cmd.append(file_path)
    try:
        build_res = _run_command_timed(
            build_cmd,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=True,
            memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_res.returncode != 0:
        if json_output:
            data: dict[str, Any] = {
                "returncode": build_res.returncode,
                "timing": {"build_s": build_res.duration_s},
            }
            if build_res.stdout:
                data["build_stdout"] = build_res.stdout
            if build_res.stderr:
                data["build_stderr"] = build_res.stderr
            payload = _json_payload(
                "compare",
                "error",
                data=data,
                errors=["build failed"],
            )
            _emit_json(payload, json_output=True)
        else:
            err = build_res.stderr or build_res.stdout
            if err:
                print(err, end="", file=sys.stderr)
        return build_res.returncode

    try:
        build_payload = json.loads(build_res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        return _fail(
            "Failed to parse build JSON output.", json_output, command="compare"
        )
    output_str = build_payload.get("data", {}).get("output") or build_payload.get(
        "output"
    )
    if not output_str:
        return _fail(
            "Build output missing in JSON payload.", json_output, command="compare"
        )
    output_path = _resolve_binary_output(output_str)
    if output_path is None:
        return _fail(
            f"Compiled binary not found at {output_str}.",
            json_output,
            command="compare",
        )

    molt_res = _run_command_timed(
        [str(output_path), *script_args],
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )

    stdout_match = cpy_res.stdout == molt_res.stdout
    stderr_match = cpy_res.stderr == molt_res.stderr
    exit_match = cpy_res.returncode == molt_res.returncode
    compare_ok = stdout_match and stderr_match and exit_match

    if json_output:
        data = {
            "entry": str(source_path),
            "python": python_exe,
            "output": str(output_path),
            "returncodes": {
                "cpython": cpy_res.returncode,
                "molt": molt_res.returncode,
                "build": build_res.returncode,
            },
            "match": {
                "stdout": stdout_match,
                "stderr": stderr_match,
                "exitcode": exit_match,
            },
            "timing": {
                "cpython_run_s": cpy_res.duration_s,
                "molt_build_s": build_res.duration_s,
                "molt_run_s": molt_res.duration_s,
                "molt_total_s": build_res.duration_s + molt_res.duration_s,
            },
            "cpython_stdout": cpy_res.stdout,
            "cpython_stderr": cpy_res.stderr,
            "molt_stdout": molt_res.stdout,
            "molt_stderr": molt_res.stderr,
        }
        payload = _json_payload(
            "compare",
            "ok" if compare_ok else "error",
            data=data,
        )
        _emit_json(payload, json_output=True)
        return 0 if compare_ok else 1

    print("Compare (CPython vs Molt):")
    print(
        f"- CPython run: {_format_duration(cpy_res.duration_s)} "
        f"(rc={cpy_res.returncode})"
    )
    print(f"- Molt build: {_format_duration(build_res.duration_s)}")
    print(
        f"- Molt run: {_format_duration(molt_res.duration_s)} "
        f"(rc={molt_res.returncode})"
    )
    total = build_res.duration_s + molt_res.duration_s
    print(f"- Molt total: {_format_duration(total)}")
    if cpy_res.duration_s > 0 and molt_res.duration_s > 0:
        speedup = cpy_res.duration_s / molt_res.duration_s
        print(f"- Molt speedup (run): {speedup:.2f}x")
    print(
        "- Output match: "
        f"stdout={'yes' if stdout_match else 'no'}, "
        f"stderr={'yes' if stderr_match else 'no'}, "
        f"exitcode={'yes' if exit_match else 'no'}"
    )
    if not compare_ok:
        if not stdout_match:
            print(
                f"- Stdout mismatch: CPython={len(cpy_res.stdout)} bytes, "
                f"Molt={len(molt_res.stdout)} bytes"
            )
        if not stderr_match:
            print(
                f"- Stderr mismatch: CPython={len(cpy_res.stderr)} bytes, "
                f"Molt={len(molt_res.stderr)} bytes"
            )
        if not exit_match:
            print(
                f"- Exitcode mismatch: CPython={cpy_res.returncode}, "
                f"Molt={molt_res.returncode}"
            )
        if verbose:
            print("CPython stdout:")
            print(cpy_res.stdout, end="" if cpy_res.stdout.endswith("\n") else "\n")
            print("Molt stdout:")
            print(molt_res.stdout, end="" if molt_res.stdout.endswith("\n") else "\n")
            print("CPython stderr:", file=sys.stderr)
            print(
                cpy_res.stderr,
                end="" if cpy_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
            print("Molt stderr:", file=sys.stderr)
            print(
                molt_res.stderr,
                end="" if molt_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
    return 0 if compare_ok else 1


def parity_run(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="parity-run",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="parity-run")

    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}",
                json_output,
                command="parity-run",
            )

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))

    python_exe = _resolve_python_exe(python_exe)
    if module:
        command = [python_exe, "-m", module, *script_args]
    else:
        command = [python_exe, str(source_path), *script_args]

    if timing:
        run_res = _run_command_timed(
            command,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        if json_output:
            data: dict[str, Any] = {
                "python": python_exe,
                "entry": module if module is not None else str(source_path),
                "returncode": run_res.returncode,
                "timing": {"cpython_run_s": run_res.duration_s},
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "parity-run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (CPython parity-run):", file=sys.stderr)
            print(
                f"- run: {_format_duration(run_res.duration_s)} "
                f"(rc={run_res.returncode})",
                file=sys.stderr,
            )
        return run_res.returncode

    return _run_command(
        command,
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="parity-run",
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )


def diff(
    file_path: str | None,
    python_version: str | None,
    build_profile: BuildProfile | None = None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "diff")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    cmd = [sys.executable, "tests/molt_diff.py"]
    if python_version:
        cmd.extend(["--python-version", python_version])
    if build_profile is not None:
        cmd.extend(["--build-profile", build_profile])
    if file_path:
        cmd.append(file_path)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="diff",
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )


def _normalize_internal_batch_stdlib_profile(
    params: Mapping[str, Any],
) -> tuple[str | None, str | None]:
    raw = params.get("stdlib_profile", "micro")
    if not isinstance(raw, str):
        return None, "stdlib_profile must be a string"
    if raw not in {"micro", "full"}:
        return None, "stdlib_profile must be 'micro' or 'full'"
    return raw, None


def _internal_batch_build_server(
    *, json_output: bool = False, verbose: bool = False
) -> int:
    del json_output
    del verbose

    def _emit_response(payload: dict[str, Any]) -> None:
        sys.stdout.write(json.dumps(payload, sort_keys=True) + "\n")
        sys.stdout.flush()

    for raw_line in sys.stdin:
        if not raw_line.strip():
            continue
        req_id: Any = None
        try:
            request = json.loads(raw_line)
        except json.JSONDecodeError as exc:
            _emit_response(
                {
                    "id": None,
                    "ok": False,
                    "error": f"invalid request JSON: {exc}",
                }
            )
            continue
        if not isinstance(request, dict):
            _emit_response(
                {"id": None, "ok": False, "error": "request must be an object"}
            )
            continue
        req_id = request.get("id")
        op = request.get("op")
        if op == "ping":
            _emit_response({"id": req_id, "ok": True, "pong": True})
            continue
        if op == "shutdown":
            _emit_response({"id": req_id, "ok": True, "shutdown": True})
            return 0
        if op != "build":
            _emit_response(
                {"id": req_id, "ok": False, "error": f"unsupported op: {op!r}"}
            )
            continue

        params = request.get("params")
        if not isinstance(params, dict):
            _emit_response({"id": req_id, "ok": False, "error": "missing build params"})
            continue
        env_overrides_raw = params.get("env_overrides", {})
        if not isinstance(env_overrides_raw, dict) or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in env_overrides_raw.items()
        ):
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": "env_overrides must be a string->string object",
                }
            )
            continue
        env_overrides: dict[str, str] = dict(env_overrides_raw)
        stdlib_profile, stdlib_profile_error = _normalize_internal_batch_stdlib_profile(
            params
        )
        if stdlib_profile_error is not None:
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": stdlib_profile_error,
                }
            )
            continue
        assert stdlib_profile is not None
        env_overrides["MOLT_STDLIB_PROFILE"] = stdlib_profile
        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        try:
            with _temporary_env_overrides(env_overrides):
                with redirect_stdout(stdout_buf), redirect_stderr(stderr_buf):
                    rc = build(
                        file_path=params.get("file_path"),
                        target=cast(Target, params.get("target", "native")),
                        parse_codec=cast(ParseCodec, params.get("codec", "msgpack")),
                        type_hint_policy=cast(
                            TypeHintPolicy, params.get("type_hints", "check")
                        ),
                        fallback_policy=cast(
                            FallbackPolicy, params.get("fallback", "error")
                        ),
                        type_facts_path=params.get("type_facts"),
                        pgo_profile=params.get("pgo_profile"),
                        runtime_feedback=params.get("runtime_feedback"),
                        output=params.get("output"),
                        json_output=bool(params.get("json_output", False)),
                        verbose=bool(params.get("verbose", False)),
                        deterministic=bool(params.get("deterministic", True)),
                        deterministic_warn=bool(
                            params.get("deterministic_warn", False)
                        ),
                        trusted=bool(params.get("trusted", False)),
                        capabilities=params.get("capabilities"),
                        cache=bool(params.get("cache", True)),
                        cache_dir=params.get("cache_dir"),
                        cache_report=bool(params.get("cache_report", False)),
                        sysroot=params.get("sysroot"),
                        emit_ir=params.get("emit_ir"),
                        emit=cast(EmitMode | None, params.get("emit")),
                        out_dir=params.get("out_dir"),
                        profile=cast(BuildProfile, params.get("profile", "dev")),
                        linked=bool(params.get("linked", False)),
                        linked_output=params.get("linked_output"),
                        require_linked=bool(params.get("require_linked", False)),
                        respect_pythonpath=bool(
                            params.get("respect_pythonpath", False)
                        ),
                        module=params.get("module"),
                        diagnostics_verbosity=params.get("diagnostics_verbosity"),
                        python_version=params.get("python_version"),
                        stdlib_profile=stdlib_profile,
                    )
        except Exception as exc:  # pragma: no cover - defensive server hardening
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": f"batch build server exception: {exc}",
                    "stdout": stdout_buf.getvalue(),
                    "stderr": stderr_buf.getvalue(),
                }
            )
            continue
        _emit_response(
            {
                "id": req_id,
                "ok": rc == 0,
                "returncode": rc,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        )
    return 0


def lint(json_output: bool = False, verbose: bool = False) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "lint")
    if root_error is not None:
        return root_error
    project = DxProject(root)
    try:
        env = project.canonical_env()
        project.require_project_python("lint")
        commands = project.split_command_sequence(
            project.commands().get("lint"), "lint"
        )
    except DxConfigError as exc:
        if json_output:
            _emit_json(
                _json_payload("lint", "error", errors=[str(exc)]),
                json_output=True,
            )
        else:
            print(f"lint: {exc}", file=sys.stderr)
        return 2
    results: list[dict[str, Any]] = []
    for cmd in commands:
        if verbose and not json_output:
            print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
        result = _run_completed_command(
            [str(part) for part in cmd],
            cwd=root,
            env=env,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        result_data: dict[str, Any] = {
            "cmd": cmd,
            "returncode": result.returncode,
        }
        if json_output:
            if result.stdout:
                result_data["stdout"] = result.stdout
            if result.stderr:
                result_data["stderr"] = result.stderr
        results.append(result_data)
        if result.returncode != 0:
            if json_output:
                _emit_json(
                    _json_payload(
                        "lint",
                        "error",
                        data={"commands": results},
                    ),
                    json_output=True,
                )
            return result.returncode
    if json_output:
        _emit_json(
            _json_payload(
                "lint",
                "ok",
                data={"returncode": 0, "commands": results},
            ),
            json_output=True,
        )
    return 0


def test(
    suite: str,
    file_path: str | None,
    python_version: str | None,
    pytest_args: list[str],
    build_profile: BuildProfile | None = None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "test")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if suite == "dev":
        cmd = [sys.executable, "tools/dev.py", "test"]
    elif suite == "diff":
        cmd = [sys.executable, "tests/molt_diff.py"]
        if python_version:
            cmd.extend(["--python-version", python_version])
        if build_profile is not None:
            cmd.extend(["--build-profile", build_profile])
        if file_path:
            cmd.append(file_path)
    else:
        cmd = ["uv", "run", "--python", "3.12", "pytest", "-q"]
        if file_path:
            cmd.append(file_path)
        cmd.extend(pytest_args)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="test",
        memory_guard_prefix="MOLT_DIFF" if suite == "diff" else "MOLT_TEST_SUITE",
    )


def bench(
    wasm: bool,
    bench_args: list[str],
    bench_script: list[str] | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "bench")
    if root_error is not None:
        return root_error
    tool = "tools/bench_wasm.py" if wasm else "tools/bench.py"
    cmd = [sys.executable, tool]
    for script in bench_script or []:
        cmd.extend(["--script", script])
    cmd.extend(bench_args)
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="bench",
        memory_guard_prefix="MOLT_BENCH",
    )


def profile(
    profile_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "profile")
    if root_error is not None:
        return root_error
    cmd = [sys.executable, "tools/profile.py", *profile_args]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="profile",
        memory_guard_prefix="MOLT_BENCH",
    )


def extension_build(
    project: str | None = None,
    out_dir: str | None = None,
    molt_abi: str | None = None,
    capabilities: CapabilityInput | None = None,
    deterministic: bool = True,
    profile: BuildProfile = "release",
    target: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    project_root = Path(project).expanduser() if project else Path.cwd()
    if not project_root.is_absolute():
        project_root = (Path.cwd() / project_root).absolute()
    if not project_root.exists() or not project_root.is_dir():
        return _fail(
            f"Project directory not found: {project_root}",
            json_output,
            command="extension-build",
        )

    pyproject = _load_toml(project_root / "pyproject.toml")
    project_meta = pyproject.get("project")
    extension_meta = _config_value(pyproject, ["tool", "molt", "extension"])
    errors: list[str] = []
    warnings: list[str] = []

    if not isinstance(project_meta, dict):
        return _fail(
            "pyproject.toml must contain a [project] table.",
            json_output,
            command="extension-build",
        )
    if not isinstance(extension_meta, dict):
        return _fail(
            "pyproject.toml must contain [tool.molt.extension].",
            json_output,
            command="extension-build",
        )

    project_name = project_meta.get("name")
    project_version = project_meta.get("version")
    if not isinstance(project_name, str) or not project_name.strip():
        errors.append("project.name must be a non-empty string")
    if not isinstance(project_version, str) or not project_version.strip():
        errors.append("project.version must be a non-empty string")

    module_name = extension_meta.get("module")
    if not isinstance(module_name, str):
        errors.append("tool.molt.extension.module must be a string")
        module_name = ""
    module_parts = _module_parts(module_name)
    if module_parts is None:
        errors.append("tool.molt.extension.module must be a dotted Python identifier")
        module_parts = ["extension"]

    raw_sources = _coerce_str_list(
        extension_meta.get("sources"),
        "tool.molt.extension.sources",
        errors,
        allow_empty=False,
    )
    if not raw_sources:
        errors.append("tool.molt.extension.sources must include at least one source")
    source_paths: list[Path] = []
    for entry in raw_sources:
        source_path = Path(entry).expanduser()
        if not source_path.is_absolute():
            source_path = (project_root / source_path).absolute()
        if not source_path.exists() or not source_path.is_file():
            errors.append(f"source file not found: {source_path}")
            continue
        source_paths.append(source_path)

    include_dirs_raw = _coerce_str_list(
        extension_meta.get("include_dirs") or extension_meta.get("include-dirs"),
        "tool.molt.extension.include_dirs",
        errors,
    )
    include_paths: list[Path] = []
    for entry in include_dirs_raw:
        include_path = Path(entry).expanduser()
        if not include_path.is_absolute():
            include_path = (project_root / include_path).absolute()
        include_paths.append(include_path)

    compile_args = _coerce_str_list(
        extension_meta.get("extra_compile_args")
        or extension_meta.get("extra-compile-args"),
        "tool.molt.extension.extra_compile_args",
        errors,
    )
    link_args = _coerce_str_list(
        extension_meta.get("extra_link_args") or extension_meta.get("extra-link-args"),
        "tool.molt.extension.extra_link_args",
        errors,
    )

    effects = _normalize_effects(extension_meta.get("effects"))
    determinism_mode = "deterministic" if deterministic else "nondet"
    determinism_raw = extension_meta.get("determinism")
    if determinism_raw is not None:
        if not isinstance(determinism_raw, str):
            errors.append(
                "tool.molt.extension.determinism must be 'deterministic' or 'nondet'"
            )
        else:
            normalized = determinism_raw.strip().lower()
            if normalized not in {"deterministic", "nondet"}:
                errors.append(
                    "tool.molt.extension.determinism must be 'deterministic' or "
                    "'nondet'"
                )
            else:
                determinism_mode = normalized
    if deterministic:
        determinism_mode = "deterministic"

    requested_target = (target or "native").strip()
    if not requested_target:
        requested_target = "native"
    normalized_target = requested_target.lower()
    runtime_target_triple: str | None = None
    manifest_target_triple = _host_target_triple()
    if normalized_target == "native":
        runtime_target_triple = None
    elif normalized_target == "wasm" or normalized_target.startswith("wasm32"):
        errors.append("Extension build only supports native target triples, not wasm")
    else:
        if any(ch.isspace() for ch in requested_target):
            errors.append("target must be 'native' or a Rust target triple")
        runtime_target_triple = normalized_target
        manifest_target_triple = normalized_target

    capability_input: CapabilityInput | None = capabilities
    if capability_input is None:
        cfg_capabilities = extension_meta.get("capabilities")
        if isinstance(cfg_capabilities, (str, list, dict)):
            capability_input = cfg_capabilities
    if capability_input is None:
        errors.append(
            "Missing extension capabilities: set tool.molt.extension.capabilities "
            "or pass --capabilities."
        )
    capabilities_list: list[str] = []
    capability_profiles: list[str] = []
    if capability_input is not None:
        spec = _parse_capabilities_spec(capability_input)
        if spec.errors:
            errors.append("Invalid capabilities: " + ", ".join(spec.errors))
        else:
            capabilities_list = spec.capabilities or []
            capability_profiles = spec.profiles

    cwd_root = _find_project_root(Path.cwd())
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "extension-build")
    if root_error is not None:
        return root_error

    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        False,
        "extension-build",
    )
    if lock_error is not None:
        return lock_error

    default_abi = _default_molt_c_api_version(molt_root)
    abi_raw = molt_abi or extension_meta.get("molt_c_api_version") or default_abi
    if not isinstance(abi_raw, str):
        errors.append("molt ABI must be a string")
        abi_raw = default_abi
    abi_version = abi_raw.strip()
    if _MOLT_C_API_VERSION_RE.match(abi_version) is None:
        errors.append(
            "Invalid molt ABI version. Expected MAJOR[.MINOR[.PATCH]] "
            f"(got {abi_version!r})."
        )
    abi_major = abi_version.split(".", 1)[0] if abi_version else "0"
    abi_tag = f"molt_abi{abi_major}"

    if errors:
        return _fail(
            "Extension build configuration errors: " + "; ".join(errors),
            json_output,
            command="extension-build",
        )

    output_root = Path(out_dir).expanduser() if out_dir else Path("dist")
    if not output_root.is_absolute():
        output_root = (project_root / output_root).absolute()
    output_root.mkdir(parents=True, exist_ok=True)

    cargo_timeout, timeout_err = _resolve_timeout_env("MOLT_CARGO_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="extension-build")
    runtime_cargo_profile, runtime_profile_err = _resolve_cargo_profile_name("release")
    if runtime_profile_err:
        return _fail(runtime_profile_err, json_output, command="extension-build")
    if runtime_target_triple:
        _ensure_rustup_target(runtime_target_triple, warnings)
    runtime_lib = _runtime_lib_path(
        molt_root,
        runtime_cargo_profile,
        runtime_target_triple,
    )
    if not _ensure_runtime_lib(
        runtime_lib,
        runtime_target_triple,
        json_output,
        runtime_cargo_profile,
        molt_root,
        cargo_timeout,
    ):
        return _fail("Runtime build failed", json_output, command="extension-build")

    include_root = molt_root / "include"
    if not include_root.exists():
        return _fail(
            f"Missing Molt header root: {include_root}",
            json_output,
            command="extension-build",
        )

    cc = os.environ.get("CC", "clang")
    cc_cmd = shlex.split(cc)
    if not cc_cmd:
        return _fail(
            "Compiler command is empty. Set CC or install clang.",
            json_output,
            command="extension-build",
        )
    if runtime_target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = runtime_target_triple
        if cross_cc:
            cc_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            cc_cmd = ["zig", "cc"]
            normalized = _zig_target_query(runtime_target_triple)
            if normalized != runtime_target_triple:
                warnings.append(
                    f"Zig target normalized to {normalized} from {runtime_target_triple}."
                )
            target_arg = normalized
        else:
            return _fail(
                "Cross-target extension build requires zig or MOLT_CROSS_CC "
                f"(missing for {runtime_target_triple}).",
                json_output,
                command="extension-build",
            )
        if not cc_cmd:
            return _fail(
                "Compiler command is empty. Set MOLT_CROSS_CC or install zig.",
                json_output,
                command="extension-build",
            )
        cc_cmd.extend(["-target", target_arg])

    dist_name = _normalize_name(str(project_name)).replace("-", "_")
    wheel_version = _wheel_version_token(str(project_version))
    target_triple = manifest_target_triple
    platform_tag = _wheel_token(target_triple)
    python_tag = "py3"
    wheel_name = (
        f"{dist_name}-{wheel_version}-{python_tag}-{abi_tag}-{platform_tag}.whl"
    )
    wheel_path = output_root / wheel_name

    build_env = os.environ.copy()
    # Supply-chain: always set SOURCE_DATE_EPOCH for release builds for reproducibility
    if deterministic or profile == "release":
        build_env.setdefault("SOURCE_DATE_EPOCH", "315532800")

    module_rel = Path(
        *module_parts[:-1],
        module_parts[-1] + _extension_binary_suffix(runtime_target_triple),
    )
    compile_commands: list[list[str]] = []
    link_command: list[str] = []

    with tempfile.TemporaryDirectory(prefix="molt_ext_build_", dir=output_root) as td:
        build_tmp = Path(td)
        object_paths: list[Path] = []
        for idx, source_path in enumerate(source_paths):
            object_path = build_tmp / f"{idx}_{source_path.stem}.o"
            cmd = [*cc_cmd, "-c", str(source_path), "-o", str(object_path)]
            cmd.extend(["-I", str(include_root), "-I", str(project_root)])
            for include_path in include_paths:
                cmd.extend(["-I", str(include_path)])
            if os.name != "nt":
                cmd.append("-fPIC")
            if deterministic:
                prefix = str(project_root)
                cmd.append(f"-ffile-prefix-map={prefix}=.")
                cmd.append(f"-fdebug-prefix-map={prefix}=.")
            cmd.extend(compile_args)
            result = _run_completed_command(
                cmd,
                cwd=project_root,
                env=build_env,
                capture_output=True,
                memory_guard_prefix="MOLT_BUILD",
            )
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip()
                if not detail:
                    detail = f"compiler exited with code {result.returncode}"
                return _fail(
                    f"Failed compiling {source_path.name}: {detail}",
                    json_output,
                    command="extension-build",
                )
            compile_commands.append(cmd)
            object_paths.append(object_path)

        built_extension = build_tmp / module_rel
        built_extension.parent.mkdir(parents=True, exist_ok=True)
        link_command = [*cc_cmd, "-shared"]
        link_command.extend(str(path) for path in object_paths)
        link_command.append(str(runtime_lib))
        link_command.extend(["-o", str(built_extension)])
        _append_darwin_runtime_frameworks(
            link_command,
            target_triple=runtime_target_triple,
        )
        if sys.platform == "darwin" and runtime_target_triple is None:
            link_command.extend(["-undefined", "dynamic_lookup"])
            for framework in ["Metal", "Foundation", "QuartzCore", "AppKit"]:
                has_framework = any(
                    link_command[idx] == "-framework"
                    and idx + 1 < len(link_command)
                    and link_command[idx + 1] == framework
                    for idx in range(len(link_command))
                )
                if not has_framework:
                    link_command.extend(["-framework", framework])
            if "-lobjc" not in link_command:
                link_command.append("-lobjc")
        cargo_search, cargo_libs = _collect_cargo_native_link_deps(runtime_lib)
        link_command.extend(cargo_search)
        link_command.extend(cargo_libs)
        link_command.extend(link_args)
        link_result = _run_completed_command(
            link_command,
            cwd=project_root,
            env=build_env,
            capture_output=True,
            memory_guard_prefix="MOLT_BUILD",
        )
        if link_result.returncode != 0:
            detail = link_result.stderr.strip() or link_result.stdout.strip()
            if not detail:
                detail = f"linker exited with code {link_result.returncode}"
            return _fail(
                f"Failed linking extension module: {detail}",
                json_output,
                command="extension-build",
            )

        if not built_extension.exists():
            return _fail(
                "Link succeeded but extension artifact is missing.",
                json_output,
                command="extension-build",
            )

        extension_bytes = built_extension.read_bytes()
        extension_archive_path = module_rel.as_posix()
        manifest_payload: dict[str, Any] = {
            "schema_version": 1,
            "name": str(project_name),
            "version": str(project_version),
            "module": ".".join(module_parts),
            "sources": [str(path) for path in source_paths],
            "molt_c_api_version": abi_version,
            "abi_tag": abi_tag,
            "python_tag": python_tag,
            "target_triple": target_triple,
            "platform_tag": platform_tag,
            "capabilities": capabilities_list,
            "capability_profiles": capability_profiles,
            "deterministic": deterministic,
            "determinism": determinism_mode,
            "effects": effects,
            "wheel": wheel_name,
            "extension": extension_archive_path,
            "build": {
                "compiler": cc_cmd,
                "compiler_target": runtime_target_triple or "native",
                "runtime_lib": str(runtime_lib),
                "include_dirs": [str(include_root), str(project_root)]
                + [str(path) for path in include_paths],
                "extra_compile_args": compile_args,
                "extra_link_args": link_args,
            },
        }
        manifest_bytes = (
            json.dumps(manifest_payload, sort_keys=True, indent=2).encode("utf-8")
            + b"\n"
        )

        dist_info = f"{dist_name}-{wheel_version}.dist-info"
        wheel_metadata = "\n".join(
            [
                "Wheel-Version: 1.0",
                "Generator: molt extension build",
                "Root-Is-Purelib: false",
                f"Tag: {python_tag}-{abi_tag}-{platform_tag}",
                "",
            ]
        ).encode("utf-8")
        package_metadata = "\n".join(
            [
                "Metadata-Version: 2.1",
                f"Name: {project_name}",
                f"Version: {project_version}",
                "Summary: Molt C extension package",
                "",
            ]
        ).encode("utf-8")

        wheel_entries: list[tuple[str, bytes]] = [
            (extension_archive_path, extension_bytes),
            ("extension_manifest.json", manifest_bytes),
            (f"{dist_info}/WHEEL", wheel_metadata),
            (f"{dist_info}/METADATA", package_metadata),
        ]
        record_path = f"{dist_info}/RECORD"
        record_lines = [_wheel_record_line(path, data) for path, data in wheel_entries]
        record_lines.append(f"{record_path},,")
        record_bytes = ("\n".join(record_lines) + "\n").encode("utf-8")

        with _atomic_zip_file(wheel_path) as zf:
            for path, data in wheel_entries:
                _write_zip_member(zf, path, data)
            _write_zip_member(zf, record_path, record_bytes)

    wheel_sha = _sha256_file(wheel_path)
    extension_sha = hashlib.sha256(extension_bytes).hexdigest()
    sidecar_payload = dict(manifest_payload)
    sidecar_payload["wheel_sha256"] = wheel_sha
    sidecar_payload["extension_sha256"] = extension_sha
    if deterministic:
        sidecar_payload["generated_at_utc"] = "1970-01-01T00:00:00Z"
    else:
        sidecar_payload["generated_at_utc"] = (
            dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
        )
    manifest_path = output_root / "extension_manifest.json"
    _atomic_write_json(manifest_path, sidecar_payload, sort_keys=True, indent=2)

    if json_output:
        payload = _json_payload(
            "extension-build",
            "ok",
            data={
                "project": str(project_root),
                "wheel": str(wheel_path),
                "manifest": str(manifest_path),
                "module": ".".join(module_parts),
                "molt_c_api_version": abi_version,
                "abi_tag": abi_tag,
                "target_triple": target_triple,
                "build_target": runtime_target_triple or "native",
                "platform_tag": platform_tag,
                "deterministic": deterministic,
                "determinism": determinism_mode,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "wheel_sha256": wheel_sha,
                "extension_sha256": extension_sha,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Built extension wheel: {wheel_path}")
        print(f"Wrote extension manifest: {manifest_path}")
        if verbose:
            print(f"Target triple: {target_triple}")
            print(f"Build target: {runtime_target_triple or 'native'}")
            print(f"Molt C API version: {abi_version}")
            print(f"Capabilities: {json.dumps(capabilities_list)}")
            print(f"Compile steps: {len(compile_commands)}")
    return 0



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
        return _internal_batch_build_server(
            json_output=args.json,
            verbose=args.verbose,
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
            return extension_build(
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
            return _run_script_cross(
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
            return _run_script_cross(
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
        return run_script(
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
        return compare(
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
        return parity_run(
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
        return test(
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
        return diff(
            args.path,
            args.python_version,
            cast(BuildProfile | None, diff_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        return bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.bench_script,
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        return profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        return lint(args.json, args.verbose)
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
        return _deploy(
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
