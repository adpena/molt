from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import native_link_command

_NATIVE_LINK_COMMAND_NAMES = (
    "_build_native_link_command",
    "_build_native_link_driver_command",
    "_resolve_available_fast_linker",
    "_resolve_dev_linker",
    "_resolve_native_linker_hint",
    "_windows_coff_library_command",
)

_NATIVE_LINK_COMMAND_DEFINITIONS = (
    "def _build_native_link_command(",
    "def _build_native_link_driver_command(",
    "def _resolve_available_fast_linker(",
    "def _resolve_dev_linker(",
    "def _resolve_native_linker_hint(",
    "def _windows_coff_library_command(",
)


def test_cli_native_link_command_authority_is_single_home() -> None:
    for name in _NATIVE_LINK_COMMAND_NAMES:
        assert getattr(cli, name) is getattr(native_link_command, name)

    cli_source = inspect.getsource(cli)
    for marker in _NATIVE_LINK_COMMAND_DEFINITIONS:
        assert marker not in cli_source
