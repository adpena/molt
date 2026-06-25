from __future__ import annotations

import os
from pathlib import Path
from uuid import uuid4

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[2]


def _export_value(stdout: str, key: str) -> str:
    line = next(
        line for line in stdout.splitlines() if line.startswith(f"export {key}=")
    )
    return line.split('"', 2)[1].replace("\\\\", "\\")


def test_run_context_env_exports_short_backend_daemon_socket_dir() -> None:
    result = run_native_test_process(
        [
            "uv",
            "run",
            "--python",
            "3.12",
            "python",
            "tools/run_context_env.py",
            "--dx",
            "--format",
            "posix",
            "--prefer-external-artifacts",
        ],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert 'export MOLT_EXT_ROOT="' in result.stdout
    assert 'export MOLT_SESSION_ID="' in result.stdout
    assert 'export MOLT_BACKEND_DAEMON_SOCKET_DIR="' in result.stdout
    socket_dir = _export_value(result.stdout, "MOLT_BACKEND_DAEMON_SOCKET_DIR")
    assert Path(socket_dir).name.startswith("molt-backend-")
    assert len(socket_dir) < 80


def test_run_context_env_prefers_external_artifact_root() -> None:
    external_root = Path("/tmp") / f"molt-throughput-env-{uuid4().hex}" / "Molt"
    env = dict(os.environ)
    for key in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
    ):
        env.pop(key, None)
    env.update(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        }
    )

    result = run_native_test_process(
        [
            "uv",
            "run",
            "--python",
            "3.12",
            "python",
            "tools/run_context_env.py",
            "--dx",
            "--format",
            "posix",
            "--prefer-external-artifacts",
        ],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert _export_value(result.stdout, "MOLT_EXT_ROOT") == str(external_root.resolve())
    assert _export_value(result.stdout, "CARGO_TARGET_DIR") == str(
        external_root.resolve() / "target"
    )
    socket_dir = _export_value(result.stdout, "MOLT_BACKEND_DAEMON_SOCKET_DIR")
    assert Path(socket_dir).name.startswith("molt-backend-")


def test_run_context_env_prints_powershell_dx_facts() -> None:
    env = dict(os.environ)
    for key in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_BACKEND_DAEMON_SOCKET_DIR",
        "SCCACHE_DIR",
    ):
        env.pop(key, None)

    result = run_native_test_process(
        [
            "uv",
            "run",
            "--python",
            "3.12",
            "python",
            "tools/run_context_env.py",
            "--dx",
            "--format",
            "powershell",
        ],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert "$env:MOLT_SESSION_ID = " in result.stdout
    assert "$env:MOLT_BACKEND_DAEMON_SOCKET_DIR = " in result.stdout
    assert "$env:SCCACHE_DIR = " in result.stdout
