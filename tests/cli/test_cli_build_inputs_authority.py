from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_inputs

_BUILD_INPUT_NAMES = (
    "_VALID_AUDIT_SINKS",
    "_append_rustflags",
    "_build_args_lib_paths",
    "_build_args_respect_pythonpath",
    "_capability_ambient_env_for_cache",
    "_capability_config_cache_digest",
    "_capability_config_cache_digest_from_env",
    "_collect_env_overrides",
    "_enable_native_arch_rustflags",
    "_is_stdlib_path",
    "_latest_mtime",
    "_load_molt_config",
    "_merge_module_graph_with_reason",
    "_native_arch_perf_requested",
    "_native_arch_perf_requested_cached",
    "_package_root_for_override",
    "_parse_audit_log_flag",
    "_parse_io_mode_flag",
    "_parse_type_gate_flag",
    "_prepare_build_config",
    "_prepare_build_inputs",
    "_prepare_build_preamble",
    "_prepare_build_roots",
    "_resolve_build_entry",
    "_resolve_entry_module",
    "_resolve_module_root_resolution",
    "_resolve_module_roots",
    "_resolve_wrapper_build_entry",
)


def test_cli_build_inputs_authority_is_single_home() -> None:
    for name in _BUILD_INPUT_NAMES:
        assert hasattr(build_inputs, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _BUILD_INPUT_NAMES:
        assert f"def {name}(" not in cli_source
    assert "_VALID_AUDIT_SINKS =" not in cli_source
