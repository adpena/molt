#!/usr/bin/env python3
"""Bootstrap Molt CLI in a local venv and then run molt.cli.

This is used by the packaged installers to avoid manual setup.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
import venv


def _venv_python(venv_dir: Path) -> Path:
    if os.name == "nt":
        return venv_dir / "Scripts" / "python.exe"
    return venv_dir / "bin" / "python"


def _ensure_python_version() -> None:
    if sys.version_info < (3, 12):
        raise SystemExit(
            "molt: Python 3.12+ is required. Install a newer Python or run the install script."
        )


def _find_wheel(root: Path) -> Path:
    wheels_dir = root / "share" / "molt" / "wheels"
    if not wheels_dir.exists():
        raise SystemExit(f"molt: wheels directory missing at {wheels_dir}")
    wheels = sorted(wheels_dir.glob("molt-*.whl"))
    if not wheels:
        raise SystemExit("molt: no wheel found in share/molt/wheels")
    return wheels[-1]


def _create_venv(venv_dir: Path) -> None:
    builder = venv.EnvBuilder(with_pip=True)
    builder.create(venv_dir)


def _install_wheel(venv_python: Path, wheel: Path) -> None:
    subprocess.check_call([str(venv_python), "-m", "pip", "install", str(wheel)])


def _exec_molt(venv_python: Path, args: list[str]) -> None:
    os.execv(str(venv_python), [str(venv_python), "-m", "molt.cli", *args])


def _resolve_root() -> Path:
    override = os.environ.get("MOLT_BUNDLE_ROOT")
    if override:
        return Path(override).expanduser().resolve()
    return Path(__file__).resolve().parents[2]


def _resolve_venv(root: Path) -> Path:
    override = os.environ.get("MOLT_VENV")
    if override:
        return Path(override).expanduser().resolve()
    home_override = os.environ.get("MOLT_HOME")
    if home_override:
        return Path(home_override).expanduser().resolve() / "venv"
    return Path.home() / ".molt" / "venv"


def main() -> None:
    _ensure_python_version()
    root = _resolve_root()
    if "MOLT_PROJECT_ROOT" not in os.environ:
        os.environ["MOLT_PROJECT_ROOT"] = os.getcwd()
    os.chdir(root)

    venv_dir = _resolve_venv(root)
    venv_python = _venv_python(venv_dir)
    if not venv_python.exists():
        _create_venv(venv_dir)
        wheel = _find_wheel(root)
        _install_wheel(venv_python, wheel)

    _exec_molt(venv_python, sys.argv[1:])


if __name__ == "__main__":
    main()
