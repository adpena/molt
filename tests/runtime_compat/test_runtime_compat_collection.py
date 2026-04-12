from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def test_runtime_compat_scripts_are_not_collected_as_pytest_modules() -> None:
    env = os.environ.copy()
    pythonpath = str(ROOT / "src")
    if env.get("PYTHONPATH"):
        pythonpath = f"{pythonpath}{os.pathsep}{env['PYTHONPATH']}"
    env["PYTHONPATH"] = pythonpath

    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "--collect-only",
            "tests/runtime_compat/scripts",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode in {0, 5}, result.stdout + result.stderr
    assert "ERROR collecting" not in result.stdout + result.stderr
    assert "no tests collected" in result.stdout + result.stderr


def test_runtime_compat_harness_module_is_not_collected_as_pytest_test() -> None:
    env = os.environ.copy()
    pythonpath = str(ROOT / "src")
    if env.get("PYTHONPATH"):
        pythonpath = f"{pythonpath}{os.pathsep}{env['PYTHONPATH']}"
    env["PYTHONPATH"] = pythonpath

    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "--collect-only",
            "tests/runtime_compat/test_runtime_compat.py",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode in {0, 5}, result.stdout + result.stderr
    assert "fixture 'lib' not found" not in result.stdout + result.stderr
    assert "ERROR collecting" not in result.stdout + result.stderr
