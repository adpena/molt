from __future__ import annotations

import os
import sys
import textwrap
from pathlib import Path

from tests.native_process_guard import run_native_test_process


def _env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("MOLT_EXT_ROOT", str(root))
    env.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    env.setdefault(
        "MOLT_DIFF_CARGO_TARGET_DIR", env.get("CARGO_TARGET_DIR", str(root / "target"))
    )
    env.setdefault("MOLT_CACHE", str(root / ".molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(root / "tmp" / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(root / "tmp"))
    env.setdefault("UV_CACHE_DIR", str(root / ".uv-cache"))
    env.setdefault("TMPDIR", str(root / "tmp"))
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
