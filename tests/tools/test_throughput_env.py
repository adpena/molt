from __future__ import annotations

from pathlib import Path

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[2]


def test_throughput_env_exports_short_backend_daemon_socket_dir() -> None:
    result = run_native_test_process(
        ["bash", "tools/throughput_env.sh", "--print"],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert 'export MOLT_BACKEND_DAEMON_SOCKET_DIR="' in result.stdout
    line = next(
        line
        for line in result.stdout.splitlines()
        if line.startswith("export MOLT_BACKEND_DAEMON_SOCKET_DIR=")
    )
    socket_dir = line.split('"', 2)[1]
    assert socket_dir.endswith("/tmp/daemon_sock")
    assert len(socket_dir) < 80
