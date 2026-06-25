#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ENV_ROOT = ROOT / "tmp" / "uv-project-envs"


def _slug(value: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "-", value.strip()).strip("-._")
    return slug or "default"


def project_environment_path(
    *,
    python: str,
    purpose: str,
    repo_root: Path = ROOT,
) -> Path:
    return (
        repo_root / "tmp" / "uv-project-envs" / f"{_slug(purpose)}__py{_slug(python)}"
    ).resolve()


def uv_project_env(
    *,
    python: str,
    purpose: str,
    env: Mapping[str, str] | None = None,
    repo_root: Path = ROOT,
    explicit: str | None = None,
) -> dict[str, str]:
    merged = dict(os.environ if env is None else env)
    path = (
        Path(explicit).expanduser()
        if explicit
        else project_environment_path(
            python=python,
            purpose=purpose,
            repo_root=repo_root,
        )
    )
    if not path.is_absolute():
        path = (repo_root / path).resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    merged["UV_PROJECT_ENVIRONMENT"] = str(path)
    return merged


def _parse_command(command: Sequence[str]) -> list[str]:
    parsed = list(command)
    if parsed and parsed[0] == "--":
        parsed = parsed[1:]
    return parsed


def run_command(command: Sequence[str], *, env: Mapping[str, str]) -> int:
    if os.name == "nt":
        return subprocess.call(list(command), env=dict(env))
    os.execvpe(command[0], list(command), dict(env))
    return 127


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Run uv with a canonical repo-local project environment, so "
            "multi-Python lanes do not rewrite the shared interactive .venv."
        )
    )
    parser.add_argument("--python", required=True)
    parser.add_argument("--purpose", default="command")
    parser.add_argument("--venv", default=None)
    parser.add_argument(
        "--print-env",
        action="store_true",
        help="Print the resolved UV_PROJECT_ENVIRONMENT path before execution.",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)

    env = uv_project_env(
        python=args.python,
        purpose=args.purpose,
        explicit=args.venv,
    )
    if args.print_env:
        print(env["UV_PROJECT_ENVIRONMENT"], flush=True)

    command = _parse_command(args.command)
    if not command:
        if args.print_env:
            return 0
        parser.error("command is required after --")
    return run_command(command, env=env)


if __name__ == "__main__":
    raise SystemExit(main())
