from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_results

_BUILD_RESULTS_NAMES = (
    "_attach_build_metadata",
    "_attach_process_output",
    "_build_cache_info",
    "_build_common_build_json_data",
    "_build_native_link_error_data",
    "_build_native_link_success_data",
    "_emit_build_success_json",
    "_emit_native_link_result",
    "_emit_non_native_build_result",
    "_post_link_strip",
    "_write_link_fingerprint_if_needed",
)

_BUILD_RESULTS_DEFINITIONS = (
    "def _attach_build_metadata(",
    "def _attach_process_output(",
    "def _build_cache_info(",
    "def _build_common_build_json_data(",
    "def _build_native_link_error_data(",
    "def _build_native_link_success_data(",
    "def _emit_build_success_json(",
    "def _emit_native_link_result(",
    "def _emit_non_native_build_result(",
    "def _post_link_strip(",
    "def _write_link_fingerprint_if_needed(",
)


def test_cli_build_results_authority_is_single_home() -> None:
    for name in _BUILD_RESULTS_NAMES:
        assert hasattr(build_results, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_RESULTS_DEFINITIONS:
        assert marker not in cli_source
