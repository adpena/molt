from __future__ import annotations

import os
import sys
import textwrap
from pathlib import Path

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process


def _env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="native-async-shutdown",
        session_id=os.environ.get("MOLT_SESSION_ID") or "native-async-shutdown",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def test_native_asyncio_shutdown_releases_attr_ic_class_owner(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "asyncio_shutdown_refcount.py"
    src.write_text(
        textwrap.dedent(
            """
            import asyncio

            async def work():
                return 7

            print(asyncio.run(work()))
            """
        ),
        encoding="utf-8",
    )

    env = _env(root)
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--trusted",
            "--stdlib-profile",
            "full",
            "--out-dir",
            str(tmp_path),
            str(src),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    binary = tmp_path / "asyncio_shutdown_refcount_molt"
    run = run_native_test_process(
        [str(binary)],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert "refcount underflow" not in run.stderr
    assert run.stdout == "7\n"
