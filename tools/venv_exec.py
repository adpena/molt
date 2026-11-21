#!/usr/bin/env python3
"""Run a command in the shared development environment.

``uv run --no-project python tools/venv_exec.py python ...`` is the canonical
entry: it selects (and on first use syncs) the uv environment of this
checkout's lock through ``molt.uv_environment``, then replaces itself with the
command, so no extra process stays in the guarded process tree.
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


from molt import uv_environment  # noqa: E402
from molt.process_spawn import forward_command  # noqa: E402
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder  # noqa: E402


class VenvExecError(RuntimeError):
    pass


def _resolve_path(raw: str, *, repo_root: Path) -> Path:
    # Lexical normalization only: resolving would replace an environment's
    # entry links with its base interpreter.
    return Path(os.path.abspath(repo_root / Path(raw).expanduser()))


def resolve_venv(
    *,
    repo_root: Path = ROOT,
    env: Mapping[str, str] | None = None,
    explicit: str | None = None,
) -> Path:
    """The environment a command runs in, syncing the shared one if needed.

    ``--venv``/``MOLT_VENV`` borrow an external environment; an explicit
    ``UV_PROJECT_ENVIRONMENT`` names one; otherwise the checkout's lock
    selects the shared development environment.
    """
    source = os.environ if env is None else env
    raw = explicit or source.get("MOLT_VENV", "").strip()
    if raw:
        return _resolve_path(raw, repo_root=repo_root)
    if raw := source.get("UV_PROJECT_ENVIRONMENT", "").strip():
        return _resolve_path(raw, repo_root=repo_root)
    try:
        return uv_environment.environment(repo_root, environ=source)
    except uv_environment.UvEnvironmentError as exc:
        raise VenvExecError(str(exc)) from exc


def venv_bin_dir(venv: Path) -> Path:
    return venv / ("Scripts" if os.name == "nt" else "bin")


def venv_python(venv: Path) -> Path:
    """The interpreter entry of ``venv``; a borrowed one may lack ``python``."""
    bin_dir = venv_bin_dir(venv)
    names = (
        ("python.exe", "python3.exe", "python")
        if os.name == "nt"
        else ("python3", "python")
    )
    for name in names:
        if (candidate := bin_dir / name).is_file():
            return candidate
    raise VenvExecError(
        f"environment Python not found under {bin_dir}; pass --venv/MOLT_VENV or "
        "unset UV_PROJECT_ENVIRONMENT to use the shared environment"
    )


def venv_env(
    *,
    venv: Path,
    env: Mapping[str, str] | None = None,
    managed: bool = False,
    repo_root: Path | None = None,
) -> ProcessEnvironment:
    """``env`` activating ``venv``. A managed environment also selects its
    bytecode cache, and puts ``repo_root``'s ``src`` first on ``PYTHONPATH``
    when the caller names the checkout whose code it runs. Nothing defaults
    to this module's checkout: a pinned controller that imports this module
    builds gates for another checkout (HF-969)."""
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
        merged["UV_PROJECT_ENVIRONMENT"] = str(venv)
        merged["UV_NO_SYNC"] = "1"
        merged["PYTHONPYCACHEPREFIX"] = str(uv_environment.bytecode_prefix(venv))
        merged.pop("PYTHONDONTWRITEBYTECODE", None)
        if repo_root is not None:
            from molt.dx import ensure_repo_src_pythonpath

            ensure_repo_src_pythonpath(repo_root, merged)
    return ProcessEnvironment(merged)


def resolve_command(
    command: Sequence[str], *, venv: Path, managed: bool = False
) -> list[str]:
    """``command`` with a leading Python alias bound to ``venv``'s interpreter.

    A managed command also selects the environment's shared bytecode cache
    as an interpreter option, which ``-I`` cannot drop.
    """
    if not command:
        raise ValueError("command is required")
    resolved = list(command)
    from tools.proof_command_argv import is_python_command

    if is_python_command(resolved[0]):
        alias = Path(resolved[0]).name
        if os.name == "nt" and not alias.lower().endswith(".exe"):
            alias += ".exe"
        if not managed and Path(alias).stem.lower() == "python":
            # A borrowed environment may lack `python`; take its entry.
            resolved[0] = str(venv_python(venv))
        else:
            executable = venv_bin_dir(venv) / alias
            if not executable.is_file():
                raise VenvExecError(
                    f"Python executable alias {alias!r} is absent from {venv}"
                )
            resolved[0] = str(executable)
        if managed:
            from tools.proof_command_argv import python_environment_command

            resolved = python_environment_command(resolved, environment_root=venv)
    return resolved


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        allow_abbrev=False,
        description=(
            "Execute a command in the shared development environment, without "
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
        managed = not (args.venv or os.environ.get("MOLT_VENV"))
        venv = resolve_venv(explicit=args.venv)
        env = venv_env(venv=venv, managed=managed, repo_root=ROOT)
        return forward_command(
            resolve_command(command, venv=venv, managed=managed), env=env
        )
    except (VenvExecError, ValueError) as exc:
        parser.exit(127, f"venv_exec: {exc}\n")
    except FileNotFoundError as exc:
        parser.exit(127, f"venv_exec: command not found: {exc.filename}\n")
    except OSError as exc:
        parser.exit(127, f"venv_exec: exec failed: {exc}\n")
    return 127


if __name__ == "__main__":
    raise SystemExit(main())
