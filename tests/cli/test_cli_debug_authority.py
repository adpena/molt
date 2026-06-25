from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import debug_helpers

_DEBUG_HANDLER_NAMES = (
    "_handle_debug_bisect",
    "_handle_debug_command",
    "_handle_debug_diff",
    "_handle_debug_ir",
    "_handle_debug_perf",
    "_handle_debug_reduce",
    "_handle_debug_repro",
    "_handle_debug_trace",
    "_handle_debug_verify",
)


def test_cli_debug_handlers_authority_is_single_home() -> None:
    for name in _DEBUG_HANDLER_NAMES:
        assert hasattr(debug_helpers, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _DEBUG_HANDLER_NAMES:
        assert f"def {name}(" not in cli_source
