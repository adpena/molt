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


from molt.dx import development_artifact_env  # noqa: E402
from molt.process_spawn import forward_command  # noqa: E402
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder  # noqa: E402


def _require_managed_selection(env: Mapping[str, str]) -> None:
    from molt.dx import DxConfigError

    if env.get("MOLT_VENV"):
        raise DxConfigError(
            "managed UV execution cannot replace an explicitly borrowed MOLT_VENV"
        )


def project_environment_path(
    *,
    python: str,
    purpose: str,
    repo_root: Path = ROOT,
    env: Mapping[str, str] | None = None,
) -> Path:
    from molt.dx import RunContext

    selected = ProcessEnvironmentBuilder(os.environ if env is None else env)
    selected["MOLT_UV_PROJECT_PURPOSE"] = purpose
    selected["MOLT_UV_PROJECT_PYTHON"] = python
    return RunContext(repo_root).uv_project_env_dir(selected)


def uv_project_env(
    *,
    python: str,
    purpose: str,
    env: Mapping[str, str] | None = None,
    repo_root: Path = ROOT,
    explicit: str | None = None,
    create_dirs: bool = True,
) -> ProcessEnvironment:
    from molt.dx import DxProject

    merged = ProcessEnvironmentBuilder(os.environ if env is None else env)
    _require_managed_selection(merged)
    merged["MOLT_UV_PROJECT_PURPOSE"] = purpose
    merged["MOLT_UV_PROJECT_PYTHON"] = python
    if explicit:
        explicit_path = Path(explicit).expanduser()
        if not explicit_path.is_absolute():
            explicit_path = repo_root / explicit_path
        merged["UV_PROJECT_ENVIRONMENT"] = str(explicit_path.absolute())
    # Provisioning is an explicit operation in main; this lookup never installs,
    # updates or adopts an image, including during queue identity selection.
    resolved = development_artifact_env(
        repo_root, merged, session_prefix="uv-project-env", create_dirs=create_dirs
    )
    project = DxProject(repo_root)
    return project.projected_uv_run_env(
        resolved, root=project.uv_project_env_dir(resolved)
    )


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

    from molt.checkout_provisioning import ProvisioningError, ensure
    from molt.dx import DxConfigError, DxProject
    from tools.venv_exec import resolve_command

    selected = ProcessEnvironmentBuilder(os.environ)
    selected["MOLT_UV_PROJECT_PURPOSE"] = args.purpose
    selected["MOLT_UV_PROJECT_PYTHON"] = args.python
    try:
        _require_managed_selection(selected)
    except DxConfigError as exc:
        parser.error(str(exc))
    if args.venv:
        explicit = Path(args.venv).expanduser()
        selected["UV_PROJECT_ENVIRONMENT"] = str(
            explicit if explicit.is_absolute() else (ROOT / explicit).absolute()
        )
    try:
        image = ensure("uv-environment", ROOT, selected, python=args.python)
        assert image.environment_root is not None
        selected["UV_PROJECT_ENVIRONMENT"] = str(image.environment_root)
        env = DxProject(ROOT).normalized_uv_run_env(
            development_artifact_env(ROOT, selected, create_dirs=False), python=None
        )
    except (ProvisioningError, DxConfigError) as exc:
        parser.error(str(exc))
    from molt.checkout_state import checkout_state_environment_roots, checkout_state_use

    with checkout_state_use(checkout_state_environment_roots(ROOT, env)):
        if args.print_env:
            print(env["UV_PROJECT_ENVIRONMENT"], flush=True)
        command = _parse_command(args.command)
        if not command:
            if args.print_env:
                return 0
            parser.error("command is required after --")
        if Path(command[0]).name.lower() in {"uv", "uv.exe"}:
            from tools.proof_command_argv import (
                _uv_prefix_and_payload,
                uv_environment_command,
            )

            if command[1:2] != ["run"]:
                parser.error("published development images admit only no-sync uv run")
            prefix, payload = _uv_prefix_and_payload(command)
            command = uv_environment_command(
                [
                    *prefix,
                    *resolve_command(
                        payload, venv=image.environment_root, managed=True
                    ),
                ],
                repo_root=ROOT,
                environment_root=image.environment_root,
                python_selector=args.python,
            )
        else:
            command = resolve_command(
                command, venv=image.environment_root, managed=True
            )
        return forward_command(command, env=env)


if __name__ == "__main__":
    raise SystemExit(main())
