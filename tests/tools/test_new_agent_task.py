from __future__ import annotations

import os
import shutil
from pathlib import Path
from uuid import uuid4

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[2]


def _export_value(stdout: str, key: str) -> str:
    line = next(
        line for line in stdout.splitlines() if line.startswith(f"export {key}=")
    )
    return line.split('"', 2)[1]


def test_new_agent_task_scaffolds_canonical_agent_env() -> None:
    task = f"unit-agent-{uuid4().hex}"
    base = REPO_ROOT / "logs" / "agents" / task
    artifact_root = (Path("/tmp") / f"molt-agent-artifacts-{uuid4().hex}").resolve()
    socket_root = (Path("/tmp") / f"molt-agent-sockets-{uuid4().hex}").resolve()
    env = dict(os.environ)
    for key in (
        "MOLT_SESSION_ID",
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
        "SCCACHE_DIR",
    ):
        env.pop(key, None)
    env.update(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(artifact_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "MOLT_BACKEND_DAEMON_SOCKET_ROOT": str(socket_root),
        }
    )

    try:
        result = run_native_test_process(
            ["bash", "tools/new-agent-task.sh", task],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

        assert result.returncode == 0, result.stderr
        assert f"Created task scaffold at {base}" in result.stdout

        env_sh = base / "env.sh"
        progress_log = base / "progress.log"
        reports = list(base.glob("report_*.md"))
        assert env_sh.exists()
        assert progress_log.exists()
        assert len(reports) == 1

        env_text = env_sh.read_text()
        assert _export_value(env_text, "MOLT_SESSION_ID").startswith(f"agent-{task}-")
        assert Path(_export_value(env_text, "MOLT_EXT_ROOT")) == artifact_root
        assert _export_value(env_text, "CARGO_TARGET_DIR") == str(
            artifact_root / "target"
        )
        assert _export_value(env_text, "MOLT_DIFF_CARGO_TARGET_DIR") == str(
            artifact_root / "target"
        )
        assert _export_value(env_text, "SCCACHE_DIR") == str(artifact_root / ".sccache")
        assert _export_value(env_text, "MOLT_BACKEND_DAEMON_SOCKET_DIR").startswith(
            str(socket_root / "molt-backend-")
        )

        report_text = reports[0].read_text()
        assert f"- Env: {base}/env.sh" in report_text
        assert f"- MOLT_SESSION_ID: agent-{task}-" in report_text
        assert f"- CARGO_TARGET_DIR: {artifact_root / 'target'}" in report_text
        assert f'source "{base}/env.sh"' in report_text
        assert "initialized task=" in progress_log.read_text()
    finally:
        shutil.rmtree(base, ignore_errors=True)
