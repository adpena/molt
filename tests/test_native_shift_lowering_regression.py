from __future__ import annotations

import os
import sys
from pathlib import Path

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process


def _native_env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="native-shift-lowering",
        session_id=os.environ.get("MOLT_SESSION_ID") or "native-shift-lowering",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    return env


def test_native_shift_ops_survive_simple_lowering_chain(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "shift_chain.py"
    src.write_text(
        "print(5 << 3)\n"
        "print(1024 >> 3)\n"
        "print((5 << 3) | 7)\n"
        "a = 5 << 3\n"
        "b = a | 7\n"
        "print(b & 0xff)\n",
        encoding="utf-8",
    )

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(src),
        ],
        cwd=root,
        env=_native_env(root),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["40", "128", "47", "47"]
