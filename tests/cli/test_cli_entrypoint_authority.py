from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import entrypoint
from molt.cli import entrypoint_dispatch
from molt.cli import entrypoint_parser
from molt.cli.config_resolution import _select_capability_input


def test_cli_entrypoint_dispatch_and_parser_authorities_are_single_home() -> None:
    assert callable(entrypoint.main)
    assert callable(entrypoint_dispatch._dispatch_entrypoint_command)
    assert callable(entrypoint_parser._build_entrypoint_parser)
    assert not hasattr(cli, "_dispatch_entrypoint_command")
    assert not hasattr(cli, "_build_entrypoint_parser")

    root_main_source = inspect.getsource(cli.main)
    assert "ArgumentParser" not in root_main_source
    assert "add_parser" not in root_main_source
    assert "_entrypoint.main" in root_main_source

    entrypoint_source = inspect.getsource(entrypoint)
    assert "ArgumentParser(" not in entrypoint_source
    assert ".add_parser(" not in entrypoint_source
    assert "if args.command ==" not in entrypoint_source
    assert "_dispatch_entrypoint_command(" in entrypoint_source
    assert "_build_entrypoint_parser()" in entrypoint_source

    dispatch_source = inspect.getsource(entrypoint_dispatch)
    assert "def _dispatch_entrypoint_command(" in dispatch_source
    assert "if args.command ==" in dispatch_source

    parser_source = inspect.getsource(entrypoint_parser)
    assert "def _build_entrypoint_parser(" in parser_source
    assert "ArgumentParser(" in parser_source
    assert ".add_parser(" in parser_source

    root_module_source = inspect.getsource(cli)
    assert "def build(" in root_module_source
    assert "def main(" in root_module_source
    assert "if args.command ==" not in root_module_source
    assert "ArgumentParser(" not in root_module_source


def test_run_options_after_source_are_not_silently_forwarded_to_program() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()

    args = parser.parse_args(
        ["run", "app.py", "--python-version", "3.14", "--profile", "release"]
    )

    assert args.file == "app.py"
    assert args.python_version == "3.14"
    assert args.profile == "release"
    assert args.script_args == []


def test_run_double_dash_owns_option_shaped_program_arguments() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()

    args = parser.parse_args(
        ["run", "app.py", "--python-version", "3.14", "--", "--profile", "user"]
    )

    assert args.python_version == "3.14"
    assert args.profile is None
    assert args.script_args == ["--profile", "user"]


def test_capability_precedence_preserves_explicit_deny_all() -> None:
    inherited = ["net"]
    assert _select_capability_input(None, [], inherited) == []
    assert _select_capability_input(None, "", inherited) == ""
    assert _select_capability_input(None, None, inherited) is inherited

    dispatch_source = inspect.getsource(entrypoint_dispatch)
    assert "args.capabilities or" not in dispatch_source
