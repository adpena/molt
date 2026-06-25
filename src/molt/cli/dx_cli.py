from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys

from molt.dx import DX_ENV_KEYS, DxProject, dx_env_payload, render_env
from molt.cli.command_runtime import _CLI_MEMORY_GUARD_PREFIX, _run_completed_command
from molt.cli.output import emit_json as _emit_json
from molt.cli.project_roots import _find_molt_root


def _dx_project_from_cwd() -> DxProject:
    root = _find_molt_root(Path.cwd()) or Path(__file__).resolve().parents[3]
    return DxProject(root)


def _handle_env(args: argparse.Namespace) -> int:
    project = _dx_project_from_cwd()
    env = project.dx_env(os.environ, create_dirs=args.create_dirs)
    keys = tuple(key for key in DX_ENV_KEYS if key in env)
    if args.json:
        _emit_json(dx_env_payload(env, keys), json_output=True)
        return 0
    sys.stdout.write(render_env(env, keys, args.format))
    sys.stdout.write("\n")
    return 0


def _handle_run(args: argparse.Namespace) -> int:
    command = list(args.child_command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("molt dx run: command required after --", file=sys.stderr)
        return 2
    project = _dx_project_from_cwd()
    env = project.dx_env(os.environ, create_dirs=True)
    result = _run_completed_command(
        command,
        cwd=project.root,
        env=env,
        capture_output=False,
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )
    return result.returncode


def handle_dx_command(args: argparse.Namespace) -> int:
    if args.dx_command == "env":
        return _handle_env(args)
    if args.dx_command == "run":
        return _handle_run(args)
    print("molt dx: subcommand required", file=sys.stderr)
    return 2


def add_dx_parser(
    subparsers: argparse._SubParsersAction[argparse.ArgumentParser],
) -> argparse.ArgumentParser:
    dx_parser = subparsers.add_parser(
        "dx",
        help="Inspect and run with canonical cross-platform developer environment facts",
        description=(
            "Resolve Molt developer-environment facts from the current checkout. "
            "The command syntax is the same on Windows, macOS, and Linux; OS and "
            "architecture differences are resolved by the tool."
        ),
    )
    dx_subparsers = dx_parser.add_subparsers(dest="dx_command", title="dx commands")

    env_parser = dx_subparsers.add_parser(
        "env",
        help="Print canonical developer environment facts",
        description=(
            "Print the canonical Molt developer environment: artifact roots, "
            "session id, daemon socket directory, shared sccache directory, and "
            "cache-retention defaults."
        ),
    )
    env_parser.add_argument(
        "--format",
        choices=("dotenv", "posix", "powershell", "cmd", "json"),
        default="dotenv",
        help="Output format (default: dotenv, the shell-neutral form).",
    )
    env_parser.add_argument(
        "--create-dirs",
        action="store_true",
        help="Create resolved artifact/cache/socket directories before printing.",
    )
    env_parser.add_argument("--json", action="store_true", help="Emit JSON output.")

    run_parser = dx_subparsers.add_parser(
        "run",
        help="Run a command under the canonical developer environment",
        description=(
            "Run a child command with the same canonical Molt DX environment that "
            "`molt dx env` prints. Use `--` before the child command."
        ),
    )
    run_parser.add_argument("child_command", nargs=argparse.REMAINDER)
    return dx_parser
