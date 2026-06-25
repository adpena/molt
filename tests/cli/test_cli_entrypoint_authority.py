from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import entrypoint


def test_cli_entrypoint_owns_parser_and_dispatch() -> None:
    assert callable(entrypoint.main)

    root_main_source = inspect.getsource(cli.main)
    assert "ArgumentParser" not in root_main_source
    assert "add_parser" not in root_main_source
    assert "_entrypoint.main" in root_main_source

    root_module_source = inspect.getsource(cli)
    assert "def build(" in root_module_source
    assert "def main(" in root_module_source
    assert "ArgumentParser(" not in root_module_source
