from __future__ import annotations

import argparse
import inspect
import json
import sys
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import arg_helpers
from molt.cli import build_command
from molt.cli import deps
from molt.cli import entrypoint
from molt.cli import entrypoint_dispatch
from molt.cli import entrypoint_parser
from molt.cli.build_request import BuildRequest
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
    assert "entrypoint.main()" in root_main_source

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
    assert "def build(" not in root_module_source
    assert "def main(" in root_module_source
    assert "def build(" in inspect.getsource(build_command)
    assert "if args.command ==" not in root_module_source
    assert "ArgumentParser(" not in root_module_source


@pytest.mark.parametrize("returncode", [0, 7])
def test_dispatcher_leaves_maintenance_to_build_scope(monkeypatch, returncode):
    from molt import dx

    order = []

    def build(_request):
        order.append("returned")
        return returncode

    def collect(_env, *, cwd):
        pytest.fail("the dispatcher cannot schedule an opaque library build")

    monkeypatch.setattr(build_command, "build", build)
    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    assert (
        entrypoint._build(BuildRequest(file_path="app.py", cache=False)) == returncode
    )
    assert order == ["returned"]


@pytest.mark.parametrize(
    "command", ["build", "completion", "dx", "doctor", "internal-backend-build"]
)
def test_outer_maintenance_excludes_build_metadata_and_internal_commands(
    monkeypatch, command
):
    from types import SimpleNamespace
    from molt import dx

    args = SimpleNamespace(command=command)
    monkeypatch.setattr(
        entrypoint,
        "_build_entrypoint_parser",
        lambda: SimpleNamespace(parse_args=lambda: args),
    )
    monkeypatch.setattr(
        entrypoint_dispatch, "_dispatch_entrypoint_command", lambda *_a, **_k: 7
    )

    def forbidden(*_args, **_kwargs):
        pytest.fail("an ineligible outer command captured maintenance policy")

    monkeypatch.setattr(dx, "checkout_state_maintenance_environment", forbidden)
    assert entrypoint.main() == 7


def test_outer_guard_infrastructure_failure_is_published_before_maintenance(
    tmp_path, monkeypatch, capsys
):
    import json
    from types import SimpleNamespace
    from molt import dx
    from molt.process_guard import (
        GuardInfrastructureError,
        guard_infrastructure_exit_code,
    )

    args = SimpleNamespace(command="run", json=True)
    selected = tmp_path / "selected"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(selected))
    monkeypatch.setattr(
        entrypoint,
        "_build_entrypoint_parser",
        lambda: SimpleNamespace(parse_args=lambda: args),
    )
    result = SimpleNamespace(
        infrastructure_failure=SimpleNamespace(
            phase="fixture", details=("unresolved custody",)
        ),
        child_returncode=7,
    )

    def failure(*_args, **_kwargs):
        raise GuardInfrastructureError(["fixture"], result)

    monkeypatch.setattr(entrypoint_dispatch, "_dispatch_entrypoint_command", failure)
    observed = []

    def collect(env, *, cwd):
        payload = json.loads(capsys.readouterr().out)
        assert payload["status"] == "error"
        assert payload["data"]["returncode"] == guard_infrastructure_exit_code()
        assert env["MOLT_EXT_ROOT"] == str(selected)
        observed.append("published failure")

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    assert entrypoint.main() == guard_infrastructure_exit_code()
    assert observed == ["published failure"]


@pytest.mark.parametrize(
    "command", [["build", "app.py"], ["factgraph", "app.py", "main"]]
)
@pytest.mark.parametrize("policy", ["error", "bridge"])
def test_removed_fallback_option_is_refused_by_the_command_parser(
    command: list[str], policy: str
) -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    with pytest.raises(SystemExit) as error:
        parser.parse_args([*command, "--fallback", policy])
    assert error.value.code == 2


@pytest.mark.parametrize("policy", ["error", "bridge"])
def test_removed_build_policy_field_is_refused_before_build_execution(
    policy: str,
) -> None:
    with pytest.raises(TypeError, match="fallback_policy"):
        BuildRequest(file_path="retired-policy.py", fallback_policy=policy)


@pytest.mark.parametrize("config_name", ["molt.toml", "pyproject.toml"])
@pytest.mark.parametrize("policy", ["error", "bridge"])
def test_removed_project_fallback_is_refused_before_build_dispatch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    config_name: str,
    policy: str,
) -> None:
    section = "build" if config_name == "molt.toml" else "tool.molt.build"
    (tmp_path / config_name).write_text(
        f'[{section}]\nfallback = "{policy}"\n', encoding="utf-8"
    )
    source = tmp_path / "app.py"
    source.write_text("print('supported source')\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(sys, "argv", ["molt", "build", str(source), "--json"])

    def forbidden_build(_request: BuildRequest) -> int:
        pytest.fail("retired configuration reached the build")

    assert entrypoint.main(build_fn=forbidden_build) == 2
    captured = capsys.readouterr()
    assert captured.err == ""
    payload = json.loads(captured.out)
    assert payload["status"] == "error" and payload["command"] == "build"
    assert payload["data"]["returncode"] == 2
    assert payload["errors"] == [
        "build configuration 'fallback' was removed; unsupported constructs fail at compile time"
    ]


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
    monkeypatch.setattr(entrypoint, "_build_entrypoint_parser", lambda: parser)
    monkeypatch.setattr(deps, "install_add", persist)
    monkeypatch.setattr(deps, "install", install)
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


def _old_cache_entry(cache: Path) -> Path:
    entry = cache / "module_lowering" / "entry.json"
    entry.parent.mkdir(parents=True, exist_ok=True)
    entry.write_bytes(b"old immutable cache payload")
    import os

    os.utime(entry, (1, 1))
    return entry


def test_cli_build_applies_the_cache_budget_to_its_cache_once_a_day(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # HF-184: a CLI build applies the budget to the cache it used, after it
    # succeeded, at most once a day; failures, --no-cache builds and
    # MOLT_CACHE_PRUNE=0 leave the cache alone.
    from molt import cache_eviction

    cache = tmp_path / "cache"
    entry = _old_cache_entry(cache)
    monkeypatch.setenv("MOLT_CACHE_MAX_GB", "0")
    monkeypatch.delenv("MOLT_CACHE_PRUNE", raising=False)
    returncode = 1
    monkeypatch.setattr(build_command, "build", lambda _request: returncode)
    request = BuildRequest(file_path="app.py", cache_dir=str(cache))

    assert entrypoint._build(request) == 1
    assert entrypoint._build(BuildRequest(file_path="app.py", cache=False)) == 1
    assert entry.exists()
    returncode = 0
    assert entrypoint._build(BuildRequest(file_path="app.py", cache=False)) == 0
    monkeypatch.setenv("MOLT_CACHE_PRUNE", "0")
    assert entrypoint._build(request) == 0
    assert entry.exists()

    monkeypatch.delenv("MOLT_CACHE_PRUNE")
    assert entrypoint._build(request) == 0
    assert not entry.exists()
    entry = _old_cache_entry(cache)

    def no_second_sweep(*_args: object, **_kwargs: object) -> None:
        pytest.fail("a build within a day of the last sweep must not sweep")

    monkeypatch.setattr(cache_eviction, "automatic_retention", no_second_sweep)
    assert entrypoint._build(request) == 0
    assert entry.exists()


def test_cache_prune_command_applies_the_budget_and_reports_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    cache = tmp_path / "cache"
    entry = _old_cache_entry(cache)
    for argv in (
        ["molt", "cache", "prune", "--cache-dir", str(cache), "--max-gb", "0",
         "--dry-run", "--json"],
        ["molt", "cache", "prune", "--cache-dir", str(cache), "--max-gb", "0",
         "--json"],
    ):  # fmt: skip
        monkeypatch.setattr(sys, "argv", argv)
        assert entrypoint.main() == 0
        payload = json.loads(capsys.readouterr().out)
        assert payload["command"] == "cache prune" and payload["status"] == "ok"
        assert payload["data"]["entries_removed"] == 1
        assert payload["data"]["bytes_removed"] == len(b"old immutable cache payload")
        assert entry.exists() is ("--dry-run" in argv)
