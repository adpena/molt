"""The ``molt.cli`` package facade: PEP 562 lazy re-exports.

``molt.cli/__init__`` binds this module's ``__getattr__`` and ``__dir__`` as its
own, so ``from molt.cli import <name>`` resolves post-lowering names on first
access while ``import molt.cli`` stays backend-free. The registry below is the
single authority for those names; ``python_source_closure.toml`` derives the
package's dynamic import targets from it.
"""

from __future__ import annotations

import importlib
import sys

_PACKAGE = "molt.cli"


def _package_namespace() -> dict[str, object]:
    return sys.modules[_PACKAGE].__dict__


# --- Lazy post-lowering re-exports (PEP 562) -------------------------------
# The frontend import-scan / analysis / lowering caches key their tooling
# fingerprint on the set of source files reachable *by module-level import*
# from the frontend/module drivers. Importing ``molt.cli`` therefore must not
# eagerly pull the backend / native-link / cargo / daemon / toolchain layer:
# those are needed only when a build actually runs, never to compute a
# lowering. Each post-lowering submodule's public names are re-exported
# lazily below so ``from molt.cli import <name>`` keeps working (resolved on
# first access) while ``import molt.cli`` stays backend-free and the static
# lowering-scope reachability excludes the backend. ``None`` as the source
# attribute means the exported name is the submodule object itself.
_LAZY_REEXPORTS: dict[str, tuple[str, str | None]] = {
    "_ARTIFACT_SYNC_STATE_CACHE": ("artifact_sync", "_ARTIFACT_SYNC_STATE_CACHE"),
    "_BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION": (
        "backend_execution",
        "_BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION",
    ),
    "_BACKEND_CODEGEN_REQUEST_ENV_KNOBS": (
        "backend_execution",
        "_BACKEND_CODEGEN_REQUEST_ENV_KNOBS",
    ),
    "_BACKEND_DAEMON_ORPHAN_SWEEP_DONE": (
        "backend_execution",
        "_BACKEND_DAEMON_ORPHAN_SWEEP_DONE",
    ),
    "_BACKEND_DAEMON_PROTOCOL_VERSION": (
        "backend_execution",
        "_BACKEND_DAEMON_PROTOCOL_VERSION",
    ),
    "_BACKEND_DIAGNOSTIC_ENV_KNOBS": (
        "backend_diagnostics",
        "_BACKEND_DIAGNOSTIC_ENV_KNOBS",
    ),
    "_BACKEND_REQUEST_ENV_KNOBS": ("backend_execution", "_BACKEND_REQUEST_ENV_KNOBS"),
    "_BACKEND_RESOURCE_ENV_KNOBS": ("backend_execution", "_BACKEND_RESOURCE_ENV_KNOBS"),
    "_BUILD_ESSENTIAL_FLAGS": ("arg_helpers", "_BUILD_ESSENTIAL_FLAGS"),
    "_BackendDaemonIdentity": ("backend_execution", "_BackendDaemonIdentity"),
    "_BuildHelpFormatter": ("arg_helpers", "_BuildHelpFormatter"),
    "_CARGO_PROFILE_NAME_RE": ("cargo_profiles", "_CARGO_PROFILE_NAME_RE"),
    "_DAEMON_CONFIG_DIGEST_SCHEMA_VERSION": (
        "backend_execution",
        "_DAEMON_CONFIG_DIGEST_SCHEMA_VERSION",
    ),
    "_DEFAULT_BACKEND_FEATURES": ("backend_execution", "_DEFAULT_BACKEND_FEATURES"),
    "_FALSY_ENV_VALUES": ("backend_diagnostics", "_FALSY_ENV_VALUES"),
    "_MoltHelpFormatter": ("arg_helpers", "_MoltHelpFormatter"),
    "_NATIVE_CODEGEN_ENV_KNOBS": ("backend_execution", "_NATIVE_CODEGEN_ENV_KNOBS"),
    "_NativeBinaryInvalid": ("native_binary", "_NativeBinaryInvalid"),
    "_PYTHON_WARNING_RE": ("backend_diagnostics", "_PYTHON_WARNING_RE"),
    "_SHARED_STDLIB_CACHE_SCHEMA_VERSION": (
        "backend_cache",
        "_SHARED_STDLIB_CACHE_SCHEMA_VERSION",
    ),
    "_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION": (
        "backend_cache",
        "_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION",
    ),
    "_SHARED_STDLIB_PARTITION_SCHEMA_VERSION": (
        "backend_cache",
        "_SHARED_STDLIB_PARTITION_SCHEMA_VERSION",
    ),
    "_VALIDATE_PROOF_BYPASS_ENV": (
        "toolchain_validation",
        "_VALIDATE_PROOF_BYPASS_ENV",
    ),
    "_VALIDATE_SUITE_CHOICES": ("toolchain_validation", "_VALIDATE_SUITE_CHOICES"),
    "_WASM_CODEGEN_ENV_KNOBS": ("backend_execution", "_WASM_CODEGEN_ENV_KNOBS"),
    "_active_artifact_profile_dirs": (
        "cargo_profiles",
        "_active_artifact_profile_dirs",
    ),
    "_add_debug_shared_selector_args": (
        "arg_helpers",
        "_add_debug_shared_selector_args",
    ),
    "_append_darwin_runtime_frameworks": (
        "native_toolchain",
        "_append_darwin_runtime_frameworks",
    ),
    "_artifact_content_looks_valid": (
        "runtime_fingerprints",
        "_artifact_content_looks_valid",
    ),
    "_artifact_needs_rebuild": ("runtime_fingerprints", "_artifact_needs_rebuild"),
    "_artifact_sync_state_matches": ("artifact_sync", "_artifact_sync_state_matches"),
    "_artifact_sync_state_path": ("artifact_sync", "_artifact_sync_state_path"),
    "_assert_native_binary_valid": ("native_binary", "_assert_native_binary_valid"),
    "_backend_artifact_source_key": ("backend_cache", "_backend_artifact_source_key"),
    "_backend_bin_path": ("backend_execution", "_backend_bin_path"),
    "_backend_bin_path_cached": ("backend_execution", "_backend_bin_path_cached"),
    "_backend_binary_identity": ("backend_execution", "_backend_binary_identity"),
    "_backend_cache_artifact_path": ("backend_cache", "_backend_cache_artifact_path"),
    "_backend_codegen_env_digest": ("backend_execution", "_backend_codegen_env_digest"),
    "_backend_codegen_env_inputs": ("backend_execution", "_backend_codegen_env_inputs"),
    "_backend_codegen_env_inputs_cached": (
        "backend_execution",
        "_backend_codegen_env_inputs_cached",
    ),
    "_backend_daemon_binary_is_newer": (
        "backend_execution",
        "_backend_daemon_binary_is_newer",
    ),
    "_backend_daemon_command_has_socket": (
        "backend_execution",
        "_backend_daemon_command_has_socket",
    ),
    "_backend_daemon_command_matches_identity": (
        "backend_execution",
        "_backend_daemon_command_matches_identity",
    ),
    "_backend_daemon_compile_request_bytes": (
        "backend_execution",
        "_backend_daemon_compile_request_bytes",
    ),
    "_backend_daemon_config_digest": (
        "backend_execution",
        "_backend_daemon_config_digest",
    ),
    "_backend_daemon_empty_response_error": (
        "backend_execution",
        "_backend_daemon_empty_response_error",
    ),
    "_backend_daemon_enabled": ("backend_daemon_config", "_backend_daemon_enabled"),
    "_backend_daemon_enabled_cached": (
        "backend_daemon_config",
        "_backend_daemon_enabled_cached",
    ),
    "_backend_daemon_freshness_inputs": (
        "backend_execution",
        "_backend_daemon_freshness_inputs",
    ),
    "_backend_daemon_health_from_response": (
        "backend_execution",
        "_backend_daemon_health_from_response",
    ),
    "_backend_daemon_health_probe": (
        "backend_execution",
        "_backend_daemon_health_probe",
    ),
    "_backend_daemon_identity_for_pid": (
        "backend_execution",
        "_backend_daemon_identity_for_pid",
    ),
    "_backend_daemon_identity_from_health": (
        "backend_execution",
        "_backend_daemon_identity_from_health",
    ),
    "_backend_daemon_identity_is_verified": (
        "backend_execution",
        "_backend_daemon_identity_is_verified",
    ),
    "_backend_daemon_identity_matches_context": (
        "backend_execution",
        "_backend_daemon_identity_matches_context",
    ),
    "_backend_daemon_identity_path": (
        "backend_execution",
        "_backend_daemon_identity_path",
    ),
    "_backend_daemon_identity_process_matches": (
        "backend_execution",
        "_backend_daemon_identity_process_matches",
    ),
    "_backend_daemon_job_failure_message": (
        "backend_execution",
        "_backend_daemon_job_failure_message",
    ),
    "_backend_daemon_log_mark": ("backend_daemon_logs", "_backend_daemon_log_mark"),
    "_backend_daemon_log_max_bytes": (
        "backend_daemon_logs",
        "_backend_daemon_log_max_bytes",
    ),
    "_backend_daemon_log_max_bytes_cached": (
        "backend_daemon_logs",
        "_backend_daemon_log_max_bytes_cached",
    ),
    "_backend_daemon_log_path": ("backend_execution", "_backend_daemon_log_path"),
    "_backend_daemon_log_since": ("backend_daemon_logs", "_backend_daemon_log_since"),
    "_backend_daemon_log_tail": ("backend_daemon_logs", "_backend_daemon_log_tail"),
    "_backend_daemon_paths_bundle": ("backend_daemon_paths", "_backend_daemon_paths"),
    "_backend_daemon_paths_cached": (
        "backend_execution",
        "_backend_daemon_paths_cached",
    ),
    "_backend_daemon_ping": ("backend_execution", "_backend_daemon_ping"),
    "_backend_daemon_ping_health": ("backend_execution", "_backend_daemon_ping_health"),
    "_backend_daemon_process_command": (
        "backend_execution",
        "_backend_daemon_process_command",
    ),
    "_backend_daemon_request": ("backend_execution", "_backend_daemon_request"),
    "_backend_daemon_request_bytes": (
        "backend_execution",
        "_backend_daemon_request_bytes",
    ),
    "_backend_daemon_request_on_socket": (
        "backend_execution",
        "_backend_daemon_request_on_socket",
    ),
    "_backend_daemon_request_payload_bytes": (
        "backend_execution",
        "_backend_daemon_request_payload_bytes",
    ),
    "_backend_daemon_response_failure_message": (
        "backend_execution",
        "_backend_daemon_response_failure_message",
    ),
    "_backend_daemon_retryable_error": (
        "backend_execution",
        "_backend_daemon_retryable_error",
    ),
    "_backend_daemon_skip_output_sync_flags": (
        "backend_cache",
        "_backend_daemon_skip_output_sync_flags",
    ),
    "_backend_daemon_socket_dir": ("backend_execution", "_backend_daemon_socket_dir"),
    "_backend_daemon_socket_path": ("backend_execution", "_backend_daemon_socket_path"),
    "_backend_daemon_socket_path_error": (
        "backend_daemon_paths",
        "_backend_daemon_socket_path_error",
    ),
    "_backend_daemon_spawn_probe_timeout": (
        "backend_daemon_startup",
        "_backend_daemon_spawn_probe_timeout",
    ),
    "_backend_daemon_start_timeout": (
        "backend_daemon_startup",
        "_backend_daemon_start_timeout",
    ),
    "_backend_daemon_start_timeout_cached": (
        "backend_daemon_startup",
        "_backend_daemon_start_timeout_cached",
    ),
    "_backend_daemon_text_field": ("backend_execution", "_backend_daemon_text_field"),
    "_backend_daemon_wait_until_ready": (
        "backend_execution",
        "_backend_daemon_wait_until_ready",
    ),
    "_backend_features_for_build_target": (
        "backend_execution",
        "_backend_features_for_build_target",
    ),
    "_backend_features_for_target": (
        "backend_execution",
        "_backend_features_for_target",
    ),
    "_backend_ir": ("backend_ir", None),
    "_build_args_has_cache_flag": ("arg_helpers", "_build_args_has_cache_flag"),
    "_build_args_has_capabilities_flag": (
        "arg_helpers",
        "_build_args_has_capabilities_flag",
    ),
    "_build_args_has_json_flag": ("wrapper_build", "_build_args_has_json_flag"),
    "_build_args_has_profile_flag": ("arg_helpers", "_build_args_has_profile_flag"),
    "_build_args_has_python_version_flag": (
        "wrapper_build",
        "_build_args_has_python_version_flag",
    ),
    "_build_args_has_trusted_flag": ("arg_helpers", "_build_args_has_trusted_flag"),
    "_build_native_link_driver_command": (
        "native_link_command",
        "_build_native_link_driver_command",
    ),
    "_build_native_link_plan": ("native_link_command", "_build_native_link_plan"),
    "_build_slot": ("cargo_execution", "_build_slot"),
    "_build_toolchain_report": ("setup_readiness", "_build_toolchain_report"),
    "_canonical_env_defaults": ("setup_readiness", "_canonical_env_defaults"),
    "_capture_json_cli_result": ("debug_helpers", "_capture_json_cli_result"),
    "_cargo_build_env": ("cargo_execution", "_cargo_build_env"),
    "_cargo_setup_advice": ("setup_readiness", "_cargo_setup_advice"),
    "_clang_setup_advice": ("setup_readiness", "_clang_setup_advice"),
    "_cli_hash_seed_reexec_argv": ("arg_helpers", "_cli_hash_seed_reexec_argv"),
    "_codesign_binary": ("native_toolchain", "_codesign_binary"),
    "_collect_cargo_native_link_deps": (
        "native_link_deps",
        "_collect_cargo_native_link_deps",
    ),
    "_collect_setup_actions": ("setup_readiness", "_collect_setup_actions"),
    "_command_executable_matches_backend": (
        "backend_execution",
        "_command_executable_matches_backend",
    ),
    "_command_has_path_separator": ("backend_execution", "_command_has_path_separator"),
    "_compile_with_backend_daemon": (
        "backend_execution",
        "_compile_with_backend_daemon",
    ),
    "_completion_script": ("completion", "_completion_script"),
    "_darwin_binary_imports_validation_error": (
        "native_binary",
        "_darwin_binary_imports_validation_error",
    ),
    "_darwin_binary_magic_error": ("native_binary", "_darwin_binary_magic_error"),
    "_debug_eval_base_env": ("debug_helpers", "_debug_eval_base_env"),
    "_debug_helpers": ("debug_helpers", None),
    "_default_validate_summary_path": (
        "toolchain_validation",
        "_default_validate_summary_path",
    ),
    "_detect_llvm_backend_toolchain": (
        "setup_readiness",
        "_detect_llvm_backend_toolchain",
    ),
    "_detect_macos_arch": ("native_toolchain", "_detect_macos_arch"),
    "_effective_split_worker_table_base": (
        "wasm",
        "_effective_split_worker_table_base",
    ),
    "_emit_debug_payload": ("debug_helpers", "_emit_debug_payload"),
    "_emit_wrapper_build_failure": ("wrapper_build", "_emit_wrapper_build_failure"),
    "_emit_wrapper_build_success_signals": (
        "wrapper_build",
        "_emit_wrapper_build_success_signals",
    ),
    "_emitted_name_matches_module_symbol": (
        "backend_cache",
        "_emitted_name_matches_module_symbol",
    ),
    "_encode_stdlib_module_symbols": ("backend_cache", "_encode_stdlib_module_symbols"),
    "_ensure_cli_hash_seed": ("arg_helpers", "_ensure_cli_hash_seed"),
    "_ensure_mlir_backend_binary": ("mlir_backend", "_ensure_mlir_backend_binary"),
    "_ensure_rustup_target": ("setup_readiness", "_ensure_rustup_target"),
    "_env_requests_backend_diagnostics": (
        "backend_diagnostics",
        "_env_requests_backend_diagnostics",
    ),
    "_expected_binary_format_for_target": (
        "native_binary",
        "_expected_binary_format_for_target",
    ),
    "_extract_emit_arg": ("arg_helpers", "_extract_emit_arg"),
    "_extract_out_dir_arg": ("arg_helpers", "_extract_out_dir_arg"),
    "_extract_output_arg": ("arg_helpers", "_extract_output_arg"),
    "_find_mlir_backend_binary": ("mlir_backend", "_find_mlir_backend_binary"),
    "_flush_standard_streams": ("arg_helpers", "_flush_standard_streams"),
    "_format_validate_guard_summary": (
        "toolchain_validation",
        "_format_validate_guard_summary",
    ),
    "_forward_compilation_warnings": (
        "backend_diagnostics",
        "_forward_compilation_warnings",
    ),
    "_generate_split_worker_js": ("wasm", "_generate_split_worker_js"),
    "_generate_split_wrangler_jsonc": ("wasm", "_generate_split_wrangler_jsonc"),
    "_hash_runtime_file": ("runtime_fingerprints", "_hash_runtime_file"),
    "_is_protected_runtime_entrypoint": (
        "backend_cache",
        "_is_protected_runtime_entrypoint",
    ),
    "_is_stdlib_owned_symbol": ("backend_cache", "_is_stdlib_owned_symbol"),
    "_is_user_owned_symbol": ("backend_cache", "_is_user_owned_symbol"),
    "_is_valid_cached_backend_artifact": (
        "backend_cache",
        "_is_valid_cached_backend_artifact",
    ),
    "_is_valid_static_library_artifact": (
        "runtime_fingerprints",
        "_is_valid_static_library_artifact",
    ),
    "_is_windows_process_model": ("arg_helpers", "_is_windows_process_model"),
    "_llvm_backend_advice": ("setup_readiness", "_llvm_backend_advice"),
    "_llvm_sys_prefix_env_var": ("setup_readiness", "_llvm_sys_prefix_env_var"),
    "_load_artifact_cleanup_module": ("maintenance", "_load_artifact_cleanup_module"),
    "_load_debug_oracle": ("debug_helpers", "_load_debug_oracle"),
    "_materialize_cached_backend_artifact": (
        "backend_cache",
        "_materialize_cached_backend_artifact",
    ),
    "_maybe_enable_native_cpu": ("cargo_execution", "_maybe_enable_native_cpu"),
    "_merge_debug_manifest": ("debug_helpers", "_merge_debug_manifest"),
    "_mlir_backend_executable_name": ("mlir_backend", "_mlir_backend_executable_name"),
    "_module_symbol_name": ("backend_cache", "_module_symbol_name"),
    "_native_main_stub_snippets": ("native_main_stub", "_native_main_stub_snippets"),
    "_native_nm_command": ("native_symbol_inspection", "_native_nm_command"),
    "_native_object_global_symbol_sets": (
        "native_symbol_inspection",
        "_native_object_global_symbol_sets",
    ),
    "_native_object_has_unresolved_module_chunks": (
        "backend_cache",
        "_native_object_has_unresolved_module_chunks",
    ),
    "_native_stdlib_object_split_enabled": (
        "backend_cache",
        "_native_stdlib_object_split_enabled",
    ),
    "_native_target_is_windows": ("native_link_deps", "_native_target_is_windows"),
    "_normalize_native_symbol_name": (
        "native_symbol_inspection",
        "_normalize_native_symbol_name",
    ),
    "_parse_wrapper_build_contract_payload": (
        "wrapper_build",
        "_parse_wrapper_build_contract_payload",
    ),
    "_path_freshness_fingerprint": ("backend_execution", "_path_freshness_fingerprint"),
    "_persist_validate_summary": ("toolchain_validation", "_persist_validate_summary"),
    "_pid_alive": ("backend_execution", "_pid_alive"),
    "_planned_update_steps": ("toolchain_validation", "_planned_update_steps"),
    "_planned_validate_steps": ("toolchain_validation", "_planned_validate_steps"),
    "_process_exit_code": ("arg_helpers", "_process_exit_code"),
    "_publish_immutable_backend_cache_artifact": (
        "backend_cache",
        "_publish_immutable_backend_cache_artifact",
    ),
    "_python_setup_advice": ("setup_readiness", "_python_setup_advice"),
    "_reachable_function_names_for_stdlib_cache": (
        "backend_cache",
        "_reachable_function_names_for_stdlib_cache",
    ),
    "_read_artifact_sync_state": ("artifact_sync", "_read_artifact_sync_state"),
    "_read_backend_daemon_identity": (
        "backend_execution",
        "_read_backend_daemon_identity",
    ),
    "_read_native_global_symbol_facts": (
        "native_symbol_inspection",
        "_read_native_global_symbol_facts",
    ),
    "_read_runtime_fingerprint": ("runtime_fingerprints", "_read_runtime_fingerprint"),
    "_read_shared_stdlib_partition_functions": (
        "backend_cache",
        "_read_shared_stdlib_partition_functions",
    ),
    "_read_stdlib_cache_key": ("backend_cache", "_read_stdlib_cache_key"),
    "_read_wrapper_build_cache_contract": (
        "wrapper_build",
        "_read_wrapper_build_cache_contract",
    ),
    "_reexec_cli_with_hash_seed": ("arg_helpers", "_reexec_cli_with_hash_seed"),
    "_remove_backend_daemon_identity": (
        "backend_execution",
        "_remove_backend_daemon_identity",
    ),
    "_remove_shared_stdlib_cache_artifacts": (
        "backend_cache",
        "_remove_shared_stdlib_cache_artifacts",
    ),
    "_render_native_main_stub": ("native_main_stub", "_render_native_main_stub"),
    "_required_llvm_backend_major": ("setup_readiness", "_required_llvm_backend_major"),
    "_resolve_available_fast_linker": (
        "native_link_command",
        "_resolve_available_fast_linker",
    ),
    "_resolve_backend_cargo_profile_name": (
        "cargo_profiles",
        "_resolve_backend_cargo_profile_name",
    ),
    "_resolve_backend_cargo_profile_name_cached": (
        "cargo_profiles",
        "_resolve_backend_cargo_profile_name_cached",
    ),
    "_resolve_backend_profile": ("cargo_profiles", "_resolve_backend_profile"),
    "_resolve_backend_profile_cached": (
        "cargo_profiles",
        "_resolve_backend_profile_cached",
    ),
    "_resolve_binary_output": ("arg_helpers", "_resolve_binary_output"),
    "_resolve_cargo_profile_name": ("cargo_profiles", "_resolve_cargo_profile_name"),
    "_resolve_cargo_profile_name_cached": (
        "cargo_profiles",
        "_resolve_cargo_profile_name_cached",
    ),
    "_resolve_dev_linker": ("native_link_command", "_resolve_dev_linker"),
    "_resolve_macos_sdk_root": ("native_toolchain", "_resolve_macos_sdk_root"),
    "_resolve_native_linker_hint": (
        "native_link_command",
        "_resolve_native_linker_hint",
    ),
    "_resolve_validate_summary_path": (
        "toolchain_validation",
        "_resolve_validate_summary_path",
    ),
    "_resolved_env_dir_from_root": ("setup_readiness", "_resolved_env_dir_from_root"),
    "_rotate_backend_daemon_log_if_large": (
        "backend_daemon_logs",
        "_rotate_backend_daemon_log_if_large",
    ),
    "_run_bolt_post_link": ("native_toolchain", "_run_bolt_post_link"),
    "_run_cargo_with_sccache_retry": (
        "cargo_execution",
        "_run_cargo_with_sccache_retry",
    ),
    "_run_debug_eval_command": ("debug_helpers", "_run_debug_eval_command"),
    "_run_mlir_backend_pipeline": ("mlir_backend", "_run_mlir_backend_pipeline"),
    "_run_wrapper_build": ("wrapper_build", "_run_wrapper_build"),
    "_runtime_artifact_fingerprint_matches": (
        "runtime_fingerprints",
        "_runtime_artifact_fingerprint_matches",
    ),
    "_runtime_callable_symbols_digest": (
        "runtime_callable_symbols",
        "_runtime_callable_symbols_digest",
    ),
    "_runtime_callable_symbols_file": (
        "runtime_callable_symbols",
        "_runtime_callable_symbols_file",
    ),
    "_runtime_lib_freshness_candidates": (
        "backend_execution",
        "_runtime_lib_freshness_candidates",
    ),
    "_rustup_setup_advice": ("setup_readiness", "_rustup_setup_advice"),
    "_shared_cache_lock": ("backend_cache", "_shared_cache_lock"),
    "_shared_cache_lock_dir_cached": ("backend_cache", "_shared_cache_lock_dir_cached"),
    "_shared_stdlib_cache_key": ("backend_cache", "_shared_stdlib_cache_key"),
    "_shared_stdlib_cache_lock": ("backend_cache", "_shared_stdlib_cache_lock"),
    "_shared_stdlib_cache_matches_key": (
        "backend_cache",
        "_shared_stdlib_cache_matches_key",
    ),
    "_shared_stdlib_cache_matches_key_locked": (
        "backend_cache",
        "_shared_stdlib_cache_matches_key_locked",
    ),
    "_shared_stdlib_cache_mismatch_detail": (
        "backend_cache",
        "_shared_stdlib_cache_mismatch_detail",
    ),
    "_shared_stdlib_cache_payload_ir": (
        "backend_cache",
        "_shared_stdlib_cache_payload_ir",
    ),
    "_shared_stdlib_compiler_fingerprint": (
        "backend_cache",
        "_shared_stdlib_compiler_fingerprint",
    ),
    "_shared_stdlib_manifest": ("backend_cache", "_shared_stdlib_manifest"),
    "_shared_stdlib_native_symbol_closure_issue": (
        "backend_cache",
        "_shared_stdlib_native_symbol_closure_issue",
    ),
    "_shared_stdlib_publish_lock_path": (
        "backend_cache",
        "_shared_stdlib_publish_lock_path",
    ),
    "_short_backend_daemon_socket_dir": (
        "backend_execution",
        "_short_backend_daemon_socket_dir",
    ),
    "_short_backend_daemon_socket_dir_impl": (
        "backend_daemon_paths",
        "_short_backend_daemon_socket_dir",
    ),
    "_smoke_probe_native_binary": ("native_binary", "_smoke_probe_native_binary"),
    "_source_tree_freshness_fingerprint": (
        "backend_execution",
        "_source_tree_freshness_fingerprint",
    ),
    "_split_backend_daemon_command": (
        "backend_execution",
        "_split_backend_daemon_command",
    ),
    "_stage_backend_output_and_caches": (
        "backend_cache",
        "_stage_backend_output_and_caches",
    ),
    "_stage_runtime_callable_symbols_for_native_codegen": (
        "runtime_callable_symbols",
        "_stage_runtime_callable_symbols_for_native_codegen",
    ),
    "_stage_shared_stdlib_object_for_link": (
        "backend_cache",
        "_stage_shared_stdlib_object_for_link",
    ),
    "_start_backend_daemon": ("backend_execution", "_start_backend_daemon"),
    "_stdlib_module_symbols": ("backend_cache", "_stdlib_module_symbols"),
    "_stdlib_object_cache_path": ("backend_cache", "_stdlib_object_cache_path"),
    "_stdlib_object_count_sidecar_path": (
        "backend_cache",
        "_stdlib_object_count_sidecar_path",
    ),
    "_stdlib_object_digest_sidecar_path": (
        "backend_cache",
        "_stdlib_object_digest_sidecar_path",
    ),
    "_stdlib_object_key_sidecar_path": (
        "backend_cache",
        "_stdlib_object_key_sidecar_path",
    ),
    "_stdlib_object_manifest_sidecar_path": (
        "backend_cache",
        "_stdlib_object_manifest_sidecar_path",
    ),
    "_stdlib_object_partition_manifest_sidecar_path": (
        "backend_cache",
        "_stdlib_object_partition_manifest_sidecar_path",
    ),
    "_stored_fingerprint_matches_source_metadata": (
        "runtime_fingerprints",
        "_stored_fingerprint_matches_source_metadata",
    ),
    "_strip_leading_double_dash": ("arg_helpers", "_strip_leading_double_dash"),
    "_sweep_orphaned_backend_daemon_locks": (
        "backend_execution",
        "_sweep_orphaned_backend_daemon_locks",
    ),
    "_sweep_orphaned_backend_daemon_locks_once": (
        "backend_execution",
        "_sweep_orphaned_backend_daemon_locks_once",
    ),
    "_target_is_host_executable": ("native_binary", "_target_is_host_executable"),
    "_temporary_backend_output_path": (
        "backend_cache",
        "_temporary_backend_output_path",
    ),
    "_terminate_backend_daemon_identity": (
        "backend_execution",
        "_terminate_backend_daemon_identity",
    ),
    "_try_cached_backend_candidates": (
        "backend_cache",
        "_try_cached_backend_candidates",
    ),
    "_unix_socket_path_exceeds_limit": (
        "backend_daemon_paths",
        "_unix_socket_path_exceeds_limit",
    ),
    "_unresolved_stdlib_module_symbols": (
        "backend_cache",
        "_unresolved_stdlib_module_symbols",
    ),
    "_uv_setup_advice": ("setup_readiness", "_uv_setup_advice"),
    "_validate_guard_prefix": ("toolchain_validation", "_validate_guard_prefix"),
    "_validate_native_binary_format": (
        "native_binary",
        "_validate_native_binary_format",
    ),
    "_validate_proof_bypass_errors": (
        "toolchain_validation",
        "_validate_proof_bypass_errors",
    ),
    "_validate_shared_stdlib_cache_contract": (
        "backend_cache",
        "_validate_shared_stdlib_cache_contract",
    ),
    "_validation_guard_summary": ("toolchain_validation", "_validation_guard_summary"),
    "_wrapper_build_cache_input": ("wrapper_build", "_wrapper_build_cache_input"),
    "_wrapper_build_cache_manifest_path": (
        "wrapper_build",
        "_wrapper_build_cache_manifest_path",
    ),
    "_wrapper_build_cache_semantic_env": (
        "wrapper_build",
        "_wrapper_build_cache_semantic_env",
    ),
    "_wrapper_build_default_binary_path": (
        "wrapper_build",
        "_wrapper_build_default_binary_path",
    ),
    "_wrapper_target_python": ("wrapper_build", "_wrapper_target_python"),
    "_write_artifact_sync_payload": ("artifact_sync", "_write_artifact_sync_payload"),
    "_write_artifact_sync_state": ("artifact_sync", "_write_artifact_sync_state"),
    "_write_backend_daemon_identity": (
        "backend_execution",
        "_write_backend_daemon_identity",
    ),
    "_write_backend_daemon_ir_lease": (
        "backend_execution",
        "_write_backend_daemon_ir_lease",
    ),
    "_write_backend_ir_json_file": ("backend_execution", "_write_backend_ir_json_file"),
    "_write_backend_ir_lease": ("backend_execution", "_write_backend_ir_lease"),
    "_write_runtime_fingerprint": (
        "runtime_fingerprints",
        "_write_runtime_fingerprint",
    ),
    "_write_wrapper_build_cache_manifest": (
        "wrapper_build",
        "_write_wrapper_build_cache_manifest",
    ),
    "_zig_target_query": ("compiler_target", "_zig_target_query"),
    "clean": ("maintenance", "clean"),
    "completion": ("arg_helpers", "completion"),
    "doctor": ("setup_readiness", "doctor"),
    "package": ("package_distribution", "package"),
    "publish": ("package_distribution", "publish"),
    "setup": ("setup_readiness", "setup"),
    "show_config": ("maintenance", "show_config"),
    "update_repo": ("toolchain_validation", "update_repo"),
    "validate": ("toolchain_validation", "validate"),
    "verify": ("package_distribution", "verify"),
}


class _LazyPostLoweringModule:
    """Deferred proxy for a post-lowering ``molt.cli`` submodule.

    Bound eagerly to a module-level name so intra-module functions can use it
    as a bare global, while the underlying submodule (which transitively pulls
    the backend) is imported only on first attribute access -- keeping package
    import backend-free and out of the static lowering scope.
    """

    __slots__ = ("_module_name", "_module")

    def __init__(self, module_name: str) -> None:
        object.__setattr__(self, "_module_name", module_name)
        object.__setattr__(self, "_module", None)

    def _load(self):
        module = object.__getattribute__(self, "_module")
        if module is None:
            module = importlib.import_module(
                f"molt.cli.{object.__getattribute__(self, '_module_name')}"
            )
            object.__setattr__(self, "_module", module)
        return module

    def __getattr__(self, name: str):
        return getattr(self._load(), name)


# Internally-referenced post-lowering module aliases: bound as lazy proxies so
# the build command handlers below can call e.g. ``_build_pipeline.run(...)``
# without importing the backend at package-import time.
_build_inputs = _LazyPostLoweringModule("build_inputs")
_build_pipeline = _LazyPostLoweringModule("build_pipeline")


def _scoped_environ_updates(*args, **kwargs):
    """Lazy wrapper for :func:`molt.cli.wrapper_build._scoped_environ_updates`.

    ``wrapper_build`` is part of the post-lowering layer; defer its import to
    call time so it does not load at package import.
    """
    from molt.cli.wrapper_build import _scoped_environ_updates as _impl

    return _impl(*args, **kwargs)


def __getattr__(name: str):
    entry = _LAZY_REEXPORTS.get(name)
    if entry is None:
        # Explicit submodule imports use Python's import machinery. Attribute
        # lookup must not invent dependencies outside the finite registry.
        raise AttributeError(f"module {_PACKAGE!r} has no attribute {name!r}")

    module = importlib.import_module(f"molt.cli.{entry[0]}")
    value = module if entry[1] is None else getattr(module, entry[1])
    _package_namespace()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(_package_namespace()) | set(_LAZY_REEXPORTS))
