from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import entrypoint
from molt.cli import entrypoint_parser


def test_cli_entrypoint_dispatch_and_parser_authorities_are_single_home() -> None:
    assert callable(entrypoint.main)
    assert callable(entrypoint_parser._build_entrypoint_parser)
    assert not hasattr(cli, "_build_entrypoint_parser")

    root_main_source = inspect.getsource(cli.main)
    assert "ArgumentParser" not in root_main_source
    assert "add_parser" not in root_main_source
    assert "_entrypoint.main" in root_main_source

    entrypoint_source = inspect.getsource(entrypoint)
    assert "ArgumentParser(" not in entrypoint_source
    assert ".add_parser(" not in entrypoint_source
    assert "_build_entrypoint_parser()" in entrypoint_source

    parser_source = inspect.getsource(entrypoint_parser)
    assert "def _build_entrypoint_parser(" in parser_source
    assert "ArgumentParser(" in parser_source
    assert ".add_parser(" in parser_source

    root_module_source = inspect.getsource(cli)
    assert "def build(" in root_module_source
    assert "def main(" in root_module_source
    assert "ArgumentParser(" not in root_module_source
