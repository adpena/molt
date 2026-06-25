from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import native_main_stub

_NATIVE_MAIN_STUB_NAMES = (
    "_native_main_stub_snippets",
    "_render_native_main_stub",
)


def test_cli_native_main_stub_authority_is_single_home() -> None:
    for name in _NATIVE_MAIN_STUB_NAMES:
        assert getattr(cli, name) is getattr(native_main_stub, name)

    cli_source = inspect.getsource(cli)
    for name in _NATIVE_MAIN_STUB_NAMES:
        assert f"def {name}(" not in cli_source
