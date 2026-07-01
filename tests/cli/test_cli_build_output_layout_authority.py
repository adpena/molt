from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import arg_helpers
from molt.cli import build_output_layout

_BUILD_OUTPUT_LAYOUT_NAMES = (
    "_BUILD_OR_DEPLOY_PROFILE_CHOICES",
    "_BUILD_PROFILE_CHOICES",
    "_DEPLOY_PROFILE_CHOICES",
    "_DEPLOY_PROFILE_DEFAULTS",
    "_OUTPUT_BASE_SAFE_RE",
    "_default_build_root",
    "_default_build_root_cached",
    "_resolve_build_output_layout",
    "_resolve_cache_root",
    "_resolve_cache_root_cached",
    "_resolve_out_dir",
    "_resolve_out_dir_cached",
    "_resolve_output_path",
    "_resolve_output_roots",
    "_resolve_sysroot",
    "_resolve_sysroot_cached",
    "_safe_output_base",
    "_wasm_runtime_root",
    "_wasm_runtime_root_cached",
)

_BUILD_OUTPUT_LAYOUT_DEFINITIONS = (
    "_OUTPUT_BASE_SAFE_RE =",
    "_DEPLOY_PROFILE_DEFAULTS: dict",
    "_BUILD_PROFILE_CHOICES =",
    "def _safe_output_base(",
    "def _wasm_runtime_root_cached(",
    "def _wasm_runtime_root(",
    "def _default_build_root_cached(",
    "def _default_build_root(",
    "def _resolve_cache_root_cached(",
    "def _resolve_cache_root(",
    "def _resolve_out_dir_cached(",
    "def _resolve_out_dir(",
    "def _resolve_sysroot_cached(",
    "def _resolve_sysroot(",
    "def _resolve_output_roots(",
    "def _resolve_output_path(",
    "def _resolve_build_output_layout(",
)


def test_cli_build_output_layout_authority_is_single_home() -> None:
    for name in _BUILD_OUTPUT_LAYOUT_NAMES:
        assert hasattr(build_output_layout, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_OUTPUT_LAYOUT_DEFINITIONS:
        assert marker not in cli_source


def test_arg_helpers_use_build_output_layout_authority_directly() -> None:
    source = inspect.getsource(arg_helpers)
    assert "cli._BUILD_PROFILE_CHOICES" not in source
    assert "from molt.cli.build_output_layout import _BUILD_PROFILE_CHOICES" in source
