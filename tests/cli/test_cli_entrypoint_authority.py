from __future__ import annotations

import argparse
import inspect
import json
import sys
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import arg_helpers
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


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
def test_completion_reuses_the_parser_that_selected_the_command(
    shell: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    build_parser = entrypoint_parser._build_entrypoint_parser
    calls = 0

    def selected_parser() -> argparse.ArgumentParser:
        nonlocal calls
        calls += 1
        assert calls == 1, "completion rebuilt the command parser"
        parser = build_parser()
        parser.add_argument("--completion-authority-probe", action="store_true")
        subparsers = next(
            action
            for action in parser._actions
            if isinstance(action, argparse._SubParsersAction)
        )
        subparsers.add_parser("completion-visible-probe")
        subparsers.add_parser("completion-hidden-probe", help=argparse.SUPPRESS)
        return parser

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(entrypoint, "_ensure_cli_hash_seed", lambda: None)
    monkeypatch.setattr(entrypoint, "_build_entrypoint_parser", selected_parser)
    monkeypatch.setattr(entrypoint_parser, "_build_entrypoint_parser", selected_parser)
    monkeypatch.setattr(sys, "argv", ["molt", "completion", "--shell", shell, "--json"])

    assert entrypoint.main() == 0
    assert calls == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["command"] == "completion"
    assert payload["status"] == "ok"
    script = payload["data"]["script"]
    assert "--completion-authority-probe" in script
    assert "completion-visible-probe" in script
    assert "completion-hidden-probe" not in script


def test_other_commands_do_not_project_shell_completion(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def unexpected_projection(*_args: object, **_kwargs: object) -> str:
        raise AssertionError("ordinary CLI commands must not project shell completion")

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(entrypoint, "_ensure_cli_hash_seed", lambda: None)
    monkeypatch.setattr(arg_helpers, "_completion_script", unexpected_projection)
    monkeypatch.setattr(sys, "argv", ["molt", "config", "--json"])

    assert entrypoint.main() == 0
    assert json.loads(capsys.readouterr().out)["command"] == "config"


def test_install_leading_literal_is_owned_by_the_selected_parser(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    subparsers = next(
        action
        for action in parser._actions
        if isinstance(action, argparse._SubParsersAction)
    )
    subparsers.choices["install"].set_defaults(_install_add_command="persist-probe")
    observed: list[tuple[str, list[str] | None]] = []

    def persist(packages: list[str], **_kwargs: object) -> int:
        observed.append(("persist", packages))
        return 13

    def install(*, packages: list[str] | None, **_kwargs: object) -> int:
        observed.append(("install", packages))
        return 17

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(entrypoint, "_ensure_cli_hash_seed", lambda: None)
    monkeypatch.setattr(entrypoint, "_build_entrypoint_parser", lambda: parser)
    monkeypatch.setattr(entrypoint_dispatch, "install_add", persist)
    monkeypatch.setattr(entrypoint_dispatch, "install", install)
    monkeypatch.setattr(sys, "argv", ["molt", "install", "persist-probe", "demo>=1"])
    assert entrypoint.main() == 13
    monkeypatch.setattr(sys, "argv", ["molt", "install", "add", "other-package"])
    assert entrypoint.main() == 17
    assert observed == [
        ("persist", ["demo>=1"]),
        ("install", ["add", "other-package"]),
    ]

    monkeypatch.setattr(
        sys, "argv", ["molt", "completion", "--shell", "bash", "--json"]
    )
    assert entrypoint.main() == 0
    assert "persist-probe" in json.loads(capsys.readouterr().out)["data"]["script"]


@pytest.mark.parametrize(
    ("command", "positional", "tail"),
    [
        ("compare", "file", "script_args"),
        ("parity-run", "file", "script_args"),
        ("test", "path", "pytest_args"),
    ],
)
def test_remainder_commands_forward_options_after_their_positional(
    command: str, positional: str, tail: str
) -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    before = parser.parse_args([command, "--json", "app.py"])
    after = parser.parse_args([command, "app.py", "--json"])

    assert getattr(before, positional) == getattr(after, positional) == "app.py"
    assert before.json is True
    assert getattr(before, tail) == []
    assert after.json is False
    assert getattr(after, tail) == ["--json"]
