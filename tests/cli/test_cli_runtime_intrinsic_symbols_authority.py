from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import runtime_intrinsic_symbols

_RUNTIME_INTRINSIC_SYMBOL_NAMES = (
    "_runtime_intrinsic_symbols_digest",
    "_runtime_intrinsic_symbols_file",
    "_stage_runtime_intrinsic_symbols_for_native_codegen",
)


def test_cli_runtime_intrinsic_symbols_authority_is_single_home() -> None:
    for name in _RUNTIME_INTRINSIC_SYMBOL_NAMES:
        assert getattr(cli, name) is getattr(runtime_intrinsic_symbols, name)

    cli_source = inspect.getsource(cli)
    for name in _RUNTIME_INTRINSIC_SYMBOL_NAMES:
        assert f"def {name}(" not in cli_source
