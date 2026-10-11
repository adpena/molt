from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path
from uuid import uuid4

import pytest

from molt.dx import cargo_target_dir_for_artifact_root
from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[2]


def _export_value(stdout: str, key: str) -> str:
    line = next(
        line for line in stdout.splitlines() if line.startswith(f"export {key}=")
    )
    return line.split('"', 2)[1].replace("\\\\", "\\")


@pytest.mark.parametrize("pinned", [False, True], ids=["init-named", "pinned"])
@pytest.mark.usefixtures("developer_host_context")
def test_new_agent_task_scaffolds_canonical_agent_env(pinned: bool) -> None:
    task = f"unit-agent-{uuid4().hex}"
    pinned_session = f"lane-{uuid4().hex}"
    base = REPO_ROOT / "logs" / "agents" / task
    artifact_root = (Path("/tmp") / f"molt-agent-artifacts-{uuid4().hex}").resolve()
    socket_root = (Path("/tmp") / f"molt-agent-sockets-{uuid4().hex}").resolve()
    env = dict(os.environ)
    for key in (
        "MOLT_SESSION_ID",
        "MOLT_SESSION_ID_GENERATED",
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
        "SCCACHE_DIR",
        "SCCACHE_CACHE_SIZE",
        "MOLT_BACKEND_DAEMON_SOCKET_DIR",
    ):
        env.pop(key, None)
    env.update(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(artifact_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "MOLT_BACKEND_DAEMON_SOCKET_ROOT": str(socket_root),
        }
    )
    if pinned:
        # A launcher's generated-session marker must not unpin --session.
        env["MOLT_SESSION_ID_GENERATED"] = "1"

    try:
        result = run_native_test_process(
            [
                sys.executable,
                "tools/agent_coordination.py",
                "init",
                task,
                *(["--session", pinned_session] if pinned else []),
            ],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

        assert result.returncode == 0, result.stderr
        assert f"Created task scaffold at {base}" in result.stdout

        env_sh = base / "env.sh"
        env_ps1 = base / "env.ps1"
        progress_log = base / "progress.log"
        reports = list(base.glob("report_*.md"))
        assert env_sh.exists()
        assert env_ps1.exists()
        assert progress_log.exists()
        assert len(reports) == 1

        env_text = env_sh.read_text(encoding="utf-8")
        ps_text = env_ps1.read_text(encoding="utf-8")
        session_id = _export_value(env_text, "MOLT_SESSION_ID")
        assert Path(_export_value(env_text, "MOLT_EXT_ROOT")) == artifact_root
        cargo_target = Path(_export_value(env_text, "CARGO_TARGET_DIR"))
        if pinned:
            # Only a pinned session gets its own Cargo target.
            assert session_id == pinned_session
            assert "MOLT_SESSION_ID_GENERATED" not in env_text
            assert cargo_target.parent == artifact_root / "target" / "sessions"
            assert cargo_target == cargo_target_dir_for_artifact_root(
                artifact_root, session_id
            )
        else:
            # A session init names itself shares the warm persistent target;
            # a per-PID target would build cold for every task.
            assert session_id.startswith(f"agent-{task}-")
            assert _export_value(env_text, "MOLT_SESSION_ID_GENERATED") == "1"
            assert cargo_target == artifact_root / "target"
        assert _export_value(env_text, "MOLT_DIFF_CARGO_TARGET_DIR") == str(
            cargo_target
        )
        assert _export_value(env_text, "SCCACHE_DIR") == str(artifact_root / ".sccache")
        assert _export_value(env_text, "MOLT_BACKEND_DAEMON_SOCKET_DIR").startswith(
            str(socket_root / "molt-backend-")
        )
        assert "$env:MOLT_SESSION_ID = " in ps_text
        assert "$env:SCCACHE_DIR = " in ps_text

        report_text = reports[0].read_text(encoding="utf-8")
        assert f"- Env: {env_sh}" in report_text
        assert f"- Env PowerShell: {env_ps1}" in report_text
        assert f"- MOLT_SESSION_ID: {session_id}" in report_text
        assert f"- CARGO_TARGET_DIR: {cargo_target}" in report_text
        assert "molt dx run -- <command>" in report_text
        assert f'source "{env_sh}"' in report_text
        assert "initialized task=" in progress_log.read_text(encoding="utf-8")
    finally:
        shutil.rmtree(base, ignore_errors=True)
        # The scaffold lives in the checkout by design; leave the checkout as
        # the session found it (HF-135). rmdir refuses a parent that a
        # parallel test or a real task still uses.
        for directory in (base.parent, base.parent.parent):
            try:
                directory.rmdir()
            except OSError:
                break
