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
    # molt.cli.arg_helpers
    '_BUILD_ESSENTIAL_FLAGS': ('arg_helpers', '_BUILD_ESSENTIAL_FLAGS'),
    '_BuildHelpFormatter': ('arg_helpers', '_BuildHelpFormatter'),
    '_MoltHelpFormatter': ('arg_helpers', '_MoltHelpFormatter'),
    '_add_debug_shared_selector_args': ('arg_helpers', '_add_debug_shared_selector_args'),
    '_build_args_has_cache_flag': ('arg_helpers', '_build_args_has_cache_flag'),
    '_build_args_has_capabilities_flag': ('arg_helpers', '_build_args_has_capabilities_flag'),
    '_build_args_has_profile_flag': ('arg_helpers', '_build_args_has_profile_flag'),
    '_build_args_has_trusted_flag': ('arg_helpers', '_build_args_has_trusted_flag'),
    '_cli_hash_seed_reexec_argv': ('arg_helpers', '_cli_hash_seed_reexec_argv'),
    '_ensure_cli_hash_seed': ('arg_helpers', '_ensure_cli_hash_seed'),
    '_extract_emit_arg': ('arg_helpers', '_extract_emit_arg'),
    '_extract_out_dir_arg': ('arg_helpers', '_extract_out_dir_arg'),
    '_extract_output_arg': ('arg_helpers', '_extract_output_arg'),
    '_flush_standard_streams': ('arg_helpers', '_flush_standard_streams'),
    '_is_windows_process_model': ('arg_helpers', '_is_windows_process_model'),
    '_process_exit_code': ('arg_helpers', '_process_exit_code'),
    '_reexec_cli_with_hash_seed': ('arg_helpers', '_reexec_cli_with_hash_seed'),
    '_resolve_binary_output': ('arg_helpers', '_resolve_binary_output'),
    '_strip_leading_double_dash': ('arg_helpers', '_strip_leading_double_dash'),
    'completion': ('arg_helpers', 'completion'),
    # molt.cli.artifact_sync
    '_ARTIFACT_SYNC_STATE_CACHE': ('artifact_sync', '_ARTIFACT_SYNC_STATE_CACHE'),
    '_artifact_sync_state_matches': ('artifact_sync', '_artifact_sync_state_matches'),
    '_artifact_sync_state_matches_stat': ('artifact_sync', '_artifact_sync_state_matches_stat'),
    '_artifact_sync_state_path': ('artifact_sync', '_artifact_sync_state_path'),
    '_read_artifact_sync_state': ('artifact_sync', '_read_artifact_sync_state'),
    '_write_artifact_sync_payload': ('artifact_sync', '_write_artifact_sync_payload'),
    '_write_artifact_sync_state': ('artifact_sync', '_write_artifact_sync_state'),
    # molt.cli.backend_cache
    '_DEAD_FUNCTION_ELIM_REFERENCE_KINDS': ('backend_cache', '_DEAD_FUNCTION_ELIM_REFERENCE_KINDS'),
    '_SHARED_STDLIB_CACHE_SCHEMA_VERSION': ('backend_cache', '_SHARED_STDLIB_CACHE_SCHEMA_VERSION'),
    '_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION': ('backend_cache', '_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION'),
    '_SHARED_STDLIB_PARTITION_SCHEMA_VERSION': ('backend_cache', '_SHARED_STDLIB_PARTITION_SCHEMA_VERSION'),
    '_backend_cache_artifact_path': ('backend_cache', '_backend_cache_artifact_path'),
    '_backend_daemon_skip_output_sync_flags': ('backend_cache', '_backend_daemon_skip_output_sync_flags'),
    '_emitted_name_matches_module_symbol': ('backend_cache', '_emitted_name_matches_module_symbol'),
    '_encode_stdlib_module_symbols': ('backend_cache', '_encode_stdlib_module_symbols'),
    '_is_protected_runtime_entrypoint': ('backend_cache', '_is_protected_runtime_entrypoint'),
    '_is_stdlib_owned_symbol': ('backend_cache', '_is_stdlib_owned_symbol'),
    '_is_user_owned_symbol': ('backend_cache', '_is_user_owned_symbol'),
    '_is_valid_cached_backend_artifact': ('backend_cache', '_is_valid_cached_backend_artifact'),
    '_materialize_cached_backend_artifact': ('backend_cache', '_materialize_cached_backend_artifact'),
    '_module_symbol_name': ('backend_cache', '_module_symbol_name'),
    '_native_artifact_source_key': ('backend_cache', '_native_artifact_source_key'),
    '_native_nm_command': ('backend_cache', '_native_nm_command'),
    '_native_object_global_symbol_sets': ('backend_cache', '_native_object_global_symbol_sets'),
    '_native_object_global_symbols_result': ('backend_cache', '_native_object_global_symbols_result'),
    '_native_object_has_unresolved_module_chunks': ('backend_cache', '_native_object_has_unresolved_module_chunks'),
    '_native_stdlib_object_split_enabled': ('backend_cache', '_native_stdlib_object_split_enabled'),
    '_normalize_native_symbol_name': ('backend_cache', '_normalize_native_symbol_name'),
    '_publish_immutable_backend_cache_artifact': ('backend_cache', '_publish_immutable_backend_cache_artifact'),
    '_reachable_function_names_for_stdlib_cache': ('backend_cache', '_reachable_function_names_for_stdlib_cache'),
    '_read_shared_stdlib_partition_functions': ('backend_cache', '_read_shared_stdlib_partition_functions'),
    '_read_stdlib_cache_key': ('backend_cache', '_read_stdlib_cache_key'),
    '_remove_shared_stdlib_cache_artifacts': ('backend_cache', '_remove_shared_stdlib_cache_artifacts'),
    '_shared_cache_lock': ('backend_cache', '_shared_cache_lock'),
    '_shared_cache_lock_dir_cached': ('backend_cache', '_shared_cache_lock_dir_cached'),
    '_shared_stdlib_cache_key': ('backend_cache', '_shared_stdlib_cache_key'),
    '_shared_stdlib_cache_lock': ('backend_cache', '_shared_stdlib_cache_lock'),
    '_shared_stdlib_cache_matches_key': ('backend_cache', '_shared_stdlib_cache_matches_key'),
    '_shared_stdlib_cache_matches_key_locked': ('backend_cache', '_shared_stdlib_cache_matches_key_locked'),
    '_shared_stdlib_cache_mismatch_detail': ('backend_cache', '_shared_stdlib_cache_mismatch_detail'),
    '_shared_stdlib_cache_payload_ir': ('backend_cache', '_shared_stdlib_cache_payload_ir'),
    '_shared_stdlib_compiler_fingerprint': ('backend_cache', '_shared_stdlib_compiler_fingerprint'),
    '_shared_stdlib_manifest': ('backend_cache', '_shared_stdlib_manifest'),
    '_shared_stdlib_native_symbol_closure_issue': ('backend_cache', '_shared_stdlib_native_symbol_closure_issue'),
    '_shared_stdlib_publish_lock_path': ('backend_cache', '_shared_stdlib_publish_lock_path'),
    '_stage_backend_output_and_caches': ('backend_cache', '_stage_backend_output_and_caches'),
    '_stage_shared_stdlib_object_for_link': ('backend_cache', '_stage_shared_stdlib_object_for_link'),
    '_stdlib_module_symbols': ('backend_cache', '_stdlib_module_symbols'),
    '_stdlib_object_cache_path': ('backend_cache', '_stdlib_object_cache_path'),
    '_stdlib_object_count_sidecar_path': ('backend_cache', '_stdlib_object_count_sidecar_path'),
    '_stdlib_object_digest_sidecar_path': ('backend_cache', '_stdlib_object_digest_sidecar_path'),
    '_stdlib_object_key_sidecar_path': ('backend_cache', '_stdlib_object_key_sidecar_path'),
    '_stdlib_object_manifest_sidecar_path': ('backend_cache', '_stdlib_object_manifest_sidecar_path'),
    '_stdlib_object_partition_manifest_sidecar_path': ('backend_cache', '_stdlib_object_partition_manifest_sidecar_path'),
    '_temporary_backend_output_path': ('backend_cache', '_temporary_backend_output_path'),
    '_try_cached_backend_candidates': ('backend_cache', '_try_cached_backend_candidates'),
    '_unresolved_stdlib_module_symbols': ('backend_cache', '_unresolved_stdlib_module_symbols'),
    '_validate_shared_stdlib_cache_contract': ('backend_cache', '_validate_shared_stdlib_cache_contract'),
    # molt.cli.backend_daemon_config
    '_backend_daemon_enabled': ('backend_daemon_config', '_backend_daemon_enabled'),
    '_backend_daemon_enabled_cached': ('backend_daemon_config', '_backend_daemon_enabled_cached'),
    # molt.cli.backend_daemon_logs
    '_backend_daemon_log_mark': ('backend_daemon_logs', '_backend_daemon_log_mark'),
    '_backend_daemon_log_max_bytes': ('backend_daemon_logs', '_backend_daemon_log_max_bytes'),
    '_backend_daemon_log_max_bytes_cached': ('backend_daemon_logs', '_backend_daemon_log_max_bytes_cached'),
    '_backend_daemon_log_since': ('backend_daemon_logs', '_backend_daemon_log_since'),
    '_backend_daemon_log_tail': ('backend_daemon_logs', '_backend_daemon_log_tail'),
    '_rotate_backend_daemon_log_if_large': ('backend_daemon_logs', '_rotate_backend_daemon_log_if_large'),
    # molt.cli.backend_daemon_paths
    '_backend_daemon_paths_bundle': ('backend_daemon_paths', '_backend_daemon_paths'),
    '_backend_daemon_socket_path_error': ('backend_daemon_paths', '_backend_daemon_socket_path_error'),
    '_short_backend_daemon_socket_dir_impl': ('backend_daemon_paths', '_short_backend_daemon_socket_dir'),
    '_unix_socket_path_exceeds_limit': ('backend_daemon_paths', '_unix_socket_path_exceeds_limit'),
    # molt.cli.backend_daemon_startup
    '_backend_daemon_spawn_probe_timeout': ('backend_daemon_startup', '_backend_daemon_spawn_probe_timeout'),
    '_backend_daemon_start_timeout': ('backend_daemon_startup', '_backend_daemon_start_timeout'),
    '_backend_daemon_start_timeout_cached': ('backend_daemon_startup', '_backend_daemon_start_timeout_cached'),
    # molt.cli.backend_diagnostics
    '_BACKEND_DIAGNOSTIC_ENV_KNOBS': ('backend_diagnostics', '_BACKEND_DIAGNOSTIC_ENV_KNOBS'),
    '_FALSY_ENV_VALUES': ('backend_diagnostics', '_FALSY_ENV_VALUES'),
    '_PYTHON_WARNING_RE': ('backend_diagnostics', '_PYTHON_WARNING_RE'),
    '_env_requests_backend_diagnostics': ('backend_diagnostics', '_env_requests_backend_diagnostics'),
    '_forward_compilation_warnings': ('backend_diagnostics', '_forward_compilation_warnings'),
    # molt.cli.backend_execution
    '_BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION': ('backend_execution', '_BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION'),
    '_BACKEND_CODEGEN_REQUEST_ENV_KNOBS': ('backend_execution', '_BACKEND_CODEGEN_REQUEST_ENV_KNOBS'),
    '_BACKEND_DAEMON_ORPHAN_SWEEP_DONE': ('backend_execution', '_BACKEND_DAEMON_ORPHAN_SWEEP_DONE'),
    '_BACKEND_DAEMON_PROTOCOL_VERSION': ('backend_execution', '_BACKEND_DAEMON_PROTOCOL_VERSION'),
    '_BACKEND_REQUEST_ENV_KNOBS': ('backend_execution', '_BACKEND_REQUEST_ENV_KNOBS'),
    '_BACKEND_RESOURCE_ENV_KNOBS': ('backend_execution', '_BACKEND_RESOURCE_ENV_KNOBS'),
    '_BackendDaemonIdentity': ('backend_execution', '_BackendDaemonIdentity'),
    '_DAEMON_CONFIG_DIGEST_SCHEMA_VERSION': ('backend_execution', '_DAEMON_CONFIG_DIGEST_SCHEMA_VERSION'),
    '_DEFAULT_BACKEND_FEATURES': ('backend_execution', '_DEFAULT_BACKEND_FEATURES'),
    '_NATIVE_CODEGEN_ENV_KNOBS': ('backend_execution', '_NATIVE_CODEGEN_ENV_KNOBS'),
    '_NATIVE_RELOCATABLE_LINKER_ENV_KEYS': ('backend_execution', '_NATIVE_RELOCATABLE_LINKER_ENV_KEYS'),
    '_WASM_CODEGEN_ENV_KNOBS': ('backend_execution', '_WASM_CODEGEN_ENV_KNOBS'),
    '_backend_bin_path': ('backend_execution', '_backend_bin_path'),
    '_backend_bin_path_cached': ('backend_execution', '_backend_bin_path_cached'),
    '_backend_binary_identity': ('backend_execution', '_backend_binary_identity'),
    '_backend_codegen_env_digest': ('backend_execution', '_backend_codegen_env_digest'),
    '_backend_codegen_env_inputs': ('backend_execution', '_backend_codegen_env_inputs'),
    '_backend_codegen_env_inputs_cached': ('backend_execution', '_backend_codegen_env_inputs_cached'),
    '_backend_daemon_binary_is_newer': ('backend_execution', '_backend_daemon_binary_is_newer'),
    '_backend_daemon_command_has_socket': ('backend_execution', '_backend_daemon_command_has_socket'),
    '_backend_daemon_command_matches_identity': ('backend_execution', '_backend_daemon_command_matches_identity'),
    '_backend_daemon_compile_request_bytes': ('backend_execution', '_backend_daemon_compile_request_bytes'),
    '_backend_daemon_config_digest': ('backend_execution', '_backend_daemon_config_digest'),
    '_backend_daemon_empty_response_error': ('backend_execution', '_backend_daemon_empty_response_error'),
    '_backend_daemon_freshness_inputs': ('backend_execution', '_backend_daemon_freshness_inputs'),
    '_backend_daemon_health_from_response': ('backend_execution', '_backend_daemon_health_from_response'),
    '_backend_daemon_health_probe': ('backend_execution', '_backend_daemon_health_probe'),
    '_backend_daemon_identity_for_pid': ('backend_execution', '_backend_daemon_identity_for_pid'),
    '_backend_daemon_identity_from_health': ('backend_execution', '_backend_daemon_identity_from_health'),
    '_backend_daemon_identity_is_verified': ('backend_execution', '_backend_daemon_identity_is_verified'),
    '_backend_daemon_identity_matches_context': ('backend_execution', '_backend_daemon_identity_matches_context'),
    '_backend_daemon_identity_path': ('backend_execution', '_backend_daemon_identity_path'),
    '_backend_daemon_identity_process_matches': ('backend_execution', '_backend_daemon_identity_process_matches'),
    '_backend_daemon_job_failure_message': ('backend_execution', '_backend_daemon_job_failure_message'),
    '_backend_daemon_log_path': ('backend_execution', '_backend_daemon_log_path'),
    '_backend_daemon_paths_cached': ('backend_execution', '_backend_daemon_paths_cached'),
    '_backend_daemon_ping': ('backend_execution', '_backend_daemon_ping'),
    '_backend_daemon_ping_health': ('backend_execution', '_backend_daemon_ping_health'),
    '_backend_daemon_process_command': ('backend_execution', '_backend_daemon_process_command'),
    '_backend_daemon_request': ('backend_execution', '_backend_daemon_request'),
    '_backend_daemon_request_bytes': ('backend_execution', '_backend_daemon_request_bytes'),
    '_backend_daemon_request_on_socket': ('backend_execution', '_backend_daemon_request_on_socket'),
    '_backend_daemon_request_payload_bytes': ('backend_execution', '_backend_daemon_request_payload_bytes'),
    '_backend_daemon_response_failure_message': ('backend_execution', '_backend_daemon_response_failure_message'),
    '_backend_daemon_retryable_error': ('backend_execution', '_backend_daemon_retryable_error'),
    '_backend_daemon_socket_dir': ('backend_execution', '_backend_daemon_socket_dir'),
    '_backend_daemon_socket_path': ('backend_execution', '_backend_daemon_socket_path'),
    '_backend_daemon_text_field': ('backend_execution', '_backend_daemon_text_field'),
    '_backend_daemon_wait_until_ready': ('backend_execution', '_backend_daemon_wait_until_ready'),
    '_backend_features_for_build_target': ('backend_execution', '_backend_features_for_build_target'),
    '_backend_features_for_target': ('backend_execution', '_backend_features_for_target'),
    '_command_executable_matches_backend': ('backend_execution', '_command_executable_matches_backend'),
    '_command_has_path_separator': ('backend_execution', '_command_has_path_separator'),
    '_compile_with_backend_daemon': ('backend_execution', '_compile_with_backend_daemon'),
    '_native_relocatable_linker_identity': ('backend_execution', '_native_relocatable_linker_identity'),
    '_native_relocatable_linker_selection': ('backend_execution', '_native_relocatable_linker_selection'),
    '_path_freshness_fingerprint': ('backend_execution', '_path_freshness_fingerprint'),
    '_pid_alive': ('backend_execution', '_pid_alive'),
    '_read_backend_daemon_identity': ('backend_execution', '_read_backend_daemon_identity'),
    '_remove_backend_daemon_identity': ('backend_execution', '_remove_backend_daemon_identity'),
    '_runtime_lib_freshness_candidates': ('backend_execution', '_runtime_lib_freshness_candidates'),
    '_short_backend_daemon_socket_dir': ('backend_execution', '_short_backend_daemon_socket_dir'),
    '_source_tree_freshness_fingerprint': ('backend_execution', '_source_tree_freshness_fingerprint'),
    '_split_backend_daemon_command': ('backend_execution', '_split_backend_daemon_command'),
    '_start_backend_daemon': ('backend_execution', '_start_backend_daemon'),
    '_sweep_orphaned_backend_daemon_locks': ('backend_execution', '_sweep_orphaned_backend_daemon_locks'),
    '_sweep_orphaned_backend_daemon_locks_once': ('backend_execution', '_sweep_orphaned_backend_daemon_locks_once'),
    '_terminate_backend_daemon_identity': ('backend_execution', '_terminate_backend_daemon_identity'),
    '_write_backend_daemon_identity': ('backend_execution', '_write_backend_daemon_identity'),
    '_write_backend_daemon_ir_lease': ('backend_execution', '_write_backend_daemon_ir_lease'),
    '_write_backend_ir_json_file': ('backend_execution', '_write_backend_ir_json_file'),
    '_write_backend_ir_lease': ('backend_execution', '_write_backend_ir_lease'),
    # molt.cli.backend_ir
    '_backend_ir': ('backend_ir', None),
    # molt.cli.cargo_execution
    '_build_slot': ('cargo_execution', '_build_slot'),
    '_cargo_build_env': ('cargo_execution', '_cargo_build_env'),
    '_maybe_enable_native_cpu': ('cargo_execution', '_maybe_enable_native_cpu'),
    '_maybe_enable_sccache': ('cargo_execution', '_maybe_enable_sccache'),
    '_run_cargo_with_sccache_retry': ('cargo_execution', '_run_cargo_with_sccache_retry'),
    # molt.cli.cargo_profiles
    '_CARGO_PROFILE_NAME_RE': ('cargo_profiles', '_CARGO_PROFILE_NAME_RE'),
    '_active_artifact_profile_dirs': ('cargo_profiles', '_active_artifact_profile_dirs'),
    '_resolve_backend_cargo_profile_name': ('cargo_profiles', '_resolve_backend_cargo_profile_name'),
    '_resolve_backend_cargo_profile_name_cached': ('cargo_profiles', '_resolve_backend_cargo_profile_name_cached'),
    '_resolve_backend_profile': ('cargo_profiles', '_resolve_backend_profile'),
    '_resolve_backend_profile_cached': ('cargo_profiles', '_resolve_backend_profile_cached'),
    '_resolve_cargo_profile_name': ('cargo_profiles', '_resolve_cargo_profile_name'),
    '_resolve_cargo_profile_name_cached': ('cargo_profiles', '_resolve_cargo_profile_name_cached'),
    # molt.cli.commands
    '_commands': ('commands', None),
    # molt.cli.completion
    '_completion_script': ('completion', '_completion_script'),
    # molt.cli.maintenance
    '_load_artifact_cleanup_module': ('maintenance', '_load_artifact_cleanup_module'),
    'clean': ('maintenance', 'clean'),
    'show_config': ('maintenance', 'show_config'),
    # molt.cli.mlir_backend
    '_find_mlir_backend_binary': ('mlir_backend', '_find_mlir_backend_binary'),
    '_ensure_mlir_backend_binary': ('mlir_backend', '_ensure_mlir_backend_binary'),
    '_mlir_backend_executable_name': ('mlir_backend', '_mlir_backend_executable_name'),
    '_run_mlir_backend_pipeline': ('mlir_backend', '_run_mlir_backend_pipeline'),
    # molt.cli.native_binary
    '_NativeBinaryInvalid': ('native_binary', '_NativeBinaryInvalid'),
    '_assert_native_binary_valid': ('native_binary', '_assert_native_binary_valid'),
    '_darwin_binary_imports_validation_error': ('native_binary', '_darwin_binary_imports_validation_error'),
    '_darwin_binary_magic_error': ('native_binary', '_darwin_binary_magic_error'),
    '_expected_binary_format_for_target': ('native_binary', '_expected_binary_format_for_target'),
    '_smoke_probe_native_binary': ('native_binary', '_smoke_probe_native_binary'),
    '_target_is_host_executable': ('native_binary', '_target_is_host_executable'),
    '_validate_native_binary_format': ('native_binary', '_validate_native_binary_format'),
    # molt.cli.native_link_command
    '_build_native_link_plan': ('native_link_command', '_build_native_link_plan'),
    '_build_native_link_driver_command': ('native_link_command', '_build_native_link_driver_command'),
    '_resolve_available_fast_linker': ('native_link_command', '_resolve_available_fast_linker'),
    '_resolve_dev_linker': ('native_link_command', '_resolve_dev_linker'),
    '_resolve_native_linker_hint': ('native_link_command', '_resolve_native_linker_hint'),
    '_windows_coff_library_command': ('native_link_command', '_windows_coff_library_command'),
    # molt.cli.native_link_deps
    '_collect_cargo_native_link_deps': ('native_link_deps', '_collect_cargo_native_link_deps'),
    '_native_target_is_windows': ('native_link_deps', '_native_target_is_windows'),
    # molt.cli.native_main_stub
    '_native_main_stub_snippets': ('native_main_stub', '_native_main_stub_snippets'),
    '_render_native_main_stub': ('native_main_stub', '_render_native_main_stub'),
    # molt.cli.native_toolchain
    '_append_darwin_runtime_frameworks': ('native_toolchain', '_append_darwin_runtime_frameworks'),
    '_codesign_binary': ('native_toolchain', '_codesign_binary'),
    '_detect_macos_arch': ('native_toolchain', '_detect_macos_arch'),
    '_resolve_macos_sdk_root': ('native_toolchain', '_resolve_macos_sdk_root'),
    '_run_bolt_post_link': ('native_toolchain', '_run_bolt_post_link'),
    '_zig_target_query': ('native_toolchain', '_zig_target_query'),
    # molt.cli.package_distribution
    'package': ('package_distribution', 'package'),
    'publish': ('package_distribution', 'publish'),
    'verify': ('package_distribution', 'verify'),
    # molt.cli.runtime_fingerprints
    '_artifact_content_looks_valid': ('runtime_fingerprints', '_artifact_content_looks_valid'),
    '_artifact_needs_rebuild': ('runtime_fingerprints', '_artifact_needs_rebuild'),
    '_hash_runtime_file': ('runtime_fingerprints', '_hash_runtime_file'),
    '_is_valid_static_library_artifact': ('runtime_fingerprints', '_is_valid_static_library_artifact'),
    '_read_runtime_fingerprint': ('runtime_fingerprints', '_read_runtime_fingerprint'),
    '_runtime_artifact_fingerprint_matches': ('runtime_fingerprints', '_runtime_artifact_fingerprint_matches'),
    '_runtime_fingerprint': ('runtime_fingerprints', '_runtime_fingerprint'),
    '_stored_fingerprint_matches_source_metadata': ('runtime_fingerprints', '_stored_fingerprint_matches_source_metadata'),
    '_write_runtime_fingerprint': ('runtime_fingerprints', '_write_runtime_fingerprint'),
    # molt.cli.runtime_callable_symbols
    '_runtime_callable_symbols_digest': ('runtime_callable_symbols', '_runtime_callable_symbols_digest'),
    '_runtime_callable_symbols_file': ('runtime_callable_symbols', '_runtime_callable_symbols_file'),
    '_stage_runtime_callable_symbols_for_native_codegen': ('runtime_callable_symbols', '_stage_runtime_callable_symbols_for_native_codegen'),
    # molt.cli.setup_readiness
    '_build_toolchain_report': ('setup_readiness', '_build_toolchain_report'),
    '_canonical_env_defaults': ('setup_readiness', '_canonical_env_defaults'),
    '_cargo_setup_advice': ('setup_readiness', '_cargo_setup_advice'),
    '_clang_setup_advice': ('setup_readiness', '_clang_setup_advice'),
    '_collect_setup_actions': ('setup_readiness', '_collect_setup_actions'),
    '_detect_llvm_backend_toolchain': ('setup_readiness', '_detect_llvm_backend_toolchain'),
    '_ensure_rustup_target': ('setup_readiness', '_ensure_rustup_target'),
    '_llvm_backend_advice': ('setup_readiness', '_llvm_backend_advice'),
    '_llvm_sys_prefix_env_var': ('setup_readiness', '_llvm_sys_prefix_env_var'),
    '_python_setup_advice': ('setup_readiness', '_python_setup_advice'),
    '_required_llvm_backend_major': ('setup_readiness', '_required_llvm_backend_major'),
    '_resolved_env_dir_from_root': ('setup_readiness', '_resolved_env_dir_from_root'),
    '_rustup_setup_advice': ('setup_readiness', '_rustup_setup_advice'),
    '_uv_setup_advice': ('setup_readiness', '_uv_setup_advice'),
    'doctor': ('setup_readiness', 'doctor'),
    'setup': ('setup_readiness', 'setup'),
    # molt.cli.toolchain_validation
    '_VALIDATE_PROOF_BYPASS_ENV': ('toolchain_validation', '_VALIDATE_PROOF_BYPASS_ENV'),
    '_VALIDATE_SUITE_CHOICES': ('toolchain_validation', '_VALIDATE_SUITE_CHOICES'),
    '_default_validate_summary_path': ('toolchain_validation', '_default_validate_summary_path'),
    '_format_validate_guard_summary': ('toolchain_validation', '_format_validate_guard_summary'),
    '_persist_validate_summary': ('toolchain_validation', '_persist_validate_summary'),
    '_planned_update_steps': ('toolchain_validation', '_planned_update_steps'),
    '_planned_validate_steps': ('toolchain_validation', '_planned_validate_steps'),
    '_resolve_validate_summary_path': ('toolchain_validation', '_resolve_validate_summary_path'),
    '_validate_guard_prefix': ('toolchain_validation', '_validate_guard_prefix'),
    '_validate_proof_bypass_errors': ('toolchain_validation', '_validate_proof_bypass_errors'),
    '_validation_guard_summary': ('toolchain_validation', '_validation_guard_summary'),
    'update_repo': ('toolchain_validation', 'update_repo'),
    'validate': ('toolchain_validation', 'validate'),
    # molt.cli.wasm
    '_effective_split_worker_table_base': ('wasm', '_effective_split_worker_table_base'),
    '_generate_split_worker_js': ('wasm', '_generate_split_worker_js'),
    '_generate_split_wrangler_jsonc': ('wasm', '_generate_split_wrangler_jsonc'),
    # molt.cli.wrapper_build
    '_build_args_has_json_flag': ('wrapper_build', '_build_args_has_json_flag'),
    '_build_args_has_python_version_flag': ('wrapper_build', '_build_args_has_python_version_flag'),
    '_emit_wrapper_build_failure': ('wrapper_build', '_emit_wrapper_build_failure'),
    '_emit_wrapper_build_success_signals': ('wrapper_build', '_emit_wrapper_build_success_signals'),
    '_parse_wrapper_build_contract_payload': ('wrapper_build', '_parse_wrapper_build_contract_payload'),
    '_read_wrapper_build_cache_contract': ('wrapper_build', '_read_wrapper_build_cache_contract'),
    '_run_wrapper_build': ('wrapper_build', '_run_wrapper_build'),
    '_wrapper_build_cache_input': ('wrapper_build', '_wrapper_build_cache_input'),
    '_wrapper_build_cache_manifest_path': ('wrapper_build', '_wrapper_build_cache_manifest_path'),
    '_wrapper_build_cache_semantic_env': ('wrapper_build', '_wrapper_build_cache_semantic_env'),
    '_wrapper_build_default_binary_path': ('wrapper_build', '_wrapper_build_default_binary_path'),
    '_wrapper_target_python': ('wrapper_build', '_wrapper_target_python'),
    '_write_wrapper_build_cache_manifest': ('wrapper_build', '_write_wrapper_build_cache_manifest'),
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
            import importlib

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
    import importlib

    entry = _LAZY_REEXPORTS.get(name)
    if entry is None:
        # Bare submodule attribute access, e.g. ``molt.cli.backend_binary``. Under
        # the old eager imports these were incidentally bound as package attributes;
        # preserve that by importing the submodule lazily. Consistent with the
        # backend-free package import contract: the backend loads only on EXPLICIT
        # access here, never on ``import molt.cli``.
        if (
            not name.startswith("__")
            and importlib.util.find_spec(f"molt.cli.{name}") is not None
        ):
            module = importlib.import_module(f"molt.cli.{name}")
            globals()[name] = module
            return module
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

    module = importlib.import_module(f"molt.cli.{entry[0]}")
    value = module if entry[1] is None else getattr(module, entry[1])
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(_LAZY_REEXPORTS))
from molt.cli import debug_helpers as _debug_helpers
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
    STUB_MODULES,
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
            bolt_requested=bolt,
            bolt_training_cmd=bolt_training_cmd,
            fact_graph_request=fact_graph_request,
        )


def main() -> int:
    from molt.cli import entrypoint as _entrypoint

    return _entrypoint.main(build_fn=build)
