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


from molt.process_spawn import forward_command  # noqa: E402
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder  # noqa: E402


class VenvExecError(RuntimeError):
    pass


def _resolve_path(raw: str, *, repo_root: Path) -> Path:
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = repo_root / path
    return path.resolve()


def resolve_venv(
    *,
    repo_root: Path = ROOT,
    env: Mapping[str, str] | None = None,
    explicit: str | None = None,
) -> Path:
    source = ProcessEnvironment(os.environ if env is None else env)
    # MOLT_VENV/--venv deliberately borrow an external environment. UV selects
    # an exact managed generation; no checkout or main-worktree alias exists.
    raw = explicit or source.get("MOLT_VENV")
    if raw:
        return _resolve_path(raw, repo_root=repo_root)
    if raw := source.get("UV_PROJECT_ENVIRONMENT", "").strip():
        selected = Path(raw).expanduser()
        return (selected if selected.is_absolute() else repo_root / selected).absolute()
    from molt.dx import DxConfigError, RunContext

    try:
        return RunContext(repo_root).uv_project_env_dir(source)
    except DxConfigError as exc:
        raise VenvExecError(str(exc)) from exc


def venv_bin_dir(venv: Path) -> Path:
    scripts = venv / ("Scripts" if os.name == "nt" else "bin")
    return scripts


def venv_python(venv: Path) -> Path:
    bin_dir = venv_bin_dir(venv)
    candidates = (
        ("python.exe", "python3.exe", "python")
        if os.name == "nt"
        else ("python3", "python")
    )
    for name in candidates:
        candidate = bin_dir / name
        if candidate.is_file():
            return candidate
    raise VenvExecError(
        f"virtualenv Python not found under {bin_dir}; run "
        "`python -m molt.checkout_provisioning ensure uv-environment` "
        "or pass --venv/MOLT_VENV"
    )


def venv_env(
    *,
    venv: Path,
    env: Mapping[str, str] | None = None,
    managed: bool = False,
) -> ProcessEnvironment:
    merged = ProcessEnvironmentBuilder(os.environ if env is None else env)
    bin_dir = venv_bin_dir(venv)
    old_path = merged.get("PATH", "")
    merged["VIRTUAL_ENV"] = str(venv)
    # Every gate launched here must decode and print UTF-8 regardless of the
    # host console codec; the Windows cp1252 default aborts tools whose help or
    # diagnostics contain a non-cp1252 character (the recurring M43 class).
    merged["PYTHONUTF8"] = "1"
    merged["PYTHONIOENCODING"] = "utf-8"
    merged["PATH"] = (
        str(bin_dir) if not old_path else f"{bin_dir}{os.pathsep}{old_path}"
    )
    if managed:
        from molt.python_environment_location import environment_bytecode_prefix

        merged["UV_PROJECT_ENVIRONMENT"] = str(venv)
        merged["PYTHONPYCACHEPREFIX"] = str(environment_bytecode_prefix(venv))
        merged["PYTHONDONTWRITEBYTECODE"] = "1"
        merged["UV_NO_SYNC"] = "1"
    return ProcessEnvironment(merged)


def resolve_command(
    command: Sequence[str], *, venv: Path, managed: bool = False
) -> list[str]:
    if not command:
        raise ValueError("command is required")
    resolved = list(command)
    from tools.proof_command_argv import is_python_command

    if is_python_command(resolved[0]):
        selected = Path(resolved[0])
        if managed and selected.parent != Path("."):
            if selected.absolute().parent != venv_bin_dir(venv).absolute():
                raise VenvExecError(
                    "Python executable conflicts with the selected development image"
                )
        alias = selected.name
        if os.name == "nt" and not alias.lower().endswith(".exe"):
            alias += ".exe"
        if managed or selected.stem.lower() != "python":
            executable = venv_bin_dir(venv) / alias
            if not executable.is_file():
                raise VenvExecError(
                    f"Python executable alias {alias!r} is absent from the selected environment"
                )
            resolved[0] = str(executable)
        else:
            resolved[0] = str(venv_python(venv))
        if managed:
            from tools.proof_command_argv import python_environment_command

            resolved = python_environment_command(resolved, environment_root=venv)
    return resolved


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        allow_abbrev=False,
        description=(
            "Execute a command with the repository virtualenv activated, without "
            "leaving an extra process-manager child in the guarded process tree."
        ),
    )
    parser.add_argument("--venv", default=None)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = argument_parser()
    args = parser.parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("command is required after --")
    try:
        venv = resolve_venv(explicit=args.venv)
        managed = not (args.venv or os.environ.get("MOLT_VENV"))
        if managed:
            from molt.dx import DxConfigError, DxProject
            from molt.source_build_environment import admit_development_environment
            from molt.source_build_environment_schema import (
                SourceBuildEnvironmentError,
            )

            try:
                venv = admit_development_environment(
                    venv, repo_root=ROOT, groups=DxProject(ROOT).dependency_groups()
                )
            except (DxConfigError, SourceBuildEnvironmentError) as exc:
                raise VenvExecError(str(exc)) from exc
        env = venv_env(venv=venv, managed=managed)
        resolved = resolve_command(command, venv=venv, managed=managed)
        return forward_command(resolved, env=env)
    except (VenvExecError, ValueError) as exc:
        parser.exit(127, f"venv_exec: {exc}\n")
    except FileNotFoundError as exc:
        parser.exit(127, f"venv_exec: command not found: {exc.filename}\n")
    except OSError as exc:
        parser.exit(127, f"venv_exec: exec failed: {exc}\n")
    return 127


if __name__ == "__main__":
    raise SystemExit(main())
