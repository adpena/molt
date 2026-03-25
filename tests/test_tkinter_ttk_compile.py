from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]


def _artifact_root() -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        return Path(configured).expanduser()
    return REPO_ROOT


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _build_env() -> dict[str, str]:
    artifact_root = _artifact_root()
    tmp_root = artifact_root / "tmp"
    env = os.environ.copy()
    env["PYTHONPATH"] = str(REPO_ROOT / "src")
    env["MOLT_EXT_ROOT"] = str(artifact_root)
    env["CARGO_TARGET_DIR"] = str(artifact_root / "target" / "tkinter-compile")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(artifact_root / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(tmp_root / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(tmp_root)
    env["UV_CACHE_DIR"] = str(artifact_root / ".uv-cache")
    env["TMPDIR"] = str(tmp_root)
    env["MOLT_BACKEND_DAEMON_SOCKET_DIR"] = "/tmp/molt_backend_sockets"
    env["MOLT_BACKEND_DAEMON"] = "0"
    env["MOLT_USE_SCCACHE"] = "0"
    return env


def test_tkinter_ttk_script_compiles_via_cli_build() -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo not available")
    if shutil.which("clang") is None:
        pytest.skip("clang not available")
    python = _python_executable()
    if not python:
        pytest.skip("python executable unavailable")

    tmp_root = _artifact_root() / "tmp"
    tmp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tkinter-ttk-compile-", dir=tmp_root) as td:
        workdir = Path(td)
        source = workdir / "tkinter_ttk_compile.py"
        output = workdir / "tkinter_ttk_compile_molt"
        source.write_text(
            "import tkinter as tk\n"
            "import tkinter.ttk as ttk\n"
            "\n"
            "root = tk.Tk(useTk=False)\n"
            "frame = ttk.Frame(root)\n"
            "print(type(frame).__name__)\n",
            encoding="utf-8",
        )

        build = subprocess.run(
            [
                python,
                "-m",
                "molt.cli",
                "build",
                "--fallback",
                "error",
                "--output",
                str(output),
                str(source),
            ],
            cwd=REPO_ROOT,
            env=_build_env(),
            capture_output=True,
            text=True,
        )
        assert build.returncode == 0, (
            "tkinter.ttk compile failed\n"
            f"stdout:\n{build.stdout}\n"
            f"stderr:\n{build.stderr}"
        )
        assert output.exists() or output.with_suffix(".exe").exists(), (
            "expected compiled tkinter.ttk binary artifact was not produced"
        )
