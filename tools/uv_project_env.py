#!/usr/bin/env python3
"""Run a command in the shared development environment of another CPython.

``--python`` names a release or minor; the environment holds the same
dependency groups as the default one (``molt.uv_environment``).
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"


from molt.process_spawn import forward_command  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402


def project_environment_path(
    *,
    python: str,
    repo_root: Path = ROOT,
    env: Mapping[str, str] | None = None,
) -> Path:
    """The shared environment of ``python``, syncing it on first use."""
    from molt.checkout_provisioning import ensure

    source = os.environ if env is None else env
    root = ensure("uv-environment", repo_root, source, python=python).environment_root
    assert root is not None
    return root


def uv_project_env(
    *,
    python: str,
    env: Mapping[str, str] | None = None,
    repo_root: Path = ROOT,
    explicit: str | None = None,
) -> ProcessEnvironment:
    """``env`` for a command in ``python``'s shared environment."""
    from molt.dx import DxConfigError
    from tools.venv_exec import venv_env

    source = dict(os.environ if env is None else env)
    if source.get("MOLT_VENV"):
        raise DxConfigError(
            "managed uv execution cannot replace an explicitly borrowed MOLT_VENV"
        )
    if explicit:
        path = Path(explicit).expanduser()
        root = path if path.is_absolute() else (repo_root / path).absolute()
    else:
        root = project_environment_path(python=python, repo_root=repo_root, env=source)
    return venv_env(venv=root, env=source, managed=True, repo_root=repo_root)


def _parse_command(command: Sequence[str]) -> list[str]:
    parsed = list(command)
    if parsed and parsed[0] == "--":
        parsed = parsed[1:]
    return parsed


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        allow_abbrev=False,
        description=(
            "Run a command in the shared development environment of one CPython "
            "release, beside the default environment."
        ),
    )
    parser.add_argument("--python", required=True)
    parser.add_argument("--venv", default=None)
    parser.add_argument(
        "--print-env",
        action="store_true",
        help="Print the selected UV_PROJECT_ENVIRONMENT path before execution.",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = argument_parser()
    args = parser.parse_args(argv)

    from molt.checkout_provisioning import ProvisioningError
    from molt.dx import DxConfigError
    from tools.venv_exec import resolve_command

    try:
        env = uv_project_env(python=args.python, explicit=args.venv)
    except (ProvisioningError, DxConfigError) as exc:
        parser.error(str(exc))
    root = Path(env["UV_PROJECT_ENVIRONMENT"])
    if args.print_env:
        print(root, flush=True)
    command = _parse_command(args.command)
    if not command:
        if args.print_env:
            return 0
        parser.error("command is required after --")
    return forward_command(resolve_command(command, venv=root), env=env)


if __name__ == "__main__":
    raise SystemExit(main())
