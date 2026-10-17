from __future__ import annotations

import os
import shutil
from pathlib import Path
from uuid import uuid4

import pytest

from molt.dx import GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV, checkout_component
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


@pytest.mark.parametrize("pinned_session", [None, "throughput-pinned"])
def test_run_context_env_prefers_external_artifact_root(
    pinned_session: str | None,
) -> None:
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
        "MOLT_SESSION_ID",
        # Hosted CI custody owns every root; this case is the developer host.
        GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV,
    ):
        env.pop(key, None)
    env.update(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        }
    )
    if pinned_session is not None:
        env["MOLT_SESSION_ID"] = pinned_session

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
    session_id = _export_value(result.stdout, "MOLT_SESSION_ID")
    target_dir = _export_value(result.stdout, "CARGO_TARGET_DIR")
    assert session_id
    # Each checkout gets its own stable target under the shared root (HF-45).
    expected_target = (
        external_root.resolve() / "target" / "checkouts" / checkout_component(REPO_ROOT)
    )
    if pinned_session is not None:
        expected_target = expected_target / "sessions" / pinned_session
        assert session_id == pinned_session
    else:
        assert session_id not in target_dir
    assert target_dir == str(expected_target)
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
    assert "$env:PYTHONPATH = " in result.stdout
    assert "$env:MOLT_BACKEND_DAEMON_SOCKET_DIR = " in result.stdout
    assert "$env:SCCACHE_DIR = " in result.stdout


def test_throughput_apply_does_not_control_shared_sccache(tmp_path: Path) -> None:
    bash = shutil.which("bash")
    if bash is None:
        pytest.skip("throughput shell activation requires Bash")
    env = dict(os.environ)
    env.pop(GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV, None)
    roots = {
        "MOLT_EXT_ROOT": "artifacts",
        "MOLT_CACHE": "cache",
        "CARGO_TARGET_DIR": "target",
        "MOLT_DIFF_CARGO_TARGET_DIR": "target",
        "MOLT_TARGET_ROOT": "tools",
        "MOLT_DIFF_ROOT": "diff",
        "MOLT_DIFF_TMPDIR": "tmp",
        "UV_CACHE_DIR": "uv-cache",
        "TMPDIR": "tmp",
        "MOLT_BACKEND_DAEMON_SOCKET_DIR": "sockets",
        "SCCACHE_DIR": "sccache",
    }
    env.update({key: str(tmp_path / value) for key, value in roots.items()})
    env.update(
        MOLT_CACHE_PRUNE="0",
        MOLT_USE_SCCACHE="0",
        MOLT_EXTERNAL_MIN_FREE_GB="0",
        MOLT_SESSION_ID="throughput-no-server-control",
    )
    # The function is discoverable by command -v. FD3 records attempted control
    # even when the script suppresses stdout/stderr and ignores a failed command.
    script = """exec 3>&1
sccache() { printf 'UNOWNED_SCCACHE_CONTROL:%s\n' "$*" >&3; return 73; }
export -f sccache
exec "$1" "$2" --apply
"""
    result = run_native_test_process(
        [bash, "-c", script, "throughput-control", bash, "tools/throughput_env.sh"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "Configured throughput env:" in result.stdout
    assert "UNOWNED_SCCACHE_CONTROL:" not in result.stdout
    assert (tmp_path / "sccache").is_dir()
