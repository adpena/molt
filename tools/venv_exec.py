#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
from collections.abc import Mapping, Sequence
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


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
    source = os.environ if env is None else env
    raw = explicit or source.get("MOLT_VENV")
    if raw:
        return _resolve_path(raw, repo_root=repo_root)
    default = (repo_root / ".venv").resolve()
    active = source.get("VIRTUAL_ENV")
    if active and _resolve_path(active, repo_root=repo_root) == default:
        return default
    return default


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
        f"virtualenv Python not found under {bin_dir}; run `uv sync --frozen` "
        "or pass --venv/MOLT_VENV"
    )


def venv_env(
    *,
    venv: Path,
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    merged = dict(os.environ if env is None else env)
    bin_dir = venv_bin_dir(venv)
    old_path = merged.get("PATH", "")
    merged["VIRTUAL_ENV"] = str(venv)
    merged["PATH"] = (
        str(bin_dir) if not old_path else f"{bin_dir}{os.pathsep}{old_path}"
    )
    return merged


def resolve_command(command: Sequence[str], *, venv: Path) -> list[str]:
    if not command:
        raise ValueError("command is required")
    resolved = list(command)
    head = Path(resolved[0]).name.lower()
    if head in {"python", "python3", "python.exe", "python3.exe"}:
        resolved[0] = str(venv_python(venv))
    return resolved


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Execute a command with the repository virtualenv activated, without "
            "leaving an extra process-manager child in the guarded process tree."
        )
    )
    parser.add_argument("--venv", default=None)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("command is required after --")
    try:
        venv = resolve_venv(explicit=args.venv)
        env = venv_env(venv=venv)
        resolved = resolve_command(command, venv=venv)
        os.execvpe(resolved[0], resolved, env)
    except VenvExecError as exc:
        parser.exit(127, f"venv_exec: {exc}\n")
    except FileNotFoundError as exc:
        parser.exit(127, f"venv_exec: command not found: {exc.filename}\n")
    except OSError as exc:
        parser.exit(127, f"venv_exec: exec failed: {exc}\n")
    return 127


if __name__ == "__main__":
    raise SystemExit(main())
