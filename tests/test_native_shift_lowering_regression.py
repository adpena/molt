from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def _native_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env["MOLT_EXT_ROOT"] = str(root)
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(root / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(root / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(root / "tmp")
    env["UV_CACHE_DIR"] = str(root / ".uv-cache")
    env["TMPDIR"] = str(root / "tmp")
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

    run = subprocess.run(
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
