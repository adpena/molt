from __future__ import annotations

import molt.cli as cli
from molt.cli import commands


_COMMAND_AUTHORITY_NAMES = (
    "_deploy",
    "_format_duration",
    "_internal_batch_build_server",
    "_normalize_internal_batch_stdlib_profile",
    "_resolve_python_exe",
    "_run_command",
    "_run_command_timed",
    "_run_script_cross",
    "bench",
    "compare",
    "diff",
    "extension_build",
    "lint",
    "parity_run",
    "profile",
    "run_script",
    "test",
)


def test_cli_commands_are_owned_outside_root() -> None:
    for name in _COMMAND_AUTHORITY_NAMES:
        assert hasattr(commands, name), name
        assert not hasattr(cli, name), name
