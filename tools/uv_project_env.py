#!/usr/bin/env python3
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


from molt.dx import development_artifact_env, uv_project_env_component  # noqa: E402
from molt.process_spawn import forward_command  # noqa: E402


def _session_id(*, python: str, purpose: str, env: Mapping[str, str]) -> str:
    if env.get("MOLT_SESSION_ID"):
        return env["MOLT_SESSION_ID"]
    return f"{uv_project_env_component(purpose)}__py{uv_project_env_component(python)}"


def project_environment_path(
    *,
    python: str,
    purpose: str,
    repo_root: Path = ROOT,
    env: Mapping[str, str] | None = None,
) -> Path:
    env_view = dict(os.environ if env is None else env)
    env_view["MOLT_UV_PROJECT_PURPOSE"] = purpose
    env_view["MOLT_UV_PROJECT_PYTHON"] = python
    resolved = development_artifact_env(
        repo_root,
        env_view,
        session_prefix="uv-project-env",
        session_id=_session_id(python=python, purpose=purpose, env=env_view),
        create_dirs=False,
    )
    return Path(resolved["UV_PROJECT_ENVIRONMENT"]).expanduser().resolve()


def uv_project_env(
    *,
    python: str,
    purpose: str,
    env: Mapping[str, str] | None = None,
    repo_root: Path = ROOT,
    explicit: str | None = None,
) -> dict[str, str]:
    merged = dict(os.environ if env is None else env)
    merged["MOLT_UV_PROJECT_PURPOSE"] = purpose
    merged["MOLT_UV_PROJECT_PYTHON"] = python
    if explicit:
        explicit_path = Path(explicit).expanduser()
        if not explicit_path.is_absolute():
            explicit_path = repo_root / explicit_path
        merged["UV_PROJECT_ENVIRONMENT"] = str(explicit_path.resolve())
    merged = development_artifact_env(
        repo_root,
        merged,
        session_prefix="uv-project-env",
        session_id=_session_id(python=python, purpose=purpose, env=merged),
        create_dirs=True,
    )
    return merged


def _parse_command(command: Sequence[str]) -> list[str]:
    parsed = list(command)
    if parsed and parsed[0] == "--":
        parsed = parsed[1:]
    return parsed


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        allow_abbrev=False,
        description=(
            "Run uv with the canonical Molt developer project environment, so "
            "multi-Python lanes do not rewrite the shared interactive .venv or "
            "spill artifacts outside the DX resolver."
        ),
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
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = argument_parser()
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
    return forward_command(command, env=env)


if __name__ == "__main__":
    raise SystemExit(main())
